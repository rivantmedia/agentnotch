#Requires -Version 7.3
<#
.SYNOPSIS
    The installer smoke test: what a user gets, on a runner that has never seen Agent Notch.

.DESCRIPTION
    DESIGN-WIN §7.5. Run by agentnotch-windows.yml after the installer is built:

        windows\scripts\agentnotch-smoke.ps1 -Installer out\AgentNotch-<V>-Setup.exe -Version <V> `
            -Updates off -Artifacts out\smoke -Bins target\debug

    It installs the app, builds a temporary Claude setup (P) from the fixtures under
    smoke\profile, runs the commands and the launches the phases name, and uninstalls. It never
    runs claude (only fake-claude.exe, which the fork's test build makes), touches nothing
    outside P, the install folder, the app's own data and the HKCU keys it names, and stops
    only the processes it started, by their handle (never by image name).

    Phases are rows of a table (Get-PhaseTable): a number, a name, an optional gate and a
    body. Phases 4-12 are rows too (the real app: before consent, the deep link, Turn on, every hook
    entry, Turn off, sign-in and sync against a fake website, an update over the running app, a
    manual reinstall). A gate is a flag in smoke\gates.json: the
    package a phase waits for has not landed, so the phase prints a warning, is listed in the
    step summary and is not run. With -Release any closed gate fails the run before phase 0,
    so no release ships with a phase unrun.

    Every command line is `Start-Process agentnotch.exe -Wait -PassThru`: the exit code is the
    process's, the text is <data>\<command>.log (the exe is a GUI-subsystem program, so nothing
    reaches a pipe).

    Everything the run leaves is in -Artifacts: one log per phase, the doctor's report,
    smoke-results.json and smoke-summary.md (also appended to the step summary).
#>
[CmdletBinding()]
param(
    # The installer under test.
    [string]$Installer = '',
    # The version it was built as (VERSION).
    [string]$Version = '',
    # Whether this build carries the update key and feed (a release build does).
    [ValidateSet('on', 'off')]
    [string]$Updates = 'off',
    # The key id the doctor must show when updates are on (derived by the release tool).
    [string]$KeyId = '',
    # Where the logs, reports and screenshots go.
    [string]$Artifacts = 'out\smoke',
    # The folder holding fake-claude.exe and pipe-test-server.exe (the fork crates' test build).
    [string]$Bins = 'target\debug',
    # A release run: every gate must be open.
    [switch]$Release,
    # smoke\gates.json; a parameter so the tests can point it elsewhere.
    [string]$GatesFile = (Join-Path $PSScriptRoot 'smoke\gates.json')
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

# --- gates ---------------------------------------------------------------------------------

# The gates the table of phases may name. gates.json must list exactly these (a typo in a gate
# name would otherwise turn a phase into a silent skip) and say what each one waits for.
$script:KnownGates = @('engine', 'selftest', 'cloud', 'glue', 'realClaude', 'rustContract')

function Read-Gates {
    param([Parameter(Mandatory)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) { throw "the gates file $Path does not exist" }
    $json = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json -AsHashtable
    $gates = [ordered]@{}
    foreach ($name in $script:KnownGates) {
        if (-not $json.ContainsKey($name)) { throw "gates.json has no '$name' gate" }
        if ($json[$name] -isnot [bool]) { throw "gates.json: '$name' must be true or false" }
        $gates[$name] = $json[$name]
    }
    foreach ($name in $json.Keys) {
        if ($name -ne '_why' -and $name -notin $script:KnownGates) { throw "gates.json names an unknown gate '$name'" }
    }
    if (-not $json.ContainsKey('_why') -or $json['_why'] -isnot [System.Collections.IDictionary]) {
        throw "gates.json has no '_why' object"
    }
    foreach ($name in $script:KnownGates) {
        if (-not $json['_why'].Contains($name) -or -not [string]$json['_why'][$name]) {
            throw "gates.json: '_why' does not say what '$name' waits for"
        }
    }
    [pscustomobject]@{ Open = $gates; Why = $json['_why'] }
}

function Test-GateOpen {
    param([Parameter(Mandatory)]$Gates, [Parameter(Mandatory)][string]$Name)
    if ($Name -notin $script:KnownGates) { throw "unknown gate '$Name'" }
    [bool]$Gates.Open[$Name]
}

# A release run runs every phase: a closed gate is a refusal, before anything is installed.
function Assert-GatesForRelease {
    param([Parameter(Mandatory)]$Gates, [switch]$Release)
    if (-not $Release) { return }
    $closed = @($script:KnownGates | Where-Object { -not $Gates.Open[$_] })
    if ($closed.Count) {
        $lines = $closed | ForEach-Object { "  $_ (waiting for $($Gates.Why[$_]))" }
        throw ("a release must run every smoke phase, but these gates are closed:`n" + ($lines -join "`n"))
    }
}

function Get-GateWarning {
    param([Parameter(Mandatory)]$Gates, [Parameter(Mandatory)][string]$Number, [Parameter(Mandatory)][string]$Gate, [string]$Part = '')
    $what = if ($Part) { "phase $Number ($Part)" } else { "phase $Number" }
    "::warning::$what not run: waiting for $($Gates.Why[$Gate])"
}

# --- hashing -------------------------------------------------------------------------------

function Get-FileSha256 {
    param([Parameter(Mandatory)][string]$Path)
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# Every file under Root: relative path (forward slashes) -> SHA-256. A missing Root is empty.
# Empty folders are not recorded: what the smoke test compares is bytes. Paths matching a -Skip
# pattern (relative, before the prefix) are never opened: P's AppData holds the running app's
# WebView2 profile, whose databases are locked while it runs.
function Get-TreeHash {
    param([Parameter(Mandatory)][string]$Root, [string]$Prefix = '', [string[]]$Skip = @())
    $hashes = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $hashes }
    $full = (Resolve-Path -LiteralPath $Root).ProviderPath
    $files = Get-ChildItem -LiteralPath $full -Recurse -File -Force | Sort-Object FullName
    foreach ($file in $files) {
        $relative = [IO.Path]::GetRelativePath($full, $file.FullName).Replace('\', '/')
        if (@($Skip | Where-Object { $relative -like $_ }).Count) { continue }
        $hashes[$Prefix + $relative] = Get-FileSha256 -Path $file.FullName
    }
    $hashes
}

# What differs between two hash tables, one line each: changed, added, removed. Empty = same.
function Compare-Hashes {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [string]$Filter = '', [string[]]$Exclude = @())
    $differences = [Collections.Generic.List[string]]::new()
    $inScope = {
        param($p)
        if ($Filter -and $p -notlike $Filter) { return $false }
        foreach ($pattern in $Exclude) { if ($p -like $pattern) { return $false } }
        $true
    }
    foreach ($path in $Before.Keys) {
        if (-not (& $inScope $path)) { continue }
        if (-not $After.Contains($path)) { $differences.Add("removed: $path") }
        elseif ($After[$path] -cne $Before[$path]) { $differences.Add("changed: $path") }
    }
    foreach ($path in $After.Keys) {
        if ((& $inScope $path) -and -not $Before.Contains($path)) { $differences.Add("added: $path") }
    }
    [string[]]$differences.ToArray()
}

# The runner's own Claude profile, found the way `dirs` and Claude Code find it: the Known
# Folder, not USERPROFILE (which the app runs with, pointing at P).
function Get-KnownProfileRoot { [Environment]::GetFolderPath('UserProfile') }

function Get-KnownProfileHashes {
    param([string]$Root = (Get-KnownProfileRoot))
    $hashes = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Root)) { return $hashes }
    foreach ($entry in Get-ChildItem -LiteralPath $Root -Force -Filter '.claude*' | Sort-Object Name) {
        if ($entry.PSIsContainer) {
            foreach ($pair in (Get-TreeHash -Root $entry.FullName -Prefix "$($entry.Name)/").GetEnumerator()) { $hashes[$pair.Key] = $pair.Value }
        } else {
            $hashes[$entry.Name] = Get-FileSha256 -Path $entry.FullName
        }
    }
    $hashes
}

# The app's own data folders on this runner (all expected absent before the install).
function Get-AppDataLocations {
    param([string]$Roaming = $env:APPDATA, [string]$Local = $env:LOCALAPPDATA)
    $found = [Collections.Generic.List[string]]::new()
    if ($Roaming -and (Test-Path -LiteralPath $Roaming)) {
        foreach ($entry in Get-ChildItem -LiteralPath $Roaming -Force -Filter 'Agent Notch*') { $found.Add($entry.FullName) }
    }
    if ($Local) {
        foreach ($name in 'com.rivantmedia.agentnotch', 'Agent Notch') {
            $path = Join-Path $Local $name
            if (Test-Path -LiteralPath $path) { $found.Add($path) }
        }
    }
    [string[]]$found.ToArray()
}

# --- the command line ----------------------------------------------------------------------

# <data>\<command>.log: the command is the first argument ("control status" logs to control.log).
function Get-CliLogPath {
    param([Parameter(Mandatory)][string]$Data, [Parameter(Mandatory)][string[]]$Arguments)
    Join-Path $Data ("$($Arguments[0]).log")
}

# Where the app's own data lives for a run: the app resolves its folders through the shell, and
# the shell may honour the USERPROFILE the app runs with (the temporary profile) or not. Both
# places are looked at, so the check is about what the app did, not about that detail.
function Get-DataRoots {
    param([Parameter(Mandatory)][string]$Profile, [string]$Roaming = $env:APPDATA)
    @($Roaming, (Join-Path $Profile 'AppData\Roaming'))
}

# The first existing <root>\<folder>\<file>, or $null.
function Find-DataFile {
    param([Parameter(Mandatory)][string[]]$Roots, [Parameter(Mandatory)][string]$Folder, [Parameter(Mandatory)][string]$File)
    foreach ($root in $Roots) {
        $path = Join-Path (Join-Path $root $Folder) $File
        if (Test-Path -LiteralPath $path -PathType Leaf) { return $path }
    }
    $null
}

function Merge-Environment {
    param([hashtable[]]$Layers)
    $merged = @{}
    foreach ($layer in $Layers) { foreach ($key in $layer.Keys) { $merged[$key] = [string]$layer[$key] } }
    $merged
}

function Start-AppProcess {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$Arguments = @(), [hashtable]$Environment = @{}, [switch]$Wait)
    $start = @{ FilePath = $Exe; PassThru = $true }
    if ($Arguments.Count) { $start.ArgumentList = $Arguments }
    if ($Environment.Count) { $start.Environment = $Environment }
    if ($Wait) { $start.Wait = $true }
    Start-Process @start
}

# One CLI call: the exit code from the process, the text from the command's log file.
function Invoke-Cli {
    param([Parameter(Mandatory)][string[]]$Arguments, [hashtable]$Environment = @{})
    $logs = @(foreach ($root in Get-DataRoots -Profile $script:P) { Get-CliLogPath -Data (Join-Path $root 'Agent Notch') -Arguments $Arguments })
    foreach ($log in $logs) { Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue }
    $process = Start-AppProcess -Exe $script:AppExe -Arguments $Arguments -Environment $Environment -Wait
    $written = @($logs | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    $text = if ($written) { [string](Get-Content -LiteralPath $written[0] -Raw) } else { '' }
    [pscustomobject]@{ ExitCode = $process.ExitCode; Text = $text; Log = if ($written) { $written[0] } else { $logs -join ' or ' } }
}

function Wait-Until {
    param([Parameter(Mandatory)][scriptblock]$Ready, [Parameter(Mandatory)][int]$Seconds, [Parameter(Mandatory)][string]$What)
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Ready)) {
        if ((Get-Date) -gt $deadline) { throw "timed out after $Seconds s: $What" }
        Start-Sleep -Milliseconds 500
    }
}

# --- processes: started and stopped by handle, never by name -----------------------------------

$script:OwnProcesses = [Collections.Generic.List[System.Diagnostics.Process]]::new()

function Register-OwnProcess {
    param([Parameter(Mandatory)][System.Diagnostics.Process]$Process)
    $script:OwnProcesses.Add($Process)
    $Process
}

function Stop-OwnProcess {
    param([Parameter(Mandatory)][System.Diagnostics.Process]$Process)
    if (-not $Process.HasExited) {
        try { $Process.Kill($true) } catch { <# it ended between the check and the kill #> }
    }
    [void]$Process.WaitForExit(10000)
}

function Stop-OwnProcesses {
    foreach ($process in $script:OwnProcesses) { Stop-OwnProcess -Process $process }
    $script:OwnProcesses.Clear()
}

# --- the temporary Claude setup (P) ----------------------------------------------------------------

# What is copied where. Style: crlf-bom is how Windows editors write a settings file (the
# installer must keep both); plain is written as UTF-8 with LF.
function Get-ProfileLayout {
    @(
        @{ From = 'claude\settings.json';      To = '.claude\settings.json';       Style = 'crlf-bom' }
        @{ From = 'claude-work\settings.json'; To = '.claude-work\settings.json';  Style = 'plain' }
        @{ From = 'claude.json';               To = '.claude.json';                Style = 'plain' }
        @{ From = 'claude-work\claude.json';   To = '.claude-work\.claude.json';   Style = 'plain' }
        @{ From = 'transcript.jsonl';          To = '.claude\projects\C--smoke-work-app\5d1e0a7b-3c21-4f7e-9a0b-1c2d3e4f5a6b.jsonl'; Style = 'plain' }
    )
}

# The fixtures are kept with LF and no BOM (git normalises line endings); the shapes the
# installer must preserve are made here.
function ConvertTo-FixtureBytes {
    param([Parameter(Mandatory)][string]$Text, [Parameter(Mandatory)][ValidateSet('crlf-bom', 'plain')][string]$Style)
    $lf = $Text.Replace("`r`n", "`n")
    $utf8 = [Text.UTF8Encoding]::new($false)
    if ($Style -eq 'plain') { return , $utf8.GetBytes($lf) }
    $body = $utf8.GetBytes($lf.Replace("`n", "`r`n"))
    , ([byte[]](0xEF, 0xBB, 0xBF) + $body)
}

function New-SmokeProfile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$Fixtures,
        [Parameter(Mandatory)][string]$FakeClaude
    )
    if (Test-Path -LiteralPath $Root) { Remove-Item -LiteralPath $Root -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    foreach ($item in Get-ProfileLayout) {
        $source = Join-Path $Fixtures $item.From
        $target = Join-Path $Root $item.To
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
        $text = [IO.File]::ReadAllText($source)
        [IO.File]::WriteAllBytes($target, (ConvertTo-FixtureBytes -Text $text -Style $item.Style))
    }
    # A profile has its AppData folders. The shell resolves them through the USERPROFILE the app
    # runs with and, when they do not exist, answers with nothing: the app then logs to its
    # current folder instead of its data folder (seen on the runner).
    New-Item -ItemType Directory -Force -Path (Join-Path $Root 'AppData\Roaming'), (Join-Path $Root 'AppData\Local') | Out-Null
    # Where Claude Code's installer puts claude.exe; the engine finds it there. The fake only
    # answers --version and the usage probe, and logs what it was asked.
    $bin = Join-Path $Root '.local\bin'
    New-Item -ItemType Directory -Force -Path $bin | Out-Null
    Copy-Item -LiteralPath $FakeClaude -Destination (Join-Path $bin 'claude.exe')
    Get-TreeHash -Root $Root
}

# --- the doctor ------------------------------------------------------------------------------------

# The credential names the doctor's report must never contain.
function Find-CredentialNames {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    foreach ($secret in '.credentials.json', 'claudeAiOauth', 'accessToken') {
        if ($Text.Contains($secret)) { $secret }
    }
}

# The doctor's full line set (DESIGN-WIN §7.5 phase 3): what is missing, one description each.
function Test-DoctorReport {
    param(
        [Parameter(Mandatory)][AllowEmptyString()][string]$Report,
        [Parameter(Mandatory)][string]$Version,
        [ValidateSet('on', 'off')][string]$Updates = 'off',
        [string]$KeyId = ''
    )
    $key = if ($KeyId) { [regex]::Escape($KeyId) } else { '\S+' }
    $updatesLine = if ($Updates -eq 'on') { "^updates: on .*key=$key signed-version=required" } else { '^updates: off \(built from source\)' }
    $expected = [ordered]@{
        'the header'                   = '^Agent Notch doctor v' + [regex]::Escape($Version) + '(\s|$)'
        'the updates line'             = $updatesLine
        'two accounts'                 = '^accounts: 2(\s|$)'
        'hooks: consent=unasked'       = '^hooks: consent=unasked'
        'the deep link is registered'  = '^deep-link: registered'
        'no pipe instance'             = '^pipe: .*no instance running'
        'the elevation line'           = '^elevated: app='
        'the Smart App Control line'   = '^smart-app-control:'
        'the support folder'           = '^support: .*AppData\\Local\\com\.rivantmedia\.agentnotch\\Claude'
    }
    $missing = [Collections.Generic.List[string]]::new()
    foreach ($name in $expected.Keys) {
        if ($Report -notmatch "(?m)$($expected[$name])") { $missing.Add($name) }
    }
    [string[]]$missing.ToArray()
}

# --- the report ------------------------------------------------------------------------------------

function Format-Summary {
    param([Parameter(Mandatory)]$Results, [Parameter(Mandatory)]$Gated)
    $lines = [Collections.Generic.List[string]]::new()
    $lines.Add('### Installer smoke test')
    $lines.Add('')
    $lines.Add('| Phase | Name | Result | Seconds |')
    $lines.Add('|---|---|---|---|')
    foreach ($r in $Results) {
        $lines.Add("| $($r.Number) | $($r.Name) | $($r.Status) | $([math]::Round($r.Seconds, 1)) |")
    }
    $failed = @($Results | Where-Object { $_.Status -eq 'failed' })
    foreach ($r in $failed) { $lines.Add(''); $lines.Add("Phase $($r.Number) failed: $($r.Error)") }
    if ($Gated.Count) {
        $lines.Add('')
        $lines.Add('Not run (waiting for the package named):')
        foreach ($g in $Gated) { $lines.Add("- phase $($g.Number)$(if ($g.Part) { " ($($g.Part))" }): $($g.WaitingFor)") }
    }
    ($lines -join "`n") + "`n"
}

# --- the phase runner ----------------------------------------------------------------------------------

$script:Results = [Collections.Generic.List[object]]::new()
$script:Gated = [Collections.Generic.List[object]]::new()
$script:PhaseLog = $null

function ConvertTo-Slug { param([string]$Text) ($Text.ToLowerInvariant() -replace '[^a-z0-9]+', '-').Trim('-') }

function Write-PhaseLog {
    param([Parameter(ValueFromPipeline)][AllowEmptyString()][string]$Text)
    process {
        Write-Host $Text
        if ($script:PhaseLog) { Add-Content -LiteralPath $script:PhaseLog -Value $Text }
    }
}

# A part of a phase that waits for a package: the warning, and a line in the summary.
function Skip-GatedPart {
    param([Parameter(Mandatory)][string]$Number, [Parameter(Mandatory)][string]$Gate, [string]$Part = '')
    Write-Host (Get-GateWarning -Gates $script:GateSet -Number $Number -Gate $Gate -Part $Part)
    $script:Gated.Add([pscustomobject]@{ Number = $Number; Part = $Part; WaitingFor = $script:GateSet.Why[$Gate] })
}

function Invoke-Phase {
    param([Parameter(Mandatory)]$Phase)
    $record = [pscustomobject]@{ Number = $Phase.Number; Name = $Phase.Name; Status = 'running'; Seconds = 0.0; Error = '' }
    $script:Results.Add($record)
    if ($Phase.ContainsKey('Gate') -and -not (Test-GateOpen -Gates $script:GateSet -Name $Phase.Gate)) {
        $record.Status = 'gated'
        Skip-GatedPart -Number $Phase.Number -Gate $Phase.Gate
        return $true
    }
    Write-Host "::group::Phase $($Phase.Number): $($Phase.Name)"
    $script:PhaseLog = Join-Path $script:ArtifactsDir ("phase-{0}-{1}.log" -f $Phase.Number, (ConvertTo-Slug $Phase.Name))
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        $null = & $Phase.Body
        $record.Status = 'passed'
    } catch {
        $record.Status = 'failed'
        $record.Error = $_.Exception.Message
        Write-PhaseLog "FAILED: $($record.Error)"
        Write-PhaseLog $_.ScriptStackTrace
    } finally {
        $record.Seconds = $watch.Elapsed.TotalSeconds
        $script:PhaseLog = $null
        Write-Host '::endgroup::'
    }
    if ($record.Status -eq 'failed') { Write-Host "::error::smoke phase $($Phase.Number) ($($Phase.Name)) failed: $($record.Error)" }
    $record.Status -eq 'passed'
}

