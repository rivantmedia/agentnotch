#Requires -Version 7.3
# The parts of windows/scripts/smoke/real-claude.ps1 that decide what the hermetic job hands
# Claude Code and how it judges the run: the gate, the scrubbed environment, the temporary
# account and settings, the facts lookup, the ring id, the allow-lists, the stream-json reading
# and the fake API's log. Dot-sourcing the script defines its functions and runs nothing (and
# never Claude Code: what needs the Windows runner is not called here).
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$windowsDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $windowsDir 'scripts/smoke/real-claude.ps1')

$failures = [Collections.Generic.List[string]]::new()
function Test-Case([string]$Name, [scriptblock]$Body) {
    try {
        & $Body
        "ok   $Name"
    } catch {
        "FAIL $Name`n     $($_.Exception.Message)"
        $failures.Add($Name)
    }
}
function Assert-Equal($Actual, $Expected, [string]$What = 'value') {
    if ($Actual -cne $Expected) { throw "$What is <$Actual>, expected <$Expected>" }
}
function Assert-True($Condition, [string]$What) {
    if (-not $Condition) { throw "expected: $What" }
}
function Assert-Throws([scriptblock]$Body, [string]$Pattern) {
    try { & $Body } catch {
        if ($_.Exception.Message -notmatch $Pattern) { throw "threw <$($_.Exception.Message)>, expected <$Pattern>" }
        return
    }
    throw "did not throw (expected <$Pattern>)"
}

$scratch = Join-Path ([IO.Path]::GetTempPath()) "agentnotch-real-claude-tests-$PID"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
function Write-GatesFile([string]$Path, [bool]$RealClaude) {
    $names = 'engine', 'selftest', 'cloud', 'glue', 'realClaude', 'rustContract'
    $body = [ordered]@{}
    foreach ($name in $names) { $body[$name] = ($name -eq 'realClaude' -and $RealClaude) }
    $why = [ordered]@{}
    foreach ($name in $names) { $why[$name] = "package for $name" }
    $body['_why'] = $why
    $body | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Path
}

Test-Case 'the gate: closed says what it waits for and that nothing runs; open says so' {
    $closed = Join-Path $scratch 'closed.json'
    Write-GatesFile $closed $false
    $gate = Get-RealClaudeGate -Gates (Read-Gates -Path $closed)
    Assert-Equal $gate.Open $false 'Open'
    Assert-True ($gate.Message -match 'waiting for the realClaude gate \(package for realClaude\)') "the reason: $($gate.Message)"
    $open = Join-Path $scratch 'open.json'
    Write-GatesFile $open $true
    Assert-Equal (Get-RealClaudeGate -Gates (Read-Gates -Path $open)).Open $true 'Open'
}

