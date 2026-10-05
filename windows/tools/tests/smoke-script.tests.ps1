#Requires -Version 7.3
# The parts of windows/scripts/agentnotch-smoke.ps1 that decide what a smoke run is and whether
# it passes: the gates, the refusal of a release with a closed gate, the hash comparison, the
# command line's log rule, the temporary Claude setup, the doctor's line set and the report.
# Dot-sourcing the script defines its functions and runs nothing; what needs a Windows runner
# (the install, the launches, the registry) is not called here.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$windowsDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script = Join-Path $windowsDir 'scripts/agentnotch-smoke.ps1'
$fixtures = Join-Path $windowsDir 'scripts/smoke/profile'
$committedGates = Join-Path $windowsDir 'scripts/smoke/gates.json'
. $script

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
        if ($_.Exception.Message -notmatch $Pattern) {
            throw "threw <$($_.Exception.Message)>, expected a message matching <$Pattern>"
        }
        return
    }
    throw "did not throw (expected <$Pattern>)"
}

$scratch = Join-Path ([IO.Path]::GetTempPath()) "agentnotch-smoke-tests-$PID"
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
function New-Scratch([string]$Name) {
    $path = Join-Path $scratch $Name
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    $path
}
function Write-GatesFile([string]$Path, [hashtable]$Open, $Why = $null) {
    $names = 'engine', 'selftest', 'cloud', 'glue', 'realClaude', 'rustContract'
    $body = [ordered]@{}
    foreach ($name in $names) { $body[$name] = [bool]$Open[$name] }
    $body['_why'] = if ($null -ne $Why) { $Why } else {
        $why = [ordered]@{}
        foreach ($name in $names) { $why[$name] = "package for $name" }
        $why
    }
    $body | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $Path
}
$allClosed = @{}
$allOpen = @{ engine = $true; selftest = $true; cloud = $true; glue = $true; realClaude = $true; rustContract = $true }