# --- the context (set once per run) --------------------------------------------------------------------

function Initialize-Context {
    param([Parameter(Mandatory)][string]$Installer, [Parameter(Mandatory)][string]$Artifacts, [Parameter(Mandatory)][string]$Bins)
    $temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $script:Installer = (Resolve-Path -LiteralPath $Installer).ProviderPath
    $script:ArtifactsDir = (New-Item -ItemType Directory -Force -Path $Artifacts).FullName
    $script:Bins = (Resolve-Path -LiteralPath $Bins).ProviderPath
    $script:Temp = $temp
    $script:P = Join-Path $temp 'profile'
    $script:InstallDir = Join-Path $env:LOCALAPPDATA 'Agent Notch'
    $script:AppExe = Join-Path $script:InstallDir 'agentnotch.exe'
    $script:HookExe = Join-Path $script:InstallDir 'agentnotch-hook.exe'
    $script:UninstallExe = Join-Path $script:InstallDir 'uninstall.exe'
    $script:UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Agent Notch'
    $script:RunKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    $script:DataRoots = @(Get-DataRoots -Profile $script:P)
    $script:FixtureDir = Join-Path $PSScriptRoot 'smoke\profile'
    $script:BaselineDir = Join-Path $PSScriptRoot '..\agentnotch-ui-tests\baselines'
    $script:FakeClaudeLog = Join-Path $temp 'fake-claude.log'
    $script:UsageFixture = Join-Path $script:FixtureDir 'usage.json'
    $script:KnownProfileBefore = $null
    $script:ProfileHashes = $null
}

# What the app and its commands run with: the temporary profile as the home, no toasts, and the
# fake claude's settings.
function Get-AppEnvironment {
    @{
        USERPROFILE                 = $script:P
        AGENTNOTCH_NO_NOTIFICATIONS = '1'
        FAKE_CLAUDE_LOG             = $script:FakeClaudeLog
        FAKE_CLAUDE_VERSION         = '2.1.282 (Claude Code)'
        FAKE_CLAUDE_USAGE           = $script:UsageFixture
    }
}

function Add-WindowsHelpers {
    if ('AgentNotch.Windows' -as [type]) { return }
    Add-Type -Namespace AgentNotch -Name Windows -MemberDefinition @'
delegate bool EnumProc(System.IntPtr hwnd, System.IntPtr param);
[DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, System.IntPtr param);
[DllImport("user32.dll")] static extern bool IsWindowVisible(System.IntPtr hwnd);
[DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint pid);
[DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern uint GetLongPathNameW(string path, System.Text.StringBuilder buffer, uint size);
// The long spelling of a path (an 8.3 short path is expanded); null when the path does not exist.
public static string LongPath(string path) {
    var buffer = new System.Text.StringBuilder(1024);
    uint n = GetLongPathNameW(path, buffer, (uint)buffer.Capacity);
    return n == 0 || n > buffer.Capacity ? null : buffer.ToString();
}
public static int Visible(uint pid) {
    int count = 0;
    EnumWindows((h, p) => { uint owner; GetWindowThreadProcessId(h, out owner); if (owner == pid && IsWindowVisible(h)) count++; return true; }, System.IntPtr.Zero);
    return count;
}
'@
}

# --- phase 0: preflight --------------------------------------------------------------------------------

# The checks of §6.1 step 2 (the workflow runs them as its own step too; a regression in either
# shows in both).
function Get-PreflightProblems {
    $problems = [Collections.Generic.List[string]]::new()
    if (-not ('AgentNotch.Desktop' -as [type])) {
        Add-Type -Namespace AgentNotch -Name Desktop -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true)] public static extern System.IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
[DllImport("user32.dll")] public static extern bool CloseDesktop(System.IntPtr desktop);
'@
    }
    if (-not [Environment]::UserInteractive) { $problems.Add('the session is not interactive') }
    $desktop = [AgentNotch.Desktop]::OpenInputDesktop(0, $false, 0x0001)
    if ($desktop -eq [IntPtr]::Zero) { $problems.Add('there is no input desktop') } else { [void][AgentNotch.Desktop]::CloseDesktop($desktop) }
    if (-not (Test-Path "$env:ProgramFiles\Git\bin\bash.exe")) { $problems.Add('Git Bash is missing') }
    foreach ($name in 'claude', 'claude.exe', 'claude.cmd') {
        $found = Get-Command $name -ErrorAction SilentlyContinue
        if ($found) { $problems.Add("a real Claude Code is reachable: $($found.Source)") }
    }
    foreach ($path in "$env:APPDATA\npm\claude.cmd", "$env:USERPROFILE\.local\bin\claude.exe") {
        if (Test-Path $path) { $problems.Add("a real Claude Code is installed: $path") }
    }
    if (Get-Process -Name agentnotch, codenotch -ErrorAction SilentlyContinue) {
        $problems.Add('agentnotch.exe or codenotch.exe is already running')
    }
    [string[]]$problems.ToArray()
}

function Invoke-PreflightPhase {
    $problems = @(Get-PreflightProblems)
    if ($problems) { throw ($problems -join '; ') }
    if (-not (Test-Path -LiteralPath $script:Installer -PathType Leaf)) { throw "the installer $script:Installer does not exist" }
    $fake = Join-Path $script:Bins 'fake-claude.exe'
    if (-not (Test-Path -LiteralPath $fake)) { throw "fake-claude.exe is not in $script:Bins (the fork crates' test build makes it)" }

    # The runner's own profile and the app's own data folders: the last phase finds the profile
    # unchanged, so a regression that writes to the real one fails there. The data folders must
    # not exist yet either: this runner has never seen Agent Notch.
    $script:KnownProfileBefore = Get-KnownProfileHashes
    $count = $script:KnownProfileBefore.Count
    Write-PhaseLog "the runner's profile ($(Get-KnownProfileRoot)): $count Claude file(s)"
    if ($count) { throw "the runner's own profile already holds Claude files: $(@($script:KnownProfileBefore.Keys | Select-Object -First 5) -join ', ')" }
    $existing = @(Get-AppDataLocations)
    if ($existing) { throw "the runner already holds Agent Notch data: $($existing -join ', ')" }
    Write-PhaseLog 'no Claude files in the runner profile, no Agent Notch data: as expected'
    if ($env:GITHUB_STEP_SUMMARY) {
        $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        "Smoke test runner: elevated=$elevated" >> $env:GITHUB_STEP_SUMMARY
    }
}

# --- phase 1: install ----------------------------------------------------------------------------------

function Invoke-InstallPhase {
    Start-Process -FilePath $script:Installer -ArgumentList '/S' -Wait
    Wait-Until { (Test-Path $script:AppExe) -and (Test-Path $script:HookExe) -and (Test-Path $script:UninstallExe) } 60 'the installed files'
    Write-PhaseLog "installed into $script:InstallDir"
    $key = Get-ItemProperty $script:UninstallKey
    if ($key.DisplayName -ne 'Agent Notch') { throw "uninstall key DisplayName is '$($key.DisplayName)'" }
    if ($key.DisplayVersion -ne $Version) { throw "uninstall key DisplayVersion is '$($key.DisplayVersion)', expected '$Version'" }
    if ($key.Publisher -ne 'Rivant Media') { throw "uninstall key Publisher is '$($key.Publisher)'" }
    $open = (Get-ItemProperty 'HKCU:\Software\Classes\agentnotch\shell\open\command').'(default)'
    if ($open -ne "`"$script:AppExe`" `"%1`"") { throw "agentnotch: is registered as '$open'" }
    $programs = [Environment]::GetFolderPath('Programs')
    $shortcut = @(Get-ChildItem -LiteralPath $programs -Recurse -Filter 'Agent Notch.lnk' -ErrorAction SilentlyContinue)
    if (-not $shortcut) { throw "no Start-menu shortcut 'Agent Notch.lnk' under $programs" }
    Write-PhaseLog "Start-menu shortcut: $($shortcut[0].FullName)"
}

# --- phase 2: the temporary Claude setup -----------------------------------------------------------------

function Invoke-BuildProfilePhase {
    $script:ProfileHashes = New-SmokeProfile -Root $script:P -Fixtures $script:FixtureDir -FakeClaude (Join-Path $script:Bins 'fake-claude.exe')
    Write-PhaseLog "P = $script:P, $($script:ProfileHashes.Count) file(s):"
    foreach ($pair in $script:ProfileHashes.GetEnumerator()) { Write-PhaseLog "  $($pair.Value)  $($pair.Key)" }
    $settings = [IO.File]::ReadAllBytes((Join-Path $script:P '.claude\settings.json'))
    if ($settings[0] -ne 0xEF -or $settings[1] -ne 0xBB -or $settings[2] -ne 0xBF) { throw 'the default settings.json has no byte order mark' }
    $bare = [regex]::Matches([Text.Encoding]::UTF8.GetString($settings), "(?<!`r)`n").Count
    if ($bare) { throw "the default settings.json has $bare bare LF line ending(s)" }
    if ((Get-FileSha256 (Join-Path $script:P '.local\bin\claude.exe')) -ne (Get-FileSha256 (Join-Path $script:Bins 'fake-claude.exe'))) {
        throw 'P\.local\bin\claude.exe is not the fake'
    }
}

# --- phase 3: the doctor, and a sealed launch -------------------------------------------------------------

function Invoke-DoctorPhase {
    $run = Invoke-Cli -Arguments @('doctor') -Environment (Get-AppEnvironment)
    Set-Content -LiteralPath (Join-Path $script:ArtifactsDir 'doctor.log') -Value $run.Text
    Write-PhaseLog $run.Text
    if ($run.ExitCode -ne 0) { throw "doctor exited with $($run.ExitCode)" }
    if (-not $run.Text.Trim()) {
        # Where did the report go? Ask again with less of the environment, and look for the file.
        Write-PhaseLog "exit code $($run.ExitCode); asking again with no environment of ours, then with USERPROFILE only"
        foreach ($variant in @{}, @{ USERPROFILE = $script:P }) {
            $again = Invoke-Cli -Arguments @('doctor') -Environment $variant
            Write-PhaseLog "  variant [$($variant.Keys -join ',')]: exit $($again.ExitCode), $($again.Text.Length) characters, log $($again.Log)"
        }
        foreach ($root in @($env:APPDATA, $env:LOCALAPPDATA, $script:P, (Get-KnownProfileRoot))) {
            Get-ChildItem -LiteralPath $root -Filter '*.log' -Recurse -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notmatch 'EBWebView|\\Temp\\|\\Microsoft\\' } |
                ForEach-Object { Write-PhaseLog "  found $($_.FullName)" }
        }
        throw "the doctor wrote no $($run.Log)"
    }
    $named = @(Find-CredentialNames -Text $run.Text)
    if ($named) { throw "the doctor's report names $($named -join ', ')" }
    if (Test-GateOpen -Gates $script:GateSet -Name 'engine') {
        $missing = @(Test-DoctorReport -Report $run.Text -Version $Version -Updates $Updates -KeyId $KeyId)
        if ($missing) { throw "the doctor's report lacks: $($missing -join '; ')" }
    } else {
        Skip-GatedPart -Number '3' -Gate 'engine' -Part "the doctor's line set"
    }
}

# The app's own data folders, hashed (all roots). A sealed run must leave them as they were.
function Get-AppDataTrees {
    $trees = [ordered]@{}
    foreach ($root in $script:DataRoots) {
        foreach ($pair in (Get-TreeHash -Root (Join-Path $root 'Agent Notch') -Prefix "$root/").GetEnumerator()) { $trees[$pair.Key] = $pair.Value }
    }
    $trees
}

# A sealed run's own data folder: deleted before and after each run (DESIGN-WIN §7.4).
function Remove-SealedData {
    foreach ($root in $script:DataRoots) { Remove-Item -LiteralPath (Join-Path $root 'Agent Notch Sealed') -Recurse -Force -ErrorAction SilentlyContinue }
}

# A sealed launch (fixture data only: no Claude folder, no network, no child process) stays up
# and shows its notch. While the selftest gate is closed this is what proves a build starts at
# all; once it is open, phase 4 (the self-test) replaces it.
function Invoke-SealedLaunchPhase {
    Remove-SealedData
    $dataBefore = Get-AppDataTrees
    $sealedEnvironment = Merge-Environment @((Get-AppEnvironment), @{ AGENTNOTCH_SAFE_MODE = '1' })
    $app = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $sealedEnvironment)
    Start-Sleep -Seconds 20
    if ($app.HasExited) { throw "the sealed app exited with $($app.ExitCode)" }
    Add-WindowsHelpers
    $visible = [AgentNotch.Windows]::Visible([uint32]$app.Id)
    Stop-OwnProcess -Process $app
    if ($visible -lt 1) { throw 'the sealed app shows no window' }
    $runLogPath = Find-DataFile -Roots $script:DataRoots -Folder 'Agent Notch Sealed' -File 'run.log'
    if (-not $runLogPath) { throw "the sealed run left no run.log under $($script:DataRoots -join ' or ')" }
    Copy-Item -LiteralPath $runLogPath -Destination (Join-Path $script:ArtifactsDir 'sealed-run.log')
    $runLog = Get-Content -LiteralPath $runLogPath -Raw
    if ($runLog -notmatch 'an: hub started \(sealed\)') { throw "the sealed hub did not start: $runLog" }
    $changed = @(Compare-Hashes -Before $dataBefore -After (Get-AppDataTrees))
    if ($changed) { throw "a sealed run wrote to the app's own data folder: $($changed -join ', ')" }
    $changedP = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P -Skip 'AppData/*') -Exclude 'AppData/*')
    if ($changedP) { throw "a sealed run changed P: $($changedP -join ', ')" }
    Remove-SealedData
}

# --- phase 4: the sealed self-test and the snapshots ---------------------------------------------------------

# The edges the self-test must report, in the order WP9 runs them; "floating" has the notch hidden.
$script:SelfTestEdges = @('right', 'left', 'top', 'bottom', 'floating')
# The three pages whose round trip, CSP violations, errors and invariants are reported.
$script:SelfTestPages = @('notch', 'settings', 'agentnotch-panel')
# The three runs: the page scale the report must name, and the value of the app's sealed-only
# AGENTNOTCH_SELF_TEST_SCALE that makes it (none at 100 %). WebView2 takes a page's scale from its
# window and ignores --force-device-scale-factor, so the app sets each page's scale itself, as
# scripts\agentnotch-selftest.ps1 (WP9) asks it to.
$script:SelfTestScales = @(
    @{ Scale = 1.0;  Name = '100';  ScaleSwitch = '' }
    @{ Scale = 1.25; Name = '125';  ScaleSwitch = '1.25' }
    @{ Scale = 1.5;  Name = '150';  ScaleSwitch = '1.5' }
)
# The snapshots every build must produce (WP9's fixed states); a page that exposes more states
# adds names to the manifest, and each of those is checked the same way.
$script:SnapshotStates = @(
    foreach ($edge in 'right', 'left', 'top', 'bottom') { "notch-$edge" }
    foreach ($edge in 'right', 'left', 'top', 'bottom') { "panel-list-$edge" }
    'panel-floating', 'panel-chat', 'settings-claude'
)

# A JSON true and nothing else: a string "true" or the number 1 is not a pass.
function Test-IsTrue { param($Value) $Value -is [bool] -and $Value }

function Test-HashKey {
    param($Table, [string]$Key)
    $Table -is [System.Collections.IDictionary] -and $Table.Contains($Key)
}

# The self-test report (WP9's JSON, read with ConvertFrom-Json -AsHashtable) against what the
# design asks (§7.4): one description per failed check, an empty list when it all holds. The
# report's own failures list is echoed, so a red run says what the app saw.
function Test-SelfTestReport {
    param([Parameter(Mandatory)]$Report, [double]$Scale = 1.0)
    $problems = [Collections.Generic.List[string]]::new()
    if ($Report -isnot [System.Collections.IDictionary]) { return @('the report is not a JSON object') }
    $own = @(if (Test-HashKey $Report 'failures') { @($Report['failures']) } else { @() })
    if (Test-HashKey $Report 'error') { if ($Report['error']) { $problems.Add("the app says: $($Report['error'])") } }
    foreach ($failure in $own) { $problems.Add("report failure: $failure") }
    if (-not (Test-HashKey $Report 'ok') -or -not (Test-IsTrue $Report['ok'])) { $problems.Add('ok is not true') }
    if (-not (Test-HashKey $Report 'failures')) { $problems.Add('the report has no failures list') }
    if ((Test-HashKey $Report 'scale') -and $Report['scale'] -is [ValueType] -and [math]::Abs([double]$Report['scale'] - $Scale) -gt 0.01) {
        $problems.Add("scale is $($Report['scale']), the run asked for $Scale")
    }

    $edges = @(if (Test-HashKey $Report 'edges') { @($Report['edges']) } else { @() })
    foreach ($name in $script:SelfTestEdges) {
        $edge = $edges | Where-Object { $_ -is [System.Collections.IDictionary] -and $_['edge'] -eq $name } | Select-Object -First 1
        if (-not $edge) { $problems.Add("edge ${name}: missing from the report"); continue }
        # With the notch hidden (floating) there is no tail to keep inside a card and nothing to
        # stand above: the report may say so with null (or leave the field out). Anywhere else
        # these must be true.
        $floating = $name -eq 'floating'
        foreach ($check in 'inside_work_area', 'tail_inside_corners', 'topmost', 'above_notch') {
            $value = if (Test-HashKey $edge $check) { $edge[$check] } else { $null }
            if (Test-IsTrue $value) { continue }
            if ($floating -and $check -in 'tail_inside_corners', 'above_notch' -and $null -eq $value) { continue }
            $problems.Add("edge ${name}: $check is $(if ($null -eq $value) { 'missing' } else { $value })")
        }
        $auto = if (Test-HashKey $edge 'auto') { $edge['auto'] } else { $null }
        foreach ($check in 'no_activate', 'gate_shut', 'gate_opens_on_confirmation') {
            $value = if (Test-HashKey $auto $check) { $auto[$check] } else { $null }
            if (-not (Test-IsTrue $value)) { $problems.Add("edge ${name}: auto.$check is $(if ($null -eq $value) { 'missing' } else { $value })") }
        }
        if ((Test-HashKey $edge 'failures') -and @($edge['failures']).Count) {
            $problems.Add("edge ${name}: " + (@($edge['failures']) -join '; '))
        }
    }

    $pages = if (Test-HashKey $Report 'pages') { $Report['pages'] } else { $null }
    foreach ($name in $script:SelfTestPages) {
        $page = if (Test-HashKey $pages $name) { $pages[$name] } else { $null }
        if (-not $page) { $problems.Add("page ${name}: missing from the report"); continue }
        if (-not (Test-HashKey $page 'round_trip') -or -not (Test-IsTrue $page['round_trip'])) { $problems.Add("page ${name}: the an_call round trip did not succeed") }
        foreach ($list in 'csp_violations', 'errors') {
            if (-not (Test-HashKey $page $list)) { $problems.Add("page ${name}: $list is missing"); continue }
            $items = @($page[$list])
            if ($items.Count) { $problems.Add("page ${name}: $($items.Count) in ${list}: " + (($items | ForEach-Object { [string]$_ }) -join ' | ')) }
        }
        $invariants = if (Test-HashKey $page 'invariants') { $page['invariants'] } else { $null }
        if ($invariants -isnot [System.Collections.IDictionary] -or $invariants.Count -eq 0) { $problems.Add("page ${name}: no invariants reported"); continue }
        foreach ($invariant in $invariants.Keys) {
            if (-not (Test-IsTrue $invariants[$invariant])) { $problems.Add("page ${name}: invariant '$invariant' is $($invariants[$invariant])") }
        }
    }
    [string[]]$problems.ToArray()
}

