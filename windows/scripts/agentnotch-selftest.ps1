<#
.SYNOPSIS
  Runs Agent Notch's sealed self-test at 100, 125 and 150 % page scale and one sealed snapshot
  run, and fails on anything wrong (DESIGN-WIN 7.4, 5.6).

  The 125 and 150 % runs ask the app for the scale (AGENTNOTCH_SELF_TEST_SCALE): WebView2 takes
  a page's scale from its window and ignores --force-device-scale-factor, so the app sets each
  page's scale itself. A scaled run whose report names another scale fails.

.DESCRIPTION
  Every run is sealed (AGENTNOTCH_SAFE_MODE=1): fixture data only, no Claude folder, no network,
  no child process, and its data in %APPDATA%\Agent Notch Sealed, which is deleted before and
  after each run. The exe is a GUI-subsystem program: the exit code comes from the process
  object, the text from run.log.

  Written into -Out:
    selftest-<scale>.json, selftest-<scale>-run.log   (scale 1, 1.25, 1.5)
    snapshots\*.png, snapshots\manifest.json, snapshots-run.log

  Comparing the snapshots with their baselines is not done here (the smoke test does that).
  It stops only processes it started, by id, and refuses to start when any agentnotch.exe runs:
  a sealed run beside a running Agent Notch forwards its arguments to it and exits 0 with no
  report.

.PARAMETER Exe
  The agentnotch.exe to run. Never an installed one that is in use.

.PARAMETER Out
  Where the reports, logs and snapshots go. Made when missing.
#>
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Out
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# One run's time, from the start to the exit. The app's own deadline is 120 s.
$RunSeconds = 180
$Scales = @('1', '1.25', '1.5')
$EdgeNames = @('right', 'left', 'top', 'bottom', 'floating')
$PageNames = @('notch', 'settings', 'agentnotch-panel')
$SnapshotNames = @(
  'notch-right', 'notch-left', 'notch-top', 'notch-bottom',
  'panel-list-right', 'panel-list-left', 'panel-list-top', 'panel-list-bottom',
  'panel-floating', 'panel-chat', 'settings-claude'
)
$SwitchVariables = @(
  'AGENTNOTCH_SAFE_MODE', 'AGENTNOTCH_PANEL_SELF_TEST', 'AGENTNOTCH_SELF_TEST_OUT',
  'AGENTNOTCH_SELF_TEST_SCALE', 'AGENTNOTCH_SNAPSHOT_CLAUDE'
)

$failures = New-Object System.Collections.Generic.List[string]
function Add-Failure([string]$line) {
  $failures.Add($line)
  Write-Host "FAILED: $line"
}

# A property of a parsed JSON object, or $null when it has none (strict mode would throw).
function Get-Prop($object, [string]$name) {
  if ($null -eq $object) { return $null }
  $property = $object.PSObject.Properties[$name]
  if ($null -eq $property) { return $null }
  return $property.Value
}

# True only for a real boolean true: a missing property, $null or "true" text is not.
function Is-True($value) {
  return ($value -is [bool]) -and $value
}

function Clear-Switches {
  foreach ($name in $SwitchVariables) {
    Remove-Item -Path "Env:$name" -ErrorAction SilentlyContinue
  }
}

$Exe = (Resolve-Path -LiteralPath $Exe).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path

$appData = $env:APPDATA
if (-not $appData) { throw 'APPDATA is not set' }
$sealedData = Join-Path $appData 'Agent Notch Sealed'
$realData = Join-Path $appData 'Agent Notch'
$realDataBefore = Test-Path -LiteralPath $realData

function Assert-NoAgentNotch([string]$when) {
  $running = @(Get-Process -Name 'agentnotch' -ErrorAction SilentlyContinue)
  if ($running.Count -gt 0) {
    $ids = ($running | ForEach-Object { $_.Id }) -join ', '
    throw "agentnotch.exe is running ($when; process $ids). Close it first: a sealed run beside a running copy hands its launch to that copy and writes no report."
  }
}

# The sealed folder is recreated by every run. A lingering WebView2 process may hold it for a
# moment after the app has exited.
function Remove-SealedData {
  for ($try = 0; $try -lt 20; $try++) {
    Remove-Item -LiteralPath $sealedData -Recurse -Force -ErrorAction SilentlyContinue
    if (-not (Test-Path -LiteralPath $sealedData)) { return }
    Start-Sleep -Milliseconds 500
  }
  throw "could not delete $sealedData"
}

