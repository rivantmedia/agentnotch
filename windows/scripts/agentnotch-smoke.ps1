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
    body. Phases 4-9 are rows too (the real app: before consent, the deep link, Turn on, every hook
    entry, Turn off); the next sub-task adds 10-12. A gate is a flag in smoke\gates.json: the
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
# Empty folders are not recorded: what the smoke test compares is bytes.
function Get-TreeHash {
    param([Parameter(Mandatory)][string]$Root, [string]$Prefix = '')
    $hashes = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $hashes }
    $full = (Resolve-Path -LiteralPath $Root).ProviderPath
    $files = Get-ChildItem -LiteralPath $full -Recurse -File -Force | Sort-Object FullName
    foreach ($file in $files) {
        $relative = [IO.Path]::GetRelativePath($full, $file.FullName).Replace('\', '/')
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
    $changedP = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P) -Exclude 'AppData/*')
    if ($changedP) { throw "a sealed run changed P: $($changedP -join ', ')" }
    Remove-SealedData
}

# --- phase 4: the sealed self-test and the snapshots ---------------------------------------------------------

# The edges the self-test must report, in the order WP9 runs them; "floating" has the notch hidden.
$script:SelfTestEdges = @('right', 'left', 'top', 'bottom', 'floating')
# The three pages whose round trip, CSP violations, errors and invariants are reported.
$script:SelfTestPages = @('notch', 'settings', 'agentnotch-panel')
# The three runs: the page scale the report must name, and the WebView2 argument that makes it.
$script:SelfTestScales = @(
    @{ Scale = 1.0;  Name = '100';  WebViewArguments = '' }
    @{ Scale = 1.25; Name = '125';  WebViewArguments = '--force-device-scale-factor=1.25' }
    @{ Scale = 1.5;  Name = '150';  WebViewArguments = '--force-device-scale-factor=1.5' }
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
        if ($run.WebViewArguments) { $environment['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = $run.WebViewArguments }
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
    $changedP = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P) -Exclude 'AppData/*')
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
    try { ConvertFrom-CdpOutput -Text $run.Stdout } catch { throw "$($_.Exception.Message) (cdp $($Arguments -join ' '); stderr: $($run.Stderr.Trim()))" }
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
function Wait-ControlStatus {
    param([Parameter(Mandatory)][scriptblock]$Check, [Parameter(Mandatory)][int]$Seconds, [Parameter(Mandatory)][string]$What)
    $deadline = (Get-Date).AddSeconds($Seconds)
    $last = $null
    while ($true) {
        $last = $null
        try { $last = Get-ControlStatus } catch { $last = "no status: $($_.Exception.Message)" }
        if ($last -isnot [string]) {
            $problems = @(& $Check $last)
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
function Test-SettingsStyle {
    param([Parameter(Mandatory)][byte[]]$Bytes)
    if ($Bytes.Length -lt 3 -or $Bytes[0] -ne 0xEF -or $Bytes[1] -ne 0xBB -or $Bytes[2] -ne 0xBF) { 'the byte order mark is gone' }
    $bare = [regex]::Matches([Text.Encoding]::UTF8.GetString($Bytes), "(?<!`r)`n").Count
    if ($bare) { "$bare bare LF line ending(s): CRLF was not kept" }
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
    $environment = Merge-Environment @((Get-AppEnvironment), $script:ScrubSentinels, @{ WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$($script:CdpPort)" })
    $script:LiveApp = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $environment)
    Write-PhaseLog "the real app is running as process $($script:LiveApp.Id), DevTools on port $($script:CdpPort)"
    Start-Sleep -Seconds 30
    if ($script:LiveApp.HasExited) { throw "the app exited with $($script:LiveApp.ExitCode) within 30 s" }

    $expect = [ordered]@{ transport = 'listening'; accounts = '2'; hook_consent = 'unasked'; readings = '2' }
    $status = Wait-ControlStatus -Seconds 60 -What 'control status before consent' -Check ({ param($s) Test-ControlStatus -Status $s -Expect $expect }.GetNewClosure())
    Write-PhaseLog ("control status: " + (($status.GetEnumerator() | ForEach-Object { "$($_.Key): $($_.Value)" }) -join ', '))

    $changed = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P) -Exclude 'AppData/*')
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
    $cloud = @(Get-ChildItem -LiteralPath $support -Force -Filter 'cloud-*' -ErrorAction SilentlyContinue)
    if ($cloud) { throw "the support folder holds $($cloud.Name -join ', ') before sign-in" }
    if ($null -ne (Get-RunValue)) { throw 'a Run value exists although autostart was never turned on' }

    $runLog = Get-LiveRunLogText
    Save-LiveRunLog
    if ($runLog -notmatch '(?m)an: hub started(?! \(sealed\))') { throw 'run.log has no "an: hub started"' }
    if ($runLog -notmatch '(?m)an: pipe listening') { throw 'run.log has no "an: pipe listening"' }
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
    & $add (Test-SettingsStyle -Bytes $bytes)
    $now = ConvertFrom-SettingsBytes -Bytes $bytes
    $originalBytes = [IO.File]::ReadAllBytes((Join-Path $script:OriginalsDir "$Name.settings.json"))
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
        # Settings says why the status line of .claude-work was left alone.
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
            if ([Convert]::ToBase64String($got) -cne [Convert]::ToBase64String($want)) {
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
        $differences = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P) -Exclude 'AppData/*')
        foreach ($line in (Test-OnlyBackupsAdded -Differences $differences)) { $problems.Add($line) }
        foreach ($line in $differences) { Write-PhaseLog "  P: $line" }
        if ($problems.Count) { throw ("after Turn off:`n  " + ($problems -join "`n  ")) }
    } finally {
        Save-LiveRunLog
    }
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
    $changed = @(Compare-Hashes -Before $script:ProfileHashes -After (Get-TreeHash -Root $script:P) -Filter '.claude*/settings.json')
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
# and not run. Phases 10-12 of the design are added here, between 9 and 13.
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
    Write-Results
}
exit ([int](-not $ok))