try {

# --- gates -------------------------------------------------------------------------------------

Test-Case 'the committed gates file reads and says what every gate waits for' {
    $gates = Read-Gates -Path $committedGates
    Assert-Equal ($gates.Open.Keys -join ',') 'engine,selftest,cloud,glue,realClaude,rustContract' 'gate names'
    foreach ($name in $gates.Open.Keys) { Assert-True ($gates.Why[$name].Length -gt 8) "a reason for $name" }
}

Test-Case 'a gate is open exactly when the file says true' {
    $file = Join-Path $scratch 'mixed.json'
    Write-GatesFile $file @{ engine = $true; glue = $false }
    $gates = Read-Gates -Path $file
    Assert-True (Test-GateOpen -Gates $gates -Name 'engine') 'engine open'
    Assert-True (-not (Test-GateOpen -Gates $gates -Name 'glue')) 'glue closed'
    Assert-True (-not (Test-GateOpen -Gates $gates -Name 'cloud')) 'cloud closed'
    Assert-Throws { Test-GateOpen -Gates $gates -Name 'enigne' } "unknown gate 'enigne'"
}

Test-Case 'a gates file with a missing, unknown or non-boolean gate is refused' {
    $file = Join-Path $scratch 'bad.json'
    Write-GatesFile $file $allClosed
    $json = Get-Content $file -Raw | ConvertFrom-Json -AsHashtable

    $missing = [ordered]@{} + $json; $missing.Remove('cloud')
    $missing | ConvertTo-Json -Depth 4 | Set-Content $file
    Assert-Throws { Read-Gates -Path $file } "no 'cloud' gate"

    $extra = [ordered]@{} + $json; $extra['clod'] = $false
    $extra | ConvertTo-Json -Depth 4 | Set-Content $file
    Assert-Throws { Read-Gates -Path $file } "unknown gate 'clod'"

    $text = [ordered]@{} + $json; $text['glue'] = 'yes'
    $text | ConvertTo-Json -Depth 4 | Set-Content $file
    Assert-Throws { Read-Gates -Path $file } "'glue' must be true or false"

    $noWhy = [ordered]@{} + $json; $noWhy['_why']['engine'] = ''
    $noWhy | ConvertTo-Json -Depth 4 | Set-Content $file
    Assert-Throws { Read-Gates -Path $file } "does not say what 'engine' waits for"

    Assert-Throws { Read-Gates -Path (Join-Path $scratch 'nothing.json') } 'does not exist'
}

Test-Case 'a gated phase warns with the package it waits for' {
    $file = Join-Path $scratch 'warn.json'
    Write-GatesFile $file $allClosed
    $gates = Read-Gates -Path $file
    Assert-Equal (Get-GateWarning -Gates $gates -Number '14' -Gate 'glue') '::warning::phase 14 not run: waiting for package for glue' 'warning'
    Assert-Equal (Get-GateWarning -Gates $gates -Number '3' -Gate 'engine' -Part "the doctor's line set") `
        "::warning::phase 3 (the doctor's line set) not run: waiting for package for engine" 'part warning'
}

Test-Case 'a release refuses a closed gate and names it; a branch run does not mind' {
    $file = Join-Path $scratch 'release.json'
    Write-GatesFile $file @{ engine = $true; selftest = $true; cloud = $true; glue = $false; realClaude = $true; rustContract = $false }
    $gates = Read-Gates -Path $file
    Assert-Throws { Assert-GatesForRelease -Gates $gates -Release } 'glue \(waiting for package for glue\)'
    Assert-Throws { Assert-GatesForRelease -Gates $gates -Release } 'rustContract'
    Assert-Equal (@(Assert-GatesForRelease -Gates $gates).Count) 0 'a branch run'
    Write-GatesFile $file $allOpen
    Assert-Equal (@(Assert-GatesForRelease -Gates (Read-Gates -Path $file) -Release).Count) 0 'a release with every gate open'
}

Test-Case 'the script run with -Release and a closed gate fails before any phase, and says why' {
    $file = Join-Path $scratch 'refuse.json'
    Write-GatesFile $file @{ engine = $true }
    $artifacts = Join-Path $scratch 'refuse-out'
    $pwsh = Join-Path $PSHOME ($IsWindows ? 'pwsh.exe' : 'pwsh')
    $output = & $pwsh -NoProfile -NonInteractive -File $script -Installer nowhere.exe -Version 1.2.3 -Updates on -KeyId abc `
        -Artifacts $artifacts -GatesFile $file -Release 2>&1 | Out-String
    Assert-Equal $LASTEXITCODE 1 'exit code'
    Assert-True ($output -match 'a release must run every smoke phase') "the refusal in <$output>"
    Assert-True ($output -match 'selftest') 'the closed gates are named'
    Assert-True (-not (Test-Path $artifacts)) 'nothing was created before the refusal'
}

Test-Case 'with every gate open a release gets past the gates (and stops only for being off Windows)' {
    if ($IsWindows) { return }
    $file = Join-Path $scratch 'open.json'
    Write-GatesFile $file $allOpen
    $pwsh = Join-Path $PSHOME 'pwsh'
    $output = & $pwsh -NoProfile -NonInteractive -File $script -Installer nowhere.exe -Version 1.2.3 -Updates on -KeyId abc `
        -Artifacts (Join-Path $scratch 'open-out') -GatesFile $file -Release 2>&1 | Out-String
    Assert-Equal $LASTEXITCODE 1 'exit code'
    Assert-True ($output -match 'runs on Windows only') "the Windows-only stop in <$output>"
}

Test-Case 'a release needs updates on and a key id' {
    $file = Join-Path $scratch 'args.json'
    Write-GatesFile $file $allOpen
    $pwsh = Join-Path $PSHOME ($IsWindows ? 'pwsh.exe' : 'pwsh')
    $off = & $pwsh -NoProfile -NonInteractive -File $script -Installer x -Version 1.2.3 -GatesFile $file -Release 2>&1 | Out-String
    Assert-True ($off -match 'needs -Updates on') "updates in <$off>"
    $noKey = & $pwsh -NoProfile -NonInteractive -File $script -Installer x -Version 1.2.3 -Updates on -GatesFile $file -Release 2>&1 | Out-String
    Assert-True ($noKey -match 'needs -KeyId') "the key id in <$noKey>"
}

# --- hashing -----------------------------------------------------------------------------------

Test-Case 'a tree hash lists every file with forward slashes, and a missing folder is empty' {
    $root = New-Scratch 'tree'
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'a/b') | Out-Null
    [IO.File]::WriteAllText((Join-Path $root 'top.txt'), 'top')
    [IO.File]::WriteAllText((Join-Path $root 'a/b/deep.txt'), 'deep')
    $hashes = Get-TreeHash -Root $root
    Assert-Equal ($hashes.Keys -join ',') 'a/b/deep.txt,top.txt' 'paths'
    Assert-Equal $hashes['top.txt'] (Get-FileSha256 (Join-Path $root 'top.txt')) 'hash'
    Assert-Equal $hashes['top.txt'] '28720365c5e7476a011e4f43ac003ee5f16247a263b9d623aa85ed311d73bf39' 'sha256 of top'
    Assert-Equal $hashes['top.txt'].Length 64 'hash length'
    Assert-Equal $hashes['top.txt'] $hashes['top.txt'].ToLowerInvariant() 'lower case'
    Assert-Equal (Get-TreeHash -Root (Join-Path $root 'absent')).Count 0 'a missing folder'
    Assert-Equal ((Get-TreeHash -Root $root -Prefix 'p/').Keys -join ',') 'p/a/b/deep.txt,p/top.txt' 'prefix'
    # A skipped path is never opened: one the running app holds locked would fail the hash.
    Assert-Equal ((Get-TreeHash -Root $root -Skip 'a/*').Keys -join ',') 'top.txt' 'skipped'
}

Test-Case 'the hash comparison reports changed, added and removed files, and nothing when equal' {
    $root = New-Scratch 'compare'
    [IO.File]::WriteAllText((Join-Path $root 'same.json'), '1')
    [IO.File]::WriteAllText((Join-Path $root 'edit.json'), '1')
    [IO.File]::WriteAllText((Join-Path $root 'gone.json'), '1')
    $before = Get-TreeHash -Root $root
    Assert-Equal @(Compare-Hashes -Before $before -After (Get-TreeHash -Root $root)).Count 0 'identical'
    [IO.File]::WriteAllText((Join-Path $root 'edit.json'), '2')
    Remove-Item (Join-Path $root 'gone.json')
    [IO.File]::WriteAllText((Join-Path $root 'new.bak'), '3')
    $diff = @(Compare-Hashes -Before $before -After (Get-TreeHash -Root $root))
    Assert-Equal ($diff -join ';') 'changed: edit.json;removed: gone.json;added: new.bak' 'differences'
}

Test-Case 'the hash comparison can look at one kind of file only' {
    $before = [ordered]@{ '.claude/settings.json' = 'a'; '.claude/x.bak' = 'b' }
    $after = [ordered]@{ '.claude/settings.json' = 'c'; '.claude/x.bak' = 'd'; '.claude/y/settings.json' = 'e' }
    $diff = @(Compare-Hashes -Before $before -After $after -Filter '*settings.json')
    Assert-Equal ($diff -join ';') 'changed: .claude/settings.json;added: .claude/y/settings.json' 'only settings files'
}

Test-Case 'the runner profile snapshot sees every .claude entry and nothing else' {
    $home1 = New-Scratch 'known-profile'
    New-Item -ItemType Directory -Force -Path (Join-Path $home1 '.claude/projects') | Out-Null
    [IO.File]::WriteAllText((Join-Path $home1 '.claude/projects/x.jsonl'), 'x')
    [IO.File]::WriteAllText((Join-Path $home1 '.claude.json'), '{}')
    [IO.File]::WriteAllText((Join-Path $home1 'notes.txt'), 'unrelated')
    $snapshot = Get-KnownProfileHashes -Root $home1
    Assert-Equal ($snapshot.Keys -join ',') '.claude/projects/x.jsonl,.claude.json' 'entries'
    Assert-Equal (Get-KnownProfileHashes -Root (Join-Path $home1 'absent')).Count 0 'a missing profile'
}

Test-Case 'the app data locations are found by name under the two roots' {
    $roaming = New-Scratch 'roaming'; $local = New-Scratch 'local'
    Assert-Equal @(Get-AppDataLocations -Roaming $roaming -Local $local).Count 0 'a clean runner'
    New-Item -ItemType Directory -Force -Path (Join-Path $roaming 'Agent Notch Sealed'), (Join-Path $local 'com.rivantmedia.agentnotch'), (Join-Path $roaming 'Other') | Out-Null
    $found = @(Get-AppDataLocations -Roaming $roaming -Local $local | ForEach-Object { Split-Path -Leaf $_ })
    Assert-Equal ($found -join ',') 'Agent Notch Sealed,com.rivantmedia.agentnotch' 'found'
}

# --- the command line ----------------------------------------------------------------------------

Test-Case 'a command logs to <data>\<first argument>.log' {
    Assert-Equal (Get-CliLogPath -Data 'data' -Arguments @('doctor')) (Join-Path 'data' 'doctor.log') 'doctor'
    Assert-Equal (Get-CliLogPath -Data 'data' -Arguments @('control', 'status')) (Join-Path 'data' 'control.log') 'control status'
    Assert-Equal (Get-CliLogPath -Data 'data' -Arguments @('autostart', 'on')) (Join-Path 'data' 'autostart.log') 'autostart on'
}

Test-Case 'environment layers merge with the later one winning' {
    $merged = Merge-Environment @(@{ A = '1'; B = '2' }, @{ B = '3'; C = 4 })
    Assert-Equal "$($merged.A)$($merged.B)$($merged.C)" '134' 'merged'
}

Test-Case 'the data roots are the real roaming folder and one inside the temporary profile, and a data file is found in either' {
    $roaming = New-Scratch 'data-real'; $profileDir = New-Scratch 'data-profile'
    $roots = @(Get-DataRoots -Profile $profileDir -Roaming $roaming)
    Assert-Equal $roots.Count 2 'two roots'
    Assert-Equal $roots[0] $roaming 'the real one first'
    Assert-Equal $roots[1] (Join-Path $profileDir 'AppData\Roaming') 'the profile one second'
    Assert-Equal ($null -eq (Find-DataFile -Roots $roots -Folder 'Agent Notch' -File 'doctor.log')) $true 'nothing yet'
    $inProfile = Join-Path $roots[1] 'Agent Notch'
    New-Item -ItemType Directory -Force -Path $inProfile | Out-Null
    [IO.File]::WriteAllText((Join-Path $inProfile 'doctor.log'), 'x')
    Assert-Equal (Find-DataFile -Roots $roots -Folder 'Agent Notch' -File 'doctor.log') (Join-Path $inProfile 'doctor.log') 'found in the profile'
}

Test-Case 'a hash comparison can leave a folder out' {
    $before = [ordered]@{ '.claude/settings.json' = 'a' }
    $after = [ordered]@{ '.claude/settings.json' = 'a'; 'AppData/Roaming/Agent Notch/x.log' = 'b'; '.local/y' = 'c' }
    Assert-Equal (@(Compare-Hashes -Before $before -After $after -Exclude 'AppData/*') -join ';') 'added: .local/y' 'excluded'
}

# --- the temporary Claude setup -------------------------------------------------------------------

Test-Case 'a settings fixture becomes CRLF with a byte order mark; a plain one stays LF without' {
    $crlf = ConvertTo-FixtureBytes -Text "{`n  `"a`": 1`n}`n" -Style 'crlf-bom'
    Assert-Equal ($crlf[0..2] -join ',') '239,187,191' 'BOM'
    $text = [Text.Encoding]::UTF8.GetString($crlf, 3, $crlf.Length - 3)
    Assert-Equal $text "{`r`n  `"a`": 1`r`n}`r`n" 'CRLF text'
    $already = ConvertTo-FixtureBytes -Text "x`r`ny`n" -Style 'crlf-bom'
    Assert-Equal ([Text.Encoding]::UTF8.GetString($already, 3, $already.Length - 3)) "x`r`ny`r`n" 'no doubled CR'
    $plain = ConvertTo-FixtureBytes -Text "{`r`n}`r`n" -Style 'plain'
    Assert-Equal ($plain -join ',') ([Text.Encoding]::UTF8.GetBytes("{`n}`n") -join ',') 'plain'
}

Test-Case 'the profile is built exactly as the design says' {
    $fake = Join-Path $scratch 'fake-claude.exe'
    [IO.File]::WriteAllText($fake, 'not really an exe')
    $root = Join-Path $scratch 'P'
    $hashes = New-SmokeProfile -Root $root -Fixtures $fixtures -FakeClaude $fake
    $expected = '.claude.json', '.claude-work/.claude.json', '.claude-work/settings.json', '.claude/settings.json', '.local/bin/claude.exe'
    foreach ($path in $expected) { Assert-True ($hashes.Contains($path)) "P holds $path" }
    Assert-True (@($hashes.Keys | Where-Object { $_ -like '.claude/projects/*/*.jsonl' }).Count -eq 1) 'one transcript'
    Assert-Equal $hashes.Count 6 'file count'
    Assert-Equal $hashes['.local/bin/claude.exe'] (Get-FileSha256 $fake) 'the fake is claude.exe'
    foreach ($folder in 'AppData/Roaming', 'AppData/Local') { Assert-True (Test-Path (Join-Path $root $folder) -PathType Container) "P holds an empty $folder (the shell resolves the app's data folders through it)" }

    $default = [IO.File]::ReadAllBytes((Join-Path $root '.claude/settings.json'))
    Assert-Equal ($default[0..2] -join ',') '239,187,191' 'default settings: BOM'
    $text = [Text.Encoding]::UTF8.GetString($default)
    Assert-Equal ([regex]::Matches($text, "(?<!`r)`n").Count) 0 'default settings: bare LF'
    Assert-True ($text.Contains('"command": "echo foreign-hook"')) 'a foreign hook'
    Assert-True ($text.Contains('"command": "echo smoke-status"')) 'a plain echo status line'
    Assert-True ($text.Contains('✓')) 'a non-ASCII character survives'
    $work = Get-Content (Join-Path $root '.claude-work/settings.json') -Raw
    Assert-True ($work.Contains('node C:\\tools\\sl.js')) 'the work status line is node C:\tools\sl.js'

    $one = Get-Content (Join-Path $root '.claude.json') -Raw | ConvertFrom-Json
    $two = Get-Content (Join-Path $root '.claude-work/.claude.json') -Raw | ConvertFrom-Json
    Assert-True ($one.oauthAccount.accountUuid -ne $two.oauthAccount.accountUuid) 'two different accounts'
    Assert-True ($one.oauthAccount.emailAddress -ne $two.oauthAccount.emailAddress) 'two different emails'
    Assert-True ($one.oauthAccount.emailAddress.EndsWith('.invalid')) 'a made-up address'

    # Built again from nothing, the same bytes.
    Assert-Equal @(Compare-Hashes -Before $hashes -After (New-SmokeProfile -Root $root -Fixtures $fixtures -FakeClaude $fake)).Count 0 'rebuild'
}

Test-Case 'the usage fixture is a get_usage answer the probe parser takes as a reading (five-hour and weekly windows)' {
    # fake-claude answers get_usage with this object as the control response's `response`.
    $usage = Get-Content (Join-Path $fixtures 'usage.json') -Raw | ConvertFrom-Json
    Assert-True $usage.rate_limits_available 'rate limits available'
    Assert-Equal $usage.rate_limits.five_hour.utilization 12 'five hour'
    Assert-Equal $usage.rate_limits.seven_day.utilization 34 'weekly'
    # Without `limits` the parser takes the answer for Claude Code's seeded fallback, which the
    # probe can only date from .claude.json's cache (the fixtures have none): no reading.
    Assert-Equal (@($usage.rate_limits.limits | ForEach-Object { $_.kind }) -join ',') 'session,weekly_all' 'the limits list'
}

# --- the doctor -----------------------------------------------------------------------------------

$doctorOff = @'
Agent Notch doctor v1.2.3
updates: off (built from source)
accounts: 2 (a@example.invalid, b@example.invalid)
hooks: consent=unasked
deep-link: registered
pipe: \\.\pipe\agentnotch-hook-S-1-5-21 no instance running
elevated: app=false hook=false
smart-app-control: off
support: C:\Users\runneradmin\AppData\Local\com.rivantmedia.agentnotch\Claude
'@

Test-Case 'a complete doctor report for a build from source has nothing missing' {
    Assert-Equal @(Test-DoctorReport -Report $doctorOff -Version '1.2.3').Count 0 'missing'
}

Test-Case 'a release build shows the derived key id and a required signed version' {
    $on = $doctorOff.Replace('updates: off (built from source)', 'updates: on feed=https://example.invalid/latest.json key=ABC123 signed-version=required')
    Assert-Equal @(Test-DoctorReport -Report $on -Version '1.2.3' -Updates on -KeyId 'ABC123').Count 0 'on'
    Assert-Equal (@(Test-DoctorReport -Report $on -Version '1.2.3' -Updates on -KeyId 'OTHER') -join ';') 'the updates line' 'wrong key id'
    Assert-Equal (@(Test-DoctorReport -Report $on.Replace('required', 'not required') -Version '1.2.3' -Updates on -KeyId 'ABC123') -join ';') 'the updates line' 'not required'
    Assert-Equal (@(Test-DoctorReport -Report $doctorOff -Version '1.2.3' -Updates on -KeyId 'ABC123') -join ';') 'the updates line' 'off where on is expected'
    Assert-Equal (@(Test-DoctorReport -Report $on -Version '1.2.3' -Updates off) -join ';') 'the updates line' 'on where off is expected'
}

Test-Case 'each missing doctor line is named' {
    $broken = $doctorOff.Replace('accounts: 2', 'accounts: 1').Replace('deep-link: registered', 'deep-link: unknown').Replace('v1.2.3', 'v9.9.9')
    $missing = @(Test-DoctorReport -Report $broken -Version '1.2.3')
    Assert-Equal ($missing -join ';') 'the header;two accounts;the deep link is registered' 'missing'
    Assert-Equal @(Test-DoctorReport -Report '' -Version '1.2.3').Count 9 'an empty report lacks everything'
}

Test-Case 'the credential names are found in a report, and only those' {
    $names = @('.cred' + 'entials.json'; 'claudeAi' + 'Oauth'; 'access' + 'Token')
    Assert-Equal @(Find-CredentialNames -Text $doctorOff).Count 0 'a clean report'
    Assert-Equal @(Find-CredentialNames -Text '').Count 0 'an empty report'
    foreach ($name in $names) { Assert-Equal (@(Find-CredentialNames -Text "x $name y") -join ',') $name "found $name" }
    Assert-Equal @(Find-CredentialNames -Text ($names -join ' ')).Count 3 'all three'
}

# --- the report ------------------------------------------------------------------------------------

Test-Case 'the summary lists every phase, the failure, and what is waiting on a package' {
    $results = @(
        [pscustomobject]@{ Number = '0'; Name = 'preflight'; Status = 'passed'; Seconds = 1.26; Error = '' }
        [pscustomobject]@{ Number = '14'; Name = 'autostart'; Status = 'gated'; Seconds = 0.0; Error = '' }
        [pscustomobject]@{ Number = '15'; Name = 'uninstall'; Status = 'failed'; Seconds = 3.0; Error = 'the Run value is still there' }
    )
    $gated = @(
        [pscustomobject]@{ Number = '3'; Part = "the doctor's line set"; WaitingFor = 'WP9' }
        [pscustomobject]@{ Number = '14'; Part = ''; WaitingFor = 'WP9 (the CLI)' }
    )
    $text = Format-Summary -Results $results -Gated $gated
    Assert-True ($text.Contains('| 0 | preflight | passed | 1.3 |')) 'a passed row'
    Assert-True ($text.Contains('| 14 | autostart | gated | 0 |')) 'a gated row'
    Assert-True ($text.Contains('Phase 15 failed: the Run value is still there')) 'the failure'
    Assert-True ($text.Contains("- phase 3 (the doctor's line set): WP9")) 'a gated part'
    Assert-True ($text.Contains('- phase 14: WP9 (the CLI)')) 'a gated phase'
    Assert-True (-not (Format-Summary -Results @($results[0]) -Gated @()).Contains('Not run')) 'no gated section when nothing waits'
}

# --- the phase runner ------------------------------------------------------------------------------

Test-Case 'the runner passes, fails and gates a phase, and logs each into the artifacts folder' {
    $file = Join-Path $scratch 'runner.json'
    Write-GatesFile $file @{ engine = $true }
    $script:GateSet = Read-Gates -Path $file
    $script:ArtifactsDir = New-Scratch 'runner-out'
    $script:Results = [Collections.Generic.List[object]]::new()
    $script:Gated = [Collections.Generic.List[object]]::new()
    $ran = [Collections.Generic.List[string]]::new()

    $passed = Invoke-Phase -Phase @{ Number = '7'; Name = 'A passing phase'; Body = { $ran.Add('pass'); Write-PhaseLog 'hello'; 'stray output' } }
    Assert-Equal $passed $true 'a passing phase returns exactly true, whatever its body printed'
    $gated = Invoke-Phase -Phase @{ Number = '8'; Name = 'Gated'; Gate = 'cloud'; Body = { $ran.Add('gated') } }
    Assert-Equal $gated $true 'a gated phase does not stop the run'
    $open = Invoke-Phase -Phase @{ Number = '9'; Name = 'Open gate'; Gate = 'engine'; Body = { $ran.Add('open') } }
    Assert-Equal $open $true 'an open gate runs the body'
    $failed = Invoke-Phase -Phase @{ Number = '10'; Name = 'Failing'; Body = { throw 'it broke' } }
    Assert-Equal $failed $false 'a failing phase stops the run'

    Assert-Equal ($ran -join ',') 'pass,open' 'bodies run'
    Assert-Equal (($script:Results | ForEach-Object { $_.Status }) -join ',') 'passed,gated,passed,failed' 'statuses'
    Assert-Equal $script:Results[3].Error 'it broke' 'the error'
    Assert-Equal $script:Gated.Count 1 'one gated entry'
    Assert-Equal $script:Gated[0].WaitingFor 'package for cloud' 'what it waits for'
    $log = Get-Content (Join-Path $script:ArtifactsDir 'phase-7-a-passing-phase.log') -Raw
    Assert-True ($log.Contains('hello')) 'the phase log'
    Assert-True ((Get-Content (Join-Path $script:ArtifactsDir 'phase-10-failing.log') -Raw).Contains('FAILED: it broke')) 'the failure is logged'
    Assert-True (-not (Test-Path (Join-Path $script:ArtifactsDir 'phase-8-gated.log'))) 'a gated phase makes no log'
}

Test-Case 'a hook run reports its exit code, its output and its time (a stand-in script on a Unix host)' {
    if ($IsWindows) { return }
    $quiet = Join-Path $scratch 'hook-quiet.sh'
    Set-Content $quiet "#!/bin/sh`ncat >/dev/null`nexit 0`n"
    $loud = Join-Path $scratch 'hook-loud.sh'
    Set-Content $loud "#!/bin/sh`ncat >/dev/null`necho printed`nexit 3`n"
    chmod +x $quiet $loud
    $a = Invoke-HookOnce -Exe $quiet -Arguments @('hook', '--exec') -Stdin '{ not json'
    Assert-Equal $a.ExitCode 0 'quiet exit'
    Assert-Equal $a.Stdout '' 'quiet stdout'
    Assert-True ($a.Seconds -lt 5) 'quiet time'
    $b = Invoke-HookOnce -Exe $loud -Arguments @() -Stdin ''
    Assert-Equal $b.ExitCode 3 'loud exit'
    Assert-Equal $b.Stdout "printed`n" 'loud stdout'
}

# --- phase 4: the self-test report and the snapshots ---------------------------------------------------

# A passing report in WP9's shape (DESIGN-WIN §7.4, the self-test contract), as JSON text: each
# case reads it, breaks one thing and expects exactly that to be named.
function New-GoodReport([double]$Scale = 1.0) {
    $edge = {
        param($name)
        $tail = if ($name -eq 'floating') { 'null' } else { 'true' }
        @"
{ "edge": "$name", "floating": $(if ($name -eq 'floating') { 'true' } else { 'false' }),
  "panel": [10, 10, 400, 600], "work_area": [0, 0, 1024, 728], "inside_work_area": true,
  "tail_offset": 0.0, "tail_limit": 10.0, "tail_inside_corners": $tail,
  "topmost": true, "above_notch": $tail,
  "auto": { "no_activate": true, "gate_shut": true, "gate_opens_on_confirmation": true },
  "failures": [] }
"@
    }
    $page = '{ "round_trip": true, "csp_violations": [], "errors": [], "invariants": { "no_text_overflow": true, "badges_inside_pill": true }, "page_hook": false }'
    $json = @"
{ "ok": true, "version": "1.1.0", "scale": $Scale, "error": null,
  "edges": [ $(($script:SelfTestEdges | ForEach-Object { & $edge $_ }) -join ', ') ],
  "pages": { "notch": $page, "settings": $page, "agentnotch-panel": $page },
  "failures": [] }
"@
    $json | ConvertFrom-Json -AsHashtable
}

Test-Case 'a passing self-test report has no problems, at each scale' {
    foreach ($scale in 1.0, 1.25, 1.5) {
        Assert-Equal (@(Test-SelfTestReport -Report (New-GoodReport $scale) -Scale $scale).Count) 0 "problems at $scale"
    }
}

Test-Case 'the report must name the scale the run asked for' {
    $problems = @(Test-SelfTestReport -Report (New-GoodReport 1.0) -Scale 1.25)
    Assert-Equal $problems.Count 1 'one problem'
    Assert-True ($problems[0] -match 'scale is 1') 'the scale is named'
}

Test-Case 'the scaled runs ask the app for their scale (AGENTNOTCH_SELF_TEST_SCALE), as WP9''s script does' {
    Assert-Equal (($script:SelfTestScales | ForEach-Object { $_.Scale }) -join ',') '1,1.25,1.5' 'the scales'
    foreach ($run in $script:SelfTestScales) {
        # WebView2 ignores --force-device-scale-factor: the app's own switch sets the page scale.
        Assert-True (-not $run.ContainsKey('WebViewArguments')) "no WebView2 argument at $($run.Name) %"
        $expected = if ($run.Scale -eq 1.0) { '' } else { [string]$run.Scale }
        Assert-Equal $run.ScaleSwitch $expected "the scale switch at $($run.Name) %"
    }
}

Test-Case 'a failing self-test report names every failed check and echoes the report failures' {
    $r = New-GoodReport
    $r['ok'] = $false
    $r['failures'] = @('right: the panel left the work area')
    $r['edges'][0]['inside_work_area'] = $false
    $r['edges'][1]['tail_inside_corners'] = $false
    $r['edges'][2]['topmost'] = $false
    $r['edges'][3]['above_notch'] = $false
    $r['edges'][3]['auto']['no_activate'] = $false
    $r['edges'][3]['auto']['gate_shut'] = $false
    $r['edges'][3]['auto']['gate_opens_on_confirmation'] = $false
    $r['edges'][3]['failures'] = @('z-order lost')
    $r['pages']['notch']['round_trip'] = $false
    $r['pages']['settings']['csp_violations'] = @('script-src blocked inline')
    $r['pages']['agentnotch-panel']['errors'] = @('TypeError: x is undefined')
    $r['pages']['agentnotch-panel']['invariants']['no_text_overflow'] = $false
    $text = (Test-SelfTestReport -Report $r) -join "`n"
    foreach ($needle in 'ok is not true', 'report failure: right: the panel left the work area',
        'edge right: inside_work_area is False', 'edge left: tail_inside_corners is False', 'edge top: topmost is False',
        'edge bottom: above_notch is False', 'auto.no_activate is False', 'auto.gate_shut is False',
        'auto.gate_opens_on_confirmation is False', 'edge bottom: z-order lost',
        'page notch: the an_call round trip did not succeed', 'script-src blocked inline',
        'TypeError: x is undefined', "invariant 'no_text_overflow' is False") {
        Assert-True ($text.Contains($needle)) "<$needle> is named in:`n$text"
    }
}

Test-Case 'a missing edge, page, field or invariant is a failure, never a pass' {
    $r = New-GoodReport
    $r['edges'] = @($r['edges'] | Where-Object { $_['edge'] -ne 'top' })
    $r['pages'].Remove('settings')
    $r['pages']['notch'].Remove('csp_violations')
    $r['pages']['agentnotch-panel']['invariants'] = @{}
    $r['edges'][0].Remove('topmost')
    $text = (Test-SelfTestReport -Report $r) -join "`n"
    foreach ($needle in 'edge top: missing from the report', 'page settings: missing from the report', 'page notch: csp_violations is missing',
        'page agentnotch-panel: no invariants reported', 'edge right: topmost is missing') {
        Assert-True ($text.Contains($needle)) "<$needle> is named in:`n$text"
    }
}

Test-Case 'only a JSON true passes: the string "true" and the number 1 do not' {
    $r = New-GoodReport
    $r['edges'][0]['topmost'] = 'true'
    $r['pages']['notch']['invariants']['no_text_overflow'] = 1
    $text = (Test-SelfTestReport -Report $r) -join "`n"
    Assert-True ($text.Contains('edge right: topmost is true')) 'a string is not true'
    Assert-True ($text.Contains("invariant 'no_text_overflow' is 1")) 'a number is not true'
}

Test-Case 'a floating edge may leave the tail and the notch order out; no other edge may' {
    $r = New-GoodReport
    $r['edges'][4].Remove('tail_inside_corners'); $r['edges'][4].Remove('above_notch')
    Assert-Equal (@(Test-SelfTestReport -Report $r).Count) 0 'floating without them'
    $r['edges'][4]['topmost'] = $null
    Assert-True ((Test-SelfTestReport -Report $r) -join ' ' -match 'edge floating: topmost is missing') 'but not topmost'
    $r = New-GoodReport
    $r['edges'][0]['above_notch'] = $null
    Assert-True ((Test-SelfTestReport -Report $r) -join ' ' -match 'edge right: above_notch is missing') 'a flat edge needs above_notch'
}

Test-Case 'the report a stub build writes ({ok:false, error}) fails with the reason the app gave' {
    $stub = '{"ok":false,"error":"the sealed self-test and snapshots aren''t available in this build"}' | ConvertFrom-Json -AsHashtable
    $text = (Test-SelfTestReport -Report $stub) -join "`n"
    Assert-True ($text.Contains("the app says: the sealed self-test")) 'the error is shown'
    Assert-True ($text.Contains('ok is not true')) 'ok is not true'
    Assert-True ($text.Contains('edge right: missing from the report')) 'edges are missing'
    Assert-Equal (@(Test-SelfTestReport -Report 'text').Count) 1 'a non-object report'
}

function New-SnapshotFolder([string]$Name, [string[]]$States, [hashtable]$Extra = @{}) {
    $dir = New-Scratch $Name
    $png = [byte[]](0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3)
    $entries = foreach ($state in $States) {
        [IO.File]::WriteAllBytes((Join-Path $dir "$state.png"), $png)
        [ordered]@{ name = $state; file = "$state.png"; width = 400; height = 300; bytes = $png.Length }
    }
    foreach ($key in $Extra.Keys) { $entries += $Extra[$key] }
    ConvertTo-Json -InputObject @($entries) | Set-Content -LiteralPath (Join-Path $dir 'manifest.json')
    $dir
}

Test-Case 'a complete snapshot folder passes; the fixed states are the ones WP9 names' {
    Assert-Equal $script:SnapshotStates.Count 11 'state count'
    $dir = New-SnapshotFolder 'shots-ok' $script:SnapshotStates
    Assert-Equal (@(Test-SnapshotManifest -Directory $dir).Count) 0 'problems'
}

Test-Case 'a missing manifest, state, file, empty file or non-PNG is named' {
    Assert-True ((Test-SnapshotManifest -Directory (New-Scratch 'shots-none')) -join ' ' -match 'no manifest.json') 'no manifest'
    $dir = New-SnapshotFolder 'shots-bad' ($script:SnapshotStates | Where-Object { $_ -ne 'panel-chat' })
    Assert-True ((Test-SnapshotManifest -Directory $dir) -join ' ' -match "no 'panel-chat' snapshot") 'a state missing'
    Remove-Item -LiteralPath (Join-Path $dir 'notch-top.png')
    [IO.File]::WriteAllBytes((Join-Path $dir 'notch-left.png'), [byte[]]@())
    [IO.File]::WriteAllBytes((Join-Path $dir 'notch-right.png'), [byte[]](1, 2, 3, 4, 5, 6, 7, 8, 9))
    $text = (Test-SnapshotManifest -Directory $dir) -join "`n"
    Assert-True ($text.Contains('notch-top: notch-top.png is missing')) 'a file missing'
    Assert-True ($text.Contains('notch-left: notch-left.png is empty')) 'an empty file'
    Assert-True ($text.Contains('notch-right: notch-right.png is not a PNG')) 'not a PNG'
}

Test-Case 'a state the manifest lists beyond the fixed ones is checked like the others' {
    $extra = @{ a = [ordered]@{ name = 'panel-undo'; file = 'panel-undo.png'; width = 1; height = 1; bytes = 1 } }
    $dir = New-SnapshotFolder 'shots-extra' $script:SnapshotStates $extra
    Assert-True ((Test-SnapshotManifest -Directory $dir) -join ' ' -match 'panel-undo: panel-undo.png is missing') 'the listed file is required'
    $escape = @{ a = [ordered]@{ name = 'esc'; file = '../outside.png'; width = 1; height = 1; bytes = 1 } }
    $dir2 = New-SnapshotFolder 'shots-escape' $script:SnapshotStates $escape
    Assert-True ((Test-SnapshotManifest -Directory $dir2) -join ' ' -match 'not a plain name') 'a path in a file name is refused'
}

Test-Case 'comparison results: same and a missing baseline pass; a difference or a size change fails with the numbers' {
    $results = @(
        @{ name = 'a.png'; status = 'same'; ok = $true }
        @{ name = 'b.png'; status = 'no-baseline'; ok = $true }
        @{ name = 'c.png'; status = 'different'; ok = $false; changed = 120; total = 1000; maxDelta = 200; diffFile = 'out\c.diff.png' }
        @{ name = 'd.png'; status = 'size-mismatch'; ok = $false; width = 400; height = 300; baselineWidth = 400; baselineHeight = 320 }
        @{ name = 'e.png'; status = 'error'; ok = $false; error = 'not a PNG (bad signature)' }
    )
    $problems = @(Get-SnapshotProblems -Results $results)
    Assert-Equal $problems.Count 3 'three problems'
    Assert-True ($problems[0] -match '^c.png: 120 of 1000 pixels differ.*largest channel change 200.*diff image: out.c.diff.png') 'the difference'
    Assert-True ($problems[1] -match '^d.png: the capture is 400x300, the baseline 400x320') 'the sizes'
    Assert-True ($problems[2] -match '^e.png: error: not a PNG') 'the error'
}

Test-Case 'the comparison runs through node on generated PNGs when node is here' {
    if (-not (Get-Command node -CommandType Application -ErrorAction SilentlyContinue)) { return }
    $dir = New-Scratch 'cmp'
    $make = {
        param($path, [byte]$grey)
        $code = "import('$((Join-Path $windowsDir 'scripts/smoke/png-diff.mjs') -replace '\\','/')').then(m => require('fs').writeFileSync(process.argv[1], m.encodePng(4, 4, new Uint8Array(64).fill($grey))))"
        & node -e $code $path
    }
    $actual = New-Item -ItemType Directory -Force -Path (Join-Path $dir 'actual')
    $base = New-Item -ItemType Directory -Force -Path (Join-Path $dir 'base')
    & $make (Join-Path $actual 'a.png') 10
    & $make (Join-Path $actual 'b.png') 10
    & $make (Join-Path $actual 'c.png') 10
    & $make (Join-Path $base 'a.png') 10
    & $make (Join-Path $base 'b.png') 250
    $results = @(Invoke-SnapshotComparison -Actual $actual -Baselines $base -Diffs (Join-Path $dir 'diffs'))
    $by = @{}; foreach ($r in $results) { $by[$r['name']] = $r['status'] }
    Assert-Equal $by['a.png'] 'same' 'a'
    Assert-Equal $by['b.png'] 'different' 'b'
    Assert-Equal $by['c.png'] 'no-baseline' 'c'
    Assert-True (Test-Path (Join-Path $dir 'diffs/b.diff.png')) 'a diff image was written'
}

Test-Case 'the sealed launch is replaced by the self-test exactly when the selftest gate is open' {
    $closedPath = Join-Path (New-Scratch 'gates-closed') 'g.json'
    Write-GatesFile $closedPath @{}
    $closed = Read-Gates -Path $closedPath
    $numbers = @(Get-PhaseTable -Gates $closed | ForEach-Object { $_.Number })
    Assert-True ('3b' -in $numbers) 'the plain sealed launch stays while the gate is closed'
    Assert-True ('4' -in $numbers) 'phase 4 is in the table (gated)'
    Assert-Equal ((Get-PhaseTable | Where-Object { $_.Number -eq '4' }).Gate) 'selftest' 'phase 4 is gated on selftest'
    $gatesPath = Join-Path (New-Scratch 'gates-open') 'g.json'
    Write-GatesFile $gatesPath @{ selftest = $true }
    $open = Read-Gates -Path $gatesPath
    $numbers = @(Get-PhaseTable -Gates $open | ForEach-Object { $_.Number })
    Assert-True ('3b' -notin $numbers) 'the plain sealed launch is gone with the gate open'
    Assert-True ('4' -in $numbers) 'phase 4 stays'
}

# --- the table of phases -----------------------------------------------------------------------------

Test-Case 'the phase table runs in the design order, with gates that exist and a body each' {
    $table = @(Get-PhaseTable)
    $numbers = @($table | ForEach-Object { $_.Number })
    foreach ($n in '0', '1', '2', '3', '13', '14', '15') { Assert-True ($n -in $numbers) "phase $n is in the table" }
    Assert-Equal (@($numbers | Select-Object -Unique).Count) $numbers.Count 'unique numbers'
    $order = @($numbers | ForEach-Object { [int]($_ -replace '\D', '') })
    Assert-Equal ($order -join ',') (($order | Sort-Object) -join ',') 'ascending'
    foreach ($phase in $table) {
        Assert-True ($phase.Body -is [scriptblock]) "phase $($phase.Number) has a body"
        if ($phase.ContainsKey('Gate')) { Assert-True ($phase.Gate -in $script:KnownGates) "phase $($phase.Number)'s gate exists" }
    }
}


# --- phases 5-9: what the UI-driven phases decide ----------------------------------------------------------------

Test-Case 'control status lines are read as key: value, and the check names what differs' {
    $text = "Agent Notch 1.1.0`r`ntransport: listening`r`naccounts: 2`r`nhook_consent: unasked`r`nreadings: 2`r`nsessions=0`r`n"
    $status = ConvertFrom-ControlStatus -Text $text
    Assert-Equal $status['transport'] 'listening' 'transport'
    Assert-Equal $status['sessions'] '0' 'a key=value line is read too'
    $expect = [ordered]@{ transport = 'listening'; accounts = '2'; hook_consent = 'unasked'; readings = '2' }
    Assert-Equal @(Test-ControlStatus -Status $status -Expect $expect).Count 0 'a matching status has no problems'
    $bad = @(Test-ControlStatus -Status (ConvertFrom-ControlStatus -Text "transport: stopped`naccounts: 2`n") -Expect $expect)
    Assert-Equal ($bad -join '|') "transport is 'stopped', expected 'listening'|no 'hook_consent' line|no 'readings' line" 'each difference is named'
    Assert-Equal (ConvertFrom-ControlStatus -Text '').Count 0 'empty text is an empty status'
}

Test-Case 'Wait-ControlStatus compares with -Expect and runs a closure -Check, as the phases call it' {
    $script:polls = 0
    function Get-ControlStatus {
        $script:polls++
        [ordered]@{ transport = 'listening'; accounts = '2'; readings = [string][math]::Min($script:polls, 2); sessions = '1' }
    }
    $seen = Wait-ControlStatus -Seconds 10 -What 'readings' -Expect ([ordered]@{ transport = 'listening'; readings = '2' })
    Assert-Equal $seen['readings'] '2' 'it waited for the readings'
    $before = @{ sessions = '0' }
    # A closure runs in a module of its own: it may use only what it captured.
    $seen = Wait-ControlStatus -Seconds 5 -What 'sessions' -Check ({ param($s) if ([int]$s['sessions'] -le [int]$before['sessions']) { 'none' } }.GetNewClosure())
    Assert-Equal $seen['sessions'] '1' 'the closure check passed'
    $failed = $null
    try { Wait-ControlStatus -Seconds 1 -What 'the cloud' -Expect @{ cloud = 'signed_in' } | Out-Null } catch { $failed = $_.Exception.Message }
    Assert-Equal $failed "timed out after 1 s: the cloud (no 'cloud' line)" 'a timeout names what it last saw'
}

Test-Case 'a private DACL is protected and names only the user and SYSTEM' {
    $allowed = @('S-1-5-21-1-2-3-1001', 'S-1-5-18')
    Assert-Equal @(Test-PrivateAcl -Protected $true -Sids @('S-1-5-21-1-2-3-1001', 'S-1-5-18') -Allowed $allowed).Count 0 'user + SYSTEM'
    Assert-Equal @(Test-PrivateAcl -Protected $true -Sids @('S-1-5-21-1-2-3-1001') -Allowed $allowed).Count 0 'the user alone'
    Assert-Equal (@(Test-PrivateAcl -Protected $false -Sids @('S-1-5-18') -Allowed $allowed) -join '|') 'the DACL inherits from the parent (it is not protected)' 'unprotected'
    Assert-Equal (@(Test-PrivateAcl -Protected $true -Sids @('S-1-5-18', 'S-1-5-32-544') -Allowed $allowed) -join '|') 'the DACL names S-1-5-32-544' 'Administrators'
    Assert-Equal (@(Test-PrivateAcl -Protected $true -Sids @() -Allowed $allowed) -join '|') 'the DACL is empty' 'empty'
}

function New-FakeClaudeLine([string[]]$Argv, [string]$Cwd, [string[]]$Names = @()) {
    ConvertTo-Json @{ argv = $Argv; cwd = $Cwd; env = $Names } -Compress -Depth 5
}

Test-Case 'the fake claude log: the probe with its exact argv in usage-probe, and --version, are fine' {
    $dir = 'C:\Users\x\AppData\Local\com.rivantmedia.agentnotch\Claude\usage-probe'
    $lines = @(
        (New-FakeClaudeLine @('--version') 'C:\anywhere')
        (New-FakeClaudeLine $script:ProbeArgv $dir @('CLAUDE_CONFIG_DIR'))
        (New-FakeClaudeLine $script:ProbeArgv ($dir.ToUpperInvariant().Replace('/', '\') + '\') @())
        (New-FakeClaudeLine $script:ProbeArgv ('\\?\' + $dir) @())
    )
    Assert-Equal @(Test-FakeClaudeLog -Lines $lines -ProbeDir $dir).Count 0 'no problems'
    Assert-Equal @(Test-FakeClaudeLog -Lines @() -ProbeDir $dir).Count 0 'an empty log is not a problem here'
}

Test-Case 'the fake claude log: another command, another argument, another folder or a scrubbed variable is named' {
    $dir = 'C:\s\usage-probe'
    $problems = @(Test-FakeClaudeLog -ProbeDir $dir -Lines @(
            (New-FakeClaudeLine @('-p', 'hello') $dir)
            (New-FakeClaudeLine ($script:ProbeArgv + '--extra') $dir)
            (New-FakeClaudeLine $script:ProbeArgv 'C:\elsewhere')
            (New-FakeClaudeLine $script:ProbeArgv $dir @('ANTHROPIC_API_KEY', 'CLAUDE_CODE_ENTRYPOINT', 'claude_pid'))
            'not json'
        ))
    Assert-True ($problems -match 'run 1 is neither the probe nor --version') 'a stray command'
    Assert-True ($problems -match 'run 2 is neither') 'an extra argument'
    Assert-True ($problems -match "run 3 \(the probe\) ran in 'C:\\elsewhere'") 'a wrong folder'
    Assert-True ($problems -match 'run 4 saw the scrubbed variable\(s\) ANTHROPIC_API_KEY, CLAUDE_CODE_ENTRYPOINT, claude_pid') 'the leaked names'
    Assert-True ($problems -match 'log line 5 is not JSON') 'garbage'
    $version = @(Test-FakeClaudeLog -ProbeDir $dir -Lines @((New-FakeClaudeLine @('--version') $dir @('CLAUDECODE'))))
    Assert-True ($version -match 'scrubbed') 'a leak counts on a --version run too'
}

Test-Case 'the probe argument list is the one of the design, with the settings as one element' {
    Assert-Equal $script:ProbeArgv.Count 10 'ten arguments'
    Assert-Equal $script:ProbeArgv[-1] '{"disableAllHooks":true}' 'the settings JSON'
    Assert-Equal ($script:ProbeArgv -join ' ') '-p --input-format stream-json --output-format stream-json --verbose --no-session-persistence --strict-mcp-config --settings {"disableAllHooks":true}' 'the line'
    foreach ($name in $script:ScrubSentinels.Keys) { Assert-True (Test-ScrubbedName $name) "$name is one the engine strips" }
    Assert-True (-not (Test-ScrubbedName 'CLAUDE_CONFIG_DIR')) 'CLAUDE_CONFIG_DIR is set again by the probe for another folder'
}

# A settings.json as the installer leaves it, built here from its parts.
function New-InstalledSettings([string]$Command, [string[]]$Arguments = $null, [switch]$NoPermissionTimeout) {
    $entry = { param($timeout)
        $h = [ordered]@{ type = 'command'; command = $Command }
        if ($null -ne $Arguments) { $h['args'] = $Arguments }
        if ($timeout) { $h['timeout'] = $timeout }
        $h
    }
    $hooks = [ordered]@{ PreToolUse = @([ordered]@{ matcher = 'Bash'; hooks = @([ordered]@{ type = 'command'; command = 'echo foreign-hook' }) }) }
    foreach ($event in $script:BaselineHookEvents) {
        $group = [ordered]@{ hooks = @(& $entry $(if ($event -eq 'PermissionRequest' -and -not $NoPermissionTimeout) { 86400 } else { $null })) }
        if ($event -in 'PreToolUse', 'PostToolUse', 'PermissionRequest', 'Notification') { $group = [ordered]@{ matcher = '*'; hooks = $group['hooks'] } }
        $hooks[$event] = @(@($hooks[$event]) + $group | Where-Object { $_ })
    }
    ([ordered]@{ theme = 'dark'; hooks = $hooks } | ConvertTo-Json -Depth 20 | ConvertFrom-Json -Depth 20)
}

Test-Case 'our hook entries: the string form with a forward-slash path, in every baseline event, passes' {
    $exe = 'C:\Users\x\p\.claude\hooks\agentnotch-hook.exe'
    $settings = New-InstalledSettings -Command 'C:/Users/x/p/.claude/hooks/agentnotch-hook.exe hook'
    Assert-Equal @(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $false).Count 0 'string form, exec not allowed'
    Assert-Equal @(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $true).Count 0 'string form, exec allowed'
    $entries = @(Get-HookEntries -Settings $settings)
    Assert-True ($entries.Count -gt 10) 'every group and the foreign entry are listed'
    Assert-Equal (@($entries | Where-Object { $_.Event -eq 'PreToolUse' })[0].Command) 'echo foreign-hook' 'the user entry comes first'
}

Test-Case 'our hook entries: an 8.3 path is compared after it is made long' {
    $exe = 'C:\Users\John Smith\p\.claude\hooks\agentnotch-hook.exe'
    $settings = New-InstalledSettings -Command 'C:/Users/JOHNSM~1/p/.claude/hooks/AGENTN~1.EXE hook'
    $resolve = { param($p) $p.Replace('JOHNSM~1', 'John Smith').Replace('AGENTN~1.EXE', 'agentnotch-hook.exe') }
    Assert-Equal @(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $false -Resolve $resolve).Count 0 'resolved'
    Assert-True (@(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $false).Count -gt 0) 'unresolved, the entries are nobody''s'
}

Test-Case 'our hook entries: the exec form needs the facts file to allow it, and exactly hook --exec' {
    $exe = 'C:\p\.claude\hooks\agentnotch-hook.exe'
    $settings = New-InstalledSettings -Command $exe -Arguments @('hook', '--exec')
    Assert-Equal @(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $true).Count 0 'allowed'
    $refused = @(Test-InstalledHooks -Settings $settings -ExpectedExe $exe -ExecFormAllowed $false)
    Assert-True ($refused -match 'exec form was written') 'not allowed'
    $wrong = New-InstalledSettings -Command $exe -Arguments @('hook')
    Assert-True (@(Test-InstalledHooks -Settings $wrong -ExpectedExe $exe -ExecFormAllowed $true) -match "args are 'hook'") 'a missing --exec'
}

Test-Case 'our hook entries: a missing event, a second command, a missing timeout and a foreign path are named' {
    $exe = 'C:\p\.claude\hooks\agentnotch-hook.exe'
    $good = 'C:/p/.claude/hooks/agentnotch-hook.exe hook'
    $noTimeout = New-InstalledSettings -Command $good -NoPermissionTimeout
    Assert-True (@(Test-InstalledHooks -Settings $noTimeout -ExpectedExe $exe -ExecFormAllowed $false) -match 'PermissionRequest timeout') 'the 86400 s timeout'
    $other = New-InstalledSettings -Command 'C:/elsewhere/agentnotch-hook.exe hook'
    $found = @(Test-InstalledHooks -Settings $other -ExpectedExe $exe -ExecFormAllowed $false)
    Assert-True ($found -match 'no hook entry of ours under PreToolUse') 'a path that is not the folder''s copy is not ours'
    $partial = New-InstalledSettings -Command $good
    $partial.hooks.PSObject.Properties.Remove('Stop')
    Assert-True (@(Test-InstalledHooks -Settings $partial -ExpectedExe $exe -ExecFormAllowed $false) -match 'no hook entry of ours under Stop') 'a missing event'
    $mixed = New-InstalledSettings -Command $good
    $mixed.hooks.Stop[0].hooks[0].command = 'C:/p/.claude/hooks/agentnotch-hook.exe statusline'
    Assert-True (@(Test-InstalledHooks -Settings $mixed -ExpectedExe $exe -ExecFormAllowed $false) -match 'no hook entry of ours under Stop') 'the wrong verb'
}

Test-Case 'the status line wrapper is recognised by its verb and its path' {
    $exe = 'C:\p\.claude\hooks\agentnotch-hook.exe'
    $wrapped = [pscustomobject]@{ Command = 'C:/p/.claude/hooks/agentnotch-hook.exe statusline'; Args = $null }
    Assert-Equal (Get-OwnForm -Entry $wrapped -ExpectedExe $exe -Verb 'statusline') 'string' 'ours'
    Assert-Equal $null (Get-OwnForm -Entry $wrapped -ExpectedExe $exe -Verb 'hook') 'not the hook verb'
    $plain = [pscustomobject]@{ Command = 'echo smoke-status'; Args = $null }
    Assert-Equal $null (Get-OwnForm -Entry $plain -ExpectedExe $exe -Verb 'statusline') 'the user''s own command'
}

Test-Case 'settings style: the byte order mark and CRLF must survive, and only the edited keys may change' {
    $text = '{"theme":"dark","env":{"A":"1"},"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo user"}]}]}}'
    $crlf = [byte[]](0xEF, 0xBB, 0xBF) + [Text.Encoding]::UTF8.GetBytes(($text -replace ',', ",`r`n"))
    Assert-Equal @(Test-SettingsStyle -Bytes $crlf).Count 0 'BOM and CRLF'
    Assert-True (@(Test-SettingsStyle -Bytes ([Text.Encoding]::UTF8.GetBytes($text))) -match 'byte order mark') 'a lost BOM'
    $lf = [byte[]](0xEF, 0xBB, 0xBF) + [Text.Encoding]::UTF8.GetBytes(($text -replace ',', ",`n"))
    Assert-True (@(Test-SettingsStyle -Bytes $lf) -match 'bare LF') 'LF endings'
    $before = ConvertFrom-SettingsBytes -Bytes $crlf
    Assert-Equal $before.theme 'dark' 'the BOM is skipped when parsing'
    $after = ConvertFrom-SettingsBytes -Bytes ([Text.Encoding]::UTF8.GetBytes(($text -replace '"dark"', '"light"')))
    Assert-Equal ((Test-OtherKeysUnchanged -Before $before -After $after) -join '|') "key 'theme' changed" 'a changed key'
    $gone = ConvertFrom-SettingsBytes -Bytes ([Text.Encoding]::UTF8.GetBytes('{"theme":"dark","extra":1,"hooks":{}}'))
    Assert-Equal ((Test-OtherKeysUnchanged -Before $before -After $gone) -join '|') "key 'env' is gone|key 'extra' was added" 'a lost and an added key'
    Assert-Equal @(Test-OtherKeysUnchanged -Before $before -After $gone -Edited @('hooks', 'env', 'extra')).Count 0 'edited keys are not compared'
}

Test-Case 'the user''s own hooks must still be there after the install' {
    $before = ConvertFrom-SettingsBytes -Bytes ([Text.Encoding]::UTF8.GetBytes('{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo foreign-hook"}]}]}}'))
    $kept = New-InstalledSettings -Command 'C:/p/h.exe hook'
    Assert-Equal @(Test-ForeignHooksKept -Before $before -After $kept).Count 0 'kept'
    $lost = ConvertFrom-SettingsBytes -Bytes ([Text.Encoding]::UTF8.GetBytes('{"hooks":{"Stop":[]}}'))
    Assert-Equal (@(Test-ForeignHooksKept -Before $before -After $lost) -join '|') "a PreToolUse hook entry of the user's is gone" 'lost'
    Assert-Equal @(Test-ForeignHooksKept -Before ([pscustomobject]@{}) -After $lost).Count 0 'a file with no hooks has nothing to keep'
}

Test-Case 'the exec form is allowed from the first version after the last one that cannot run it' {
    $facts = '{"schema":1,"versions":[{"version":"2.0.0","hook_exec_form":false},{"version":"2.1.138","hook_exec_form":false},{"version":"2.1.139","hook_exec_form":true},{"version":"2.1.286","hook_exec_form":true}],"exec_form_min":null}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $facts).ToString() '2.1.139' 'derived'
    Assert-True (Test-ExecFormAllowed -Facts $facts -ClaudeVersion '2.1.282') '2.1.282 is above it'
    Assert-True (-not (Test-ExecFormAllowed -Facts $facts -ClaudeVersion '2.1.138')) '2.1.138 is below it'
    Assert-True (-not (Test-ExecFormAllowed -Facts $facts -ClaudeVersion 'garbage')) 'an unknown version is never allowed'
    $newestUnknown = '{"versions":[{"version":"2.1.139","hook_exec_form":true},{"version":"2.1.286","hook_exec_form":null}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $newestUnknown) $null 'nothing is derived unless the newest runs the form'
    $unknown = '{"versions":[{"version":"2.1.100","hook_exec_form":null},{"version":"2.1.139","hook_exec_form":true}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $unknown).ToString() '2.1.139' 'a null (not known) is not support'
    $allTrue = '{"versions":[{"version":"2.1.100","hook_exec_form":true},{"version":"2.1.139","hook_exec_form":true}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $allTrue).ToString() '2.1.100' 'every listed version runs it'
}

Test-Case 'an explicit exec_form_min wins unless a listed version at or above it says no' {
    $explicit = '{"exec_form_min":"2.1.200","versions":[{"version":"2.1.100","hook_exec_form":false},{"version":"2.1.286","hook_exec_form":true}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $explicit).ToString() '2.1.200' 'explicit'
    $contradicted = '{"exec_form_min":"2.1.200","versions":[{"version":"2.1.250","hook_exec_form":false},{"version":"2.1.286","hook_exec_form":true}]}' | ConvertFrom-Json
    Assert-Equal (Get-ExecFormMin -Facts $contradicted).ToString() '2.1.286' 'contradicted, so derived'
    Assert-Equal (Get-ExecFormMin -Facts ('{}' | ConvertFrom-Json)) $null 'no facts, no exec form'
}

Test-Case 'the committed facts file, when it is here, can be read by the exec form rule' {
    $path = Get-FactsPath
    if (-not (Test-Path -LiteralPath $path)) { return }
    $facts = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json -Depth 50
    $null = Get-ExecFormMin -Facts $facts
    Assert-True ($facts.schema -eq 1) 'schema 1'
}

Test-Case 'the permission answers: each expected output is the proto crate''s own file, for the stdin it was made for' {
    $cases = @(Get-PermissionCases)
    Assert-Equal $cases.Count 6 'allow, always, deny, a question chip, approve plan, keep planning'
    Assert-Equal (($cases | ForEach-Object { $_.Click }) -join ',') 'allow,always,deny,option:0,approve,keep' 'the buttons'
    Assert-Equal @($cases | Where-Object { $_.Chat }).Count 1 'only Keep planning is answered from the chat'
    foreach ($case in $cases) {
        $response = Get-Content -LiteralPath (Join-Path (Get-ProtoFixtureDir) "v1-responses\$($case.Expected).json") -Raw | ConvertFrom-Json -Depth 50
        Assert-Equal $response.stdin $case.Stdin "the response fixture '$($case.Expected)' was made for $($case.Stdin)"
        $bytes = Get-ExpectedPermissionBytes -Name $case.Expected
        $out = [Text.Encoding]::UTF8.GetString($bytes) | ConvertFrom-Json -Depth 50
        Assert-Equal $out.hookSpecificOutput.hookEventName 'PermissionRequest' "$($case.Name) names the event"
        Assert-True ($bytes[-1] -ne 10) "$($case.Name) has no trailing newline"
    }
    $decision = { param($name) ([Text.Encoding]::UTF8.GetString((Get-ExpectedPermissionBytes -Name $name)) | ConvertFrom-Json -Depth 50).hookSpecificOutput.decision }
    Assert-Equal (& $decision 'allow').behavior 'allow' 'allow'
    Assert-Equal (& $decision 'deny').behavior 'deny' 'deny'
    Assert-Equal (& $decision 'keep_planning').behavior 'deny' 'keep planning is a denial'
    Assert-True ((& $decision 'keep_planning').message -match 'keep planning') 'with the reason that says so'
    Assert-Equal (& $decision 'always').updatedPermissions[0].rules[0].ruleContent 'npm run test:*' 'always allow carries the first suggestion'
    $question = Get-Content -LiteralPath (Join-Path (Get-ProtoFixtureDir) 'stdin\permission_request_question.json') -Raw | ConvertFrom-Json -Depth 50
    $firstLabel = $question.tool_input.questions[0].options[0].label
    Assert-Equal ((& $decision 'question').updatedInput.answers.PSObject.Properties.Value) $firstLabel 'the chip is option 0'
}

Test-Case 'a hook stdin keeps the fixture''s tool input and takes the smoke session''s identity' {
    $transcript = 'C:\p\.claude\projects\x\5d1e.jsonl'
    $request = New-HookStdin -Fixture 'permission_request_bash' -SessionId 'sid-1' -Transcript $transcript -Cwd 'C:\smoke\work-app' | ConvertFrom-Json -Depth 50
    $fixture = Get-Content -LiteralPath (Join-Path (Get-ProtoFixtureDir) 'stdin\permission_request_bash.json') -Raw | ConvertFrom-Json -Depth 50
    Assert-Equal $request.session_id 'sid-1' 'session'
    Assert-Equal $request.transcript_path $transcript 'transcript'
    Assert-Equal $request.cwd 'C:\smoke\work-app' 'cwd'
    Assert-Equal $request.hook_event_name 'PermissionRequest' 'the event stays'
    Assert-Equal (ConvertTo-CompactJson $request.tool_input) (ConvertTo-CompactJson $fixture.tool_input) 'the tool input is the fixture''s'
    Assert-True ($null -ne $request.permission_suggestions) 'the suggestions stay on a request'
    $pre = New-HookStdin -Fixture 'permission_request_question' -SessionId 'sid-1' -Transcript $transcript -Cwd 'C:\x' -EventName 'PreToolUse' -ToolUseId 'toolu_9' | ConvertFrom-Json -Depth 50
    Assert-Equal $pre.hook_event_name 'PreToolUse' 'PreToolUse'
    Assert-Equal $pre.tool_use_id 'toolu_9' 'a PreToolUse has the tool use id'
    Assert-Equal $pre.tool_name 'AskUserQuestion' 'and the request''s tool'
    $status = New-HookStdin -Fixture 'status_line' -SessionId 'sid-1' -Transcript $transcript -Cwd 'C:\x' | ConvertFrom-Json -Depth 50
    Assert-Equal $status.session_id 'sid-1' 'the status line takes the session too'
    Assert-Equal $status.context_window.used_percentage 37 'and keeps its numbers'
}

Test-Case 'the status line output is the original command''s, whatever line end the shell used' {
    Assert-Equal @(Test-StatusLineOutput -Stdout "smoke-status`n" -Expected 'smoke-status').Count 0 'LF'
    Assert-Equal @(Test-StatusLineOutput -Stdout "smoke-status`r`n" -Expected 'smoke-status').Count 0 'CRLF'
    Assert-Equal @(Test-StatusLineOutput -Stdout 'smoke-status' -Expected 'smoke-status').Count 0 'none'
    Assert-Equal @(Test-StatusLineOutput -Stdout '' -Expected 'smoke-status').Count 1 'nothing printed'
    Assert-Equal @(Test-StatusLineOutput -Stdout 'smoke-status extra' -Expected 'smoke-status').Count 1 'more printed'
}

Test-Case 'after Turn off the only new files in P are our backups' {
    $backups = @('added: .claude/settings.json.agentnotch-20261001-120000-123.bak', 'added: .claude/settings.json.agentnotch.original.bak', 'added: .claude-work/settings.json.agentnotch.original.bak')
    Assert-Equal @(Test-OnlyBackupsAdded -Differences $backups).Count 0 'backups'
    Assert-Equal @(Test-OnlyBackupsAdded -Differences @()).Count 0 'nothing'
    $bad = @(Test-OnlyBackupsAdded -Differences ($backups + 'added: .claude/hooks/agentnotch-hook.exe' + 'changed: .claude/settings.json' + 'removed: .claude.json' + 'added: elsewhere/settings.json.agentnotch.original.bak'))
    Assert-Equal $bad.Count 4 'a hook copy left behind, a changed or removed file and a backup elsewhere are all named'
}

Test-Case 'the cdp answer is the last line of its output; a failure carries the page''s error' {
    Assert-Equal (ConvertFrom-CdpOutput -Text "warning`n{`"ok`":true,`"result`":42}`n") 42 'a value'
    Assert-Equal (ConvertFrom-CdpOutput -Text '{"ok":true,"result":null}') $null 'null'
    Assert-Throws { ConvertFrom-CdpOutput -Text '{"ok":false,"error":"click: nothing matches [x]"}' } 'click: nothing matches'
    Assert-Throws { ConvertFrom-CdpOutput -Text '' } 'printed nothing'
    Assert-Throws { ConvertFrom-CdpOutput -Text 'oops' } 'not JSON'
}

Test-Case 'the hook runner''s answer is parsed, and a tool that failed to run is an error' {
    $runs = ConvertFrom-HookRuns -Text '{"runs":[{"source":"PreToolUse[0]","shell":"bash","exit":0,"ms":120,"stdout":"","stdout_b64":""},{"source":"x","skipped":true,"reason":"no bash"}]}'
    Assert-Equal $runs.Count 2 'two runs'
    Assert-Equal $runs[0]['ms'] 120 'its time'
    Assert-True $runs[1].ContainsKey('skipped') 'a skipped shell is visible'
    Assert-Throws { ConvertFrom-HookRuns -Text '{"error":"nope","runs":[]}' } 'run-hook.mjs: nope'
    Assert-Throws { ConvertFrom-HookRuns -Text '' } 'printed nothing'
}

Test-Case 'the selectors are in one table, and the answer selector names the button, the session and skips answered ones' {
    foreach ($name in 'ConsentTurnOn', 'HooksSwitch', 'HooksSwitchOn', 'Answer', 'OpenChat', 'Back', 'StatusLineAlone', 'OpenSettings') {
        Assert-True ($script:Ui.ContainsKey($name) -and $script:Ui[$name]) "the table has $name"
    }
    $selector = $script:Ui.Answer -f 'option:0', 'sid-1'
    Assert-Equal $selector '[data-an-action="answer"][data-an-arg="option:0"][data-an-session="sid-1"]:not(.an-answered)' 'the answer selector'
    Assert-Equal ($script:Ui.OpenChat -f 'sid-1') '[data-an-action="open-chat"][data-an-arg="sid-1"]' 'the chat selector'
    Assert-True ($script:AnswerGateSeconds -ge 0.35) 'the gate wait is not shorter than the panel''s 0.35 s'
}

Test-Case 'the page expressions are valid JavaScript (checked with node when it is here)' {
    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $node) { return }
    $armed = Get-ArmedExpression -Selector ($script:Ui.Answer -f 'option:0', "it's")
    $literal = ConvertTo-JsString "[data-an-x=`"a'b`"]"
    foreach ($expression in $armed, "!!document.querySelector($literal)") {
        $file = Join-Path $scratch 'expression.js'
        Set-Content -LiteralPath $file -Value "new Function('return ' + $(ConvertTo-Json $expression -Compress));"
        & $node.Source $file
        Assert-Equal $LASTEXITCODE 0 "node accepts: $expression"
    }
}

Test-Case 'a node script is started with exact arguments, read to its end and stopped by handle' {
    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $node) { return }
    $js = Join-Path $scratch 'echo-args.js'
    Set-Content -LiteralPath $js -Value "console.log(JSON.stringify(process.argv.slice(2)));"
    $odd = @('--env', 'A=b c', '{"disableAllHooks":true}', 'quote"inside')
    $run = Wait-NodeScript -Run (Start-NodeScript -Script $js -Arguments $odd)
    Assert-Equal $run.ExitCode 0 'exit'
    Assert-Equal ((($run.Stdout.Trim()) | ConvertFrom-Json) -join '|') ($odd -join '|') 'arguments arrive exactly'
    $slow = Join-Path $scratch 'sleep.js'
    Set-Content -LiteralPath $slow -Value "setTimeout(() => {}, 60000);"
    $started = Start-NodeScript -Script $slow
    Assert-Throws { Wait-NodeScript -Run $started -TimeoutSeconds 1 } 'did not finish within 1 s'
    Assert-True $started.Process.HasExited 'the slow one was stopped'
}

Test-Case 'a free port can be bound again right after it is handed out' {
    $port = Get-FreeTcpPort
    Assert-True ($port -gt 1023) 'a real port'
    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $port)
    $listener.Start()
    $listener.Stop()
}

Test-Case 'phases 5-9 are in the table in order, behind the engine gate, between phase 4 and phase 13' {
    $table = @(Get-PhaseTable)
    $numbers = @($table | ForEach-Object { $_.Number })
    $at = $numbers.IndexOf('5')
    Assert-Equal (($numbers[$at..($at + 4)]) -join ',') '5,6,7,8,9' 'consecutive'
    Assert-True ($numbers.IndexOf('4') -lt $at -and $numbers.IndexOf('13') -gt $at + 4) 'between 4 and 13'
    foreach ($n in '5', '6', '7', '8', '9') {
        Assert-Equal (@($table | Where-Object { $_.Number -eq $n })[0].Gate) 'engine' "phase $n waits for the engine gate"
    }
}

Test-Case 'with the engine gate closed phases 5-9 are reported as waiting and nothing is launched' {
    $gatesPath = Join-Path (New-Scratch 'gates-engine') 'g.json'
    Write-GatesFile $gatesPath @{}
    $gates = Read-Gates -Path $gatesPath
    foreach ($phase in @(Get-PhaseTable -Gates $gates | Where-Object { $_.Number -in '5', '6', '7', '8', '9' })) {
        Assert-True (-not (Test-GateOpen -Gates $gates -Name $phase.Gate)) "phase $($phase.Number) is gated"
        Assert-True ((Get-GateWarning -Gates $gates -Number $phase.Number -Gate $phase.Gate) -match "^::warning::phase $($phase.Number) not run: waiting for ") 'with its warning'
    }
}

# --- phases 10-12 -----------------------------------------------------------------------------------------

$site = 'http://127.0.0.1:4555'
$rfcChallenge = 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM'
function New-AuthorizeUrl([hashtable]$Override = @{}, [string]$Base = $site) {
    $parts = [ordered]@{
        provider              = 'google'
        redirect_to           = 'agentnotch://auth-callback'
        code_challenge        = $rfcChallenge
        code_challenge_method = 's256'
    }
    foreach ($key in $Override.Keys) { if ($null -eq $Override[$key]) { $parts.Remove($key) } else { $parts[$key] = $Override[$key] } }
    "$Base/auth/v1/authorize?" + (($parts.GetEnumerator() | ForEach-Object { "$($_.Key)=$([Uri]::EscapeDataString([string]$_.Value))" }) -join '&')
}

Test-Case 'the authorize URL the Mac builds passes (lower-case s256, the RFC 7636 challenge, the redirect escaped or not)' {
    Assert-Equal @(Test-AuthorizeUrl -Url (New-AuthorizeUrl) -Website $site).Count 0 'problems'
    Assert-Equal @(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ code_challenge_method = 'S256' }) -Website "$site/").Count 0 'upper-case S256, website with a slash'
    Assert-Equal @(Test-AuthorizeUrl -Url "$site/auth/v1/authorize?provider=google&redirect_to=agentnotch://auth-callback&code_challenge=$rfcChallenge&code_challenge_method=s256" -Website $site).Count 0 'redirect not escaped'
    Assert-Equal @(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ state = 'abc123' }) -Website $site).Count 0 'a state parameter is fine'
}

