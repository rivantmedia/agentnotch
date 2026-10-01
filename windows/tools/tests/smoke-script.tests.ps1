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

Test-Case 'the usage fixture is JSON the probe parser can read (five-hour and weekly windows)' {
    $usage = Get-Content (Join-Path $fixtures 'usage.json') -Raw | ConvertFrom-Json
    Assert-Equal $usage.five_hour.utilization 12 'five hour'
    Assert-Equal $usage.seven_day.utilization 34 'weekly'
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
    $closed = Read-Gates -Path $committedGates
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

} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failures.Count) { "$($failures.Count) failed: $($failures -join '; ')"; exit 1 }
'all smoke-script cases passed'