# The snapshot manifest (a JSON array of {name, file, width, height, bytes}) against the files
# beside it: every required state present, every listed file a non-empty PNG.
function Test-SnapshotManifest {
    param([Parameter(Mandatory)][string]$Directory, [string[]]$Required = $script:SnapshotStates)
    $problems = [Collections.Generic.List[string]]::new()
    $manifestPath = Join-Path $Directory 'manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { return @("no manifest.json in $Directory") }
    try { $entries = @(Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -AsHashtable -NoEnumerate) }
    catch { return @("manifest.json is not JSON: $($_.Exception.Message)") }
    if ($entries.Count -eq 1 -and $entries[0] -is [System.Collections.IList]) { $entries = @($entries[0]) }
    $names = [Collections.Generic.List[string]]::new()
    foreach ($entry in $entries) {
        if ($entry -isnot [System.Collections.IDictionary] -or -not (Test-HashKey $entry 'name') -or -not (Test-HashKey $entry 'file')) {
            $problems.Add('a manifest entry has no name or file'); continue
        }
        $names.Add([string]$entry['name'])
        $file = Join-Path $Directory ([string]$entry['file'])
        if ([IO.Path]::GetFileName([string]$entry['file']) -ne [string]$entry['file']) { $problems.Add("$($entry['name']): the file name $($entry['file']) is not a plain name"); continue }
        if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { $problems.Add("$($entry['name']): $($entry['file']) is missing"); continue }
        $bytes = [IO.File]::ReadAllBytes($file)
        if ($bytes.Length -eq 0) { $problems.Add("$($entry['name']): $($entry['file']) is empty"); continue }
        if ($bytes.Length -lt 8 -or $bytes[0] -ne 0x89 -or $bytes[1] -ne 0x50 -or $bytes[2] -ne 0x4E -or $bytes[3] -ne 0x47) { $problems.Add("$($entry['name']): $($entry['file']) is not a PNG") }
    }
    foreach ($state in $Required) {
        if ($state -notin $names) { $problems.Add("the manifest has no '$state' snapshot") }
    }
    [string[]]$problems.ToArray()
}

# The comparison with the baselines, through png-diff.mjs's folder form. Returns the parsed
# results ({name, status, ...}); a node that fails to run is an error, not an empty result.
function Invoke-SnapshotComparison {
    param([Parameter(Mandatory)][string]$Actual, [Parameter(Mandatory)][string]$Baselines, [Parameter(Mandatory)][string]$Diffs)
    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $node) { throw 'node is not on the PATH: the snapshot comparison needs it' }
    $tool = Join-Path $PSScriptRoot 'smoke\png-diff.mjs'
    $json = & $node.Source $tool --actual-dir $Actual --baseline-dir $Baselines --diff-dir $Diffs
    if ($LASTEXITCODE -gt 1) { throw "png-diff.mjs failed with exit code $LASTEXITCODE" }
    @((($json -join "`n") | ConvertFrom-Json -AsHashtable)['results'])
}

# One description per result that fails the comparison; a missing baseline is not one.
function Get-SnapshotProblems {
    param([Parameter(Mandatory)]$Results)
    foreach ($r in $Results) {
        switch ($r['status']) {
            'same' { }
            'no-baseline' { }
            'different' { "$($r['name']): $($r['changed']) of $($r['total']) pixels differ beyond the tolerance (largest channel change $($r['maxDelta']))$(if ($r.Contains('diffFile')) { "; diff image: $($r['diffFile'])" })" }
            'size-mismatch' { "$($r['name']): the capture is $($r['width'])x$($r['height']), the baseline $($r['baselineWidth'])x$($r['baselineHeight'])" }
            default { "$($r['name']): $($r['status'])$(if ($r.Contains('error')) { ": $($r['error'])" })" }
        }
    }
}

# One sealed run of the app to its own exit, under a hard timeout (longer than the app's own
# deadline of 120 s, so a hang shows as the app's report, not as ours).
function Invoke-SealedRun {
    param([hashtable]$Environment, [int]$TimeoutSeconds = 150)
    $sealed = Merge-Environment @((Get-AppEnvironment), @{ AGENTNOTCH_SAFE_MODE = '1' }, $Environment)
    $app = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $sealed)
    $exited = $app.WaitForExit($TimeoutSeconds * 1000)
    if (-not $exited) { Stop-OwnProcess -Process $app }
    [pscustomobject]@{ TimedOut = -not $exited; ExitCode = if ($exited) { $app.ExitCode } else { $null } }
}

function Invoke-SelfTestPhase {
    $reports = Join-Path $script:ArtifactsDir 'selftest'
    New-Item -ItemType Directory -Force -Path $reports | Out-Null
    Remove-SealedData
    $dataBefore = Get-AppDataTrees
    $problems = [Collections.Generic.List[string]]::new()

    foreach ($run in $script:SelfTestScales) {
        Write-PhaseLog "self-test at $($run.Name) %"
        $out = Join-Path $reports "selftest-$($run.Name).json"
        Remove-Item -LiteralPath $out -Force -ErrorAction SilentlyContinue
        Remove-SealedData
        $environment = @{ AGENTNOTCH_PANEL_SELF_TEST = '1'; AGENTNOTCH_SELF_TEST_OUT = $out }
        if ($run.ScaleSwitch) { $environment['AGENTNOTCH_SELF_TEST_SCALE'] = $run.ScaleSwitch }
        $result = Invoke-SealedRun -Environment $environment
        $log = Find-DataFile -Roots $script:DataRoots -Folder 'Agent Notch Sealed' -File 'run.log'
        if ($log) { Copy-Item -LiteralPath $log -Destination (Join-Path $reports "selftest-$($run.Name)-run.log") -Force }
        Remove-SealedData

        $mine = [Collections.Generic.List[string]]::new()
        if ($result.TimedOut) { $mine.Add('the app did not exit within 150 s') }
        elseif ($result.ExitCode -ne 0) { $mine.Add("the app exited with $($result.ExitCode)") }
        if (-not (Test-Path -LiteralPath $out -PathType Leaf)) {
            $mine.Add('the app wrote no report')
        } else {
            try {
                $report = Get-Content -LiteralPath $out -Raw | ConvertFrom-Json -AsHashtable
                foreach ($line in (Test-SelfTestReport -Report $report -Scale $run.Scale)) { $mine.Add($line) }
            } catch { $mine.Add("the report is not JSON: $($_.Exception.Message)") }
        }
        foreach ($line in $mine) {
            Write-PhaseLog "  FAIL ($($run.Name) %): $line"
            $problems.Add("at $($run.Name) %: $line")
        }
        if (-not $mine.Count) { Write-PhaseLog '  passed' }
    }

    # The snapshots (100 %): to a folder, then the baselines.
    Write-PhaseLog 'snapshots'
    $shots = Join-Path $script:ArtifactsDir 'snapshots'
    Remove-Item -LiteralPath $shots -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $shots | Out-Null
    $result = Invoke-SealedRun -Environment @{ AGENTNOTCH_SNAPSHOT_CLAUDE = $shots }
    $log = Find-DataFile -Roots $script:DataRoots -Folder 'Agent Notch Sealed' -File 'run.log'
    if ($log) { Copy-Item -LiteralPath $log -Destination (Join-Path $reports 'snapshots-run.log') -Force }
    Remove-SealedData
    if ($result.TimedOut) { $problems.Add('snapshots: the app did not exit within 150 s') }
    elseif ($result.ExitCode -ne 0) { $problems.Add("snapshots: the app exited with $($result.ExitCode)") }
    foreach ($line in (Test-SnapshotManifest -Directory $shots)) { $problems.Add("snapshots: $line") }
    if (-not $problems.Count) {
        $diffs = Join-Path $script:ArtifactsDir 'snapshot-diffs'
        $results = @(Invoke-SnapshotComparison -Actual $shots -Baselines $script:BaselineDir -Diffs $diffs)
        foreach ($r in $results) { Write-PhaseLog "  $($r['name']): $($r['status'])" }
        $noBaseline = @($results | Where-Object { $_['status'] -eq 'no-baseline' } | ForEach-Object { $_['name'] })
        if ($noBaseline) {
            Write-Host "::warning::no baseline yet for $($noBaseline.Count) snapshot(s) in windows/agentnotch-ui-tests/baselines: $($noBaseline -join ', ')"
            Write-PhaseLog "no baseline for: $($noBaseline -join ', ')"
        }
        foreach ($line in (Get-SnapshotProblems -Results $results)) { $problems.Add("snapshots: $line") }
    }

    $changed = @(Compare-Hashes -Before $dataBefore -After (Get-AppDataTrees))
    if ($changed) { $problems.Add("a sealed run wrote to the app's own data folder: $($changed -join ', ')") }
    $changedP = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P -Skip 'AppData/*') -Exclude 'AppData/*')
    if ($changedP) { $problems.Add("a sealed run changed P: $($changedP -join ', ')") }
    foreach ($root in $script:DataRoots) {
        if (Test-Path -LiteralPath (Join-Path $root 'Agent Notch Sealed')) { $problems.Add("Agent Notch Sealed is still there under $root") }
    }
    if ($problems.Count) { throw ("the sealed self-test failed:`n  " + ($problems -join "`n  ")) }
}

# --- phases 5-9: the real app, before consent, the deep link, Turn on, every hook entry, Turn off --------------------

# Every selector and every page command the UI-driven phases use, in ONE table: the pages
# (windows/codenotch/ui/agentnotch/*.js, WP10) own these names, so when a page changes, this is
# the only place to follow it. Page names are the ones cdp.mjs knows (by the file they serve).
$script:Ui = @{
    NotchPage       = 'notch'
    SettingsPage    = 'settings'
    PanelPage       = 'agentnotch-panel'
    # A Tauri command, called from the notch page the way the notch itself calls it.
    OpenSettings    = 'open_settings'
    # Settings > Claude Code: the consent card's emphasised button, and the hooks switch (a
    # button with role=switch; data-an-on is "1" while it is on).
    ConsentTurnOn   = '[data-an-action="consent-on"]'
    HooksSwitch     = '[data-an-action="hooks-enabled"]'
    HooksSwitchOn   = '[data-an-action="hooks-enabled"][data-an-on="1"]'
    # The words Settings shows under a folder whose status line was left alone.
    StatusLineAlone = 'Status line left alone'
    # An answering button of the panel (list row or chat bar) for one session; {0} = the
    # button's data-an-arg (allow, always, deny, option:<n>, approve, keep), {1} = session id.
    # One that was already answered stays on screen marked an-answered: never picked.
    Answer          = '[data-an-action="answer"][data-an-arg="{0}"][data-an-session="{1}"]:not(.an-answered)'
    # The list row's way into the chat ("Review plan"); {0} = session id.
    OpenChat        = '[data-an-action="open-chat"][data-an-arg="{0}"]'
    Back            = '[data-an-action="back"]'
    # An account's "Show folders" link in Settings ({0} = the account's identity id); its folder
    # rows, and their status line notes, are rendered only once it is open.
    ShowFolders     = '[data-an-action="folders"][data-an-arg="{0}"]'
    ShowFoldersText = 'Show folders'
}
# The panel's AnswerGate arms a button 0.35 s after it is on screen; clicking sooner does
# nothing, so the script waits longer than that before a click (and for the page to say armed).
$script:AnswerGateSeconds = 0.35

# --- helpers: node, the DevTools driver, the hook runner ----------------------------------------------------------

function Get-NodePath {
    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $node) { throw 'node is not on the PATH: the UI-driven phases need it' }
    $node.Source
}

# A node script started with its arguments passed exactly (no shell, no quoting to get wrong),
# its output collected while it runs. Stop-OwnProcess ends it by handle.
function Start-NodeScript {
    param([Parameter(Mandatory)][string]$Script, [string[]]$Arguments = @())
    $psi = [Diagnostics.ProcessStartInfo]::new((Get-NodePath))
    $psi.ArgumentList.Add($Script)
    foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($psi)
    [void](Register-OwnProcess $process)
    [pscustomobject]@{ Process = $process; Stdout = $process.StandardOutput.ReadToEndAsync(); Stderr = $process.StandardError.ReadToEndAsync() }
}

function Wait-NodeScript {
    param([Parameter(Mandatory)]$Run, [int]$TimeoutSeconds = 60)
    if (-not $Run.Process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-OwnProcess -Process $Run.Process
        throw "node did not finish within $TimeoutSeconds s"
    }
    $Run.Process.WaitForExit()
    [pscustomobject]@{ ExitCode = $Run.Process.ExitCode; Stdout = [string]$Run.Stdout.Result; Stderr = [string]$Run.Stderr.Result }
}

# The last line of cdp.mjs's output is its answer: {"ok":true,"result":…} or {"ok":false,"error":…}.
function ConvertFrom-CdpOutput {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    $lines = @($Text -split "`r?`n" | Where-Object { $_.Trim() })
    if (-not $lines.Count) { throw 'cdp.mjs printed nothing' }
    try { $answer = $lines[-1] | ConvertFrom-Json -AsHashtable } catch { throw "cdp.mjs printed something that is not JSON: $($lines[-1])" }
    if ($answer -isnot [System.Collections.IDictionary] -or -not $answer.ContainsKey('ok')) { throw "cdp.mjs answered $($lines[-1])" }
    if (-not $answer['ok']) { throw "cdp.mjs: $($answer['error'])" }
    $answer['result']
}

# One cdp.mjs call against the running app: eval | invoke | click | wait | errors.
function Invoke-Cdp {
    param([Parameter(Mandatory)][string[]]$Arguments, [int]$TimeoutSeconds = 60)
    $tool = Join-Path $PSScriptRoot 'smoke\cdp.mjs'
    $run = Wait-NodeScript -Run (Start-NodeScript -Script $tool -Arguments (@('--port', [string]$script:CdpPort) + $Arguments)) -TimeoutSeconds $TimeoutSeconds
    try { ConvertFrom-CdpOutput -Text $run.Stdout } catch {
        if ($_.Exception.Message -match 'fetch failed') { Write-CdpDiagnostics }
        throw "$($_.Exception.Message) (cdp $($Arguments -join ' '); stderr: $($run.Stderr.Trim()))"
    }
}

# When the DevTools port does not answer: what the WebView2 browser processes were started with,
# the ports they listen on, and the port file WebView2 writes into its user data folder. Once.
$script:CdpDiagnosed = $false
function Write-CdpDiagnostics {
    if ($script:CdpDiagnosed) { return }
    $script:CdpDiagnosed = $true
    try {
        Write-PhaseLog "DevTools port $($script:CdpPort) does not answer; the WebView2 processes:"
        $browsers = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" -ErrorAction SilentlyContinue | Where-Object { $_.CommandLine -notmatch '--type=' })
        foreach ($b in $browsers) {
            Write-PhaseLog "  pid $($b.ProcessId) (parent $($b.ParentProcessId)): $($b.CommandLine)"
            foreach ($c in @(Get-NetTCPConnection -OwningProcess $b.ProcessId -State Listen -ErrorAction SilentlyContinue)) { Write-PhaseLog "    listens on $($c.LocalAddress):$($c.LocalPort)" }
        }
        if (-not $browsers) { Write-PhaseLog '  none' }
        foreach ($key in $script:WebView2PolicyKeys) {
            $value = (Get-ItemProperty -LiteralPath $key -Name $script:WebView2PolicyValue -ErrorAction SilentlyContinue).$($script:WebView2PolicyValue)
            Write-PhaseLog "  override ${key}: $(if ($value) { $value } else { '(none)' })"
        }
        foreach ($file in @(Get-ChildItem -LiteralPath (Join-Path $script:P 'AppData\Local') -Recurse -Force -Filter 'DevToolsActivePort' -ErrorAction SilentlyContinue)) {
            Write-PhaseLog "  $($file.FullName): $((Get-Content -LiteralPath $file.FullName -Raw -ErrorAction SilentlyContinue) -replace '\s+', ' ')"
        }
    } catch { Write-PhaseLog "  (diagnostics failed: $($_.Exception.Message))" }
}

# window.__TAURI__.core.invoke(command, args) in the named page, as the page itself calls it.
function Invoke-CdpInvoke {
    param([Parameter(Mandatory)][string]$Page, [Parameter(Mandatory)][string]$Command, $Arguments = @{})
    Invoke-Cdp -Arguments @('invoke', $Page, $Command, (ConvertTo-Json $Arguments -Depth 10 -Compress))
}

# The engine's own call (`an_call`), e.g. panel_open.
function Invoke-CdpCall {
    param([Parameter(Mandatory)][string]$Page, [Parameter(Mandatory)][string]$Method, $Arguments = @{})
    Invoke-CdpInvoke -Page $Page -Command 'an_call' -Arguments @{ method = $Method; args = $Arguments }
}

function ConvertTo-JsString { param([Parameter(Mandatory)][string]$Text) ConvertTo-Json $Text -Compress }

# A real click on the first element the selector matches (waits for it to exist first).
function Invoke-CdpClick {
    param([Parameter(Mandatory)][string]$Page, [Parameter(Mandatory)][string]$Selector, [int]$WaitSeconds = 20)
    $literal = ConvertTo-JsString $Selector
    [void](Invoke-Cdp -Arguments @('wait', $Page, "!!document.querySelector($literal)", [string]($WaitSeconds * 1000)) -TimeoutSeconds ($WaitSeconds + 15))
    [void](Invoke-Cdp -Arguments @('click', $Page, $Selector))
}

# The page's expression for "this button is on screen, armed, and not disabled": the panel marks
# a button an-unarmed (and aria-disabled) until its AnswerGate has opened.
function Get-ArmedExpression {
    param([Parameter(Mandatory)][string]$Selector)
    $literal = ConvertTo-JsString $Selector
    "(() => { const b = document.querySelector($literal); return !!b && !b.disabled && !b.classList.contains('an-unarmed') && b.getAttribute('aria-disabled') !== 'true'; })()"
}

# Waits for the answering button, lets the AnswerGate open, then clicks it. Never sooner.
function Invoke-CdpAnswerClick {
    param([Parameter(Mandatory)][string]$Page, [Parameter(Mandatory)][string]$Selector, [int]$WaitSeconds = 30)
    $literal = ConvertTo-JsString $Selector
    [void](Invoke-Cdp -Arguments @('wait', $Page, "!!document.querySelector($literal)", [string]($WaitSeconds * 1000)) -TimeoutSeconds ($WaitSeconds + 15))
    Start-Sleep -Milliseconds ([int](($script:AnswerGateSeconds + 0.15) * 1000))
    [void](Invoke-Cdp -Arguments @('wait', $Page, (Get-ArmedExpression -Selector $Selector), '10000') -TimeoutSeconds 25)
    [void](Invoke-Cdp -Arguments @('click', $Page, $Selector))
}

function Get-FreeTcpPort {
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try { $listener.LocalEndpoint.Port } finally { $listener.Stop() }
}

# The DevTools port for the app's WebView2 browser. On the runner the browser ignores
# WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS (run 37300672648: the app, elevated there, started it with
# wry's switches only), so the same switches also go into WebView2's per-app registry override,
# <root>\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments, value agentnotch.exe:
# HKLM, which an elevated process reads, and HKCU. wry's own switches are repeated, so the browser
# runs as it does for a user whether the override replaces them or is appended to them.
$script:WebView2PolicyKeys = @(
    'HKLM:\SOFTWARE\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments',
    'HKCU:\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments')
$script:WebView2PolicyValue = 'agentnotch.exe'
$script:WebView2PolicyCreated = [Collections.Generic.List[string]]::new()
$script:WebView2PolicySet = [Collections.Generic.List[string]]::new()

function Get-DevToolsBrowserArguments {
    param([Parameter(Mandatory)][int]$Port)
    "--remote-debugging-port=$Port --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"
}