Test-Case 'a challenge that is not 43 base64url characters is refused' {
    foreach ($bad in 'short', ($rfcChallenge + 'A'), ($rfcChallenge.Substring(1) + '='), ($rfcChallenge.Substring(1) + '+')) {
        $problems = @(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ code_challenge = $bad }) -Website $site)
        Assert-True ($problems -match 'code_challenge .* is not 43 base64url') "refused <$bad>"
    }
}

Test-Case 'the plain method, a missing method and a missing challenge are refused' {
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ code_challenge_method = 'plain' }) -Website $site) -match "expected S256") 'plain'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ code_challenge_method = $null }) -Website $site) -match "'code_challenge_method' parameter, found 0") 'no method'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ code_challenge = $null }) -Website $site) -match "'code_challenge' parameter, found 0") 'no challenge'
}

Test-Case 'another redirect, another provider, another website and a relative URL are refused' {
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ redirect_to = 'https://evil.example/auth-callback' }) -Website $site) -match 'redirect_to is') 'redirect'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ redirect_to = 'agentnotch://other' }) -Website $site) -match 'redirect_to is') 'host of the redirect'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ provider = 'github' }) -Website $site) -match 'provider is') 'provider'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{} 'http://127.0.0.1:9') -Website $site) -match 'does not start with') 'website'
    Assert-True (@(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{} 'https://accounts.google.com') -Website $site) -match 'does not start with') 'a real host'
    Assert-True (@(Test-AuthorizeUrl -Url '/auth/v1/authorize?provider=google' -Website $site) -match 'not an absolute URL') 'relative'
    Assert-True (@(Test-AuthorizeUrl -Url '' -Website $site) -match 'not an absolute URL') 'empty'
}

