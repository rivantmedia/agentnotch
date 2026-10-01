#Requires -Version 7.2
# Runs every windows/tools/tests/*.tests.ps1, each in a pwsh of its own (a test file sets
# strict mode and dot-sources scripts; nothing of it may leak into the next). A test file is
# a plain script of asserts that throws on the first failure; this exits 1 if any did.
$ErrorActionPreference = 'Stop'
$pwsh = Join-Path $PSHOME ($IsWindows ? 'pwsh.exe' : 'pwsh')
$tests = @(Get-ChildItem -Path $PSScriptRoot -Filter '*.tests.ps1' -File | Sort-Object Name)
if (-not $tests.Count) { 'no *.tests.ps1 found'; exit 1 }
$failed = @()
foreach ($test in $tests) {
    "== $($test.Name)"
    & $pwsh -NoProfile -NonInteractive -File $test.FullName
    if ($LASTEXITCODE -ne 0) { $failed += $test.Name }
}
if ($failed.Count) { "FAILED: $($failed -join ', ')"; exit 1 }
"all $($tests.Count) test file(s) passed"