# Sets the override for $Port and returns the environment layer for the app; what it did goes to $Log.
function Get-DevToolsEnvironment {
    param([Parameter(Mandatory)][int]$Port, [scriptblock]$Log = { param($line) Write-Host $line })
    $arguments = Get-DevToolsBrowserArguments -Port $Port
    foreach ($key in $script:WebView2PolicyKeys) {
        try {
            if (-not (Test-Path -LiteralPath $key)) {
                New-Item -Path $key -Force | Out-Null
                if (-not $script:WebView2PolicyCreated.Contains($key)) { $script:WebView2PolicyCreated.Add($key) }
            }
            New-ItemProperty -LiteralPath $key -Name $script:WebView2PolicyValue -Value $arguments -PropertyType String -Force | Out-Null
            if (-not $script:WebView2PolicySet.Contains($key)) { $script:WebView2PolicySet.Add($key) }
            & $Log "WebView2 override set: $key\$($script:WebView2PolicyValue) = $arguments"
        } catch { & $Log "WebView2 override not set in ${key}: $($_.Exception.Message)" }
    }
    @{ WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port" }
}

# Takes the overrides away again: the value, and the key when this run made it.
function Clear-DevToolsOverride {
    foreach ($key in @($script:WebView2PolicySet)) {
        Remove-ItemProperty -LiteralPath $key -Name $script:WebView2PolicyValue -Force -ErrorAction SilentlyContinue
    }
    foreach ($key in @($script:WebView2PolicyCreated)) {
        $left = Get-Item -LiteralPath $key -ErrorAction SilentlyContinue
        if ($left -and -not $left.ValueCount -and -not $left.SubKeyCount) { Remove-Item -LiteralPath $key -Force -ErrorAction SilentlyContinue }
    }
    $script:WebView2PolicySet.Clear()
    $script:WebView2PolicyCreated.Clear()
}

# --- helpers: what the app prints and writes ---------------------------------------------------------------------

# `control status` is lines of "key: value"; a "key=value" line is read the same way.
function ConvertFrom-ControlStatus {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    $status = [ordered]@{}
    foreach ($line in ($Text -split "`r?`n")) {
        if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*[:=]\s*(.*?)\s*$') { $status[$Matches[1]] = $Matches[2] }
    }
    $status
}

# What differs from the expectation (key -> value), one line each. A key the report lacks is named.
function Test-ControlStatus {
    param([Parameter(Mandatory)]$Status, [Parameter(Mandatory)][System.Collections.IDictionary]$Expect)
    foreach ($key in $Expect.Keys) {
        if (-not $Status.Contains($key)) { "no '$key' line"; continue }
        if ($Status[$key] -cne [string]$Expect[$key]) { "$key is '$($Status[$key])', expected '$($Expect[$key])'" }
    }
}

function Get-ControlStatus {
    $run = Invoke-Cli -Arguments @('control', 'status') -Environment (Get-AppEnvironment)
    if ($run.ExitCode -ne 0) { throw "control status exited with $($run.ExitCode): $($run.Text)" }
    ConvertFrom-ControlStatus -Text $run.Text
}

# Polls `control status` until it passes the check or the time is up; throws what it last saw.
# -Expect compares lines with Test-ControlStatus; -Check is a script block for anything else. A
# check made with GetNewClosure runs in a module of its own that sees none of this script's
# functions, so it may use only its captured variables: a comparison goes through -Expect.
function Wait-ControlStatus {
    param([scriptblock]$Check = $null, [System.Collections.IDictionary]$Expect = $null, [Parameter(Mandatory)][int]$Seconds, [Parameter(Mandatory)][string]$What)
    if (-not $Check -and -not $Expect) { throw 'Wait-ControlStatus needs -Check or -Expect' }
    $deadline = (Get-Date).AddSeconds($Seconds)
    $last = $null
    while ($true) {
        $last = $null
        try { $last = Get-ControlStatus } catch { $last = "no status: $($_.Exception.Message)" }
        if ($last -isnot [string]) {
            $problems = @()
            if ($Expect) { $problems += @(Test-ControlStatus -Status $last -Expect $Expect) }
            if ($Check) { $problems += @(& $Check $last) }
            if (-not $problems.Count) { return $last }
            $seen = ($problems -join '; ')
        } else { $seen = $last }
        if ((Get-Date) -gt $deadline) { throw "timed out after $Seconds s: $What ($seen)" }
        Start-Sleep -Milliseconds 700
    }
}

# A protected DACL naming only the allowed principals (the user and SYSTEM), as SIDs.
function Test-PrivateAcl {
    param([Parameter(Mandatory)][bool]$Protected, [Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Sids, [Parameter(Mandatory)][string[]]$Allowed)
    if (-not $Protected) { 'the DACL inherits from the parent (it is not protected)' }
    foreach ($sid in ($Sids | Select-Object -Unique)) {
        if ($sid -notin $Allowed) { "the DACL names $sid" }
    }
    if (-not $Sids) { 'the DACL is empty' }
}

function Get-AclProblems {
    param([Parameter(Mandatory)][string]$Path)
    $acl = Get-Acl -LiteralPath $Path
    $sids = @($acl.Access | ForEach-Object { $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value })
    $me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    Test-PrivateAcl -Protected $acl.AreAccessRulesProtected -Sids $sids -Allowed @($me, 'S-1-5-18')
}

# The app's support folder: <local app data>\com.rivantmedia.agentnotch\Claude, under the real
# local app data or the temporary profile's (see Get-DataRoots).
function Find-SupportDir {
    param([Parameter(Mandatory)][string]$Profile, [string]$Local = $env:LOCALAPPDATA)
    foreach ($root in @($Local, (Join-Path $Profile 'AppData\Local'))) {
        if (-not $root) { continue }
        $path = Join-Path (Join-Path $root 'com.rivantmedia.agentnotch') 'Claude'
        if (Test-Path -LiteralPath $path -PathType Container) { return $path }
    }
    $null
}

# The usage probe's argument list, exactly (DESIGN-WIN §4.6): {"disableAllHooks":true} is ONE element.
$script:ProbeArgv = @('-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose',
    '--no-session-persistence', '--strict-mcp-config', '--settings', '{"disableAllHooks":true}')
# What the engine strips from the child's environment (usage::scrubbed_env). CLAUDE_CONFIG_DIR is
# in that list too, but the probe sets it again for a folder that is not the default one, so
# seeing it is no leak; the others, and anything under CLAUDE_CODE_ or CLAUDE_AGENT_SDK_, are.
$script:ScrubbedNames = @('CLAUDECODE', 'CLAUDE_PID', 'CLAUDE_EFFORT', 'AI_AGENT', 'CLAUDE_SECURESTORAGE_CONFIG_DIR', 'ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN')
# What phase 5 puts into the app's own environment to see the scrub work: names with a value that
# can be nothing real.
$script:ScrubSentinels = @{
    CLAUDECODE                      = 'smoke-sentinel'
    CLAUDE_PID                      = '1'
    CLAUDE_EFFORT                   = 'smoke-sentinel'
    AI_AGENT                        = 'smoke-sentinel'
    CLAUDE_SECURESTORAGE_CONFIG_DIR = 'smoke-sentinel'
    CLAUDE_CODE_ENTRYPOINT          = 'smoke-sentinel'
    CLAUDE_AGENT_SDK_VERSION        = 'smoke-sentinel'
    ANTHROPIC_API_KEY               = 'smoke-sentinel'
    ANTHROPIC_AUTH_TOKEN            = 'smoke-sentinel'
}

function Test-ScrubbedName {
    param([Parameter(Mandatory)][string]$Name)
    $upper = $Name.ToUpperInvariant()
    ($upper -in $script:ScrubbedNames) -or $upper.StartsWith('CLAUDE_CODE_') -or $upper.StartsWith('CLAUDE_AGENT_SDK_')
}

function ConvertTo-ComparablePath {
    param([AllowEmptyString()][string]$Path)
    $p = $Path
    if ($p.StartsWith('\\?\')) { $p = $p.Substring(4) }
    $p.Replace('/', '\').TrimEnd('\').ToLowerInvariant()
}

# What the fake claude's log (one JSON line per run: argv, cwd, the names of CLAUDE*/ANTHROPIC*
# variables) says about the app. Before consent the app may run `claude --version` and the
# usage probe, nothing else; a probe has the exact argument list, runs in <support>\usage-probe
# and sees none of the scrubbed variables. Returns the problems; an empty log has none (the
# readings check says whether the probe ran at all).
function Test-FakeClaudeLog {
    param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Lines, [Parameter(Mandatory)][string]$ProbeDir)
    $number = 0
    foreach ($line in $Lines) {
        $number++
        if (-not $line.Trim()) { continue }
        try { $run = $line | ConvertFrom-Json -AsHashtable } catch { "log line ${number} is not JSON"; continue }
        $argv = @($run['argv'] | ForEach-Object { [string]$_ })
        $names = @($run['env'] | ForEach-Object { [string]$_ })
        $leaked = @($names | Where-Object { Test-ScrubbedName $_ })
        if ($leaked) { "run ${number} saw the scrubbed variable(s) $($leaked -join ', ')" }
        if ($argv.Count -eq 1 -and $argv[0] -eq '--version') { continue }
        if (($argv -join "`n") -cne ($script:ProbeArgv -join "`n")) {
            "run ${number} is neither the probe nor --version: $($argv -join ' ')"
            continue
        }
        if ((ConvertTo-ComparablePath ([string]$run['cwd'])) -ne (ConvertTo-ComparablePath $ProbeDir)) {
            "run ${number} (the probe) ran in '$($run['cwd'])', not in $ProbeDir"
        }
    }
}

# --- helpers: settings.json as the installer leaves it ------------------------------------------------------------

function Get-Prop {
    param($Object, [Parameter(Mandatory)][string]$Name)
    if ($null -eq $Object) { return $null }
    $property = $Object.PSObject.Properties[$Name]
    if ($property) { $property.Value } else { $null }
}

function ConvertFrom-SettingsBytes {
    param([Parameter(Mandatory)][byte[]]$Bytes)
    $start = if ($Bytes.Length -ge 3 -and $Bytes[0] -eq 0xEF -and $Bytes[1] -eq 0xBB -and $Bytes[2] -eq 0xBF) { 3 } else { 0 }
    [Text.UTF8Encoding]::new($false).GetString($Bytes, $start, $Bytes.Length - $start) | ConvertFrom-Json -Depth 50
}

function Read-Settings { param([Parameter(Mandatory)][string]$Path) ConvertFrom-SettingsBytes -Bytes ([IO.File]::ReadAllBytes($Path)) }

# CRLF and a byte order mark are what Windows editors write; an edit must keep both.
# The file kept the shape it had: a BOM if (and only if) $Original had one, CRLF line endings in a
# CRLF file, LF in an LF file. Without $Original the file must be BOM and CRLF (.claude's shape).
function Test-SettingsStyle {
    param([Parameter(Mandatory)][byte[]]$Bytes, [byte[]]$Original = $null)
    $hasBom = { param([byte[]]$b) $b.Length -ge 3 -and $b[0] -eq 0xEF -and $b[1] -eq 0xBB -and $b[2] -eq 0xBF }
    $bareCount = { param([byte[]]$b) [regex]::Matches([Text.Encoding]::UTF8.GetString($b), "(?<!`r)`n").Count }
    $crlfCount = { param([byte[]]$b) [regex]::Matches([Text.Encoding]::UTF8.GetString($b), "`r`n").Count }
    $wantBom = if ($null -eq $Original) { $true } else { & $hasBom $Original }
    $wantCrlf = if ($null -eq $Original) { $true } else { (& $crlfCount $Original) -gt (& $bareCount $Original) }
    $bom = & $hasBom $Bytes
    if ($wantBom -and -not $bom) { 'the byte order mark is gone' }
    if ($bom -and -not $wantBom) { 'a byte order mark was added' }
    if ($wantCrlf) {
        $bare = & $bareCount $Bytes
        if ($bare) { "$bare bare LF line ending(s): CRLF was not kept" }
    } else {
        $crlf = & $crlfCount $Bytes
        if ($crlf) { "$crlf CRLF line ending(s) in a file written with LF" }
    }
}

function ConvertTo-CompactJson { param($Value) ConvertTo-Json $Value -Depth 50 -Compress }

# Every top-level key but the ones the installer edits, unchanged (compared as JSON).
function Test-OtherKeysUnchanged {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After, [string[]]$Edited = @('hooks', 'statusLine'))
    foreach ($property in $Before.PSObject.Properties) {
        if ($property.Name -in $Edited) { continue }
        $now = $After.PSObject.Properties[$property.Name]
        if (-not $now) { "key '$($property.Name)' is gone"; continue }
        if ((ConvertTo-CompactJson $property.Value) -cne (ConvertTo-CompactJson $now.Value)) { "key '$($property.Name)' changed" }
    }
    foreach ($property in $After.PSObject.Properties) {
        if ($property.Name -notin $Edited -and -not $Before.PSObject.Properties[$property.Name]) { "key '$($property.Name)' was added" }
    }
}

# Every group of every event the user had is still there after the install (as JSON).
function Test-ForeignHooksKept {
    param([Parameter(Mandatory)]$Before, [Parameter(Mandatory)]$After)
    $was = Get-Prop $Before 'hooks'
    $now = Get-Prop $After 'hooks'
    if ($null -eq $was) { return }
    foreach ($event in $was.PSObject.Properties) {
        $kept = @(@(Get-Prop $now $event.Name) | ForEach-Object { if ($null -ne $_) { ConvertTo-CompactJson $_ } })
        foreach ($group in @($event.Value)) {
            if ((ConvertTo-CompactJson $group) -notin $kept) { "a $($event.Name) hook entry of the user's is gone" }
        }
    }
}

# Every command entry of the file, in order: Event, GroupIndex, Matcher, Command, Args (or $null), Timeout.
function Get-HookEntries {
    param([Parameter(Mandatory)]$Settings)
    $hooks = Get-Prop $Settings 'hooks'
    if ($null -eq $hooks) { return }
    foreach ($event in $hooks.PSObject.Properties) {
        $groupIndex = -1
        foreach ($group in @($event.Value)) {
            $groupIndex++
            foreach ($entry in @(Get-Prop $group 'hooks')) {
                if ($null -eq $entry) { continue }
                $arguments = Get-Prop $entry 'args'
                # Assigned, not an if-expression: a one-element array would be unrolled to a string.
                $argumentList = $null
                if ($null -ne $arguments) { $argumentList = [string[]]@($arguments) }
                [pscustomobject]@{
                    Event      = $event.Name
                    GroupIndex = $groupIndex
                    Matcher    = Get-Prop $group 'matcher'
                    Command    = [string](Get-Prop $entry 'command')
                    Args       = $argumentList
                    Timeout    = Get-Prop $entry 'timeout'
                }
            }
        }
    }
}

# Whether a command is ours: a string "<exe> <verb>" (the exe unquoted, / separators, maybe the
# 8.3 form, so it is resolved) or the exec form. Returns the form ('string' | 'exec') or $null.
function Get-OwnForm {
    param(
        [Parameter(Mandatory)]$Entry, [Parameter(Mandatory)][string]$ExpectedExe,
        [string]$Verb = 'hook', [scriptblock]$Resolve = { param($p) $p }
    )
    $same = { param($a, $b) (ConvertTo-ComparablePath $a) -eq (ConvertTo-ComparablePath $b) }
    if ($null -ne $Entry.Args) {
        if ((& $same (& $Resolve $Entry.Command) $ExpectedExe) -and $Entry.Args.Count -ge 1 -and $Entry.Args[0] -eq $Verb) { return 'exec' }
        return $null
    }
    if ($Entry.Command -match '^(?<path>\S+) (?<verb>hook|statusline)$' -and $Matches['verb'] -eq $Verb) {
        if (& $same (& $Resolve $Matches['path']) $ExpectedExe) { return 'string' }
    }
    $null
}

# The events every build registers (HS§3.5's baseline; newer Claude Code versions add more).
$script:BaselineHookEvents = @('UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PermissionRequest', 'Notification', 'Stop', 'SubagentStop', 'SessionStart', 'SessionEnd', 'PreCompact')

# Our hook entries in one folder's settings.json, as the installer must leave them.
function Test-InstalledHooks {
    param(
        [Parameter(Mandatory)]$Settings, [Parameter(Mandatory)][string]$ExpectedExe,
        [Parameter(Mandatory)][bool]$ExecFormAllowed, [scriptblock]$Resolve = { param($p) $p }
    )
    $own = @(Get-HookEntries -Settings $Settings | ForEach-Object {
            $form = Get-OwnForm -Entry $_ -ExpectedExe $ExpectedExe -Resolve $Resolve
            if ($form) { [pscustomobject]@{ Entry = $_; Form = $form } }
        })
    foreach ($event in $script:BaselineHookEvents) {
        if (-not @($own | Where-Object { $_.Entry.Event -eq $event }).Count) { "no hook entry of ours under $event" }
    }
    $commands = @($own | ForEach-Object { "$($_.Entry.Command) $($_.Entry.Args -join ' ')" } | Select-Object -Unique)
    if ($commands.Count -gt 1) { "our entries do not share one command: $($commands -join ' | ')" }
    foreach ($item in $own) {
        if ($item.Form -eq 'exec' -and -not $ExecFormAllowed) { "$($item.Entry.Event): the exec form was written, which the facts file does not allow" }
        if ($item.Form -eq 'exec' -and ($item.Entry.Args -join ' ') -cne 'hook --exec') { "$($item.Entry.Event): exec form args are '$($item.Entry.Args -join ' ')', expected 'hook --exec'" }
    }
    foreach ($item in @($own | Where-Object { $_.Entry.Event -eq 'PermissionRequest' })) {
        if ($item.Entry.Timeout -ne 86400) { "PermissionRequest timeout is '$($item.Entry.Timeout)', expected 86400" }
    }
}

# --- helpers: the facts file ---------------------------------------------------------------------------------------

function ConvertTo-Version { param([string]$Text) $v = $null; if ($Text -and [version]::TryParse($Text, [ref]$v)) { $v } else { $null } }

# The first Claude Code version the exec form is known to work from, as the engine derives it
# from the committed facts file (DESIGN-WIN §6.2): an explicit exec_form_min unless a listed
# version at or above it says the form does not run; else the first checked version after the
# last one not known to run it, but only when the newest checked version does.
function Get-ExecFormMin {
    param([Parameter(Mandatory)]$Facts)
    $listed = @(@(Get-Prop $Facts 'versions') | ForEach-Object {
            $v = ConvertTo-Version ([string](Get-Prop $_ 'version'))
            if ($v) { [pscustomobject]@{ Version = $v; Exec = Get-Prop $_ 'hook_exec_form' } }
        } | Sort-Object Version)
    $explicit = ConvertTo-Version ([string](Get-Prop $Facts 'exec_form_min'))
    if ($explicit) {
        $contradicted = @($listed | Where-Object { $_.Version -ge $explicit -and $_.Exec -eq $false })
        if (-not $contradicted.Count) { return $explicit }
    }
    if (-not $listed.Count -or $listed[-1].Exec -ne $true) { return $null }
    $lastNo = -1
    for ($i = 0; $i -lt $listed.Count; $i++) { if ($listed[$i].Exec -ne $true) { $lastNo = $i } }
    $listed[$lastNo + 1].Version
}

function Test-ExecFormAllowed {
    param([Parameter(Mandatory)]$Facts, [Parameter(Mandatory)][string]$ClaudeVersion)
    $min = Get-ExecFormMin -Facts $Facts
    $have = ConvertTo-Version $ClaudeVersion
    [bool]($min -and $have -and $have -ge $min)
}

# --- helpers: the permission answers ------------------------------------------------------------------------------

# One row per answer the panel can give. Stdin is what Claude Code writes (the proto crate's
# fixtures), Expected the exact bytes the hook must print for that answer: the .stdout files of
# agentnotch-proto, which the Mac hook script made. Never taken from the app under test.
# Click is the data-an-arg of the button; Chat = the answer is given from the chat screen.
function Get-PermissionCases {
    @(
        @{ Name = 'allow';         Stdin = 'permission_request_bash';     Expected = 'allow';        Click = 'allow';    Chat = $false }
        @{ Name = 'always allow';  Stdin = 'permission_request_bash';     Expected = 'always';       Click = 'always';   Chat = $false }
        @{ Name = 'deny';          Stdin = 'permission_request_bash';     Expected = 'deny';         Click = 'deny';     Chat = $false }
        @{ Name = 'question chip'; Stdin = 'permission_request_question'; Expected = 'question';     Click = 'option:0'; Chat = $false }
        @{ Name = 'approve plan';  Stdin = 'permission_request_plan';     Expected = 'plan';         Click = 'approve';  Chat = $false }
        @{ Name = 'keep planning'; Stdin = 'permission_request_plan';     Expected = 'keep_planning'; Click = 'keep';     Chat = $true }
    )
}

function Get-ProtoFixtureDir {
    Join-Path (Split-Path -Parent $PSScriptRoot) 'agentnotch-proto\tests\fixtures'
}

# The hook printed HS§1.7's answer. Byte for byte, except that the keys of a permission suggestion
# echoed in updatedPermissions may come in another order: the hook prints the order the app's
# frame carries, and neither the Mac (JSONEncoder of [AnyCodable]) nor the engine (serde_json, whose
# frames' key order is irrelevant, DESIGN-WIN §1.4) keeps the order Claude Code sent. So the same
# length and the same JSON once every object's keys are sorted, and nothing else differs.
function Test-PermissionBytes {
    param([Parameter(Mandatory)][AllowEmptyCollection()][byte[]]$Got, [Parameter(Mandatory)][byte[]]$Want)
    if ([Convert]::ToBase64String($Got) -ceq [Convert]::ToBase64String($Want)) { return $true }
    if ($Got.Length -ne $Want.Length) { return $false }
    $utf8 = [Text.UTF8Encoding]::new($false, $true)
    try { $gotJson = $utf8.GetString($Got) | ConvertFrom-Json -AsHashtable -Depth 50; $wantJson = $utf8.GetString($Want) | ConvertFrom-Json -AsHashtable -Depth 50 } catch { return $false }
    $decision = { param($j) $j['hookSpecificOutput']['decision'] }
    if (-not ((& $decision $gotJson) -is [System.Collections.IDictionary]) -or -not (& $decision $wantJson).Contains('updatedPermissions')) { return $false }
    # Only updatedPermissions may differ in order: everything else is compared as it was printed.
    $outside = { param($j) $copy = ConvertFrom-Json (ConvertTo-Json $j -Depth 50 -Compress) -AsHashtable -Depth 50; $copy['hookSpecificOutput']['decision'].Remove('updatedPermissions'); ConvertTo-Json $copy -Depth 50 -Compress }
    if ((& $outside $gotJson) -cne (& $outside $wantJson)) { return $false }
    if ((@((& $decision $gotJson).Keys) -join ',') -cne (@((& $decision $wantJson).Keys) -join ',')) { return $false }
    (ConvertTo-SortedJson (& $decision $gotJson)['updatedPermissions']) -ceq (ConvertTo-SortedJson (& $decision $wantJson)['updatedPermissions'])
}

# Compact JSON with every object's keys sorted (ordinal), so two values compare by content.
function ConvertTo-SortedJson {
    param($Value)
    if ($null -eq $Value) { return 'null' }
    if ($Value -is [System.Collections.IDictionary]) {
        [string[]]$keys = @($Value.Keys | ForEach-Object { [string]$_ })
        [Array]::Sort($keys, [StringComparer]::Ordinal)
        $parts = foreach ($key in $keys) {
            (ConvertTo-Json ([string]$key) -Compress) + ':' + (ConvertTo-SortedJson $Value[$key])
        }
        return '{' + (@($parts) -join ',') + '}'
    }
    if ($Value -is [string]) { return (ConvertTo-Json $Value -Compress) }
    if ($Value -is [System.Collections.IEnumerable]) { return '[' + (@(foreach ($item in $Value) { ConvertTo-SortedJson $item }) -join ',') + ']' }
    ConvertTo-Json $Value -Compress
}

function Get-ExpectedPermissionBytes {
    param([Parameter(Mandatory)][string]$Name, [string]$Fixtures = (Get-ProtoFixtureDir))
    [IO.File]::ReadAllBytes((Join-Path $Fixtures "v1-responses\$Name.stdout"))
}

# A Claude Code stdin from a proto fixture, with the identity of the smoke session: only the
# session, transcript and folder change, so the tool input (which the hook echoes back) is the
# fixture's own.
function New-HookStdin {
    param(
        [Parameter(Mandatory)][string]$Fixture, [Parameter(Mandatory)][string]$SessionId,
        [Parameter(Mandatory)][string]$Transcript, [Parameter(Mandatory)][string]$Cwd,
        [string]$EventName = '', [string]$ToolUseId = '', [string]$Fixtures = (Get-ProtoFixtureDir)
    )
    $stdin = Get-Content -LiteralPath (Join-Path $Fixtures "stdin\$Fixture.json") -Raw | ConvertFrom-Json -Depth 50
    $stdin.session_id = $SessionId
    $stdin.transcript_path = $Transcript
    $stdin.cwd = $Cwd
    if ($EventName) {
        $stdin.hook_event_name = $EventName
        if ($EventName -eq 'PreToolUse') {
            # A PreToolUse has the request's tool and input, a tool_use_id, and no suggestions.
            $stdin.PSObject.Properties.Remove('permission_suggestions')
            $stdin | Add-Member -NotePropertyName tool_use_id -NotePropertyValue $ToolUseId -Force
        }
    }
    ConvertTo-Json $stdin -Depth 50 -Compress
}

# The wrapped status line's output is the original command's; the shell may change its line end.
function Test-StatusLineOutput {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Stdout, [Parameter(Mandatory)][string]$Expected)
    if ($Stdout.TrimEnd([char[]]@("`r", "`n", ' ')) -cne $Expected) { "printed '$($Stdout.TrimEnd())', expected '$Expected'" }
}

# What turning the hooks off may leave in P beside the original files: our backups, nothing else.
function Test-OnlyBackupsAdded {
    param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Differences)
    foreach ($difference in $Differences) {
        if ($difference -notmatch '^added: \.claude[^/]*/settings\.json\.agentnotch[^/]*\.bak$') { "unexpected: $difference" }
    }
}

# --- phase 5: before consent -----------------------------------------------------------------------------------

$script:CdpPort = 0
$script:LiveApp = $null

function Stop-LiveApp {
    if ($script:LiveApp) { Stop-OwnProcess -Process $script:LiveApp; $script:LiveApp = $null }
}

# The real app's run.log (a copy goes into the artifacts so a red run can be read).
function Get-LiveRunLogPath { Find-DataFile -Roots $script:DataRoots -Folder 'Agent Notch' -File 'run.log' }

function Save-LiveRunLog {
    $path = Get-LiveRunLogPath
    if ($path) { Copy-Item -LiteralPath $path -Destination (Join-Path $script:ArtifactsDir 'live-run.log') -Force -ErrorAction SilentlyContinue }
}

function Get-LiveRunLogText {
    $path = Get-LiveRunLogPath
    if ($path) { [string](Get-Content -LiteralPath $path -Raw) } else { '' }
}

function Invoke-BeforeConsentPhase {
    foreach ($root in $script:DataRoots) { Remove-Item -LiteralPath (Join-Path (Join-Path $root 'Agent Notch') 'run.log') -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $script:FakeClaudeLog -Force -ErrorAction SilentlyContinue
    $script:CdpPort = Get-FreeTcpPort
    $environment = Merge-Environment @((Get-AppEnvironment), $script:ScrubSentinels, (Get-DevToolsEnvironment -Port $script:CdpPort -Log { param($line) Write-PhaseLog $line }))
    $script:LiveApp = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $environment)
    Write-PhaseLog "the real app is running as process $($script:LiveApp.Id), DevTools on port $($script:CdpPort)"
    Start-Sleep -Seconds 30
    if ($script:LiveApp.HasExited) { throw "the app exited with $($script:LiveApp.ExitCode) within 30 s" }
    # run.log and the fake claude's log go into the artifacts whatever happens: a red phase is read from them.
    try {
        $expect = [ordered]@{ transport = 'listening'; accounts = '2'; hook_consent = 'unasked'; readings = '2' }
        $status = Wait-ControlStatus -Seconds 60 -What 'control status before consent' -Expect $expect
        Write-PhaseLog ("control status: " + (($status.GetEnumerator() | ForEach-Object { "$($_.Key): $($_.Value)" }) -join ', '))

        $changed = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P -Skip 'AppData/*') -Exclude 'AppData/*')
        if ($changed) { throw "the app wrote to P before any consent: $($changed -join ', ')" }

        $lines = if (Test-Path -LiteralPath $script:FakeClaudeLog) { @(Get-Content -LiteralPath $script:FakeClaudeLog) } else { @() }
        $support = Find-SupportDir -Profile $script:P
        if (-not $support) { throw "no support folder (com.rivantmedia.agentnotch\Claude) under the local app data or P's" }
        Write-PhaseLog "support folder: $support; fake claude was run $($lines.Count) time(s)"
        $probes = @($lines | Where-Object { $_ -match '--input-format' })
        if (-not $probes.Count) { throw 'the fake claude log shows no usage probe' }
        $logProblems = @(Test-FakeClaudeLog -Lines $lines -ProbeDir (Join-Path $support 'usage-probe'))
        if ($logProblems) { throw "the fake claude log: $($logProblems -join '; ')" }

        $aclProblems = @(Get-AclProblems -Path $support)
        if ($aclProblems) { throw "the support folder: $($aclProblems -join '; ')" }
        # cloud-folder-logins.json is the one exception: since when each folder has been signed in as
        # its account, local only and kept whether or not sync is on (the Mac's CloudSync.tick), so a
        # later backfill knows it. Nothing in it is ever sent.
        $cloud = @(Get-ChildItem -LiteralPath $support -Force -Filter 'cloud-*' -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -ne 'cloud-folder-logins.json' })
        if ($cloud) { throw "the support folder holds $($cloud.Name -join ', ') before sign-in" }
        if ($null -ne (Get-RunValue)) { throw 'a Run value exists although autostart was never turned on' }

        $runLog = Get-LiveRunLogText
        Save-LiveRunLog
        if ($runLog -notmatch '(?m)an: hub started(?! \(sealed\))') { throw 'run.log has no "an: hub started"' }
        if ($runLog -notmatch '(?m)an: pipe listening') { throw 'run.log has no "an: pipe listening"' }
    } finally {
        Save-LiveRunLog
        if (Test-Path -LiteralPath $script:FakeClaudeLog) { Copy-Item -LiteralPath $script:FakeClaudeLog -Destination (Join-Path $script:ArtifactsDir 'fake-claude.log') -Force -ErrorAction SilentlyContinue }
    }
}