Test-Case 'real Claude Code runs only on a GitHub-hosted Windows runner, whatever the gate says' {
    $hosted = @{ GITHUB_ACTIONS = 'true'; RUNNER_ENVIRONMENT = 'github-hosted'; RUNNER_TEMP = 'D:\a\_temp' }
    Assert-Equal (Get-RunnerRefusal -OnWindows $true -Environment $hosted) $null 'a GitHub-hosted Windows runner'
    Assert-True ((Get-RunnerRefusal -OnWindows $false -Environment $hosted) -match 'Windows CI runner only') 'not Windows'
    Assert-True ((Get-RunnerRefusal -OnWindows $true -Environment @{ RUNNER_ENVIRONMENT = 'github-hosted'; RUNNER_TEMP = 'x' }) -match 'GitHub Actions only') 'a PC'
    Assert-True ((Get-RunnerRefusal -OnWindows $true -Environment @{ GITHUB_ACTIONS = 'true'; RUNNER_ENVIRONMENT = 'self-hosted'; RUNNER_TEMP = 'x' }) -match "not 'self-hosted'") 'a self-hosted runner'
    Assert-True ((Get-RunnerRefusal -OnWindows $true -Environment @{ GITHUB_ACTIONS = 'true'; RUNNER_ENVIRONMENT = 'github-hosted' }) -match 'RUNNER_TEMP') 'no RUNNER_TEMP'

    # The script itself, with the gate open, on this machine: refused before anything is set up.
    $open = Join-Path $scratch 'open-run.json'
    Write-GatesFile $open $true
    $pwsh = (Get-Process -Id $PID).Path
    $artifacts = Join-Path $scratch 'rc-out'
    $output = & $pwsh -NoProfile -NonInteractive -File (Join-Path $windowsDir 'scripts/smoke/real-claude.ps1') `
        -Installer (Join-Path $scratch 'none.exe') -Artifacts $artifacts -GatesFile $open 2>&1 | Out-String
    Assert-Equal $LASTEXITCODE 1 'exit code'
    Assert-True ($output -match 'Windows CI runner only|GitHub Actions only|GitHub-hosted runner only') "the refusal: $output"
    Assert-True (-not (Test-Path -LiteralPath $artifacts)) 'nothing was set up'
}

Test-Case 'the committed gates file is readable and names realClaude' {
    $gate = Get-RealClaudeGate -Gates (Read-Gates -Path (Join-Path $windowsDir 'scripts/smoke/gates.json'))
    Assert-True ($gate.Open -is [bool]) 'a boolean gate'
}

Test-Case 'the scrubbed names: every account, endpoint and token variable of the runner' {
    foreach ($name in 'ANTHROPIC_API_KEY', 'anthropic_base_url', 'ANTHROPIC_AUTH_TOKEN', 'CLAUDE_CONFIG_DIR', 'CLAUDECODE', 'CLAUDE_CODE_OAUTH_TOKEN', 'AI_AGENT', 'GITHUB_TOKEN', 'GH_TOKEN', 'ACTIONS_RUNTIME_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL') {
        Assert-True (Test-ScrubbedClaudeName -Name $name) "$name is scrubbed"
    }
    foreach ($name in 'PATH', 'SystemRoot', 'GITHUB_WORKSPACE', 'RUNNER_TEMP', 'MY_CLAUDE') {
        Assert-True (-not (Test-ScrubbedClaudeName -Name $name)) "$name is kept"
    }
}

Test-Case "Claude Code's environment: the runner's minus the scrubbed names, plus the profile and the fake API" {
    $base = @{ PATH = 'C:\bin'; ANTHROPIC_AUTH_TOKEN = 'runner'; claude_config_dir = 'C:\real'; GITHUB_TOKEN = 'x'; USERPROFILE = 'C:\Users\runner' }
    $env = Get-ClaudeEnvironment -Base $base -Profile 'D:\t\rc\profile' -ConfigDir 'D:\t\rc\profile\.claude-real' -BaseUrl 'http://127.0.0.1:5555'
    Assert-Equal $env['PATH'] 'C:\bin' 'PATH'
    Assert-True (-not $env.ContainsKey('ANTHROPIC_AUTH_TOKEN')) 'no auth token'
    Assert-True (-not $env.ContainsKey('GITHUB_TOKEN')) 'no runner token'
    Assert-Equal $env['CLAUDE_CONFIG_DIR'] 'D:\t\rc\profile\.claude-real' 'CLAUDE_CONFIG_DIR (and only one spelling of it)'
    Assert-Equal @($env.Keys | Where-Object { $_ -ieq 'CLAUDE_CONFIG_DIR' }).Count 1 'spellings of CLAUDE_CONFIG_DIR'
    foreach ($pair in @{ USERPROFILE = 'D:\t\rc\profile'; HOME = 'D:\t\rc\profile'; APPDATA = 'D:\t\rc\profile\AppData\Roaming'; LOCALAPPDATA = 'D:\t\rc\profile\AppData\Local'; TEMP = 'D:\t\rc\profile\Temp'; ANTHROPIC_BASE_URL = 'http://127.0.0.1:5555'; ANTHROPIC_API_KEY = 'fake-key-not-a-secret'; CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = '1'; DISABLE_AUTOUPDATER = '1'; DISABLE_TELEMETRY = '1'; DISABLE_ERROR_REPORTING = '1'; ENABLE_TOOL_SEARCH = 'false' }.GetEnumerator()) {
        Assert-Equal $env[$pair.Key] $pair.Value $pair.Key
    }
    Assert-True ($env['ANTHROPIC_API_KEY'] -match 'fake') 'an obviously fake key'
}

Test-Case 'the environment block is sorted, NUL-separated and ends with a NUL' {
    $block = ConvertTo-EnvironmentBlock -Environment ([ordered]@{ b = '2'; A = '1'; c = 'x=y' })
    Assert-Equal $block "A=1`0b=2`0c=x=y`0" 'block'
}