# Starts the exe with the variables already set in this process, waits for it and gives its
# exit code, or $null after killing it at the deadline.
function Invoke-Sealed([string]$what) {
  $process = Start-Process -FilePath $Exe -PassThru
  # Reading the handle keeps the exit code available after the process is gone.
  $null = $process.Handle
  Clear-Switches
  if (-not $process.WaitForExit($RunSeconds * 1000)) {
    Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    $process.WaitForExit(10000) | Out-Null
    Add-Failure "${what}: no exit within $RunSeconds s (killed process $($process.Id))"
    return $null
  }
  $process.WaitForExit()
  return $process.ExitCode
}

function Copy-RunLog([string]$to) {
  $log = Join-Path $sealedData 'run.log'
  if (Test-Path -LiteralPath $log) {
    Copy-Item -LiteralPath $log -Destination $to -Force
  } else {
    Add-Failure "no run.log was written (expected $log)"
  }
}

# ---- the self-test's report ----

function Test-Report([string]$what, [string]$path, $exitCode, [string]$scale) {
  if ($null -ne $exitCode -and $exitCode -ne 0) { Add-Failure "${what}: exit code $exitCode" }
  if (-not (Test-Path -LiteralPath $path)) {
    Add-Failure "${what}: no report at $path"
    return
  }
  try {
    $report = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
  } catch {
    Add-Failure "${what}: the report is not JSON ($($_.Exception.Message))"
    return
  }
  if (-not (Is-True (Get-Prop $report 'ok'))) {
    Add-Failure "${what}: ok is not true (error: $(Get-Prop $report 'error'))"
  }
  foreach ($line in @(Get-Prop $report 'failures')) {
    if ($line) { Add-Failure "${what}: $line" }
  }
  $drawn = Get-Prop $report 'scale'
  Write-Host "${what}: scale $drawn, version $(Get-Prop $report 'version')"
  # The first run is the machine as it is (a hosted runner is at 100 %, a desk may not be); a
  # scaled run must have been drawn at its scale, or it proved nothing about it.
  if ($scale -ne '1') {
    $asked = [double]::Parse($scale, [System.Globalization.CultureInfo]::InvariantCulture)
    $value = $null
    if ($null -ne $drawn) { $value = $drawn -as [double] }
    if ($null -eq $value -or [math]::Abs($value - $asked) -gt 0.01) {
      Add-Failure "${what}: the pages were drawn at scale '$drawn', not $scale"
    }
  }

  $edges = @(Get-Prop $report 'edges')
  $seen = @($edges | ForEach-Object { Get-Prop $_ 'edge' })
  if (($seen -join ',') -ne ($EdgeNames -join ',')) {
    Add-Failure "${what}: the edges are '$($seen -join ',')', expected '$($EdgeNames -join ',')'"
  }
  foreach ($edge in $edges) {
    $name = Get-Prop $edge 'edge'
    foreach ($field in 'inside_work_area', 'tail_inside_corners', 'topmost', 'above_notch') {
      if (-not (Is-True (Get-Prop $edge $field))) { Add-Failure "${what}: edge ${name}: $field is not true" }
    }
    $auto = Get-Prop $edge 'auto'
    foreach ($field in 'no_activate', 'gate_shut', 'gate_opens_on_confirmation') {
      if (-not (Is-True (Get-Prop $auto $field))) { Add-Failure "${what}: edge ${name}: auto.$field is not true" }
    }
  }

  $pages = Get-Prop $report 'pages'
  foreach ($page in $PageNames) {
    $entry = Get-Prop $pages $page
    if ($null -eq $entry) {
      Add-Failure "${what}: no report for the $page page"
      continue
    }
    if (-not (Is-True (Get-Prop $entry 'round_trip'))) { Add-Failure "${what}: ${page}: round_trip is not true" }
    $csp = @(Get-Prop $entry 'csp_violations')
    if ($csp.Count -ne 0) { Add-Failure "${what}: ${page}: $($csp.Count) CSP violations: $($csp | ConvertTo-Json -Compress -Depth 4)" }
    $errors = @(Get-Prop $entry 'errors')
    if ($errors.Count -ne 0) { Add-Failure "${what}: ${page}: $($errors.Count) page errors: $($errors | ConvertTo-Json -Compress -Depth 4)" }
    $invariants = Get-Prop $entry 'invariants'
    $names = @()
    if ($null -ne $invariants) { $names = @($invariants.PSObject.Properties) }
    if ($names.Count -eq 0) { Add-Failure "${what}: ${page}: no invariants were checked" }
    foreach ($invariant in $names) {
      if (-not (Is-True $invariant.Value)) { Add-Failure "${what}: ${page}: invariant $($invariant.Name) is not true" }
    }
  }
}