# --- phase 6: the deep link with nothing pending ------------------------------------------------------------

function Invoke-DeepLinkPhase {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { throw 'the real app is not running' }
    # The shell starts the registered handler: a second copy that hands the link to the first.
    Start-Process 'agentnotch://auth-callback?code=smoke'
    Wait-Until { (Get-LiveRunLogText) -match 'an: deep link ignored \(no sign-in pending\)' } 5 'run.log to say "an: deep link ignored (no sign-in pending)"'
    Save-LiveRunLog
    # The second copy must be gone again: one agentnotch.exe, ours.
    Wait-Until { @(Get-Process -Name agentnotch -ErrorAction SilentlyContinue).Count -le 1 } 10 'the second copy to exit'
    $running = @(Get-Process -Name agentnotch -ErrorAction SilentlyContinue)
    if ($running.Count -ne 1 -or $running[0].Id -ne $script:LiveApp.Id) { throw "expected only our agentnotch.exe ($($script:LiveApp.Id)), found: $($running.Id -join ', ')" }
}

# --- phase 7: Turn on ------------------------------------------------------------------------------------------

$script:RunFolders = @('.claude', '.claude-work')

function Get-Folder { param([Parameter(Mandatory)][string]$Name) Join-Path $script:P $Name }

function Get-ExpectedHookExe { param([Parameter(Mandatory)][string]$Name) Join-Path (Join-Path (Get-Folder $Name) 'hooks') 'agentnotch-hook.exe' }

# A hook command's path as Windows spells it long (an 8.3 path is resolved; on a volume without
# 8.3 names the path comes back as it was).
function Resolve-LongPath {
    param([Parameter(Mandatory)][string]$Path)
    Add-WindowsHelpers
    $native = $Path.Replace('/', '\')
    $long = [AgentNotch.Windows]::LongPath($native)
    if ($long) { $long } else { $native }
}

function Get-FactsPath { Join-Path (Split-Path -Parent $PSScriptRoot) 'agentnotch-engine\tests\fixtures\claude-code-facts.json' }

function Get-ExecFormAllowedHere {
    $path = Get-FactsPath
    if (-not (Test-Path -LiteralPath $path)) { return $false }
    $facts = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -Depth 50
    # The fake claude says 2.1.282 and no other Claude Code is on this profile.
    Test-ExecFormAllowed -Facts $facts -ClaudeVersion '2.1.282'
}

# What the installer must have done to one run folder; returns the problems.
function Get-InstalledFolderProblems {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][bool]$StatusLineWrapped, [Parameter(Mandatory)][bool]$ExecFormAllowed)
    $folder = Get-Folder $Name
    $path = Join-Path $folder 'settings.json'
    $problems = [Collections.Generic.List[string]]::new()
    $add = { param($lines) foreach ($l in @($lines)) { if ($l) { $problems.Add("${Name}: $l") } } }
    $bytes = [IO.File]::ReadAllBytes($path)
    $originalBytes = [IO.File]::ReadAllBytes((Join-Path $script:OriginalsDir "$Name.settings.json"))
    & $add (Test-SettingsStyle -Bytes $bytes -Original $originalBytes)
    $now = ConvertFrom-SettingsBytes -Bytes $bytes
    $before = ConvertFrom-SettingsBytes -Bytes $originalBytes
    $edited = if ($StatusLineWrapped) { @('hooks', 'statusLine') } else { @('hooks') }
    & $add (Test-OtherKeysUnchanged -Before $before -After $now -Edited $edited)
    & $add (Test-ForeignHooksKept -Before $before -After $now)
    $exe = Get-ExpectedHookExe $Name
    & $add (Test-InstalledHooks -Settings $now -ExpectedExe $exe -ExecFormAllowed $ExecFormAllowed -Resolve { param($p) Resolve-LongPath -Path $p })
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { $problems.Add("${Name}: $exe does not exist") }
    elseif ((Get-FileSha256 $exe) -ne (Get-FileSha256 $script:HookExe)) { $problems.Add("${Name}: the hook copy is not a copy of the installed agentnotch-hook.exe") }

    $backups = @(Get-ChildItem -LiteralPath $folder -Force -Filter 'settings.json.agentnotch*.bak' -ErrorAction SilentlyContinue)
    if (-not $backups) { $problems.Add("${Name}: no settings.json backup beside the file") }
    foreach ($backup in $backups) { & $add (@(Get-AclProblems -Path $backup.FullName) | ForEach-Object { "$($backup.Name): $_" }) }
    $original = $backups | Where-Object { $_.Name -eq 'settings.json.agentnotch.original.bak' }
    if (-not $original) { $problems.Add("${Name}: no settings.json.agentnotch.original.bak") }
    elseif ((Get-FileSha256 $original.FullName) -ne (Get-FileSha256 (Join-Path $script:OriginalsDir "$Name.settings.json"))) { $problems.Add("${Name}: the original backup is not the original file") }

    $previous = Join-Path (Join-Path $folder 'hooks') 'agentnotch-statusline.previous.json'
    if ($StatusLineWrapped) {
        $statusLine = Get-Prop $now 'statusLine'
        $entry = [pscustomobject]@{ Command = [string](Get-Prop $statusLine 'command'); Args = $null }
        if ((Get-Prop $statusLine 'type') -ne 'command') { $problems.Add("${Name}: the status line is not a command") }
        if (-not (Get-OwnForm -Entry $entry -ExpectedExe $exe -Verb 'statusline' -Resolve { param($p) Resolve-LongPath -Path $p })) {
            $problems.Add("${Name}: the status line is not our wrapper: '$($entry.Command)'")
        }
        if (-not (Test-Path -LiteralPath $previous -PathType Leaf)) { $problems.Add("${Name}: no previous.json") }
        elseif ((Get-Content -LiteralPath $previous -Raw) -notmatch 'echo smoke-status') { $problems.Add("${Name}: previous.json does not hold the original status line") }
    } else {
        if ((ConvertTo-CompactJson (Get-Prop $now 'statusLine')) -cne (ConvertTo-CompactJson (Get-Prop $before 'statusLine'))) { $problems.Add("${Name}: the status line was changed") }
        if (Test-Path -LiteralPath $previous) { $problems.Add("${Name}: a previous.json exists for a status line that was left alone") }
    }
    [string[]]$problems.ToArray()
}

# The files of P as phase 2 made them, kept for byte comparisons: .claude\settings.json and
# .claude-work\settings.json are copied aside before anything can change them.
$script:OriginalsDir = $null

function Save-OriginalSettings {
    if (-not $script:OriginalsDir) { $script:OriginalsDir = Join-Path $script:Temp 'originals' }
    New-Item -ItemType Directory -Force -Path $script:OriginalsDir | Out-Null
    foreach ($name in $script:RunFolders) {
        Copy-Item -LiteralPath (Join-Path (Get-Folder $name) 'settings.json') -Destination (Join-Path $script:OriginalsDir "$name.settings.json") -Force
    }
}

function Invoke-TurnOnPhase {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { throw 'the real app is not running' }
    Save-OriginalSettings
    $execAllowed = Get-ExecFormAllowedHere
    Write-PhaseLog "exec form allowed by the facts file: $execAllowed"
    try {
        Invoke-CdpInvoke -Page $script:Ui.NotchPage -Command $script:Ui.OpenSettings | Out-Null
        Invoke-CdpClick -Page $script:Ui.SettingsPage -Selector $script:Ui.ConsentTurnOn
        $originals = @{}
        foreach ($name in $script:RunFolders) { $originals[$name] = Get-FileSha256 (Join-Path $script:OriginalsDir "$name.settings.json") }
        Wait-Until {
            $done = $true
            foreach ($name in $script:RunFolders) {
                $file = Join-Path (Get-Folder $name) 'settings.json'
                if ((Get-FileSha256 $file) -eq $originals[$name] -or -not (Test-Path (Get-ExpectedHookExe $name))) { $done = $false }
            }
            $done -and (Test-Path -LiteralPath (Join-Path (Join-Path (Get-Folder '.claude') 'hooks') 'agentnotch-statusline.previous.json'))
        } 10 'both run folders to hold our entries'
        Start-Sleep -Milliseconds 700   # a pass writes its files one after the other; let it finish

        $problems = [Collections.Generic.List[string]]::new()
        foreach ($line in (Get-InstalledFolderProblems -Name '.claude' -StatusLineWrapped $true -ExecFormAllowed $execAllowed)) { $problems.Add($line) }
        foreach ($line in (Get-InstalledFolderProblems -Name '.claude-work' -StatusLineWrapped $false -ExecFormAllowed $execAllowed)) { $problems.Add($line) }
        # Settings says why the status line of .claude-work was left alone (under its account's folders).
        try { Show-SettingsFolders } catch { $problems.Add("Settings would not show the folders: $($_.Exception.Message)") }
        $note = ConvertTo-JsString $script:Ui.StatusLineAlone
        try { [void](Invoke-Cdp -Arguments @('wait', $script:Ui.SettingsPage, "document.body.innerText.includes($note)", '10000')) }
        catch { $problems.Add("Settings does not show '$($script:Ui.StatusLineAlone)'") }
        if ($problems.Count) { throw ("after Turn on:`n  " + ($problems -join "`n  ")) }
        $form = @(Get-HookEntries -Settings (Read-Settings (Join-Path (Get-Folder '.claude') 'settings.json')) | Where-Object { $_.Event -eq 'PreToolUse' -and $null -ne (Get-OwnForm -Entry $_ -ExpectedExe (Get-ExpectedHookExe '.claude') -Resolve { param($p) Resolve-LongPath -Path $p }) })
        Write-PhaseLog "installed in both folders, written as: $($form[0].Command) $($form[0].Args -join ' ')"
    } finally {
        Save-LiveRunLog
    }
}

