#Requires -Version 7.2
# Parse-checks every PowerShell file of the Windows build (windows/scripts, windows/tools)
# with PowerShell's own parser, so a syntax error shows up on any machine with pwsh, long
# before a Windows runner gets to the script. Prints each error; exits 1 when there is one.
$ErrorActionPreference = 'Stop'
$windowsDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$files = @('scripts', 'tools') | ForEach-Object { Join-Path $windowsDir $_ } | Where-Object { Test-Path $_ } |
    ForEach-Object { Get-ChildItem -Path $_ -Recurse -File -Include '*.ps1', '*.psm1', '*.psd1' } |
    Where-Object { $_.FullName -notmatch '[\\/]node_modules[\\/]' } |
    Sort-Object FullName
if (-not $files) { 'no PowerShell files found'; exit 1 }
$bad = 0
foreach ($file in $files) {
    $tokens = $null; $errors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$tokens, [ref]$errors)
    $shown = [IO.Path]::GetRelativePath((Split-Path -Parent $windowsDir), $file.FullName)
    foreach ($e in $errors) {
        '{0}:{1}:{2}: {3}' -f $shown, $e.Extent.StartLineNumber, $e.Extent.StartColumnNumber, $e.Message
        $bad++
    }
    if (-not $errors) { "${shown}: parses ($($tokens.Count) tokens)" }
}
if ($bad) { "$bad parse error(s)" }
exit [int]($bad -gt 0)