Test-Case 'a string-command path: forward slashes; a space or a quote is refused' {
    Assert-Equal (ConvertTo-CommandPath 'D:\a\_temp\rc\env-probe.jsonl') 'D:/a/_temp/rc/env-probe.jsonl' 'path'
    Assert-Throws { ConvertTo-CommandPath 'C:\Program Files\x.mjs' } 'cannot be written'
    Assert-Throws { ConvertTo-CommandPath "C:\it's\x.mjs" } 'cannot be written'
}

Test-Case 'the temporary account: onboarded, signed in by the fake key only, the work folder trusted' {
    $json = New-ClaudeAccountJson -WorkDir 'D:\a\_temp\rc\profile\work' | ConvertFrom-Json -AsHashtable
    Assert-Equal $json['hasCompletedOnboarding'] $true 'onboarding'
    Assert-Equal $json['oauthAccount']['accountUuid'] '33333333-3333-4333-8333-333333333333' 'identity'
    Assert-Equal @($json['customApiKeyResponses']['approved']).Count 1 'approved keys'
    Assert-Equal $json['customApiKeyResponses']['approved'][0] 'ake-key-not-a-secret' 'the last 20 characters of the fake key'
    Assert-Equal $json['projects']['D:\a\_temp\rc\profile\work']['hasTrustDialogAccepted'] $true 'trusted (backslashes)'
    Assert-Equal $json['projects']['D:/a/_temp/rc/profile/work']['hasTrustDialogAccepted'] $true 'trusted (forward slashes)'
    $text = New-ClaudeAccountJson -WorkDir 'D:\w'
    foreach ($word in 'Token', 'token', 'refresh') { Assert-True (-not $text.Contains($word)) "no '$word' in the account file" }
}

Test-Case 'the settings before consent: the probe in string form, exec form only when allowed, a wrappable status line' {
    $make = { param($exec) New-RealClaudeSettings -NodeExe 'C:\hostedtoolcache\node\node.exe' -ProbeScript 'D:\a\repo\windows\scripts\smoke\real-claude\env-probe.mjs' -ProbeOut 'D:\a\_temp\rc\env-probe.jsonl' -StatusInput 'D:\a\_temp\rc\statusline-input.json' -ExecForm $exec | ConvertFrom-Json -AsHashtable }
    $plain = & $make $false
    foreach ($event in 'PreToolUse', 'PostToolUse') {
        $hooks = @($plain['hooks'][$event][0]['hooks'])
        Assert-Equal $plain['hooks'][$event][0]['matcher'] 'Bash' "$event matcher"
        Assert-Equal $hooks.Count 1 "$event entries without exec form"
        Assert-Equal $hooks[0]['command'] 'node D:/a/repo/windows/scripts/smoke/real-claude/env-probe.mjs D:/a/_temp/rc/env-probe.jsonl string' "$event string form"
    }
    $exec = & $make $true
    $hooks = @($exec['hooks']['PreToolUse'][0]['hooks'])
    Assert-Equal $hooks.Count 2 'entries with exec form'
    Assert-Equal $hooks[1]['command'] 'C:\hostedtoolcache\node\node.exe' 'exec command'
    Assert-Equal (@($hooks[1]['args']) -join '|') 'D:\a\repo\windows\scripts\smoke\real-claude\env-probe.mjs|D:\a\_temp\rc\env-probe.jsonl|exec' 'exec args'
    $status = $plain['statusLine']['command']
    Assert-Equal $status 'cat > D:/a/_temp/rc/statusline-input.json; echo real-claude-status' 'status line'
    # The app wraps someone else's status line only when it holds none of these (DESIGN-WIN §4.4).
    Assert-True ($status -notmatch '\\|\.ps1|\$env:|powershell|pwsh|cmd\.exe|cmd /c') 'the status line is wrappable'
    Assert-True (-not $plain['statusLine'].Contains('shell')) 'no shell key'
}