# Opens every account's folder list in Settings that is closed (a second click would close it).
function Show-SettingsFolders {
    $label = ConvertTo-JsString $script:Ui.ShowFoldersText
    $expression = "Array.from(document.querySelectorAll('[data-an-action=`"folders`"]')).filter(b => b.textContent.trim() === $label).map(b => b.getAttribute('data-an-arg')).join('\n')"
    $ids = @(([string](Invoke-Cdp -Arguments @('eval', $script:Ui.SettingsPage, $expression))) -split "`n" | Where-Object { $_ })
    foreach ($id in $ids) { [void](Invoke-Cdp -Arguments @('click', $script:Ui.SettingsPage, ($script:Ui.ShowFolders -f $id))) }
    Write-PhaseLog "Settings: opened the folders of $($ids.Count) account(s)"
}

# --- phase 8: every entry as written, then every answer ---------------------------------------------------------------

$script:SmokeSessionId = '5d1e0a7b-3c21-4f7e-9a0b-1c2d3e4f5a6b'

# A process that lives until it is stopped, standing in for Claude Code (the hooks name it as
# CLAUDE_PID). Started and stopped by handle.
function Start-DummyClaude {
    $shell = (Get-Command powershell -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $process = Start-Process -FilePath $shell -ArgumentList '-NoProfile', '-Command', 'Start-Sleep 600' -WindowStyle Hidden -PassThru
    Register-OwnProcess $process
}

function Get-HookEnvironmentArguments {
    param([Parameter(Mandatory)][int]$ClaudePid)
    @('--env', "CLAUDE_PID=$ClaudePid", '--env', "CLAUDE_CONFIG_DIR=$(Get-Folder '.claude')", '--env', 'CLAUDE_CODE_ENTRYPOINT=cli')
}

# run-hook.mjs's runs, parsed. A tool that fails to run is an error here; what the hooks did is
# for the caller to judge.
function ConvertFrom-HookRuns {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    $lines = @($Text -split "`r?`n" | Where-Object { $_.Trim() })
    if (-not $lines.Count) { throw 'run-hook.mjs printed nothing' }
    $answer = $lines[-1] | ConvertFrom-Json -AsHashtable
    if ($answer.ContainsKey('error')) { throw "run-hook.mjs: $($answer['error'])" }
    @($answer['runs'])
}

function Start-HookRun {
    param([Parameter(Mandatory)][string[]]$Arguments)
    Start-NodeScript -Script (Join-Path $PSScriptRoot 'smoke\run-hook.mjs') -Arguments $Arguments
}

function Write-StdinFile {
    param([Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string]$Json)
    $path = Join-Path $script:Temp "smoke-$Name.json"
    [IO.File]::WriteAllText($path, $Json, [Text.UTF8Encoding]::new($false))
    $path
}

function Get-SmokeTranscript { Join-Path (Get-Folder '.claude') "projects\C--smoke-work-app\$($script:SmokeSessionId).jsonl" }

# The shells a string-form entry runs under; the exec form has none to choose.
function Get-ShellsFor { param([Parameter(Mandatory)]$Entry) if ($null -ne $Entry.Args) { @('both') } else { @('bash', 'powershell') } }

function Invoke-EntriesAsWrittenPhase {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { throw 'the real app is not running' }
    $settingsPath = Join-Path (Get-Folder '.claude') 'settings.json'
    $settings = Read-Settings $settingsPath
    $dummy = Start-DummyClaude
    try {
        $environment = Get-HookEnvironmentArguments -ClaudePid $dummy.Id
        Write-PhaseLog "stand-in for Claude Code: process $($dummy.Id)"

        # 1. Every PreToolUse entry of the file as written (the foreign one too), in both shells.
        $before = Get-ControlStatus
        $pre = Write-StdinFile -Name 'pretooluse' -Json (New-HookStdin -Fixture 'permission_request_bash' -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app' -EventName 'PreToolUse' -ToolUseId 'toolu_smoke_pre')
        $entries = @(Get-HookEntries -Settings $settings | Where-Object { $_.Event -eq 'PreToolUse' })
        if (-not $entries.Count) { throw 'P\.claude\settings.json has no PreToolUse entry' }
        $ownPre = @(0..($entries.Count - 1) | Where-Object { Get-OwnForm -Entry $entries[$_] -ExpectedExe (Get-ExpectedHookExe '.claude') -Resolve { param($p) Resolve-LongPath -Path $p } })
        if (-not $ownPre.Count) { throw 'none of the PreToolUse entries is ours' }
        $attempt = 0
        $slow = $null
        while ($true) {
            $attempt++
            $runs = ConvertFrom-HookRuns -Text (Wait-NodeScript -Run (Start-HookRun -Arguments (@('--settings', $settingsPath, '--event', 'PreToolUse', '--stdin', $pre, '--timeout', '15000') + $environment))).Stdout
            $slow = $null
            foreach ($run in $runs) {
                if ($run.ContainsKey('skipped')) { throw "a shell is missing: $($run['reason'])" }
                $label = "$($run['source']) [$($run['shell'])] $($run['command'])"
                if ($run['timedOut'] -or $run['exit'] -ne 0) { throw "$label exited with $($run['exit'])$(if ($run['timedOut']) { ' (timed out)' }): $($run['stderr'])" }
                if ($run['ms'] -ge 2000 -and -not $slow) { $slow = "$label took $($run['ms']) ms" }
                Write-PhaseLog "  $label : exit 0 in $($run['ms']) ms"
            }
            # A first start on a busy runner can be slow (the antivirus scans a new file); a hook that is slow every time fails.
            if (-not $slow -or $attempt -ge 3) { break }
        }
        if ($slow) { throw $slow }
        Wait-ControlStatus -Seconds 10 -What 'control status to count the session' -Check ({ param($s) if ([int]$s['sessions'] -le [int]$before['sessions']) { "sessions is $($s['sessions'])" } }.GetNewClosure()) | Out-Null

        # 2. One PermissionRequest per answer, answered in the panel.
        Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'panel_open' -Arguments @{ route = 'sessions'; reason = 'ring_click' } | Out-Null
        $permissionEntries = @(Get-HookEntries -Settings $settings | Where-Object { $_.Event -eq 'PermissionRequest' })
        $ownPermission = @(0..([math]::Max($permissionEntries.Count, 1) - 1) | Where-Object { $_ -lt $permissionEntries.Count -and (Get-OwnForm -Entry $permissionEntries[$_] -ExpectedExe (Get-ExpectedHookExe '.claude') -Resolve { param($p) Resolve-LongPath -Path $p }) })
        if (-not $ownPermission.Count) { throw 'P\.claude\settings.json has no PermissionRequest entry of ours' }
        $permissionIndex = $ownPermission[0]
        $cases = @(Get-PermissionCases)
        for ($i = 0; $i -lt $cases.Count; $i++) {
            $case = $cases[$i]
            $shells = @(Get-ShellsFor -Entry $permissionEntries[$permissionIndex])
            $shell = $shells[$i % $shells.Count]
            $toolUseId = "toolu_smoke_$($i + 1)"
            $preCase = Write-StdinFile -Name "pre-$($i + 1)" -Json (New-HookStdin -Fixture $case.Stdin -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app' -EventName 'PreToolUse' -ToolUseId $toolUseId)
            $request = Write-StdinFile -Name "request-$($i + 1)" -Json (New-HookStdin -Fixture $case.Stdin -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app')
            $first = ConvertFrom-HookRuns -Text (Wait-NodeScript -Run (Start-HookRun -Arguments (@('--settings', $settingsPath, '--event', 'PreToolUse', '--index', [string]$ownPre[0], '--shell', $shell, '--stdin', $preCase, '--timeout', '15000') + $environment))).Stdout
            foreach ($run in $first) { if ($run['exit'] -ne 0) { throw "the PreToolUse before '$($case.Name)' exited with $($run['exit'])" } }

            $hook = Start-HookRun -Arguments (@('--settings', $settingsPath, '--event', 'PermissionRequest', '--index', [string]$permissionIndex, '--shell', $shell, '--stdin', $request, '--timeout', '90000') + $environment)
            try {
                Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'panel_open' -Arguments @{ route = 'sessions'; reason = 'ring_click' } | Out-Null
                if ($case.Chat) {
                    Invoke-CdpClick -Page $script:Ui.PanelPage -Selector ($script:Ui.OpenChat -f $script:SmokeSessionId)
                }
                Invoke-CdpAnswerClick -Page $script:Ui.PanelPage -Selector ($script:Ui.Answer -f $case.Click, $script:SmokeSessionId)
                $result = Wait-NodeScript -Run $hook -TimeoutSeconds 30
            } catch {
                Stop-OwnProcess -Process $hook.Process
                throw "answer '$($case.Name)' ($shell): $($_.Exception.Message)"
            }
            $run = @(ConvertFrom-HookRuns -Text $result.Stdout)[0]
            if ($run['timedOut'] -or $run['exit'] -ne 0) { throw "answer '$($case.Name)': the hook exited with $($run['exit'])" }
            $got = [Convert]::FromBase64String([string]$run['stdout_b64'])
            $want = Get-ExpectedPermissionBytes -Name $case.Expected
            if (-not (Test-PermissionBytes -Got $got -Want $want)) {
                throw "answer '$($case.Name)' ($shell): the hook printed`n  $([Text.Encoding]::UTF8.GetString($got))`nexpected`n  $([Text.Encoding]::UTF8.GetString($want))"
            }
            Write-PhaseLog "  answer '$($case.Name)' ($shell): printed the expected $($want.Length) bytes, exit 0"
            if ($case.Chat) { try { Invoke-CdpClick -Page $script:Ui.PanelPage -Selector $script:Ui.Back -WaitSeconds 3 } catch { <# the chat closes by itself after an answer #> } }
        }

        # 3. The wrapped status line, as written, with a status JSON.
        $readings = [int](Get-ControlStatus)['readings']
        $statusJson = Write-StdinFile -Name 'status' -Json (New-HookStdin -Fixture 'status_line' -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app')
        $statusRuns = ConvertFrom-HookRuns -Text (Wait-NodeScript -Run (Start-HookRun -Arguments (@('--settings', $settingsPath, '--status-line', '--stdin', $statusJson, '--timeout', '20000') + $environment))).Stdout
        foreach ($run in $statusRuns) {
            if ($run.ContainsKey('skipped')) { throw "a shell is missing: $($run['reason'])" }
            if ($run['exit'] -ne 0) { throw "the status line [$($run['shell'])] exited with $($run['exit'])" }
            $bad = @(Test-StatusLineOutput -Stdout ([string]$run['stdout']) -Expected 'smoke-status')
            if ($bad) { throw "the status line [$($run['shell'])] $($bad -join '; ')" }
            Write-PhaseLog "  status line [$($run['shell'])]: printed the original command's output"
        }
        Wait-ControlStatus -Seconds 10 -What 'the readings to stay or grow' -Check ({ param($s) if ([int]$s['readings'] -lt $readings) { "readings fell from $readings to $($s['readings'])" } }.GetNewClosure()) | Out-Null
    } finally {
        Stop-OwnProcess -Process $dummy
        Save-LiveRunLog
    }
}

# --- phase 9: Turn off -----------------------------------------------------------------------------------------------

function Invoke-TurnOffPhase {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { throw 'the real app is not running' }
    try {
        Invoke-CdpInvoke -Page $script:Ui.NotchPage -Command $script:Ui.OpenSettings | Out-Null
        Invoke-CdpClick -Page $script:Ui.SettingsPage -Selector $script:Ui.HooksSwitchOn
        Wait-Until {
            $same = $true
            foreach ($name in $script:RunFolders) {
                if ((Get-FileSha256 (Join-Path (Get-Folder $name) 'settings.json')) -ne $script:ProfileHashes["$name/settings.json"]) { $same = $false }
            }
            $same
        } 15 'every settings.json to be byte-identical to the original'
        $problems = [Collections.Generic.List[string]]::new()
        foreach ($name in $script:RunFolders) {
            $hooks = Join-Path (Get-Folder $name) 'hooks'
            $left = @(Get-ChildItem -LiteralPath $hooks -Force -Filter 'agentnotch*' -ErrorAction SilentlyContinue)
            if ($left) { $problems.Add("$name\hooks still holds $($left.Name -join ', ')") }
        }
        $differences = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P -Skip 'AppData/*') -Exclude 'AppData/*')
        foreach ($line in (Test-OnlyBackupsAdded -Differences $differences)) { $problems.Add($line) }
        foreach ($line in $differences) { Write-PhaseLog "  P: $line" }
        if ($problems.Count) { throw ("after Turn off:`n  " + ($problems -join "`n  ")) }
    } finally {
        Save-LiveRunLog
    }
}

# --- phases 10-12: sign-in and sync, an update over a running app, the manual reinstall -------------------------------

# More of the page's names (see $script:Ui above): Settings > Claude Code > Cloud, and the consent
# the hooks switch comes back through. The cloud rows are the page's own actions (settings-sections.js).
$script:Ui.CloudSignIn      = '[data-an-action="an-cloud-sign-in"]'
$script:Ui.CloudSignOutAsk  = '[data-an-action="an-cloud-sign-out-ask"]'
$script:Ui.CloudSignOut     = '[data-an-action="an-cloud-sign-out"]'
$script:Ui.CloudSyncOff     = '[data-an-action="an-cloud-sync"][data-an-on="0"]'
$script:Ui.CloudSyncNow     = '[data-an-action="an-cloud-sync-now"]'
$script:Ui.HooksSwitchOff   = '[data-an-action="hooks-enabled"][data-an-on="0"]'
$script:Ui.Reconsider       = '[data-an-action="reconsider"]'

# The URL the app would have opened in the browser, from the dev browser log (the app writes it
# there instead of opening it when AGENTNOTCH_DEV=1 and AGENTNOTCH_DEV_BROWSER_LOG is set).
function Get-AuthorizeUrls {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    @([regex]::Matches($Text, '(?i)https?://[^\s"''<>]+/auth/v1/authorize[^\s"''<>]*') | ForEach-Object { $_.Value })
}

# What is wrong with the sign-in URL (one line each; none = fine): Supabase's authorize on the
# fake website, Google, the app's own redirect, an S256 PKCE challenge (RFC 7636: 43 base64url
# characters), and nothing that is a secret. The verifier never leaves the app until the code
# exchange, and tokens are not made yet, so none of those may be in this URL.
function Test-AuthorizeUrl {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Url, [Parameter(Mandatory)][string]$Website)
    $uri = $null
    if ($Url -notmatch '^https?://' -or -not [Uri]::TryCreate($Url, [UriKind]::Absolute, [ref]$uri)) { 'it is not an absolute URL'; return }
    $prefix = $Website.TrimEnd('/') + '/auth/v1/authorize?'
    if (-not $Url.StartsWith($prefix, [StringComparison]::Ordinal)) { "it does not start with $prefix" }
    if ($uri.UserInfo) { 'it carries a user name or password' }
    $pairs = [Collections.Generic.List[object]]::new()
    foreach ($part in ($uri.Query.TrimStart('?') -split '&')) {
        if (-not $part) { continue }
        $name, $value = $part -split '=', 2
        $decode = { param($t) [Uri]::UnescapeDataString(([string]$t).Replace('+', ' ')) }
        $pairs.Add([pscustomobject]@{ Name = (& $decode $name); Value = (& $decode $value) })
    }
    $values = @{}
    foreach ($key in 'provider', 'redirect_to', 'code_challenge', 'code_challenge_method') {
        $found = @($pairs | Where-Object { $_.Name -ceq $key })
        if ($found.Count -ne 1) { "expected exactly one '$key' parameter, found $($found.Count)"; continue }
        $values[$key] = $found[0].Value
    }
    if ($values.ContainsKey('provider') -and $values['provider'] -cne 'google') { "provider is '$($values['provider'])', expected google" }
    if ($values.ContainsKey('redirect_to') -and $values['redirect_to'] -cne 'agentnotch://auth-callback') { "redirect_to is '$($values['redirect_to'])', expected agentnotch://auth-callback" }
    if ($values.ContainsKey('code_challenge') -and $values['code_challenge'] -cnotmatch '^[A-Za-z0-9_-]{43}$') { "code_challenge '$($values['code_challenge'])' is not 43 base64url characters (a SHA-256 digest)" }
    if ($values.ContainsKey('code_challenge_method') -and $values['code_challenge_method'] -ine 's256') { "code_challenge_method is '$($values['code_challenge_method'])', expected S256" }
    foreach ($pair in $pairs) {
        if ($pair.Name -imatch '^(access_token|refresh_token|code_verifier|verifier|apikey|api_key|secret|password|token|auth_code|code)$') { "the URL carries '$($pair.Name)'" }
    }
}

# A request log as the fake website writes it: one JSON object per line.
function ConvertFrom-RequestLog {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    foreach ($line in ($Text -split "`r?`n")) {
        if (-not $line.Trim()) { continue }
        try { $entry = $line | ConvertFrom-Json -AsHashtable } catch { continue }
        if ($entry -is [System.Collections.IDictionary]) { $entry }
    }
}

function Get-RequestCount {
    # AllowNull: Get-FakeWebsiteRequests' empty answer reaches here as $null.
    param([Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][object[]]$Requests, [Parameter(Mandatory)][string]$Method, [Parameter(Mandatory)][string]$Path)
    if ($null -eq $Requests) { return 0 }   # piped on, $null would be one item
    @($Requests | Where-Object { $_['method'] -ceq $Method -and $_['path'] -ceq $Path }).Count
}

# The two cloud switches from the `settings` snapshot the page itself draws from.
function Get-CloudSwitches {
    param([Parameter(Mandatory)][AllowNull()]$Settings)
    $cloud = if ($Settings -is [System.Collections.IDictionary] -and $Settings.Contains('cloud') -and $Settings['cloud'] -is [System.Collections.IDictionary]) { $Settings['cloud'] } else { @{} }
    [pscustomobject]@{
        Sync      = ($cloud.Contains('sync_enabled') -and $cloud['sync_enabled'] -eq $true)
        Summaries = ($cloud.Contains('summaries_enabled') -and $cloud['summaries_enabled'] -eq $true)
    }
}

# The readings the cloud holds to send (`pending_usage` of the settings snapshot's cloud).
function Get-CloudPendingUsage {
    param([Parameter(Mandatory)][AllowNull()]$Settings)
    if ($Settings -is [System.Collections.IDictionary] -and $Settings.Contains('cloud') -and $Settings['cloud'] -is [System.Collections.IDictionary] -and $Settings['cloud'].Contains('pending_usage')) {
        [int]$Settings['cloud']['pending_usage']
    } else { 0 }
}

# A .claude.json as Claude Code leaves it after a response: its own usage cache
# (`cachedUsageUtilization`, the shape `usage::parser::parse_cached_usage` reads) added, for the
# file's own login, fetched at FetchedAtMs; every other key kept. Plain JSON, no BOM.
function ConvertTo-ClaudeJsonWithCachedUsage {
    param([Parameter(Mandatory)][string]$Text, [Parameter(Mandatory)][double]$FetchedAtMs)
    $config = $Text.TrimStart([char]0xFEFF) | ConvertFrom-Json -AsHashtable
    $uuid = [string]$config['oauthAccount']['accountUuid']
    if (-not $uuid) { throw 'the .claude.json names no login' }
    $resets = { param([double]$hours) [DateTimeOffset]::FromUnixTimeMilliseconds([long]$FetchedAtMs).AddHours($hours).ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", [Globalization.CultureInfo]::InvariantCulture) }
    $config['cachedUsageUtilization'] = [ordered]@{
        accountUuid = $uuid
        fetchedAtMs = [long]$FetchedAtMs
        utilization = [ordered]@{
            five_hour = [ordered]@{ utilization = 17; resets_at = (& $resets 3) }
            seven_day = [ordered]@{ utilization = 29; resets_at = (& $resets 100) }
        }
    }
    $config | ConvertTo-Json -Depth 20
}

function Get-FakeWebsiteRequests {
    param([Parameter(Mandatory)][string]$LogFile)
    if (-not (Test-Path -LiteralPath $LogFile -PathType Leaf)) { return @() }
    @(ConvertFrom-RequestLog -Text ([string](Get-Content -LiteralPath $LogFile -Raw -ErrorAction SilentlyContinue)))
}

# smoke\fake-website.mjs on a free loopback port. Its tokens are plain strings only it accepts;
# nothing here talks to a real website or a real Supabase project.
function Start-FakeWebsite {
    param([Parameter(Mandatory)][string]$LogFile, [Parameter(Mandatory)][string]$SyncDir)
    $port = Get-FreeTcpPort
    $run = Start-NodeScript -Script (Join-Path $PSScriptRoot 'smoke\fake-website.mjs') -Arguments @('--port', [string]$port, '--log', $LogFile, '--sync-dir', $SyncDir)
    $url = "http://127.0.0.1:$port"
    Wait-Until {
        if ($run.Process.HasExited) { throw "the fake website exited with $($run.Process.ExitCode)" }
        try { (Invoke-RestMethod -Uri "$url/api/app/v1/config" -TimeoutSec 2).supabaseUrl -eq $url } catch { $false }
    } 20 'the fake website to answer'
    # That probe is a request of its own; the log the phase reads starts clean.
    Remove-Item -LiteralPath $LogFile -Force -ErrorAction SilentlyContinue
    [pscustomobject]@{ Run = $run; Process = $run.Process; Url = $url; Port = $port; Log = $LogFile; SyncDir = $SyncDir }
}

function Invoke-ContractShape {
    param([Parameter(Mandatory)][string]$BodyFile)
    $run = Wait-NodeScript -Run (Start-NodeScript -Script (Join-Path $PSScriptRoot 'smoke\contract-shape.mjs') -Arguments @($BodyFile, '--sorted', '--millis')) -TimeoutSeconds 30
    if ($run.ExitCode -ne 0) { throw "the sync request does not have the contract's shape: $($run.Stdout.Trim()) $($run.Stderr.Trim())" }
}

# The running copy of the installed app that this script did not start (the installer's /R, or
# an install that launched it): found by its image path, adopted so it is stopped by handle too.
function Find-RunningApp {
    $found = @(Get-Process -Name agentnotch -ErrorAction SilentlyContinue | Where-Object { try { $_.Path -eq $script:AppExe } catch { $false } })
    if ($found.Count) { $found[0] }
}

function Use-RunningApp {
    param([Parameter(Mandatory)][System.Diagnostics.Process]$Process)
    $script:LiveApp = Register-OwnProcess $Process
}

# The real app, with the DevTools port. $Extra is added to the app's environment (phase 10).
function Start-LiveApp {
    param([hashtable]$Extra = @{})
    Stop-LiveApp
    $script:CdpPort = Get-FreeTcpPort
    $environment = Merge-Environment @((Get-AppEnvironment), (Get-DevToolsEnvironment -Port $script:CdpPort -Log { param($line) Write-PhaseLog $line }), $Extra)
    $script:LiveApp = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $environment)
    Write-PhaseLog "the real app is running as process $($script:LiveApp.Id), DevTools on port $($script:CdpPort)"
    Wait-ControlStatus -Seconds 60 -What 'the app to listen' -Expect @{ transport = 'listening' } | Out-Null
    if ($script:LiveApp.HasExited) { throw "the app exited with $($script:LiveApp.ExitCode)" }
}

# Ends the running app the way a user would (`control quit`), by handle when it does not go.
function Stop-LiveAppGracefully {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { $script:LiveApp = $null; return }
    try { [void](Invoke-Cli -Arguments @('control', 'quit') -Environment (Get-AppEnvironment)) } catch { Write-PhaseLog "control quit: $($_.Exception.Message)" }
    if (-not $script:LiveApp.WaitForExit(15000)) { Write-PhaseLog 'the app did not quit within 15 s; stopping it by handle' }
    Stop-LiveApp
}

function Wait-SettingsPage {
    param([Parameter(Mandatory)][string]$Expression, [int]$Seconds = 20)
    [void](Invoke-Cdp -Arguments @('wait', $script:Ui.SettingsPage, $Expression, [string]($Seconds * 1000)) -TimeoutSeconds ($Seconds + 15))
}

# Opens Settings and presses an armed control of it.
function Invoke-SettingsClick {
    param([Parameter(Mandatory)][string]$Selector)
    Invoke-CdpInvoke -Page $script:Ui.NotchPage -Command $script:Ui.OpenSettings | Out-Null
    Wait-SettingsPage -Expression (Get-ArmedExpression -Selector $Selector)
    [void](Invoke-Cdp -Arguments @('click', $script:Ui.SettingsPage, $Selector))
}

function Get-SettingsSnapshot {
    Invoke-CdpCall -Page $script:Ui.SettingsPage -Method 'settings'
}

# --- phase 10: sign-in and sync against the fake website ---------------------------------------------------------------

# One line of what the app shows now (control status) and the account lines of its doctor, for the log.
function Write-AccountsSeen {
    param([Parameter(Mandatory)][string]$When)
    try { Write-PhaseLog "$When, control status: $(@((Get-ControlStatus).GetEnumerator() | ForEach-Object { "$($_.Key)=$($_.Value)" }) -join ' ')" } catch { Write-PhaseLog "$When, control status: $($_.Exception.Message)" }
    try {
        $doctor = Invoke-Cli -Arguments @('doctor') -Environment (Get-AppEnvironment)
        foreach ($line in @(($doctor.Text -split "`r?`n") | Where-Object { $_ -match '^(accounts|account|folder|store)\b' })) { Write-PhaseLog "$When, doctor: $line" }
    } catch { Write-PhaseLog "$When, doctor: $($_.Exception.Message)" }
}

function Invoke-CloudPhase {
    $log = Join-Path $script:ArtifactsDir 'fake-website.jsonl'
    $syncDir = Join-Path $script:ArtifactsDir 'fake-website-sync'
    $browserLog = Join-Path $script:ArtifactsDir 'dev-browser.log'
    foreach ($file in $log, $browserLog) { Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue }
    Remove-Item -LiteralPath $syncDir -Recurse -Force -ErrorAction SilentlyContinue
    $site = Start-FakeWebsite -LogFile $log -SyncDir $syncDir
    Write-PhaseLog "fake website on $($site.Url)"
    $claudeJsonBefore = [ordered]@{}
    try {
        # A fresh start of the app, pointed at the fake website: only this run, only 127.0.0.1.
        Write-AccountsSeen -When 'before the restart'
        Stop-LiveAppGracefully
        Start-LiveApp -Extra @{ AGENTNOTCH_WEB_URL = $site.Url; AGENTNOTCH_DEV = '1'; AGENTNOTCH_DEV_BROWSER_LOG = $browserLog }
        Write-AccountsSeen -When 'after the restart'

        # 1. Sign in: the URL the app would have opened.
        Invoke-SettingsClick -Selector $script:Ui.CloudSignIn
        # @(...): a function's empty or one-item array comes back as $null or the item itself, and
        # strict mode has no .Count on those (run 37311858421); [0] of a lone string is its first letter.
        Wait-Until { @(Get-AuthorizeUrls -Text ([string](Get-Content -LiteralPath $browserLog -Raw -ErrorAction SilentlyContinue))).Count -gt 0 } 15 'the authorize URL in the dev browser log'
        $url = @(Get-AuthorizeUrls -Text ([string](Get-Content -LiteralPath $browserLog -Raw)))[0]
        Write-PhaseLog "authorize URL: $url"
        $problems = @(Test-AuthorizeUrl -Url $url -Website $site.Url)
        if ($problems) { throw "the authorize URL: $($problems -join '; ')" }
        if (@(Get-FakeWebsiteRequests -LogFile $log | Where-Object { $_['path'] -like '/api/app/v1/config' }).Count -lt 1) { throw 'the app never asked the fake website for its config' }

        # 2. The browser's answer: the registered scheme hands the callback to the running app.
        Start-Process 'agentnotch://auth-callback?code=smoke'
        Wait-ControlStatus -Seconds 10 -What 'control status to say cloud: signed_in' -Expect @{ cloud = 'signed_in' } | Out-Null
        $support = Find-SupportDir -Profile $script:P
        if (-not $support) { throw 'no support folder' }
        $session = Join-Path $support 'cloud-session.json'
        if (-not (Test-Path -LiteralPath $session -PathType Leaf)) { throw "$session does not exist after sign-in" }
        $aclProblems = @(Get-AclProblems -Path $session)
        if ($aclProblems) { throw "cloud-session.json: $($aclProblems -join '; ')" }
        $exchange = Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/auth/v1/token'
        if ($exchange -lt 1) { throw 'the fake website saw no code exchange' }

        # 3. Consent before upload: signed in is not "sync on".
        Start-Sleep -Seconds 3
        $syncs = Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/api/app/v1/sync'
        if ($syncs -ne 0) { throw "a /sync request arrived ($syncs) although sync was never turned on" }
        $switches = Get-CloudSwitches -Settings (Get-SettingsSnapshot)
        if ($switches.Sync -or $switches.Summaries) { throw "a switch is on after sign-in (sync $($switches.Sync), summaries $($switches.Summaries))" }

        # 4. Sync on: one upload, in the contract's shape, with nothing of this machine's folders.
        Invoke-SettingsClick -Selector $script:Ui.CloudSyncOff
        Wait-ControlStatus -Seconds 10 -What 'control status to say sync: true' -Expect @{ sync = 'true' } | Out-Null
        # Something to send: readings are recorded only while sync is on, and this profile's only
        # source so far was the probe, which ran before the sign-in and is 5 minutes apart. So
        # Claude Code "answers" now: it leaves its usage cache in .claude.json, which the app reads
        # every 20 s (run 37320814137 waited for a /sync that had nothing to carry).
        # Both accounts' Claude Code do (the default folder's login lives in P\.claude.json, the
        # other's in its own folder), so each account the app shows has a reading.
        $shownAccounts = [int](Get-ControlStatus)['accounts']
        if ($shownAccounts -ne 2) { Write-Host "::warning::phase 10: the restarted app shows $shownAccounts account(s), not 2 (see the doctor lines in its log)" }
        $fetchedAt = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        foreach ($file in (Join-Path $script:P '.claude.json'), (Join-Path $script:P '.claude-work\.claude.json')) {
            $bytes = [IO.File]::ReadAllBytes($file)
            $claudeJsonBefore[$file] = $bytes
            $withUsage = ConvertTo-ClaudeJsonWithCachedUsage -Text ([Text.Encoding]::UTF8.GetString($bytes)) -FetchedAtMs $fetchedAt
            [IO.File]::WriteAllText($file, $withUsage, [Text.UTF8Encoding]::new($false))
        }
        $syncCount = { Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/api/app/v1/sync' }
        try {
            Wait-Until { (& $syncCount) -ge 1 -or (Get-CloudPendingUsage -Settings (Get-SettingsSnapshot)) -ge [Math]::Max(1, $shownAccounts) } 45 'a cached reading of each account shown to be recorded for the website'
            Write-PhaseLog "readings waiting for the website: $(Get-CloudPendingUsage -Settings (Get-SettingsSnapshot)); /sync requests so far: $(& $syncCount)"
        } catch {
            # What the cloud, the app's support folder and the file say, for the next round.
            try { Write-PhaseLog "settings cloud: $((Get-SettingsSnapshot)['cloud'] | ConvertTo-Json -Compress -Depth 6)" } catch { Write-PhaseLog "settings: $($_.Exception.Message)" }
            Write-AccountsSeen -When 'no reading'
            $supportNow = Find-SupportDir -Profile $script:P
            # Strict mode: a folder has no Length.
            if ($supportNow) { Write-PhaseLog "support files: $(@(Get-ChildItem -LiteralPath $supportNow -Force | ForEach-Object { if ($_ -is [IO.FileInfo]) { "$($_.Name)($($_.Length))" } else { "$($_.Name)\" } }) -join ' ')" }
            foreach ($file in $claudeJsonBefore.Keys) { Write-PhaseLog "$file now: $([IO.File]::ReadAllText($file) -replace '\s+', ' ')" }
            try {
                $shown = (Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'snapshot') | ConvertTo-Json -Compress -Depth 8
                Write-PhaseLog "snapshot: $($shown.Substring(0, [Math]::Min(4000, $shown.Length)))"
            } catch { Write-PhaseLog "snapshot: $($_.Exception.Message)" }
            throw
        }
        # The first pass after sync on runs at the next 20 s tick. When it came before the reading
        # it had nothing to send, and the next is 5 minutes away (as on the Mac: setSyncEnabled,
        # syncInterval); "Sync now" is what a user has for that.
        try {
            Wait-Until { (& $syncCount) -ge 1 } 21 'the scheduled /sync'
            Write-PhaseLog 'the scheduled pass sent the reading'
        } catch {
            Write-PhaseLog 'the pass after sync on came before the reading; pressing Sync now'
            Invoke-SettingsClick -Selector $script:Ui.CloudSyncNow
        }
        Wait-Until { (& $syncCount) -ge 1 } 30 'a /sync request'
        $bodies = @(Get-ChildItem -LiteralPath $syncDir -Filter 'sync-*.json' -ErrorAction SilentlyContinue | Sort-Object Name)
        if (-not $bodies.Count) { throw "the fake website saved no sync body under $syncDir" }
        Invoke-ContractShape -BodyFile $bodies[0].FullName
        $text = [string](Get-Content -LiteralPath $bodies[0].FullName -Raw)
        $escapedProfile = (ConvertTo-Json $script:P -Compress).Trim('"')
        if ($text.Contains($escapedProfile)) { throw 'the sync body holds a path under the temporary profile' }
        $syncRequest = @(Get-FakeWebsiteRequests -LogFile $log | Where-Object { $_['method'] -ceq 'POST' -and $_['path'] -ceq '/api/app/v1/sync' })[0]
        if ($syncRequest['authorization'] -ne 'present') { throw 'the /sync request carried no authorization' }
        Write-PhaseLog "sync body $($bodies[0].Name): $($text.Length) bytes, the contract's shape, no profile path"
        Copy-Item -LiteralPath $bodies[0].FullName -Destination (Join-Path $script:ArtifactsDir 'sync-request-sample.json') -Force

        # 5. Sign out: the server is told, both switches go off, the session file goes, and nothing more is sent.
        Invoke-SettingsClick -Selector $script:Ui.CloudSignOutAsk
        Wait-SettingsPage -Expression (Get-ArmedExpression -Selector $script:Ui.CloudSignOut)
        [void](Invoke-Cdp -Arguments @('click', $script:Ui.SettingsPage, $script:Ui.CloudSignOut))
        Wait-Until { (Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/auth/v1/logout') -ge 1 } 10 'the logout request'
        Wait-Until { -not (Test-Path -LiteralPath $session) } 10 'cloud-session.json to be deleted'
        Wait-ControlStatus -Seconds 10 -What 'control status to leave signed_in' -Check ({ param($s) if ($s['cloud'] -ceq 'signed_in') { 'cloud is still signed_in' } }.GetNewClosure()) | Out-Null
        $switches = Get-CloudSwitches -Settings (Get-SettingsSnapshot)
        if ($switches.Sync -or $switches.Summaries) { throw "a switch is still on after sign-out (sync $($switches.Sync), summaries $($switches.Summaries))" }
        $before = Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/api/app/v1/sync'
        Start-Sleep -Seconds 3
        $after = Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/api/app/v1/sync'
        if ($after -ne $before) { throw "a /sync request arrived after sign-out ($before -> $after)" }
        Write-PhaseLog 'signed out: /logout called, switches off, session file gone, no upload after it'
    } finally {
        Save-LiveRunLog
        Stop-OwnProcess -Process $site.Process
        # The later phases compare P with what phase 2 built.
        foreach ($file in $claudeJsonBefore.Keys) { [IO.File]::WriteAllBytes($file, $claudeJsonBefore[$file]) }
    }
    # The phases after this one run the app as a user has it: no fake website, no dev switches.
    Stop-LiveAppGracefully
    Start-LiveApp
}

# --- phase 11: an update over a running app --------------------------------------------------------------------------

# The files of P the update must leave alone: every settings.json and every hook copy.
function Get-HookFilesHash {
    $hashes = Get-TreeHash -Root $script:P -Skip 'AppData/*'
    $kept = [ordered]@{}
    foreach ($key in $hashes.Keys) {
        if ($key -like '.claude*/settings.json' -or $key -like '.claude*/hooks/*') { $kept[$key] = $hashes[$key] }
    }
    $kept
}

# Turn on, again: after phase 9 the hooks are off. The page shows a consent card, a "Turn on…"
# that opens it, or the hooks switch, whichever the engine's state calls for.
function Enable-Hooks {
    Invoke-CdpInvoke -Page $script:Ui.NotchPage -Command $script:Ui.OpenSettings | Out-Null
    $probe = "(() => { const q = (s) => !!document.querySelector(s); return q($(ConvertTo-JsString $script:Ui.ConsentTurnOn)) ? 'card' : q($(ConvertTo-JsString $script:Ui.Reconsider)) ? 'reconsider' : q($(ConvertTo-JsString $script:Ui.HooksSwitchOff)) ? 'switch' : 'none'; })()"
    Wait-Until { $script:HooksUiState = [string](Invoke-Cdp -Arguments @('eval', $script:Ui.SettingsPage, $probe)); $script:HooksUiState -ne 'none' } 20 'Settings to offer to turn the hooks on'
    $state = $script:HooksUiState
    Write-PhaseLog "Settings offers: $state"
    if ($state -eq 'reconsider') {
        Invoke-CdpClick -Page $script:Ui.SettingsPage -Selector $script:Ui.Reconsider
        $state = 'card'
    }
    $selector = if ($state -eq 'card') { $script:Ui.ConsentTurnOn } else { $script:Ui.HooksSwitchOff }
    Wait-SettingsPage -Expression (Get-ArmedExpression -Selector $selector)
    [void](Invoke-Cdp -Arguments @('click', $script:Ui.SettingsPage, $selector))
    Wait-Until {
        $done = $true
        foreach ($name in $script:RunFolders) {
            $file = Join-Path (Get-Folder $name) 'settings.json'
            if ((Get-FileSha256 $file) -eq $script:ProfileHashes["$name/settings.json"] -or -not (Test-Path (Get-ExpectedHookExe $name))) { $done = $false }
        }
        $done
    } 15 'both run folders to hold our entries again'
    Start-Sleep -Milliseconds 700   # a pass writes its files one after the other; let it finish
}

# The index, among the entries of an event, of the first one that is ours.
function Get-OwnEntryIndex {
    param([Parameter(Mandatory)]$Settings, [Parameter(Mandatory)][string]$EventName, [Parameter(Mandatory)][string]$ExpectedExe)
    $entries = @(Get-HookEntries -Settings $Settings | Where-Object { $_.Event -eq $EventName })
    for ($i = 0; $i -lt $entries.Count; $i++) {
        if (Get-OwnForm -Entry $entries[$i] -ExpectedExe $ExpectedExe -Resolve { param($p) Resolve-LongPath -Path $p }) { return [pscustomobject]@{ Index = $i; Entry = $entries[$i] } }
    }
    throw "no $EventName entry of ours in the settings.json"
}

function Invoke-UpdatePhase {
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) { Start-LiveApp }
    $execAllowed = Get-ExecFormAllowedHere
    $dummy = $null
    $hook = $null
    try {
        Enable-Hooks
        $problems = [Collections.Generic.List[string]]::new()
        foreach ($line in (Get-InstalledFolderProblems -Name '.claude' -StatusLineWrapped $true -ExecFormAllowed $execAllowed)) { $problems.Add($line) }
        foreach ($line in (Get-InstalledFolderProblems -Name '.claude-work' -StatusLineWrapped $false -ExecFormAllowed $execAllowed)) { $problems.Add($line) }
        if ($problems.Count) { throw ("after Turn on again:`n  " + ($problems -join "`n  ")) }
        $installed = Get-HookFilesHash
        Write-PhaseLog "hooks are installed again ($($installed.Count) files hashed)"

        # A PermissionRequest the app holds: the hook waits for the user's answer.
        $settingsPath = Join-Path (Get-Folder '.claude') 'settings.json'
        $settings = Read-Settings $settingsPath
        $expected = Get-ExpectedHookExe '.claude'
        $pre = Get-OwnEntryIndex -Settings $settings -EventName 'PreToolUse' -ExpectedExe $expected
        $permission = Get-OwnEntryIndex -Settings $settings -EventName 'PermissionRequest' -ExpectedExe $expected
        $shell = @(Get-ShellsFor -Entry $permission.Entry)[0]
        $dummy = Start-DummyClaude
        $environment = Get-HookEnvironmentArguments -ClaudePid $dummy.Id
        $preStdin = Write-StdinFile -Name 'update-pre' -Json (New-HookStdin -Fixture 'permission_request_bash' -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app' -EventName 'PreToolUse' -ToolUseId 'toolu_smoke_update')
        $requestStdin = Write-StdinFile -Name 'update-request' -Json (New-HookStdin -Fixture 'permission_request_bash' -SessionId $script:SmokeSessionId -Transcript (Get-SmokeTranscript) -Cwd 'C:\smoke\work-app')
        $first = ConvertFrom-HookRuns -Text (Wait-NodeScript -Run (Start-HookRun -Arguments (@('--settings', $settingsPath, '--event', 'PreToolUse', '--index', [string]$pre.Index, '--shell', $shell, '--stdin', $preStdin, '--timeout', '15000') + $environment))).Stdout
        foreach ($run in $first) { if ($run['exit'] -ne 0) { throw "the PreToolUse exited with $($run['exit'])" } }
        $hook = Start-HookRun -Arguments (@('--settings', $settingsPath, '--event', 'PermissionRequest', '--index', [string]$permission.Index, '--shell', $shell, '--stdin', $requestStdin, '--timeout', '170000') + $environment)
        Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'panel_open' -Arguments @{ route = 'sessions'; reason = 'ring_click' } | Out-Null
        # The request is on the panel: the app holds it, and the hook is waiting for the answer.
        [void](Invoke-Cdp -Arguments @('wait', $script:Ui.PanelPage, "!!document.querySelector($(ConvertTo-JsString ($script:Ui.Answer -f 'allow', $script:SmokeSessionId)))", '30000') -TimeoutSeconds 45)
        if ($hook.Process.HasExited) { throw 'the hook returned before the update: nothing was held' }
        Write-PhaseLog 'a PermissionRequest is held by the running app'

        # The update the way the updater runs it: passive, over the same install, started again after.
        $oldApp = $script:LiveApp
        $installer = Register-OwnProcess (Start-Process -FilePath $script:Installer -ArgumentList '/UPDATE', '/P', '/R' -Environment @{ USERPROFILE = $script:P } -PassThru)
        $appStopped = $null
        $hookDone = $null
        $deadline = (Get-Date).AddSeconds(240)
        while (-not ($installer.HasExited -and $appStopped -and $hookDone)) {
            if ((Get-Date) -gt $deadline) { throw "the update did not finish within 240 s (installer exited: $($installer.HasExited), app stopped: $([bool]$appStopped), hook returned: $([bool]$hookDone))" }
            $now = [DateTime]::UtcNow
            if (-not $appStopped -and $oldApp.HasExited) { $appStopped = $now }
            if (-not $hookDone -and $hook.Process.HasExited) { $hookDone = $now }
            Start-Sleep -Milliseconds 100
        }
        if ($installer.ExitCode -ne 0) { throw "the installer exited with $($installer.ExitCode)" }
        $lag = ($hookDone - $appStopped).TotalSeconds
        Write-PhaseLog ("the app stopped, the held hook returned {0:N1} s later" -f $lag)
        if ($lag -gt 2.0) { throw ("the held hook returned {0:N1} s after the app stopped (limit 2 s)" -f $lag) }
        $result = @(ConvertFrom-HookRuns -Text (Wait-NodeScript -Run $hook).Stdout)[0]
        if ($result['timedOut'] -or $result['exit'] -ne 0) { throw "the held hook exited with $($result['exit'])$(if ($result['timedOut']) { ' (timed out)' })" }
        if ([string]$result['stdout_b64'] -ne '') { throw "the held hook printed $($result['stdout']) instead of failing open" }

        # /R: the updated app is running again.
        Wait-Until { [bool](Find-RunningApp) } 60 'the app to run again after the update'
        Use-RunningApp -Process (Find-RunningApp)
        $status = Wait-ControlStatus -Seconds 60 -What 'the updated app to listen' -Expect @{ transport = 'listening' }
        if ($status['accounts'] -ne '2') { Write-Host "::warning::the app started by the installer sees $($status['accounts']) account(s), not 2: it may not run with the temporary profile as its home" }
        Write-PhaseLog ("the updated app runs as process {0}; control status: {1}" -f $script:LiveApp.Id, (($status.GetEnumerator() | ForEach-Object { "$($_.Key): $($_.Value)" }) -join ', '))

        # What the update must not touch.
        Start-Sleep -Seconds 3
        $changed = @(Compare-Hashes -Before $installed -After (Get-HookFilesHash))
        if ($changed) { throw "the update changed P: $($changed -join ', ')" }
        Write-PhaseLog 'every settings.json and hook copy in P is unchanged'
    } finally {
        if ($hook) { Stop-OwnProcess -Process $hook.Process }
        if ($dummy) { Stop-OwnProcess -Process $dummy }
        Save-LiveRunLog
    }
}

# --- phase 12: the manual reinstall path --------------------------------------------------------------------------------

function Invoke-ReinstallPhase {
    # The app is running: "Uninstall before installing" has to cope with that too.
    if (-not $script:LiveApp -or $script:LiveApp.HasExited) {
        $running = Find-RunningApp
        if ($running) { Use-RunningApp -Process $running } else { Start-LiveApp }
    }
    $hooksBefore = Get-HookFilesHash
    if (-not @($hooksBefore.Keys | Where-Object { $_ -like '.claude*/settings.json' }).Count) { throw 'no settings.json in P to compare' }
    $settingsOnly = [ordered]@{}
    foreach ($key in $hooksBefore.Keys) { if ($key -like '.claude*/settings.json') { $settingsOnly[$key] = $hooksBefore[$key] } }

    # uninstall.exe /S: what the reinstall page's default does, minus its page (no /UPDATE, nothing ticked).
    Start-Process -FilePath $script:UninstallExe -ArgumentList '/S' -Environment @{ USERPROFILE = $script:P } -Wait
    Wait-Until { -not (Test-Path $script:AppExe) -and -not (Test-Path $script:UninstallKey) } 60 'the uninstall'
    if (-not $script:LiveApp.WaitForExit(10000)) { throw 'the uninstall left the app running' }
    $changed = @(Compare-Hashes -Before $settingsOnly -After (Get-HookFilesHash) -Filter '.claude*/settings.json')
    if ($changed) { throw "the uninstall (no /REMOVEHOOKS) changed a settings.json: $($changed -join ', ')" }
    Write-PhaseLog 'uninstalled; every settings.json in P is as the hooks left it'

    Start-Process -FilePath $script:Installer -ArgumentList '/S' -Environment @{ USERPROFILE = $script:P } -Wait
    Wait-Until { (Test-Path $script:AppExe) -and (Test-Path $script:HookExe) -and (Test-Path $script:UninstallExe) } 60 'the reinstalled files'
    $changed = @(Compare-Hashes -Before $settingsOnly -After (Get-HookFilesHash) -Filter '.claude*/settings.json')
    if ($changed) { throw "the reinstall changed a settings.json: $($changed -join ', ')" }

    # The first launch after it: consent was kept (it lives with the app's data, which an uninstall keeps).
    $running = Find-RunningApp
    if ($running) { Use-RunningApp -Process $running } else { Start-LiveApp }
    $status = Wait-ControlStatus -Seconds 60 -What 'control status to say hook_consent: granted' -Expect @{ hook_consent = 'granted' }
    Write-PhaseLog ("after the first launch control status: " + (($status.GetEnumerator() | ForEach-Object { "$($_.Key): $($_.Value)" }) -join ', '))
    Start-Sleep -Seconds 3
    $changed = @(Compare-Hashes -Before $settingsOnly -After (Get-HookFilesHash) -Filter '.claude*/settings.json')
    if ($changed) { throw "the first launch changed a settings.json: $($changed -join ', ')" }
    $hooksNow = Get-HookFilesHash
    if (-not @($hooksNow.Keys | Where-Object { $_ -like '.claude*/hooks/agentnotch-hook*' }).Count) { throw 'no hook copy is left in P after the reinstall' }
    Save-LiveRunLog
}

# --- phase 13: a hook with no app --------------------------------------------------------------------------

# One run of the hook the way Claude Code starts it: piped stdin, no window. Seconds is the
# time from start to exit.
function Invoke-HookOnce {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$Arguments, [Parameter(Mandatory)][AllowEmptyString()][string]$Stdin)
    $psi = [Diagnostics.ProcessStartInfo]::new($Exe)
    foreach ($argument in $Arguments) { $psi.ArgumentList.Add($argument) }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $run = [Diagnostics.Process]::Start($psi)
    try {
        $run.StandardInput.Write($Stdin)
        $run.StandardInput.Close()
    } catch { <# the hook did not wait for its stdin: that is fine, it fails open #> }
    if (-not $run.WaitForExit(5000)) {
        $run.Kill($true)
        throw "the hook hung ($($Arguments -join ' '))"
    }
    $printed = $run.StandardOutput.ReadToEnd()
    [pscustomobject]@{ ExitCode = $run.ExitCode; Stdout = $printed; Seconds = $watch.Elapsed.TotalSeconds }
}

function Invoke-FailOpenPhase {
    # Whatever this script started is stopped (by handle); a copy still up after that is not
    # ours and not this test's to stop, so the check cannot mean what it says.
    Stop-OwnProcesses
    if (Get-Process -Name agentnotch -ErrorAction SilentlyContinue) { throw 'an agentnotch.exe is still running: a hook with no app cannot be checked' }
    $folder = Join-Path $script:Temp 'failopen'
    New-Item -ItemType Directory -Force -Path $folder | Out-Null
    $hook = Join-Path $folder 'agentnotch-hook.exe'
    Copy-Item -LiteralPath $script:HookExe -Destination $hook -Force
    $proto = Join-Path (Split-Path -Parent $PSScriptRoot) 'agentnotch-proto\tests\fixtures\v1\permission_request_bash.json'
    $stdins = [ordered]@{
        'garbage'                   = '{ not json'
        'the protocol PermissionRequest' = [IO.File]::ReadAllText($proto)
        'Claude Code PermissionRequest'  = [IO.File]::ReadAllText((Join-Path $script:FixtureDir 'permission-request-stdin.json'))
    }
    $cases = [Collections.Generic.List[object]]::new()
    foreach ($argv in @(@('hook'), @('hook', '--exec'))) { foreach ($name in $stdins.Keys) { $cases.Add(@($argv, $name)) } }
    foreach ($argv in @(@('statusline'), @('bogus'), @())) { $cases.Add(@($argv, 'garbage')) }
    # The first start of a fresh copy is scanned by the antivirus; Claude Code would pay that
    # once too, so it is not what is timed.
    [void](Invoke-HookOnce -Exe $hook -Arguments @('hook') -Stdin '{}')
    foreach ($case in $cases) {
        $argv = [string[]]$case[0]
        $label = "hook $($argv -join ' ') with $($case[1])"
        $best = $null
        # Best of three: one slow start on a busy runner must not fail a timing check; a hook
        # that is slow every time does.
        for ($try = 1; $try -le 3; $try++) {
            $run = Invoke-HookOnce -Exe $hook -Arguments $argv -Stdin $stdins[$case[1]]
            if ($run.ExitCode -ne 0) { throw "$label exited with $($run.ExitCode)" }
            if ($run.Stdout) { throw "$label printed '$($run.Stdout)'" }
            if ($null -eq $best -or $run.Seconds -lt $best) { $best = $run.Seconds }
            if ($best -lt 1.0) { break }
        }
        Write-PhaseLog ("{0}: exit 0, nothing printed, {1:N2} s" -f $label, $best)
        if ($best -ge 1.0) { throw ("$label took {0:N2} s" -f $best) }
    }
}

# --- phase 14: autostart ----------------------------------------------------------------------------------

function Get-RunValue {
    $item = Get-ItemProperty -LiteralPath $script:RunKey -ErrorAction SilentlyContinue
    if ($item -and $item.PSObject.Properties['Agent Notch']) { $item.'Agent Notch' } else { $null }
}

function Invoke-AutostartPhase {
    $on = Invoke-Cli -Arguments @('autostart', 'on') -Environment (Get-AppEnvironment)
    Write-PhaseLog $on.Text
    if ($on.ExitCode -ne 0) { throw "autostart on exited with $($on.ExitCode)" }
    $value = Get-RunValue
    if ($value -ne "`"$script:AppExe`" --silent") { throw "the Run value is '$value'" }
    $off = Invoke-Cli -Arguments @('autostart', 'off') -Environment (Get-AppEnvironment)
    Write-PhaseLog $off.Text
    if ($off.ExitCode -ne 0) { throw "autostart off exited with $($off.ExitCode)" }
    if ($null -ne (Get-RunValue)) { throw 'the Run value is still there after autostart off' }
}

# --- phase 15: uninstall ----------------------------------------------------------------------------------

function Invoke-UninstallPhase {
    # The uninstaller runs the app's hook removal for the home it is started with, so it gets P.
    # The NSIS uninstaller re-launches itself from a temporary folder, so -Wait returns early.
    Start-Process -FilePath $script:UninstallExe -ArgumentList '/S', '/REMOVEHOOKS' -Environment @{ USERPROFILE = $script:P } -Wait
    Wait-Until { -not (Test-Path $script:AppExe) -and -not (Test-Path $script:UninstallKey) } 60 'the uninstall'
    if ($null -ne (Get-RunValue)) { throw 'the Run value is still there' }
    if (Test-Path 'HKCU:\Software\Classes\agentnotch') { throw 'the agentnotch: scheme is still registered' }
    $changed = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P -Skip 'AppData/*') -Filter '.claude*/settings.json')
    if ($changed) { throw "uninstall changed what it must leave: $($changed -join ', ')" }
    $copies = @(Get-ChildItem -LiteralPath $script:P -Recurse -Force -Filter 'agentnotch-hook*' -ErrorAction SilentlyContinue)
    if ($copies) { throw "hook copies are left in P: $($copies.FullName -join ', ')" }
    # The runner's own Claude profile is exactly as phase 0 found it. (The app's own data
    # folders legitimately hold what its commands logged: an uninstall keeps them unless asked.)
    $profileChanged = @(Compare-Hashes -Before $script:KnownProfileBefore -After (Get-KnownProfileHashes))
    if ($profileChanged) { throw "the runner's own Claude profile changed: $($profileChanged -join ', ')" }
    foreach ($root in $script:DataRoots) {
        if (Test-Path -LiteralPath (Join-Path $root 'Agent Notch Sealed')) { throw "$root\Agent Notch Sealed is still there" }
    }
    Write-PhaseLog 'uninstalled; P holds the original settings and no hook copy; the runner profile is untouched'
}

# --- the table of phases ------------------------------------------------------------------------------------

# Rows run in order. Gate names a flag of gates.json: while it is closed the phase is reported
# and not run.
function Get-PhaseTable {
    param($Gates = $null)
    $rows = @(
        @{ Number = '0';  Name = 'preflight';                           Body = { Invoke-PreflightPhase } }
        @{ Number = '1';  Name = 'install';                             Body = { Invoke-InstallPhase } }
        @{ Number = '2';  Name = 'build the temporary Claude setup';    Body = { Invoke-BuildProfilePhase } }
        @{ Number = '3';  Name = 'doctor';                              Body = { Invoke-DoctorPhase } }
        @{ Number = '3b'; Name = 'sealed launch';                       Body = { Invoke-SealedLaunchPhase } }
        @{ Number = '4';  Name = 'sealed self-test and snapshots';      Gate = 'selftest'; Body = { Invoke-SelfTestPhase } }
        @{ Number = '5';  Name = 'before consent';                      Gate = 'engine'; Body = { Invoke-BeforeConsentPhase } }
        @{ Number = '6';  Name = 'deep link with nothing pending';      Gate = 'engine'; Body = { Invoke-DeepLinkPhase } }
        @{ Number = '7';  Name = 'Turn on';                             Gate = 'engine'; Body = { Invoke-TurnOnPhase } }
        @{ Number = '8';  Name = 'every entry as written, every answer'; Gate = 'engine'; Body = { Invoke-EntriesAsWrittenPhase } }
        @{ Number = '9';  Name = 'Turn off';                            Gate = 'engine'; Body = { Invoke-TurnOffPhase } }
        @{ Number = '10'; Name = 'sign-in and sync against the fake website'; Gate = 'cloud'; Body = { Invoke-CloudPhase } }
        @{ Number = '11'; Name = 'update over a running app';           Gate = 'engine'; Body = { Invoke-UpdatePhase } }
        @{ Number = '12'; Name = 'manual reinstall keeps the hooks';    Gate = 'engine'; Body = { Invoke-ReinstallPhase } }
        @{ Number = '13'; Name = 'the hook fails open';                 Body = { Invoke-FailOpenPhase } }
        @{ Number = '14'; Name = 'autostart on and off';                Gate = 'glue'; Body = { Invoke-AutostartPhase } }
        @{ Number = '15'; Name = 'uninstall with /REMOVEHOOKS';         Body = { Invoke-UninstallPhase } }
    )
    # The plain sealed launch is what the self-test replaces: with the gate open it would only
    # run the same app a fourth time.
    if ($Gates -and (Test-GateOpen -Gates $Gates -Name 'selftest')) { $rows = @($rows | Where-Object { $_.Number -ne '3b' }) }
    $rows
}

function Write-Results {
    $summary = Format-Summary -Results $script:Results -Gated $script:Gated
    Set-Content -LiteralPath (Join-Path $script:ArtifactsDir 'smoke-summary.md') -Value $summary
    $script:Results | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $script:ArtifactsDir 'smoke-results.json')
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value $summary }
}

# --- main ------------------------------------------------------------------------------------------------------

if ($MyInvocation.InvocationName -eq '.') { return }

try {
    if (-not $Installer) { throw '-Installer is required' }
    if (-not $Version) { throw '-Version is required' }
    if ($Release -and $Updates -ne 'on') { throw '-Release needs -Updates on: a release carries the update key' }
    if ($Release -and -not $KeyId) { throw '-Release needs -KeyId (the key id the doctor must show)' }
    $script:GateSet = Read-Gates -Path $GatesFile
    Assert-GatesForRelease -Gates $script:GateSet -Release:$Release
    if (-not $IsWindows) { throw 'the smoke test runs on Windows only' }
    Initialize-Context -Installer $Installer -Artifacts $Artifacts -Bins $Bins
} catch {
    Write-Host "::error::$($_.Exception.Message)"
    exit 1
}

$ok = $true
try {
    foreach ($phase in Get-PhaseTable -Gates $script:GateSet) {
        if (-not $ok) {
            $script:Results.Add([pscustomobject]@{ Number = $phase.Number; Name = $phase.Name; Status = 'not run (an earlier phase failed)'; Seconds = 0.0; Error = '' })
            continue
        }
        $ok = Invoke-Phase -Phase $phase
    }
} finally {
    Stop-OwnProcesses
    Clear-DevToolsOverride
    Write-Results
}
exit ([int](-not $ok))