# ---- the runs ----

try {
  Assert-NoAgentNotch 'before the runs'

  foreach ($scale in $Scales) {
    $what = "self-test at scale $scale"
    Write-Host "== $what"
    Remove-SealedData
    $report = Join-Path $Out "selftest-$scale.json"
    Remove-Item -LiteralPath $report -Force -ErrorAction SilentlyContinue
    Clear-Switches
    $env:AGENTNOTCH_SAFE_MODE = '1'
    $env:AGENTNOTCH_PANEL_SELF_TEST = '1'
    $env:AGENTNOTCH_SELF_TEST_OUT = $report
    if ($scale -ne '1') {
      $env:AGENTNOTCH_SELF_TEST_SCALE = $scale
    }
    $code = Invoke-Sealed $what
    Copy-RunLog (Join-Path $Out "selftest-$scale-run.log")
    Test-Report $what $report $code $scale
    Assert-NoAgentNotch "after $what"
  }

  Write-Host '== snapshots at scale 1'
  Remove-SealedData
  $shots = Join-Path $Out 'snapshots'
  Remove-Item -LiteralPath $shots -Recurse -Force -ErrorAction SilentlyContinue
  Clear-Switches
  $env:AGENTNOTCH_SAFE_MODE = '1'
  $env:AGENTNOTCH_SNAPSHOT_CLAUDE = $shots
  $code = Invoke-Sealed 'snapshots'
  Copy-RunLog (Join-Path $Out 'snapshots-run.log')
  if ($null -ne $code -and $code -ne 0) { Add-Failure "snapshots: exit code $code" }
  $manifestPath = Join-Path $shots 'manifest.json'
  if (-not (Test-Path -LiteralPath $manifestPath)) {
    Add-Failure "snapshots: no manifest at $manifestPath"
  } else {
    try {
      $manifest = @(Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json)
    } catch {
      $manifest = @()
      Add-Failure "snapshots: the manifest is not JSON ($($_.Exception.Message))"
    }
    foreach ($entry in $manifest) {
      $file = Join-Path $shots ([string](Get-Prop $entry 'file'))
      if (-not (Get-Prop $entry 'file') -or -not (Test-Path -LiteralPath $file)) {
        Add-Failure "snapshots: $(Get-Prop $entry 'name'): the listed file '$(Get-Prop $entry 'file')' does not exist"
      } elseif ((Get-Item -LiteralPath $file).Length -le 0 -or [int64](Get-Prop $entry 'bytes') -le 0) {
        Add-Failure "snapshots: $(Get-Prop $entry 'name'): $(Get-Prop $entry 'file') is empty"
      }
    }
    $listed = @($manifest | ForEach-Object { Get-Prop $_ 'name' })
    foreach ($name in $SnapshotNames) {
      if ($listed -notcontains $name) { Add-Failure "snapshots: $name is not in the manifest" }
    }
    Write-Host "snapshots: $($manifest.Count) listed"
  }
  Assert-NoAgentNotch 'after the snapshot run'

  # The sealed runs keep to their own folder.
  if (-not $realDataBefore -and (Test-Path -LiteralPath $realData)) {
    Add-Failure "the sealed runs created $realData"
  }
} catch {
  Add-Failure "the script stopped: $($_.Exception.Message)"
} finally {
  Clear-Switches
  try { Remove-SealedData } catch { Add-Failure $_.Exception.Message }
}

if ($failures.Count -gt 0) {
  Write-Host ''
  Write-Host "$($failures.Count) failures:"
  foreach ($line in $failures) { Write-Host "  $line" }
  exit 1
}
Write-Host 'The sealed self-test and the snapshots passed.'
exit 0