Test-Case 'the facts lookup: true, false and unknown' {
    $facts = '{"schema":1,"versions":[{"version":"2.1.285","hook_exec_form":true},{"version":"2.1.138","hook_exec_form":false},{"version":"2.1.0","hook_exec_form":null}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormFact -Facts $facts -Version '2.1.285') $true '2.1.285'
    Assert-Equal (Get-ExecFormFact -Facts $facts -Version '2.1.138') $false '2.1.138'
    Assert-Equal (Get-ExecFormFact -Facts $facts -Version '2.1.0') $null '2.1.0'
    Assert-Equal (Get-ExecFormFact -Facts $facts -Version '9.9.9') $null 'absent'
}

Test-Case 'the pinned version is one the committed facts file has read, and it runs exec form' {
    $facts = Get-Content -LiteralPath (Join-Path $windowsDir 'agentnotch-engine/tests/fixtures/claude-code-facts.json') -Raw | ConvertFrom-Json -Depth 50
    Assert-Equal (Get-ExecFormFact -Facts $facts -Version $script:ClaudeCodeVersion) $true "exec form for $($script:ClaudeCodeVersion)"
    Assert-True ($script:ClaudeCodeVersion -match '^\d+\.\d+\.\d+$') 'an exact version, never a tag'
    Assert-True ($script:ClaudeCodeNativeIntegrity -match '^sha512-[A-Za-z0-9+/]{86}==$') 'a sha512 integrity'
}

Test-Case 'the ring id: claude-acct- and 12 hex of SHA-256(accountUuid)' {
    $hash = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes('33333333-3333-4333-8333-333333333333'))
    $want = 'claude-acct-' + [Convert]::ToHexString($hash).ToLowerInvariant().Substring(0, 12)
    Assert-Equal (Get-RingId -AccountUuid '33333333-3333-4333-8333-333333333333') $want 'ring id'
    Assert-True ($want -match '^claude-acct-[0-9a-f]{12}$') 'shape'
}

Test-Case "the allow-lists: Claude Code's own files pass, anything else is named" {
    $names = @('settings.json', 'settings.json.agentnotch.original.bak', '.claude.json', '.claude.json.backup.1790000000000', 'history.jsonl', 'login.json', 'other.txt')
    Assert-Equal ((Get-UnexpectedNames -Names $names -AllowList $script:ConfigFileAllowList) -join ',') 'login.json,other.txt' 'unexpected'
    Assert-Equal ((Get-UnexpectedNames -Names @('4242.json', '.fleetview-heartbeat', '4242.lock') -AllowList $script:SessionsFileAllowList) -join ',') '4242.lock' 'sessions folder'
    Assert-Equal @(Get-UnexpectedNames -Names @() -AllowList $script:ConfigFileAllowList).Count 0 'nothing'
}

