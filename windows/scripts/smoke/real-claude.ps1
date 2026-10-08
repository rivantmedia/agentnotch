#Requires -Version 7.3
<#
.SYNOPSIS
    The hermetic real-Claude-Code job (Maintainer decision Q3): the installed app against the
    pinned, real Claude Code, in a temporary profile, on the Windows CI runner only.

.DESCRIPTION
    DESIGN-WIN §7.7 and the Maintainer decisions (Q3). Run by the `real-claude` job of
    agentnotch-windows.yml after the build job, on a fresh runner:

        windows\scripts\smoke\real-claude.ps1 -GateOnly            (the job's first step)
        windows\scripts\smoke\real-claude.ps1 -Installer dl\AgentNotch-<V>-Setup.exe -Artifacts out\real-claude

    This is the ONE place real Claude Code ever runs, and only here: never on a maintainer's Mac,
    never in a test, never with a real key or login (none exist on the runner, and the job is
    given no secret). Everything happens inside a temporary profile P:

      - the pinned Claude Code is installed with npm into a temporary prefix (the only network
        use), its package checked against the pinned version and integrity;
      - a Windows Firewall rule blocks the installed claude.exe (and a copy where its own
        installer would put one) from every address but loopback; disabled firewall profiles are
        turned on for the run and off again afterwards;
      - Claude Code runs with USERPROFILE/HOME/APPDATA/LOCALAPPDATA/TEMP in P, CLAUDE_CONFIG_DIR =
        P\.claude-real, an obviously fake ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL = smoke\fake-anthropic.mjs
        on 127.0.0.1, CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC / DISABLE_AUTOUPDATER /
        DISABLE_TELEMETRY / DISABLE_ERROR_REPORTING set, and every inherited ANTHROPIC_* / CLAUDE*
        variable removed;
      - the app (installed from the build job's installer) runs with USERPROFILE = P and gets
        consent through its Settings page over the DevTools port, as smoke phase 7 does.

    Scenarios (headless vs interactive was read from 2.1.285's bundle, see drive-claude.mjs):
      1. a Bash tool_use: the PreToolUse / PostToolUse entries fire (the app's, and this job's
         env probe in string form, plus exec form when the facts file says the pinned version
         runs it); the probe shows CLAUDE_PID and CLAUDE_CONFIG_DIR; sessions\<pid>.json is in
         the profile; the engine files the session under P\.claude-real's account;
      2. PermissionRequest answered from the panel: Allow (the command ran), then Deny (it did
         not; Claude Code got the denial);
      3. AskUserQuestion answered with a chip;
      4. ExitPlanMode (a plan-mode run) approved;
      5. an interactive session in a ConPTY: the status line wrapper runs Claude Code's status
         line and the original command; the ring's readings stay or grow. (An API-key session's
         status line never carries rate_limits: 2.1.285 reads the unified rate-limit headers only
         for a claude.ai login or a gateway, so the readings cannot rise here.)
    Then: the fake API saw only the fake key and no Authorization header, no login file and no
    Credential Manager item appeared, the runner's Known-Folder profile is unchanged.

    When exec form fired, the job summary records the version and the run (the evidence a person
    may copy into exec_form_min / exec_form_evidence of claude-code-facts.json).

    Everything it leaves is in -Artifacts: the fake API logs, Claude Code's event streams, the
    env probe's lines, the ConPTY screen log, the app's logs and run.log, real-claude.log.
#>
[CmdletBinding()]
param(
    # The installer the build job made.
    [string]$Installer = '',
    # Where the logs go (uploaded by the job).
    [string]$Artifacts = 'out\real-claude',
    # smoke\gates.json (a parameter so the tests can point it elsewhere).
    [string]$GatesFile = (Join-Path $PSScriptRoot 'gates.json'),
    # Only say whether the realClaude gate is open (GITHUB_OUTPUT open=true|false); installs nothing.
    [switch]$GateOnly
)

Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

# Taken before the smoke script is dot-sourced: its param block reuses some of these names.
$RcOptions = @{ Installer = $Installer; Artifacts = $Artifacts; GatesFile = $GatesFile; GateOnly = [bool]$GateOnly }
$RcDotSourced = $MyInvocation.InvocationName -eq '.'
# The smoke script's helpers (gates, hashes, processes by handle, the DevTools driver, `control
# status`, the UI selector table). Dot-sourcing it defines them and runs nothing.
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'agentnotch-smoke.ps1')

# --- what is pinned ------------------------------------------------------------------------------

# The one Claude Code this job installs. 2.1.285 because it is the newest version the committed
# facts file has read (2026-10-01) and the one whose bundle was read to decide how this job drives
# it (headless stream-json for scenarios 1-4, a ConPTY for the status line). The weekly facts job
# reports newer versions; moving the pin means reading the new bundle for the same questions and
# updating the integrity (npm view @anthropic-ai/claude-code-win32-x64@<V> dist.integrity).
$script:ClaudeCodeVersion = '2.1.285'
$script:ClaudeCodePackage = '@anthropic-ai/claude-code'
$script:ClaudeCodeNativePackage = '@anthropic-ai/claude-code-win32-x64'
$script:ClaudeCodeNativeIntegrity = 'sha512-7TR0I2gOkYBADZlazRQERyP9WHCOKTZPPUSYzoxHP5EdHNvd9ZRYfIHJwydRfECpm6EYjGZ9goB/ACPJOMVdww=='

# Obviously not a key; fake-anthropic.mjs accepts exactly this and nothing else.
$script:FakeKey = 'fake-key-not-a-secret'
# The entrypoint of the headless (stream-json) runs: the VS Code extension's (see Start-ClaudeDriver).
$script:HeadlessEntrypoint = 'claude-vscode'
$script:ConfigFolderName = '.claude-real'
# The temporary account's identity (no token: Claude Code signs in with the fake key). The
# engine files P\.claude-real under it.
$script:Identity = [ordered]@{
    accountUuid      = '33333333-3333-4333-8333-333333333333'
    emailAddress     = 'real-claude@example.invalid'
    organizationUuid = 'cccccccc-cccc-4ccc-8ccc-cccccccccccc'
    organizationName = 'Hermetic Org'
    displayName      = 'Hermetic'
}
$script:FirewallGroup = 'AgentNotch hermetic Claude Code'
# Every address but loopback (127.0.0.0/8 and ::1).
$script:NonLoopbackRanges = @('0.0.0.0-126.255.255.255', '128.0.0.0-255.255.255.255', '::2-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff')
# What Claude Code may leave at the top of its config folder and in its sessions folder; any
# other file there is reported (a login file would be one). Directories are listed in the log.
$script:ConfigFileAllowList = @(
    '^settings\.json$'
    '^settings\.json\.agentnotch[^\\/]*\.bak$'
    '^\.claude\.json$'
    '^\.claude\.json\.(backup|bak|lock|tmp)[^\\/]*$'
    '^history\.jsonl$'
    # Claude Code's own housekeeping markers (a timestamp each, no login): 2.1.285's bundle lists
    # exactly these as its sentinel files (`[".npm-cache-cleanup",".version-cleanup",
    # ".last-cleanup",".deep-link-register-failed"]`); run 37720051731 left .last-cleanup.
    '^\.(npm-cache-cleanup|version-cleanup|last-cleanup|deep-link-register-failed)$'
)
$script:SessionsFileAllowList = @('^\d+\.json$', '^\.fleetview-heartbeat$')

# --- the gate ----------------------------------------------------------------------------------------

function Get-RealClaudeGate {
    param([Parameter(Mandatory)]$Gates)
    $open = Test-GateOpen -Gates $Gates -Name 'realClaude'
    $message = if ($open) { 'the realClaude gate is open: running the pinned Claude Code' } else { "waiting for the realClaude gate ($($Gates.Why['realClaude'])): nothing installed, nothing run" }
    [pscustomobject]@{ Open = $open; Message = $message }
}

# Where real Claude Code may run: a GitHub-hosted Windows runner, thrown away after the job.
# Never a developer's PC or a self-hosted runner (whose profile and logins are real), whatever
# the gate says. Answers why not, or $null.
function Get-RunnerRefusal {
    param([bool]$OnWindows = $IsWindows, [System.Collections.IDictionary]$Environment = $null)
    if ($null -eq $Environment) {
        $Environment = @{
            GITHUB_ACTIONS     = $env:GITHUB_ACTIONS
            RUNNER_ENVIRONMENT = $env:RUNNER_ENVIRONMENT
            RUNNER_TEMP        = $env:RUNNER_TEMP
        }
    }
    if (-not $OnWindows) { return 'the hermetic Claude Code job runs on the Windows CI runner only, never on this machine' }
    if ($Environment['GITHUB_ACTIONS'] -ne 'true') { return 'the hermetic Claude Code job runs in GitHub Actions only (GITHUB_ACTIONS is not true)' }
    if ($Environment['RUNNER_ENVIRONMENT'] -ne 'github-hosted') { return "the hermetic Claude Code job runs on a GitHub-hosted runner only, not '$($Environment['RUNNER_ENVIRONMENT'])'" }
    if (-not $Environment['RUNNER_TEMP']) { return "the hermetic Claude Code job needs the runner's RUNNER_TEMP" }
    $null
}

# --- pure helpers (tested on any machine by windows/tools/tests/real-claude.tests.ps1) ----------------

# Names never handed to Claude Code from the runner: anything that could select another account,
# endpoint or login, and the runner's own tokens.
function Test-ScrubbedClaudeName {
    param([Parameter(Mandatory)][string]$Name)
    $Name -match '^(?i)(ANTHROPIC_.*|CLAUDE.*|AI_AGENT|ACTIONS_ID_TOKEN_.*|ACTIONS_RUNTIME_TOKEN|GITHUB_TOKEN|GH_TOKEN)$'
}

# Claude Code's whole environment: the runner's (minus the scrubbed names) with the temporary
# profile, the fake key and the fake API. Case-insensitive, as Windows treats names.
function Get-ClaudeEnvironment {
    param(
        [Parameter(Mandatory)][System.Collections.IDictionary]$Base,
        [Parameter(Mandatory)][string]$Profile,
        [Parameter(Mandatory)][string]$ConfigDir,
        [Parameter(Mandatory)][string]$BaseUrl
    )
    $environment = [Collections.Generic.Dictionary[string, string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($key in $Base.Keys) {
        if (-not (Test-ScrubbedClaudeName -Name ([string]$key))) { $environment[[string]$key] = [string]$Base[$key] }
    }
    $overrides = [ordered]@{
        USERPROFILE                              = $Profile
        HOME                                     = $Profile
        APPDATA                                  = "$Profile\AppData\Roaming"
        LOCALAPPDATA                             = "$Profile\AppData\Local"
        TEMP                                     = "$Profile\Temp"
        TMP                                      = "$Profile\Temp"
        CLAUDE_CONFIG_DIR                        = $ConfigDir
        ANTHROPIC_API_KEY                        = $script:FakeKey
        ANTHROPIC_BASE_URL                       = $BaseUrl
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = '1'
        DISABLE_AUTOUPDATER                      = '1'
        DISABLE_TELEMETRY                        = '1'
        DISABLE_ERROR_REPORTING                  = '1'
        # Deferred tools (ExitPlanMode, AskUserQuestion) stay in the tool list, so the fake model
        # can call them without a tool search first.
        ENABLE_TOOL_SEARCH                       = 'false'
    }
    foreach ($key in $overrides.Keys) { $environment[$key] = [string]$overrides[$key] }
    $environment
}

# A Windows environment block (KEY=VALUE, NUL-separated, sorted, one more NUL at the end; the
# marshaller adds the last).
function ConvertTo-EnvironmentBlock {
    param([Parameter(Mandatory)][System.Collections.IDictionary]$Environment)
    $keys = [string[]]@($Environment.Keys)
    [Array]::Sort($keys, [StringComparer]::OrdinalIgnoreCase)
    (($keys | ForEach-Object { "$_=$($Environment[$_])" }) -join "`0") + "`0"
}

# A path as a string command can carry it in Git Bash and PowerShell (DESIGN-WIN §3, the same
# rule as the app's string form): forward slashes, no space or quote.
function ConvertTo-CommandPath {
    param([Parameter(Mandatory)][string]$Path)
    $forward = $Path.Replace('\', '/')
    if ($forward -notmatch '^[A-Za-z0-9_.:/~-]+$') { throw "the path $Path cannot be written into a string command (it holds a space or a special character)" }
    $forward
}

# The temporary account's .claude.json: onboarding done, the fake key approved (Claude Code keeps
# the last 20 characters), the work folder trusted under every spelling Claude Code may look up.
function New-ClaudeAccountJson {
    param([Parameter(Mandatory)][string]$WorkDir)
    $projects = [ordered]@{}
    foreach ($spelling in @($WorkDir, $WorkDir.Replace('\', '/'))) {
        $projects[$spelling] = [ordered]@{ hasTrustDialogAccepted = $true; hasCompletedProjectOnboarding = $true }
    }
    $key = $script:FakeKey.Trim()
    $tail = $key.Substring([Math]::Max(0, $key.Length - 20))
    [ordered]@{
        hasCompletedOnboarding = $true
        theme                  = 'dark'
        oauthAccount           = $script:Identity
        customApiKeyResponses  = [ordered]@{ approved = @($tail); rejected = @() }
        projects               = $projects
    } | ConvertTo-Json -Depth 10
}

# The temporary settings.json, before the app's consent: this job's env probe on PreToolUse and
# PostToolUse for Bash (string form always; exec form only when the pinned version runs it, since
# an older one drops `args` and would run node bare), and a status line the app's wrapper will
# re-run with Git Bash (forward slashes, no `\`), which saves what Claude Code fed it.
function New-RealClaudeSettings {
    param(
        [Parameter(Mandatory)][string]$NodeExe, [Parameter(Mandatory)][string]$ProbeScript,
        [Parameter(Mandatory)][string]$ProbeOut, [Parameter(Mandatory)][string]$StatusInput,
        [Parameter(Mandatory)][bool]$ExecForm
    )
    $script = ConvertTo-CommandPath $ProbeScript
    $out = ConvertTo-CommandPath $ProbeOut
    $hooks = @([ordered]@{ type = 'command'; command = "node $script $out string" })
    if ($ExecForm) { $hooks += [ordered]@{ type = 'command'; command = $NodeExe; args = @($ProbeScript, $ProbeOut, 'exec') } }
    $group = { [ordered]@{ matcher = 'Bash'; hooks = $hooks } }
    [ordered]@{
        hooks      = [ordered]@{ PreToolUse = @(& $group); PostToolUse = @(& $group) }
        statusLine = [ordered]@{ type = 'command'; command = "cat > $(ConvertTo-CommandPath $StatusInput); echo real-claude-status" }
    } | ConvertTo-Json -Depth 10
}

# Whether the facts file says this version runs exec-form hook entries ($null when unknown).
function Get-ExecFormFact {
    param([Parameter(Mandatory)]$Facts, [Parameter(Mandatory)][string]$Version)
    foreach ($entry in @($Facts.versions)) {
        if ($entry.version -eq $Version) {
            $value = Get-Prop $entry 'hook_exec_form'
            if ($value -is [bool]) { return $value }
            return $null
        }
    }
    $null
}

# The ring of an account: claude-acct-<12 hex of SHA-256(accountUuid)>.
function Get-RingId {
    param([Parameter(Mandatory)][string]$AccountUuid)
    $hash = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($AccountUuid))
    'claude-acct-' + ([Convert]::ToHexString($hash).ToLowerInvariant().Substring(0, 12))
}

# Names that are not on an allow-list (regexes), one line each.
function Get-UnexpectedNames {
    param([Parameter(Mandatory)][AllowEmptyCollection()][string[]]$Names, [Parameter(Mandatory)][string[]]$AllowList)
    foreach ($name in $Names) {
        $allowed = $false
        foreach ($pattern in $AllowList) { if ($name -match $pattern) { $allowed = $true; break } }
        if (-not $allowed) { $name }
    }
}

function ConvertFrom-JsonLines {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    foreach ($line in ($Text -split "`r?`n")) {
        if (-not $line.Trim()) { continue }
        try { $line | ConvertFrom-Json -AsHashtable -Depth 64 } catch { <# a line that is not JSON (stderr noise) #> }
    }
}

function Get-BlockText {
    param($Content)
    if ($null -eq $Content) { return '' }
    if ($Content -is [string]) { return $Content }
    (@($Content) | ForEach-Object { if ($_ -is [System.Collections.IDictionary] -and $_.Contains('text')) { [string]$_['text'] } else { '' } }) -join "`n"
}

# Every tool call of a stream-json session with its result: id, name, input, is_error, text.
function Get-ToolCalls {
    param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Events)
    $calls = [ordered]@{}
    foreach ($event in $Events) {
        if ($event -isnot [System.Collections.IDictionary] -or -not $event.Contains('message')) { continue }
        $message = $event['message']
        if ($message -isnot [System.Collections.IDictionary] -or $message['content'] -is [string]) { continue }
        foreach ($block in @($message['content'])) {
            if ($block -isnot [System.Collections.IDictionary]) { continue }
            if ($event['type'] -eq 'assistant' -and $block['type'] -eq 'tool_use') {
                $calls[[string]$block['id']] = [pscustomobject]@{ Id = [string]$block['id']; Name = [string]$block['name']; Input = $block['input']; Answered = $false; IsError = $false; Text = '' }
            } elseif ($event['type'] -eq 'user' -and $block['type'] -eq 'tool_result') {
                $call = $calls[[string]$block['tool_use_id']]
                if ($call) {
                    $call.Answered = $true
                    $call.IsError = $block.Contains('is_error') -and $block['is_error'] -eq $true
                    $call.Text = Get-BlockText $block['content']
                }
            }
        }
    }
    @($calls.Values)
}

# The hook events Claude Code streamed (--include-hook-events): event -> outcomes seen.
function Get-HookOutcomes {
    param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Events)
    $outcomes = [ordered]@{}
    foreach ($event in $Events) {
        if ($event -isnot [System.Collections.IDictionary] -or $event['type'] -ne 'system' -or $event['subtype'] -ne 'hook_response') { continue }
        $name = [string]$event['hook_event']
        if (-not $outcomes.Contains($name)) { $outcomes[$name] = [Collections.Generic.List[string]]::new() }
        $outcomes[$name].Add([string]$event['outcome'])
    }
    $outcomes
}

# Claude Code's connection warm-up: at start it sends `HEAD <ANTHROPIC_BASE_URL>/api/hello` with
# no key, no body and its errors ignored (2.1.285's bundle: `preconnectFired`, `{method:"HEAD",
# signal:AbortSignal.timeout(1e4)}).catch(()=>{})`; run 37709649061 logged it). It goes to the fake
# API like everything else and carries nothing of the user's, so exactly that request may come
# keyless; any other keyless request, or this one with a body, a key or an Authorization header,
# still fails.
function Test-KeylessPreconnect {
    param([Parameter(Mandatory)]$Entry)
    $headers = @($Entry['headers'])
    $Entry['method'] -eq 'HEAD' -and $Entry['path'] -eq '/api/hello' -and $Entry['apiKey'] -eq 'absent' -and
        -not $Entry.Contains('authorization') -and -not ($headers -contains 'content-length') -and -not ($headers -contains 'transfer-encoding')
}

# What the fake Messages API's log says against hermeticity: every request carried exactly the
# fake key (the log says ok/bad/absent, never the value) and no Authorization header, apart from
# Claude Code's keyless warm-up above.
function Test-FakeApiLog {
    param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Entries, [Parameter(Mandatory)][string]$Name)
    if (-not $Entries.Count) { "${Name}: the fake API saw no request" }
    foreach ($entry in $Entries) {
        $what = "$($entry['method']) $($entry['path'])"
        if ($entry['apiKey'] -ne 'ok' -and -not (Test-KeylessPreconnect $entry)) { "${Name}: $what came with the key $($entry['apiKey'])" }
        if ($entry.Contains('authorization')) { "${Name}: $what carried an Authorization header" }
    }
    if (-not @($Entries | Where-Object { $_['path'] -eq '/v1/messages' -and $_['reply'] -eq 'turn' }).Count) { "${Name}: no scripted turn was asked for" }
}

# `cmdkey /list`: the targets it names.
function ConvertFrom-CmdkeyList {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    foreach ($line in ($Text -split "`r?`n")) {
        if ($line -match '^\s*Target:\s*(.+?)\s*$') { $Matches[1] }
    }
}

# Terminal output without its escape sequences (for finding words on a ConPTY screen).
function Remove-TerminalEscapes {
    param([Parameter(Mandatory)][AllowEmptyString()][string]$Text)
    $Text -replace '\x1b\][^\x07\x1b]*(\x07|\x1b\\)', '' -replace '\x1b\[[0-9;?<=>]*[ -/]*[@-~]', '' -replace '\x1b[()*+][0-9A-Za-z]', '' -replace '\x1b[@-Z\\-_]', ''
}

# --- the run's state ---------------------------------------------------------------------------------

$script:Log = $null
function Write-RcLog {
    param([Parameter(ValueFromPipeline)][AllowEmptyString()][string]$Text)
    process {
        Write-Host $Text
        if ($script:Log) { Add-Content -LiteralPath $script:Log -Value $Text }
    }
}

function Initialize-RealClaudeContext {
    param([Parameter(Mandatory)][string]$Installer, [Parameter(Mandatory)][string]$Artifacts)
    $temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $script:Work = Join-Path $temp 'rc'
    if (Test-Path -LiteralPath $script:Work) { Remove-Item -LiteralPath $script:Work -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $script:Work | Out-Null
    $script:Installer = (Resolve-Path -LiteralPath $Installer).ProviderPath
    $script:ArtifactsDir = (New-Item -ItemType Directory -Force -Path $Artifacts).FullName
    $script:Log = Join-Path $script:ArtifactsDir 'real-claude.log'
    $script:Temp = $script:Work
    $script:P = Join-Path $script:Work 'profile'
    $script:ConfigDir = Join-Path $script:P $script:ConfigFolderName
    $script:WorkDir = Join-Path $script:P 'work'
    $script:NpmPrefix = Join-Path $script:Work 'claude-code'
    $script:ProbeOut = Join-Path $script:Work 'env-probe.jsonl'
    $script:StatusInput = Join-Path $script:Work 'statusline-input.json'
    $script:InstallDir = Join-Path $env:LOCALAPPDATA 'Agent Notch'
    $script:AppExe = Join-Path $script:InstallDir 'agentnotch.exe'
    $script:HookExe = Join-Path $script:InstallDir 'agentnotch-hook.exe'
    $script:DataRoots = @(Get-DataRoots -Profile $script:P)
    # The smoke helpers' environment names the fake claude's settings; inert here (no fake claude).
    $script:FakeClaudeLog = Join-Path $script:Work 'no-fake-claude.log'
    $script:UsageFixture = ''
    $script:FirewallProfilesTurnedOn = @()
}

# --- Windows: install, firewall, ConPTY -----------------------------------------------------------------

function Install-App {
    Start-Process -FilePath $script:Installer -ArgumentList '/S' -Wait
    Wait-Until { (Test-Path $script:AppExe) -and (Test-Path $script:HookExe) } 60 'the installed files'
    Write-RcLog "the app is installed in $script:InstallDir"
}

# npm into a temporary prefix: the one network use of the job. Install scripts are not run (the
# wrapper package's postinstall only copies the native binary, which is used where npm put it).
function Install-ClaudeCode {
    $npm = (Get-Command npm -CommandType Application -ErrorAction Stop | Where-Object { $_.Name -like 'npm.cmd' } | Select-Object -First 1)
    if (-not $npm) { $npm = Get-Command npm -CommandType Application -ErrorAction Stop | Select-Object -First 1 }
    New-Item -ItemType Directory -Force -Path $script:NpmPrefix | Out-Null
    $npmrc = Join-Path $script:Work 'empty.npmrc'
    Set-Content -LiteralPath $npmrc -Value ''
    $arguments = @('install', '--prefix', $script:NpmPrefix, '--ignore-scripts', '--no-audit', '--no-fund', '--userconfig', $npmrc,
        '--cache', (Join-Path $script:Work 'npm-cache'), "$($script:ClaudeCodePackage)@$($script:ClaudeCodeVersion)")
    Write-RcLog "npm $($arguments -join ' ')"
    & $npm.Source @arguments 2>&1 | ForEach-Object { Write-RcLog "  $_" }
    if ($LASTEXITCODE) { throw "npm install exited with $LASTEXITCODE" }

    $native = Join-Path $script:NpmPrefix ("node_modules\" + $script:ClaudeCodeNativePackage.Replace('/', '\'))
    $manifest = Get-Content -LiteralPath (Join-Path $native 'package.json') -Raw | ConvertFrom-Json
    if ($manifest.version -ne $script:ClaudeCodeVersion) { throw "npm installed $($script:ClaudeCodeNativePackage) $($manifest.version), not $($script:ClaudeCodeVersion)" }
    $lock = Get-Content -LiteralPath (Join-Path $script:NpmPrefix 'package-lock.json') -Raw | ConvertFrom-Json -AsHashtable
    $entry = $lock['packages']["node_modules/$($script:ClaudeCodeNativePackage)"]
    if (-not $entry -or $entry['integrity'] -cne $script:ClaudeCodeNativeIntegrity) {
        throw "the integrity of $($script:ClaudeCodeNativePackage) is '$(if ($entry) { $entry['integrity'] })', pinned '$($script:ClaudeCodeNativeIntegrity)'"
    }
    $exe = Join-Path $native 'claude.exe'
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "no claude.exe in $native" }
    Write-RcLog "Claude Code $($script:ClaudeCodeVersion): $exe (integrity as pinned)"
    $exe
}

function Add-ClaudeFirewallRules {
    param([Parameter(Mandatory)][string[]]$Programs)
    $off = @(Get-NetFirewallProfile | Where-Object { [string]$_.Enabled -ne 'True' } | ForEach-Object { $_.Name })
    if ($off.Count) {
        Set-NetFirewallProfile -Name $off -Enabled True
        $script:FirewallProfilesTurnedOn = $off
        Write-RcLog "firewall profiles turned on for the run: $($off -join ', ')"
    }
    foreach ($program in $Programs) {
        New-NetFirewallRule -DisplayName "$($script:FirewallGroup): $program" -Group $script:FirewallGroup -Direction Outbound -Action Block `
            -Program $program -RemoteAddress $script:NonLoopbackRanges -Profile Any | Out-Null
        Write-RcLog "firewall: outbound from $program blocked except loopback"
    }
    $rules = @(Get-NetFirewallRule -Group $script:FirewallGroup -ErrorAction SilentlyContinue)
    if ($rules.Count -ne $Programs.Count) { throw "expected $($Programs.Count) firewall rule(s), found $($rules.Count)" }
    $still = @(Get-NetFirewallProfile | Where-Object { [string]$_.Enabled -ne 'True' })
    if ($still.Count) { throw "firewall profiles still off: $($still.Name -join ', ')" }
}

function Remove-ClaudeFirewallRules {
    Get-NetFirewallRule -Group $script:FirewallGroup -ErrorAction SilentlyContinue | Remove-NetFirewallRule -ErrorAction SilentlyContinue
    if ($script:FirewallProfilesTurnedOn.Count) {
        Set-NetFirewallProfile -Name $script:FirewallProfilesTurnedOn -Enabled False -ErrorAction SilentlyContinue
        Write-RcLog "firewall profiles turned off again: $($script:FirewallProfilesTurnedOn -join ', ')"
    }
}

function Get-CredentialTargets {
    $text = (& cmdkey.exe /list 2>&1 | Out-String)
    [string[]]@(ConvertFrom-CmdkeyList -Text $text)
}

# A pseudo console hosting one process (for Claude Code's terminal UI): its screen is read as text
# and keys are typed into it. The output pipe is always drained (a full pipe would stall the child).
function Add-ConPtyType {
    if ('AgentNotch.ConPty' -as [type]) { return }
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using Microsoft.Win32.SafeHandles;

namespace AgentNotch {
public sealed class ConPty : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct Coord { public short X; public short Y; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo {
        public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
        public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars;
        public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2;
        public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
    }
    [StructLayout(LayoutKind.Sequential)] struct StartupInfoEx { public StartupInfo StartupInfo; public IntPtr lpAttributeList; }
    [StructLayout(LayoutKind.Sequential)] struct ProcessInformation { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }

    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CreatePipe(out SafeFileHandle read, out SafeFileHandle write, IntPtr attributes, int size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern int CreatePseudoConsole(Coord size, SafeFileHandle input, SafeFileHandle output, uint flags, out IntPtr console);
    [DllImport("kernel32.dll", SetLastError = true)] static extern void ClosePseudoConsole(IntPtr console);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError = true)] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string application, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes,
        bool inheritHandles, uint flags, IntPtr environment, string directory, ref StartupInfoEx startup, out ProcessInformation info);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool CloseHandle(IntPtr handle);

    const uint ExtendedStartupInfoPresent = 0x00080000;
    const uint CreateUnicodeEnvironment = 0x00000400;
    const int UseStdHandles = 0x00000100;
    static readonly IntPtr PseudoConsoleAttribute = (IntPtr)0x00020016;

    IntPtr console = IntPtr.Zero, attributes = IntPtr.Zero, process = IntPtr.Zero;
    FileStream input, output, log;
    Thread reader;
    readonly StringBuilder screen = new StringBuilder();
    readonly Decoder decoder = new UTF8Encoding(false).GetDecoder();
    public int ProcessId { get; private set; }

    public static ConPty Start(string commandLine, string directory, string environmentBlock, short columns, short rows, string logPath) {
        var pty = new ConPty();
        SafeFileHandle inputRead, inputWrite, outputRead, outputWrite;
        if (!CreatePipe(out inputRead, out inputWrite, IntPtr.Zero, 0)) throw new InvalidOperationException("CreatePipe (input) failed: " + Marshal.GetLastWin32Error());
        if (!CreatePipe(out outputRead, out outputWrite, IntPtr.Zero, 0)) throw new InvalidOperationException("CreatePipe (output) failed: " + Marshal.GetLastWin32Error());
        int hr = CreatePseudoConsole(new Coord { X = columns, Y = rows }, inputRead, outputWrite, 0, out pty.console);
        if (hr != 0) throw new InvalidOperationException("CreatePseudoConsole failed: 0x" + hr.ToString("x8"));
        IntPtr size = IntPtr.Zero;
        InitializeProcThreadAttributeList(IntPtr.Zero, 1, 0, ref size);
        pty.attributes = Marshal.AllocHGlobal(size);
        if (!InitializeProcThreadAttributeList(pty.attributes, 1, 0, ref size)) throw new InvalidOperationException("InitializeProcThreadAttributeList failed: " + Marshal.GetLastWin32Error());
        if (!UpdateProcThreadAttribute(pty.attributes, 0, PseudoConsoleAttribute, pty.console, (IntPtr)IntPtr.Size, IntPtr.Zero, IntPtr.Zero))
            throw new InvalidOperationException("UpdateProcThreadAttribute failed: " + Marshal.GetLastWin32Error());
        var startup = new StartupInfoEx();
        startup.StartupInfo.cb = Marshal.SizeOf(typeof(StartupInfoEx));
        startup.lpAttributeList = pty.attributes;
        // Without this the child takes this job's redirected standard handles instead of the
        // pseudo console's: run 37717423874's Claude Code saw a pipe for stdout, ran as print
        // mode, wrote its answer into the job log and exited 0 with an empty screen. Null handles
        // under STARTF_USESTDHANDLES make it open the pseudo console's.
        startup.StartupInfo.dwFlags = UseStdHandles;
        startup.StartupInfo.hStdInput = IntPtr.Zero;
        startup.StartupInfo.hStdOutput = IntPtr.Zero;
        startup.StartupInfo.hStdError = IntPtr.Zero;
        IntPtr environment = Marshal.StringToHGlobalUni(environmentBlock);
        try {
            ProcessInformation info;
            if (!CreateProcessW(null, new StringBuilder(commandLine), IntPtr.Zero, IntPtr.Zero, false,
                    ExtendedStartupInfoPresent | CreateUnicodeEnvironment, environment, directory, ref startup, out info))
                throw new InvalidOperationException("CreateProcessW failed: " + Marshal.GetLastWin32Error());
            pty.process = info.hProcess;
            CloseHandle(info.hThread);
            pty.ProcessId = info.dwProcessId;
        } finally {
            Marshal.FreeHGlobal(environment);
        }
        // The pseudo console holds its own copies of these ends.
        inputRead.Dispose();
        outputWrite.Dispose();
        pty.input = new FileStream(inputWrite, FileAccess.Write, 4096, false);
        pty.output = new FileStream(outputRead, FileAccess.Read, 4096, false);
        pty.log = new FileStream(logPath, FileMode.Create, FileAccess.Write, FileShare.ReadWrite);
        pty.reader = new Thread(pty.Drain) { IsBackground = true };
        pty.reader.Start();
        return pty;
    }

    void Drain() {
        var buffer = new byte[8192];
        var chars = new char[8192 * 2];
        try {
            int n;
            while ((n = output.Read(buffer, 0, buffer.Length)) > 0) {
                lock (screen) {
                    log.Write(buffer, 0, n);
                    log.Flush();
                    int count = decoder.GetChars(buffer, 0, n, chars, 0);
                    screen.Append(chars, 0, count);
                }
            }
        } catch (IOException) {
            // the pseudo console closed
        } catch (ObjectDisposedException) {
        }
    }

    public string Text { get { lock (screen) { return screen.ToString(); } } }

    public void Type(string keys) {
        var bytes = new UTF8Encoding(false).GetBytes(keys);
        input.Write(bytes, 0, bytes.Length);
        input.Flush();
    }

    public bool WaitForExit(int milliseconds) { return WaitForSingleObject(process, (uint)milliseconds) == 0; }

    public int ExitCode {
        get { uint code; return GetExitCodeProcess(process, out code) ? unchecked((int)code) : -1; }
    }

    public void Dispose() {
        if (console != IntPtr.Zero) { ClosePseudoConsole(console); console = IntPtr.Zero; }
        if (reader != null) reader.Join(5000);
        if (input != null) input.Dispose();
        if (output != null) output.Dispose();
        if (log != null) lock (screen) { log.Dispose(); }
        if (attributes != IntPtr.Zero) { DeleteProcThreadAttributeList(attributes); Marshal.FreeHGlobal(attributes); attributes = IntPtr.Zero; }
        if (process != IntPtr.Zero) { CloseHandle(process); process = IntPtr.Zero; }
    }
}
}
'@
}

# --- the profile, the app, consent ------------------------------------------------------------------

function New-RealClaudeProfile {
    param([Parameter(Mandatory)][bool]$ExecForm)
    foreach ($dir in $script:P, $script:ConfigDir, $script:WorkDir, (Join-Path $script:P 'AppData\Roaming'), (Join-Path $script:P 'AppData\Local'), (Join-Path $script:P 'Temp')) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $utf8 = [Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText((Join-Path $script:ConfigDir '.claude.json'), (New-ClaudeAccountJson -WorkDir $script:WorkDir), $utf8)
    $settings = New-RealClaudeSettings -NodeExe (Get-NodePath) -ProbeScript (Join-Path $PSScriptRoot 'real-claude\env-probe.mjs') `
        -ProbeOut $script:ProbeOut -StatusInput $script:StatusInput -ExecForm $ExecForm
    [IO.File]::WriteAllText((Join-Path $script:ConfigDir 'settings.json'), $settings, $utf8)
    Write-RcLog "P = $script:P; CLAUDE_CONFIG_DIR = $script:ConfigDir; work folder $script:WorkDir"
    Write-RcLog "settings.json before consent:`n$settings"
}

function Start-RealClaudeApp {
    $script:CdpPort = Get-FreeTcpPort
    $environment = Merge-Environment @(@{
        USERPROFILE                 = $script:P
        AGENTNOTCH_NO_NOTIFICATIONS = '1'
    }, (Get-DevToolsEnvironment -Port $script:CdpPort -Log { param($line) Write-RcLog $line }))
    $script:LiveApp = Register-OwnProcess (Start-AppProcess -Exe $script:AppExe -Environment $environment)
    Write-RcLog "the app is running as process $($script:LiveApp.Id), DevTools on port $($script:CdpPort)"
    $expect = [ordered]@{ transport = 'listening'; accounts = '1'; hook_consent = 'unasked' }
    $status = Wait-ControlStatus -Seconds 90 -What 'the app to list the temporary account' -Expect $expect
    Write-RcLog ('control status: ' + (($status.GetEnumerator() | ForEach-Object { "$($_.Key): $($_.Value)" }) -join ', '))
}

# Turn on, from the Settings page, as smoke phase 7 does; returns the form the app wrote.
function Grant-HookConsent {
    $settingsPath = Join-Path $script:ConfigDir 'settings.json'
    $before = Get-FileSha256 $settingsPath
    $hookCopy = Join-Path $script:ConfigDir 'hooks\agentnotch-hook.exe'
    Invoke-CdpInvoke -Page $script:Ui.NotchPage -Command $script:Ui.OpenSettings | Out-Null
    Invoke-CdpClick -Page $script:Ui.SettingsPage -Selector $script:Ui.ConsentTurnOn
    Wait-Until { (Get-FileSha256 $settingsPath) -ne $before -and (Test-Path -LiteralPath $hookCopy) } 20 "the app's entries in $settingsPath"
    Start-Sleep -Milliseconds 700   # a pass writes its files one after the other; let it finish
    Wait-ControlStatus -Seconds 15 -What 'hook consent' -Expect @{ hook_consent = 'granted' } | Out-Null
    $settings = Read-Settings $settingsPath
    $own = @(Get-HookEntries -Settings $settings | Where-Object { $_.Event -eq 'PermissionRequest' } |
        Where-Object { Get-OwnForm -Entry $_ -ExpectedExe $hookCopy -Resolve { param($p) Resolve-LongPath -Path $p } })
    if (-not $own.Count) { throw 'after Turn on, settings.json has no PermissionRequest entry of the app' }
    $form = Get-OwnForm -Entry $own[0] -ExpectedExe $hookCopy -Resolve { param($p) Resolve-LongPath -Path $p }
    $probes = @(Get-HookEntries -Settings $settings | Where-Object { $_.Command -match 'env-probe\.mjs' -or ($null -ne $_.Args -and ($_.Args -join ' ') -match 'env-probe\.mjs') })
    if ($probes.Count -lt 2) { throw "after Turn on, this job's env probe entries are gone ($($probes.Count) left)" }
    Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $script:ArtifactsDir 'settings-after-consent.json') -Force
    Write-RcLog "consent given; the app wrote its entries in $form form: $($own[0].Command) $(@($own[0].Args) -join ' ')"
    $form
}

# --- the Claude Code runs -------------------------------------------------------------------------------

function Start-FakeApi {
    param([Parameter(Mandatory)][string]$Scenario, [Parameter(Mandatory)][string]$Name)
    $port = Get-FreeTcpPort
    $log = Join-Path $script:ArtifactsDir "fake-anthropic-$Name.jsonl"
    $run = Start-NodeScript -Script (Join-Path $PSScriptRoot 'fake-anthropic.mjs') -Arguments @('--port', [string]$port, '--scenario', (Join-Path $PSScriptRoot "real-claude\scenario-$Scenario.json"), '--log', $log)
    Wait-Until {
        $client = [Net.Sockets.TcpClient]::new()
        try { $client.Connect([Net.IPAddress]::Loopback, $port); $true } catch { $false } finally { $client.Dispose() }
    } 20 "the fake Messages API on port $port"
    [pscustomobject]@{ Run = $run; Url = "http://127.0.0.1:$port"; Log = $log; Name = $Name }
}

function Read-FakeApiLog {
    param([Parameter(Mandatory)]$Api)
    if (-not (Test-Path -LiteralPath $Api.Log)) { return @() }
    @(ConvertFrom-JsonLines -Text ([IO.File]::ReadAllText($Api.Log)))
}

function Get-ClaudeEnvironmentFor {
    param([Parameter(Mandatory)]$Api)
    Get-ClaudeEnvironment -Base ([Environment]::GetEnvironmentVariables()) -Profile $script:P -ConfigDir $script:ConfigDir -BaseUrl $Api.Url
}

# drive-claude.mjs with Claude Code's environment (and nothing of this script's own).
function Start-ClaudeDriver {
    param([Parameter(Mandatory)]$Api, [Parameter(Mandatory)][string]$Name, [Parameter(Mandatory)][string[]]$Prompts, [string[]]$ClaudeArgs = @())
    $events = Join-Path $script:ArtifactsDir "claude-events-$Name.jsonl"
    $state = Join-Path $script:Work "claude-state-$Name.json"
    $psi = [Diagnostics.ProcessStartInfo]::new((Get-NodePath))
    $psi.ArgumentList.Add((Join-Path $PSScriptRoot 'real-claude\drive-claude.mjs'))
    foreach ($argument in @('--claude', $script:ClaudeExe, '--cwd', $script:WorkDir, '--events', $events, '--state', $state, '--timeout', '600000')) { $psi.ArgumentList.Add($argument) }
    foreach ($prompt in $Prompts) { $psi.ArgumentList.Add('--prompt'); $psi.ArgumentList.Add($prompt) }
    foreach ($argument in $ClaudeArgs) { $psi.ArgumentList.Add('--arg'); $psi.ArgumentList.Add($argument) }
    $psi.Environment.Clear()
    $environment = Get-ClaudeEnvironmentFor -Api $Api
    # The stream-json host these runs stand for is the VS Code extension's chat panel, which starts
    # Claude Code this way with CLAUDE_CODE_ENTRYPOINT=claude-vscode (2.1.285 keeps a preset value).
    # Left unset, `-p` makes it sdk-cli, and the engine ignores sdk-* sessions as the Mac does
    # (HS 4.3, SessionFilter): run 37331085715 waited for a session the app rightly never listed.
    $environment['CLAUDE_CODE_ENTRYPOINT'] = $script:HeadlessEntrypoint
    # The app's hook exe says what it did (delivered, held, answered, no app) only with both
    # variables (agentnotch-hook trace.rs); the trace is uploaded with the other artifacts.
    $environment['AGENTNOTCH_DEV'] = '1'
    $environment['AGENTNOTCH_HOOK_TRACE'] = Join-Path $script:ArtifactsDir "hook-trace-$Name.log"
    foreach ($key in $environment.Keys) { $psi.Environment[$key] = $environment[$key] }
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($psi)
    [void](Register-OwnProcess $process)
    [pscustomobject]@{
        Run    = [pscustomobject]@{ Process = $process; Stdout = $process.StandardOutput.ReadToEndAsync(); Stderr = $process.StandardError.ReadToEndAsync() }
        Events = $events; State = $state; Name = $Name
    }
}

function Read-DriverState {
    param([Parameter(Mandatory)]$Driver)
    if (-not (Test-Path -LiteralPath $Driver.State)) { return $null }
    try { Get-Content -LiteralPath $Driver.State -Raw | ConvertFrom-Json -AsHashtable } catch { $null }
}

function Wait-DriverSession {
    param([Parameter(Mandatory)]$Driver)
    Wait-Until {
        $state = Read-DriverState $Driver
        if ($Driver.Run.Process.HasExited) { throw "Claude Code ended before its session started: $($Driver.Run.Stdout.Result) $($Driver.Run.Stderr.Result)" }
        $null -ne $state -and $state['session_id']
    } 120 "Claude Code's session to start ($($Driver.Name))"
    $state = Read-DriverState $Driver
    $mode = '?'
    if (Test-Path -LiteralPath $Driver.Events) {
        $init = @(ConvertFrom-JsonLines -Text ([IO.File]::ReadAllText($Driver.Events)) | Where-Object { $_['type'] -eq 'system' -and $_['subtype'] -eq 'init' })
        if ($init.Count) { $mode = [string]$init[0]['permissionMode'] }
    }
    Write-RcLog "Claude Code ($($Driver.Name)): process $($state['pid']), session $($state['session_id']), permission mode $mode"
    $state
}

function Wait-Driver {
    param([Parameter(Mandatory)]$Driver, [int]$TimeoutSeconds = 300)
    $result = Wait-NodeScript -Run $Driver.Run -TimeoutSeconds $TimeoutSeconds
    Write-RcLog "drive-claude ($($Driver.Name)) exited with $($result.ExitCode): $($result.Stdout.Trim())"
    if ($result.Stderr.Trim()) { Write-RcLog "  stderr: $($result.Stderr.Trim())" }
    if ($result.ExitCode -ne 0) { throw "Claude Code's $($Driver.Name) run did not finish cleanly (events: $($Driver.Events))" }
    if (-not (Test-Path -LiteralPath $Driver.Events)) { throw "Claude Code's $($Driver.Name) run printed nothing" }
    @(ConvertFrom-JsonLines -Text ([IO.File]::ReadAllText($Driver.Events)))
}

# Answers the session's pending request from the panel, the way a user does.
function Invoke-PanelAnswer {
    param([Parameter(Mandatory)][string]$SessionId, [Parameter(Mandatory)][string]$Answer)
    Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'panel_open' -Arguments @{ route = 'sessions'; reason = 'ring_click' } | Out-Null
    try { Invoke-CdpAnswerClick -Page $script:Ui.PanelPage -Selector ($script:Ui.Answer -f $Answer, $SessionId) -WaitSeconds 120 }
    catch {
        Write-PanelAnswerDiagnostics -SessionId $SessionId
        throw
    }
    Write-RcLog "  answered '$Answer' in the panel"
}

# When the answer button never came: how the engine and the panel see the session, so the next
# round knows which side dropped it.
function Write-PanelAnswerDiagnostics {
    param([Parameter(Mandatory)][string]$SessionId)
    try {
        $rows = @((Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'snapshot')['sessions'] | Where-Object { $_['session_id'] -eq $SessionId })
        Write-RcLog "  the engine's row: $(if ($rows.Count) { $rows[0] | ConvertTo-Json -Depth 8 -Compress } else { 'none' })"
        Write-RcLog "  control status: held $((Get-ControlStatus)['held'])"
    } catch { Write-RcLog "  (the engine's row could not be read: $($_.Exception.Message))" }
    $id = ConvertTo-Json $SessionId -Compress
    $expression = "(() => { const P = window.agentnotchPanel; if (!P) return 'no agentnotchPanel'; const r = P.row($id); " +
        "const lay = P._.listLayout(P._.view()); return JSON.stringify({ mode: document.body.className, hoverList: P._.state.hoverList, " +
        "row: r && { bucket: r.bucket, pending: r.pending, detail: r.detail }, " +
        "sections: lay && lay.sections ? lay.sections.map((s) => ({ bucket: s.bucket, collapsed: s.collapsed, ids: s.rows.map((x) => x.session_id) })) : null, " +
        "buttons: [...document.querySelectorAll('[data-an-session=' + JSON.stringify($id) + ']')].map((b) => b.getAttribute('data-an-action') + ':' + b.getAttribute('data-an-arg') + ':' + b.className) }); })()"
    try { Write-RcLog "  the panel: $(Invoke-Cdp -Arguments @('eval', $script:Ui.PanelPage, $expression))" }
    catch { Write-RcLog "  (the panel could not be read: $($_.Exception.Message))" }
}

# While Claude Code waits on the first permission: its registry entry and how the engine filed it.
function Test-LiveSession {
    param([Parameter(Mandatory)]$State)
    $registry = Join-Path $script:ConfigDir "sessions\$($State['pid']).json"
    Wait-Until { Test-Path -LiteralPath $registry } 30 "Claude Code's registry entry $registry"
    Copy-Item -LiteralPath $registry -Destination (Join-Path $script:ArtifactsDir 'session-registry-entry.json') -Force
    $entry = Get-Content -LiteralPath $registry -Raw | ConvertFrom-Json -AsHashtable
    if ($entry['sessionId'] -ne $State['session_id']) { throw "$registry names session $($entry['sessionId']), not $($State['session_id'])" }
    if (-not ([string]$entry['version']).StartsWith($script:ClaudeCodeVersion)) { throw "$registry says version $($entry['version'])" }
    Write-RcLog "sessions\$($State['pid']).json: status $($entry['status']), version $($entry['version'])"

    $ring = Get-RingId -AccountUuid $script:Identity.accountUuid
    $look = @{ Sessions = @(); Mine = @() }
    try {
        Wait-Until {
            $look.Sessions = @((Invoke-CdpCall -Page $script:Ui.NotchPage -Method 'snapshot')['sessions'])
            $look.Mine = @($look.Sessions | Where-Object { $_['session_id'] -eq $State['session_id'] })
            $look.Mine.Count -gt 0
        } 30 "the engine to list session $($State['session_id'])"
    } catch {
        throw "$($_.Exception.Message) (it lists: $(@($look.Sessions | ForEach-Object { $_['session_id'] }) -join ', '))"
    }
    $mine = $look.Mine
    if ($mine[0]['ring_id'] -ne $ring) { throw "the engine filed the session under $($mine[0]['ring_id']), not $ring (P\$($script:ConfigFolderName)'s account)" }
    Write-RcLog "the engine files the session under $ring ($($mine[0]['bucket']))"
}

function Get-ProbeLines { if (Test-Path -LiteralPath $script:ProbeOut) { @(ConvertFrom-JsonLines -Text ([IO.File]::ReadAllText($script:ProbeOut))) } else { @() } }

function Test-ProbeLines {
    param([Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Lines, [Parameter(Mandatory)][int]$ClaudePid, [Parameter(Mandatory)][bool]$ExecForm)
    $forms = if ($ExecForm) { @('string', 'exec') } else { @('string') }
    foreach ($form in $forms) {
        foreach ($event in 'PreToolUse', 'PostToolUse') {
            $seen = @($Lines | Where-Object { $_['form'] -eq $form -and $_['hook_event_name'] -eq $event })
            if (-not $seen.Count) { "the $form-form env probe never ran for $event"; continue }
            foreach ($line in $seen) {
                if ([string]$line['claude_pid'] -ne [string]$ClaudePid) { "the $form-form $event probe saw CLAUDE_PID '$($line['claude_pid'])', not $ClaudePid" }
                if ((ConvertTo-ComparablePath ([string]$line['claude_config_dir'])) -ne (ConvertTo-ComparablePath $script:ConfigDir)) {
                    "the $form-form $event probe saw CLAUDE_CONFIG_DIR '$($line['claude_config_dir'])'"
                }
            }
        }
    }
}

# Scenarios 1-3: one headless session, three prompts.
function Invoke-HeadlessScenarios {
    param([Parameter(Mandatory)][bool]$ExecForm)
    $api = Start-FakeApi -Scenario 'headless' -Name 'headless'
    $readingsBefore = [int](Get-ControlStatus)['readings']
    # 2.1.285 starts in auto mode when no mode is named (runs 37331085715 and 37335586678: init
    # said permissionMode auto, and the `touch` calls ran with no PermissionRequest at all). The
    # VS Code panel these runs stand for starts in its own setting, "default" unless the user
    # changed it, where a Bash call asks; so the run names that mode.
    $driver = Start-ClaudeDriver -Api $api -Name 'headless' -ClaudeArgs @('--permission-mode', 'default') -Prompts @(
        '[allow] Create the first marker file with Bash.', '[deny] Create the second marker file with Bash.', '[question] Ask me which colour to use.')
    $state = Wait-DriverSession -Driver $driver
    Test-LiveSession -State $state
    Invoke-PanelAnswer -SessionId $state['session_id'] -Answer 'allow'
    Invoke-PanelAnswer -SessionId $state['session_id'] -Answer 'deny'
    Invoke-PanelAnswer -SessionId $state['session_id'] -Answer 'option:0'
    $events = Wait-Driver -Driver $driver
    $problems = [Collections.Generic.List[string]]::new()

    $calls = @(Get-ToolCalls -Events $events)
    $bash = @($calls | Where-Object { $_.Name -eq 'Bash' })
    $commandOf = { param($call) if ($call.Input -is [System.Collections.IDictionary]) { [string]$call.Input['command'] } else { '' } }
    $allowed = @($bash | Where-Object { (& $commandOf $_) -match 'allowed' })
    $denied = @($bash | Where-Object { (& $commandOf $_) -match 'denied' })
    if (-not $allowed.Count -or -not $allowed[0].Answered -or $allowed[0].IsError) { $problems.Add("the allowed Bash call did not run: $(if ($allowed) { $allowed[0].Text })") }
    if (-not (Test-Path -LiteralPath (Join-Path $script:WorkDir 'real-claude-allowed.txt'))) { $problems.Add('the allowed command left no marker file') }
    if (-not $denied.Count -or -not $denied[0].Answered -or -not $denied[0].IsError) { $problems.Add("Claude Code did not get the denial: $(if ($denied) { $denied[0].Text })") }
    if (Test-Path -LiteralPath (Join-Path $script:WorkDir 'real-claude-denied.txt')) { $problems.Add('the denied command ran anyway') }
    $question = @($calls | Where-Object { $_.Name -eq 'AskUserQuestion' })
    if (-not $question.Count -or $question[0].IsError -or $question[0].Text -notmatch 'Red') { $problems.Add("AskUserQuestion was not answered with the chip: $(if ($question) { $question[0].Text })") }

    $outcomes = Get-HookOutcomes -Events $events
    foreach ($event in 'PreToolUse', 'PermissionRequest', 'PostToolUse') {
        if (-not $outcomes.Contains($event)) { $problems.Add("Claude Code streamed no $event hook outcome") }
        elseif (@($outcomes[$event] | Where-Object { $_ -ne 'success' }).Count) { $problems.Add("a $event hook did not succeed: $($outcomes[$event] -join ', ')") }
        else { Write-RcLog "  $event hooks: $($outcomes[$event].Count) x success" }
    }
    foreach ($line in (Test-ProbeLines -Lines (Get-ProbeLines) -ClaudePid ([int]$state['pid']) -ExecForm $ExecForm)) { $problems.Add($line) }

    $log = @(Read-FakeApiLog -Api $api)
    foreach ($line in (Test-FakeApiLog -Entries $log -Name 'headless')) { $problems.Add($line) }
    $afterResults = @($log | Where-Object { $_['reply'] -eq 'after_tool_result' }).Count
    if ($afterResults -lt 3) { $problems.Add("the fake API saw $afterResults tool result(s), expected 3 (allowed, denied, answered)") }
    $unknown = @($log | Where-Object { $_['status'] -eq 404 } | ForEach-Object { "$($_['method']) $($_['path'])" } | Select-Object -Unique)
    if ($unknown.Count) { Write-RcLog "  endpoints the fake API does not serve (404): $($unknown -join ', ')" }
    Wait-ControlStatus -Seconds 10 -What 'the readings to stay or grow' -Check ({ param($s) if ([int]$s['readings'] -lt $readingsBefore) { "readings fell from $readingsBefore to $($s['readings'])" } }.GetNewClosure()) | Out-Null
    Stop-OwnProcess -Process $api.Run.Process
    if ($problems.Count) { throw ("headless scenarios:`n  " + ($problems -join "`n  ")) }
    Write-RcLog 'scenarios 1-3 passed: hooks fired with CLAUDE_PID and CLAUDE_CONFIG_DIR, Allow ran the command, Deny stopped it, the chip answered'
}

# Scenario 4: a plan-mode session whose ExitPlanMode the panel approves.
function Invoke-PlanScenario {
    $api = Start-FakeApi -Scenario 'plan' -Name 'plan'
    $driver = Start-ClaudeDriver -Api $api -Name 'plan' -Prompts @('[plan] Plan the marker file.') -ClaudeArgs @('--permission-mode', 'plan')
    $state = Wait-DriverSession -Driver $driver
    Invoke-PanelAnswer -SessionId $state['session_id'] -Answer 'approve'
    $events = Wait-Driver -Driver $driver
    $problems = [Collections.Generic.List[string]]::new()
    $plan = @(Get-ToolCalls -Events $events | Where-Object { $_.Name -eq 'ExitPlanMode' })
    if (-not $plan.Count -or -not $plan[0].Answered -or $plan[0].IsError -or $plan[0].Text -notmatch 'approved') { $problems.Add("ExitPlanMode was not approved: $(if ($plan) { $plan[0].Text })") }
    foreach ($line in (Test-FakeApiLog -Entries @(Read-FakeApiLog -Api $api) -Name 'plan')) { $problems.Add($line) }
    Stop-OwnProcess -Process $api.Run.Process
    if ($problems.Count) { throw ("plan scenario:`n  " + ($problems -join "`n  ")) }
    Write-RcLog 'scenario 4 passed: the plan was approved from the panel'
}

# Scenario 5: the terminal UI in a ConPTY, so Claude Code runs its status line (the app's
# wrapper, which re-runs this job's original command).
function Invoke-InteractiveScenario {
    $api = Start-FakeApi -Scenario 'interactive' -Name 'interactive'
    $readingsBefore = [int](Get-ControlStatus)['readings']
    Add-ConPtyType
    $block = ConvertTo-EnvironmentBlock -Environment (Get-ClaudeEnvironmentFor -Api $api)
    $screenLog = Join-Path $script:ArtifactsDir 'conpty-screen.log'
    $commandLine = "`"$($script:ClaudeExe)`" `"[hello] Say hello.`""
    $pty = [AgentNotch.ConPty]::Start($commandLine, $script:WorkDir, $block, 120, 40, $screenLog)
    try {
        try { [void](Register-OwnProcess ([Diagnostics.Process]::GetProcessById($pty.ProcessId))) } catch { <# it already ended: the wait below says so #> }
        Write-RcLog "Claude Code (interactive) is process $($pty.ProcessId)"
        # A hashtable: Wait-Until runs the block in a scope of its own, so a plain variable set
        # there would be forgotten between polls.
        $seen = @{ Trust = $false }
        Wait-Until {
            $screen = Remove-TerminalEscapes -Text $pty.Text
            if (-not $seen.Trust -and $screen -match '(?i)trust' -and $screen -match '(?i)proceed') { $pty.Type("`r"); $seen.Trust = $true; Write-RcLog '  the folder trust question came up: confirmed' }
            if ($screen -match '(?i)custom API key') { throw "Claude Code asks about the API key (the approval in .claude.json did not match): see $screenLog" }
            if ($pty.WaitForExit(0)) { throw "Claude Code ended with $($pty.ExitCode): see $screenLog" }
            Test-Path -LiteralPath $script:StatusInput
        } 180 "the status line (the app's wrapper re-running this job's original command)"
        # The wrapper passes the original's output through; on screen it may be split by the
        # renderer's escapes, so its absence there is only reported (phase 8 checks the bytes).
        try { Wait-Until { (Remove-TerminalEscapes -Text $pty.Text) -match 'real-claude-status' } 20 'the status line text on the screen' }
        catch { Write-Host "::warning::real-claude: the status line ran, but its text was not found on the ConPTY screen (see $screenLog)" }
        Copy-Item -LiteralPath $script:StatusInput -Destination (Join-Path $script:ArtifactsDir 'statusline-input.json') -Force
        $status = Get-Content -LiteralPath $script:StatusInput -Raw | ConvertFrom-Json -AsHashtable
        $problems = [Collections.Generic.List[string]]::new()
        if ([string]$status['version'] -ne $script:ClaudeCodeVersion) { $problems.Add("the status line JSON says version '$($status['version'])'") }
        if (-not $status['session_id']) { $problems.Add('the status line JSON has no session_id') }
        Write-RcLog "status line: Claude Code $($status['version']), session $($status['session_id']), rate_limits $(if ($status.Contains('rate_limits')) { 'present' } else { 'absent (an API-key session)' })"
        Wait-ControlStatus -Seconds 15 -What 'the readings to stay or grow' -Check ({ param($s) if ([int]$s['readings'] -lt $readingsBefore) { "readings fell from $readingsBefore to $($s['readings'])" } }.GetNewClosure()) | Out-Null
        foreach ($line in (Test-FakeApiLog -Entries @(Read-FakeApiLog -Api $api) -Name 'interactive')) { $problems.Add($line) }
        if ($problems.Count) { throw ("interactive scenario:`n  " + ($problems -join "`n  ")) }
        # Ctrl+C twice is Claude Code's own way out.
        $pty.Type([string][char]3); Start-Sleep -Milliseconds 500; $pty.Type([string][char]3)
        if (-not $pty.WaitForExit(15000)) { Write-RcLog '  Claude Code did not exit on Ctrl+C within 15 s; it is stopped by handle' }
        Write-RcLog 'scenario 5 passed: the wrapped status line ran under the real terminal UI'
    } finally {
        $pty.Dispose()
        Stop-OwnProcess -Process $api.Run.Process
    }
}

# --- after the runs ---------------------------------------------------------------------------------------

function Test-NoLoginLeftBehind {
    param([Parameter(Mandatory)][string[]]$CredentialTargetsBefore, [Parameter(Mandatory)]$KnownProfileBefore)
    $problems = [Collections.Generic.List[string]]::new()
    $top = @(Get-ChildItem -LiteralPath $script:ConfigDir -Force -File | ForEach-Object { $_.Name })
    foreach ($name in (Get-UnexpectedNames -Names $top -AllowList $script:ConfigFileAllowList)) { $problems.Add("an unexpected file in the config folder: $name") }
    $sessions = Join-Path $script:ConfigDir 'sessions'
    if (Test-Path -LiteralPath $sessions) {
        $names = @(Get-ChildItem -LiteralPath $sessions -Force -File | ForEach-Object { $_.Name })
        foreach ($name in (Get-UnexpectedNames -Names $names -AllowList $script:SessionsFileAllowList)) { $problems.Add("an unexpected file in the sessions folder: $name") }
    }
    $dirs = @(Get-ChildItem -LiteralPath $script:ConfigDir -Force -Directory | ForEach-Object { $_.Name })
    Write-RcLog "config folder: files $($top -join ', '); folders $($dirs -join ', ')"
    $rootFiles = @(Get-ChildItem -LiteralPath $script:P -Force -File | ForEach-Object { $_.Name })
    if ($rootFiles.Count) { $problems.Add("files at the top of the temporary profile: $($rootFiles -join ', ')") }
    Write-RcLog "temporary profile folders: $(@(Get-ChildItem -LiteralPath $script:P -Force -Directory | ForEach-Object { $_.Name }) -join ', ')"
    if (Test-Path -LiteralPath (Join-Path $script:P '.local\bin\claude.exe')) { $problems.Add('Claude Code installed a copy of itself into the temporary profile') }

    $after = @(Get-CredentialTargets)
    $new = @($after | Where-Object { $_ -notin $CredentialTargetsBefore })
    if ($new.Count) { $problems.Add("new Credential Manager item(s): $($new -join ', ')") }
    $changed = @(Compare-Hashes -Before $KnownProfileBefore -After (Get-KnownProfileHashes))
    if ($changed.Count) { $problems.Add("the runner's own Claude profile changed: $($changed -join ', ')") }
    if ($problems.Count) { throw ("after the runs:`n  " + ($problems -join "`n  ")) }
    Write-RcLog 'no login file, no Credential Manager item, the runner profile untouched'
}

function Save-RealClaudeLogs {
    foreach ($root in $script:DataRoots) {
        $data = Join-Path $root 'Agent Notch'
        if (-not (Test-Path -LiteralPath $data)) { continue }
        $target = Join-Path $script:ArtifactsDir ("app-logs\" + ($root -replace '[:\\/]+', '_'))
        New-Item -ItemType Directory -Force -Path $target | Out-Null
        Get-ChildItem -LiteralPath $data -Filter '*.log' -File -ErrorAction SilentlyContinue | Copy-Item -Destination $target -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $script:ProbeOut) { Copy-Item -LiteralPath $script:ProbeOut -Destination $script:ArtifactsDir -Force }
}

function Write-ExecFormEvidence {
    param([Parameter(Mandatory)][string]$AppForm, [Parameter(Mandatory)][bool]$ProbeExec)
    $execFired = @(Get-ProbeLines | Where-Object { $_['form'] -eq 'exec' }).Count -gt 0
    $run = if ($env:GITHUB_RUN_ID) { "run $($env:GITHUB_RUN_ID) (attempt $($env:GITHUB_RUN_ATTEMPT)) of $($env:GITHUB_REPOSITORY)" } else { 'a local run' }
    $lines = @(
        '### Real Claude Code (hermetic)'
        ''
        "Claude Code $($script:ClaudeCodeVersion) ran scenarios 1-5 against the installed app. The app wrote its hook entries in **$AppForm** form."
    )
    if ($execFired -or $AppForm -eq 'exec') {
        $lines += "Exec form fired: Claude Code $($script:ClaudeCodeVersion), $run. Evidence for EXEC_FORM_MIN: a person may set ``exec_form_min`` / ``exec_form_evidence`` in windows/agentnotch-engine/tests/fixtures/claude-code-facts.json to this version and run."
    } elseif ($ProbeExec) {
        $lines += 'The exec-form probe entry was written but never ran.'
    }
    $lines | ForEach-Object { Write-RcLog $_ }
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value ($lines -join "`n") }
}

# --- main ------------------------------------------------------------------------------------------------

if ($RcDotSourced) { return }

try {
    $gate = Get-RealClaudeGate -Gates (Read-Gates -Path $RcOptions.GatesFile)
} catch {
    Write-Host "::error::$($_.Exception.Message)"
    exit 1
}
if ($env:GITHUB_OUTPUT) { Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "open=$(([string]$gate.Open).ToLowerInvariant())" }
if (-not $gate.Open) {
    Write-Host "::notice::real-claude: $($gate.Message)"
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY -Value "Real Claude Code (hermetic): $($gate.Message)" }
    exit 0
}
Write-Host $gate.Message
if ($RcOptions.GateOnly) { exit 0 }

$ok = $false
try {
    $refusal = Get-RunnerRefusal
    if ($refusal) { throw $refusal }
    if (-not $RcOptions.Installer) { throw '-Installer is required' }
    Initialize-RealClaudeContext -Installer $RcOptions.Installer -Artifacts $RcOptions.Artifacts
    $elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $elevated) { throw 'the firewall rule needs an elevated runner' }
    $problems = @(Get-PreflightProblems)
    if ($problems) { throw ($problems -join '; ') }
    $knownProfileBefore = Get-KnownProfileHashes
    $credentialsBefore = @(Get-CredentialTargets)
    Write-RcLog "runner profile: $($knownProfileBefore.Count) Claude file(s); Credential Manager: $($credentialsBefore.Count) item(s)"

    $facts = Get-Content -LiteralPath (Get-FactsPath) -Raw | ConvertFrom-Json -Depth 50
    $probeExec = (Get-ExecFormFact -Facts $facts -Version $script:ClaudeCodeVersion) -eq $true
    Write-RcLog "the facts file says $($script:ClaudeCodeVersion) runs exec-form entries: $probeExec"

    Install-App
    $script:ClaudeExe = Install-ClaudeCode
    New-RealClaudeProfile -ExecForm $probeExec
    Add-ClaudeFirewallRules -Programs @($script:ClaudeExe, (Join-Path $script:P '.local\bin\claude.exe'))

    Start-RealClaudeApp
    $appForm = Grant-HookConsent
    Save-LiveRunLog
    Invoke-HeadlessScenarios -ExecForm $probeExec
    Save-LiveRunLog
    Invoke-PlanScenario
    Save-LiveRunLog
    Invoke-InteractiveScenario
    Save-LiveRunLog
    Test-NoLoginLeftBehind -CredentialTargetsBefore $credentialsBefore -KnownProfileBefore $knownProfileBefore
    Write-ExecFormEvidence -AppForm $appForm -ProbeExec $probeExec
    $ok = $true
} catch {
    Write-RcLog "FAILED: $($_.Exception.Message)"
    Write-RcLog $_.ScriptStackTrace
    Write-Host "::error::real-claude: $($_.Exception.Message)"
} finally {
    Stop-OwnProcesses
    if ($IsWindows) { Clear-DevToolsOverride }
    if ($IsWindows -and $script:Log) {
        Remove-ClaudeFirewallRules
        Save-LiveRunLog
        Save-RealClaudeLogs
    }
}
exit ([int](-not $ok))
