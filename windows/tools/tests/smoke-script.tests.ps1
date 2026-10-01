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