Test-Case 'stream-json: tool calls paired with their results, hook outcomes counted, noise skipped' {
    $text = @(
        '{"type":"system","subtype":"init","session_id":"s"}'
        'not json'
        '{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"touch real-claude-allowed.txt"}}]}}'
        '{"type":"system","subtype":"hook_response","hook_event":"PreToolUse","outcome":"success"}'
        '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"","is_error":false}]}}'
        '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"touch real-claude-denied.txt"}}]}}'
        '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"Denied by user via Agent Notch","is_error":true}]}}'
        '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"AskUserQuestion","input":{}}]}}'
        '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3","content":[{"type":"text","text":"User answered: Red"}]}]}}'
        '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t4","name":"Bash","input":null}]}}'
        '{"type":"user","message":{"content":"plain"}}'
        '{"type":"system","subtype":"hook_response","hook_event":"PreToolUse","outcome":"error"}'
    ) -join "`n"
    $events = @(ConvertFrom-JsonLines -Text $text)
    Assert-Equal $events.Count 11 'parsed lines'
    $calls = @(Get-ToolCalls -Events $events)
    Assert-Equal $calls.Count 4 'calls'
    Assert-Equal "$($calls[0].Answered)/$($calls[0].IsError)" 'True/False' 'allowed'
    Assert-Equal "$($calls[1].Answered)/$($calls[1].IsError)" 'True/True' 'denied'
    Assert-Equal $calls[1].Text 'Denied by user via Agent Notch' 'denial text'
    Assert-Equal $calls[2].Text 'User answered: Red' 'answer text from blocks'
    Assert-Equal $calls[3].Answered $false 'no result yet'
    $outcomes = Get-HookOutcomes -Events $events
    Assert-Equal ($outcomes['PreToolUse'] -join ',') 'success,error' 'outcomes'
}

Test-Case "the fake API's log: only the fake key, no Authorization header, a scripted turn" {
    $good = @(@{ method = 'POST'; path = '/v1/messages'; apiKey = 'ok'; reply = 'turn' }, @{ method = 'GET'; path = '/v1/models'; apiKey = 'ok' })
    Assert-Equal @(Test-FakeApiLog -Entries $good -Name 'x').Count 0 'problems'
    $bad = @(@{ method = 'POST'; path = '/v1/messages'; apiKey = 'bad'; reply = 'turn' }, @{ method = 'POST'; path = '/v1/messages'; apiKey = 'ok'; authorization = 'present'; reply = 'passive' })
    $problems = @(Test-FakeApiLog -Entries $bad -Name 'x')
    Assert-True ($problems -match 'came with the key bad') 'a wrong key is named'
    Assert-True ($problems -match 'Authorization') 'an Authorization header is named'
    Assert-True (@(Test-FakeApiLog -Entries @() -Name 'x') -match 'no request') 'an empty log'
    Assert-True (@(Test-FakeApiLog -Entries @(@{ method = 'POST'; path = '/v1/messages'; apiKey = 'ok'; reply = 'passive' }) -Name 'x') -match 'no scripted turn') 'no turn'
}

Test-Case 'cmdkey targets and terminal escapes' {
    $list = "`r`nCurrently stored credentials:`r`n`r`n    Target: LegacyGeneric:target=git:https://github.com`r`n    Type: Generic`r`n    Target: Domain:target=fileserver`r`n"
    Assert-Equal ((ConvertFrom-CmdkeyList -Text $list) -join '|') 'LegacyGeneric:target=git:https://github.com|Domain:target=fileserver' 'targets'
    Assert-Equal @(ConvertFrom-CmdkeyList -Text '').Count 0 'none'
    $screen = "`e[?25l`e[2J`e[1;1HDo you trust`e[0m the files?`e]0;title`a `e[38;5;12mYes, proceed`e(B"
    Assert-Equal (Remove-TerminalEscapes -Text $screen) 'Do you trust the files? Yes, proceed' 'stripped'
}

Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
if ($failures.Count) { "FAILED: $($failures -join ', ')"; exit 1 }
'all real-claude cases passed'