Test-Case 'a URL that carries a secret is refused: the verifier, a token, a key, a code, a login' {
    foreach ($name in 'code_verifier', 'access_token', 'refresh_token', 'apikey', 'secret', 'auth_code', 'code') {
        $problems = @(Test-AuthorizeUrl -Url (New-AuthorizeUrl @{ $name = 'x' }) -Website $site)
        Assert-True ($problems -match "carries '$name'") "refused $name"
    }
    Assert-True (@(Test-AuthorizeUrl -Url "http://user:pw@127.0.0.1:4555/auth/v1/authorize?provider=google&redirect_to=agentnotch://auth-callback&code_challenge=$rfcChallenge&code_challenge_method=s256" -Website $site).Count -gt 0) 'user info'
}

Test-Case 'a duplicated parameter is refused (which one would the server read?)' {
    $url = (New-AuthorizeUrl) + '&provider=github'
    Assert-True (@(Test-AuthorizeUrl -Url $url -Website $site) -match "exactly one 'provider' parameter, found 2") 'duplicate'
}

Test-Case 'the authorize URL is found in the dev browser log, whatever else the log holds' {
    $good = New-AuthorizeUrl
    Assert-Equal @(Get-AuthorizeUrls -Text '').Count 0 'empty log'
    Assert-Equal (@(Get-AuthorizeUrls -Text "$good`n")[0]) $good 'a plain line'
    Assert-Equal (@(Get-AuthorizeUrls -Text "2026-10-01T10:00:00Z open $good`r`n")[0]) $good 'a stamped line'
    Assert-Equal (@(Get-AuthorizeUrls -Text "{`"url`":`"$good`"}`n")[0]) $good 'a JSON line'
    Assert-Equal @(Get-AuthorizeUrls -Text "https://example.com/other`nnothing`n").Count 0 'other URLs are not it'
    Assert-Equal @(Get-AuthorizeUrls -Text "$good`n$good`n").Count 2 'two sign-ins, two URLs'
}

Test-Case 'the fake website log is read line by line; a torn or foreign line is skipped' {
    $text = "{`"method`":`"GET`",`"path`":`"/api/app/v1/config`",`"status`":200}`n{ torn`nnot json`n{`"method`":`"POST`",`"path`":`"/api/app/v1/sync`",`"status`":200,`"authorization`":`"present`"}`r`n[1,2]`n"
    $entries = @(ConvertFrom-RequestLog -Text $text)
    Assert-Equal $entries.Count 2 'two requests'
    Assert-Equal $entries[1]['authorization'] 'present' 'fields'
    Assert-Equal (Get-RequestCount -Requests $entries -Method 'POST' -Path '/api/app/v1/sync') 1 'one sync'
    Assert-Equal (Get-RequestCount -Requests $entries -Method 'GET' -Path '/api/app/v1/sync') 0 'method counts'
    Assert-Equal (Get-RequestCount -Requests @() -Method 'POST' -Path '/auth/v1/logout') 0 'no requests'
    Assert-Equal @(ConvertFrom-RequestLog -Text '').Count 0 'empty'
}

Test-Case 'the cloud switches are read from the settings snapshot; anything but true is off' {
    $on = Get-CloudSwitches -Settings @{ cloud = @{ sync_enabled = $true; summaries_enabled = $true } }
    Assert-True ($on.Sync -and $on.Summaries) 'both on'
    $off = Get-CloudSwitches -Settings @{ cloud = @{ sync_enabled = $false; summaries_enabled = 'yes' } }
    Assert-True (-not $off.Sync -and -not $off.Summaries) 'false and a non-boolean are off'
    $none = Get-CloudSwitches -Settings @{ other = 1 }
    Assert-True (-not $none.Sync -and -not $none.Summaries) 'no cloud object is off'
    $null = Get-CloudSwitches -Settings $null
}

Test-Case 'phases 10-12 are in the table in order between 9 and 13: 10 behind the cloud gate, 11 and 12 behind the engine gate' {
    $table = @(Get-PhaseTable)
    $numbers = @($table | ForEach-Object { $_.Number })
    $at = $numbers.IndexOf('9')
    Assert-Equal (($numbers[$at..($at + 4)]) -join ',') '9,10,11,12,13' 'consecutive'
    Assert-Equal (@($table | Where-Object { $_.Number -eq '10' })[0].Gate) 'cloud' 'phase 10'
    Assert-Equal (@($table | Where-Object { $_.Number -eq '11' })[0].Gate) 'engine' 'phase 11'
    Assert-Equal (@($table | Where-Object { $_.Number -eq '12' })[0].Gate) 'engine' 'phase 12'
}

Test-Case 'with the cloud gate closed phase 10 is reported as waiting and the website is never started' {
    $gatesPath = Join-Path (New-Scratch 'gates-cloud') 'g.json'
    Write-GatesFile $gatesPath @{ engine = $true }
    $gates = Read-Gates -Path $gatesPath
    $phase = @(Get-PhaseTable -Gates $gates | Where-Object { $_.Number -eq '10' })[0]
    Assert-True (-not (Test-GateOpen -Gates $gates -Name $phase.Gate)) 'gated'
    Assert-True ((Get-GateWarning -Gates $gates -Number '10' -Gate $phase.Gate) -match '^::warning::phase 10 not run: waiting for ') 'with its warning'
}

Test-Case 'the fake website serves the sign-in answers phase 10 depends on (config names itself; the code exchange gives a session)' {
    $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue
    if (-not $node) { return }
    $log = Join-Path (New-Scratch 'fake-site') 'log.jsonl'
    $running = Start-FakeWebsite -LogFile $log -SyncDir (Join-Path (Split-Path $log) 'sync')
    try {
        Assert-True ($running.Url -match '^http://127\.0\.0\.1:\d+$') 'loopback only'
        $config = Invoke-RestMethod -Uri "$($running.Url)/api/app/v1/config"
        Assert-Equal $config.supabaseUrl $running.Url 'supabaseUrl names the website itself'
        $session = Invoke-RestMethod -Method Post -Uri "$($running.Url)/auth/v1/token?grant_type=pkce" -ContentType 'application/json' -Body '{"auth_code":"smoke","code_verifier":"v"}'
        Assert-True ([bool]$session.access_token) 'a session'
        Start-Sleep -Milliseconds 200
        Assert-Equal (Get-RequestCount -Requests (Get-FakeWebsiteRequests -LogFile $log) -Method 'POST' -Path '/auth/v1/token') 1 'the exchange is logged'
    } finally {
        Stop-OwnProcess -Process $running.Process
    }
}

} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count) { "$($failures.Count) failed: $($failures -join '; ')"; exit 1 }
'all smoke-script cases passed'
