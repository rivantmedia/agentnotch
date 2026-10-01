#Requires -Version 7.2
# The parts of windows/scripts/agentnotch-build.ps1 that decide what a build is: the config
# overlay (DESIGN-WIN §6.5), the bundler's signing command, which signing the WINDOWS_SIGN_*
# variables ask for (Q4), and the VERSION check. Dot-sourcing the script defines its
# functions and runs nothing; the ones that only make sense on Windows are not called here.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$script = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'scripts/agentnotch-build.ps1'
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
function Assert-Throws([scriptblock]$Body, [string]$Pattern) {
    try { & $Body } catch {
        if ($_.Exception.Message -notmatch $Pattern) {
            throw "threw <$($_.Exception.Message)>, expected a message matching <$Pattern>"
        }
        return
    }
    throw "did not throw (expected <$Pattern>)"
}
function ConvertTo-CompactJson($Value) { $Value | ConvertTo-Json -Depth 10 -Compress }
function Get-Plan([hashtable]$Variables) { Get-SigningPlan -Variables $Variables }

$feed = 'https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json'
$pfx = @{ WINDOWS_SIGN_PFX_BASE64 = 'cGZ4'; WINDOWS_SIGN_PFX_PASSWORD = 'secret' }
$azure = [ordered]@{
    WINDOWS_SIGN_AZURE_ENDPOINT      = 'https://eus.codesigning.azure.net'
    WINDOWS_SIGN_AZURE_ACCOUNT       = 'account'
    WINDOWS_SIGN_AZURE_PROFILE       = 'profile'
    WINDOWS_SIGN_AZURE_TENANT_ID     = 'tenant'
    WINDOWS_SIGN_AZURE_CLIENT_ID     = 'client'
    WINDOWS_SIGN_AZURE_CLIENT_SECRET = 'client-secret'
}
function Get-AzureVariables { $copy = @{}; foreach ($k in $azure.Keys) { $copy[$k] = $azure[$k] }; $copy }

# --- the overlay -------------------------------------------------------------------------

Test-Case 'a build without a key overlays the version alone' {
    Assert-Equal (ConvertTo-CompactJson (New-BuildOverlay -Version '1.2.3')) '{"version":"1.2.3"}' 'overlay'
    Assert-Equal (ConvertTo-CompactJson (New-BuildOverlay -Version '1.2.3' -UpdaterPubkey '')) '{"version":"1.2.3"}' 'overlay'
}

Test-Case 'a release build overlays the key, the feed, signed versions and passive installs, nothing else' {
    $expected = '{"version":"1.2.3","plugins":{"updater":{"pubkey":"dW50cnVzdGVk","endpoints":["' + $feed +
        '"],"requireSignedVersion":true,"windows":{"installMode":"passive"}}}}'
    Assert-Equal (ConvertTo-CompactJson (New-BuildOverlay -Version '1.2.3' -UpdaterPubkey 'dW50cnVzdGVk')) $expected 'overlay'
}

Test-Case 'the overlay file is plain JSON without a byte order mark' {
    $file = Join-Path ([IO.Path]::GetTempPath()) "agentnotch-overlay-$PID.json"
    try {
        Write-Json -Path $file -Value (New-BuildOverlay -Version '1.2.3' -UpdaterPubkey 'k')
        $bytes = [IO.File]::ReadAllBytes($file)
        Assert-Equal $bytes[0] ([byte][char]'{') 'first byte'
        $parsed = Get-Content $file -Raw | ConvertFrom-Json
        Assert-Equal $parsed.plugins.updater.endpoints.Count 1 'endpoint count'
        Assert-Equal $parsed.plugins.updater.endpoints[0] $feed 'endpoint'
    } finally {
        Remove-Item $file -Force -ErrorAction SilentlyContinue
    }
}

# --- the bundler's signing command -------------------------------------------------------

Test-Case 'the sign overlay is signCommand in object form, ending with -SignFile %1' {
    $path = 'C:\a folder\agentnotch-build.ps1'
    $overlay = New-SignOverlay -ScriptPath $path
    Assert-Equal (@($overlay.Keys) -join ',') 'bundle' 'top-level keys'
    Assert-Equal (@($overlay.bundle.Keys) -join ',') 'windows' 'bundle keys'
    Assert-Equal (@($overlay.bundle.windows.Keys) -join ',') 'signCommand' 'bundle.windows keys'
    $command = (ConvertTo-CompactJson $overlay | ConvertFrom-Json).bundle.windows.signCommand
    Assert-Equal $command.cmd 'pwsh' 'cmd'
    $arguments = @($command.args)
    Assert-Equal $arguments[-2] '-SignFile' 'second-to-last argument'
    Assert-Equal $arguments[-1] '%1' 'last argument'
    $file = [array]::IndexOf($arguments, '-File')
    if ($file -lt 0) { throw 'no -File argument' }
    Assert-Equal $arguments[$file + 1] $path 'the script after -File (one argument, spaces and all)'
}

# --- which signing the variables ask for -------------------------------------------------

Test-Case 'no signing variables: no signing' {
    Assert-Equal (Get-Plan @{}).Method 'none' 'method'
    Assert-Equal (Get-Plan @{ WINDOWS_SIGN_PFX_BASE64 = ''; WINDOWS_SIGN_AZURE_ACCOUNT = '' }).Method 'none' 'method with empty values'
}

Test-Case 'a certificate pair: certificate signing with the default timestamp server' {
    $plan = Get-Plan $pfx
    Assert-Equal $plan.Method 'certificate' 'method'
    Assert-Equal $plan.PfxBase64 'cGZ4' 'pfx'
    Assert-Equal $plan.PfxPassword 'secret' 'password'
    Assert-Equal $plan.Timestamp 'http://timestamp.digicert.com' 'timestamp'
}

Test-Case 'a certificate without a password is fine (a .pfx may have an empty one)' {
    $plan = Get-Plan @{ WINDOWS_SIGN_PFX_BASE64 = 'cGZ4' }
    Assert-Equal $plan.Method 'certificate' 'method'
    Assert-Equal $plan.PfxPassword '' 'password'
}

Test-Case 'a certificate with a timestamp server, or "none"' {
    Assert-Equal (Get-Plan ($pfx + @{ WINDOWS_SIGN_TIMESTAMP_URL = 'http://ts.example.com/rfc3161' })).Timestamp 'http://ts.example.com/rfc3161' 'timestamp'
    Assert-Equal (Get-Plan ($pfx + @{ WINDOWS_SIGN_TIMESTAMP_URL = 'none' })).Timestamp 'none' 'timestamp'
}

Test-Case 'a timestamp that is not a web address is refused' {
    Assert-Throws { Get-Plan ($pfx + @{ WINDOWS_SIGN_TIMESTAMP_URL = 'timestamp.digicert.com' }) } 'WINDOWS_SIGN_TIMESTAMP_URL must be a web address'
    Assert-Throws { Get-Plan ($pfx + @{ WINDOWS_SIGN_TIMESTAMP_URL = 'ftp://ts.example.com' }) } 'WINDOWS_SIGN_TIMESTAMP_URL must be a web address'
}

Test-Case 'all six Azure values: Azure signing' {
    $plan = Get-Plan (Get-AzureVariables)
    Assert-Equal $plan.Method 'azure' 'method'
    Assert-Equal $plan.Endpoint 'https://eus.codesigning.azure.net' 'endpoint'
    Assert-Equal $plan.Account 'account' 'account'
    Assert-Equal $plan.Profile 'profile' 'profile'
    Assert-Equal $plan.TenantId 'tenant' 'tenant'
    Assert-Equal $plan.ClientId 'client' 'client'
    Assert-Equal $plan.ClientSecret 'client-secret' 'client secret'
}

Test-Case 'five of the six Azure values: refused, naming the missing one' {
    foreach ($name in $azure.Keys) {
        $variables = Get-AzureVariables
        $variables.Remove($name)
        Assert-Throws { Get-Plan $variables } "missing: $name$"
    }
}

Test-Case 'an Azure endpoint that is not an https address is refused' {
    foreach ($endpoint in 'http://eus.codesigning.azure.net', 'eus.codesigning.azure.net', 'https://eus.codesigning.azure.net/path') {
        $variables = Get-AzureVariables
        $variables.WINDOWS_SIGN_AZURE_ENDPOINT = $endpoint
        Assert-Throws { Get-Plan $variables } 'WINDOWS_SIGN_AZURE_ENDPOINT must be'
    }
}

Test-Case 'a certificate and Azure together are refused' {
    Assert-Throws { Get-Plan ((Get-AzureVariables) + $pfx) } 'Both a certificate'
    Assert-Throws { Get-Plan ((Get-AzureVariables) + @{ WINDOWS_SIGN_PFX_PASSWORD = 'secret' }) } 'Both a certificate'
}

Test-Case 'a password without a certificate is refused' {
    Assert-Throws { Get-Plan @{ WINDOWS_SIGN_PFX_PASSWORD = 'secret' } } 'WINDOWS_SIGN_PFX_PASSWORD is set without WINDOWS_SIGN_PFX_BASE64'
}

Test-Case 'a timestamp server without a certificate is refused' {
    Assert-Throws { Get-Plan @{ WINDOWS_SIGN_TIMESTAMP_URL = 'http://timestamp.digicert.com' } } 'WINDOWS_SIGN_TIMESTAMP_URL is set without a certificate'
}

# --- the version ---------------------------------------------------------------------------

function New-Checkout([string]$Version, [string]$ConfVersion) {
    $root = Join-Path ([IO.Path]::GetTempPath()) "agentnotch-version-$PID-$([Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Force (Join-Path $root 'windows/codenotch') | Out-Null
    Set-Content -Path (Join-Path $root 'VERSION') -Value $Version
    Set-Content -Path (Join-Path $root 'windows/codenotch/tauri.conf.json') -Value (
        '{ "productName": "Agent Notch", "version": "' + $ConfVersion + '", "identifier": "x" }')
    $root
}

Test-Case 'VERSION equal to tauri.conf.json: that version' {
    $root = New-Checkout '1.4.2' '1.4.2'
    try { Assert-Equal (Get-AppVersion -Root $root) '1.4.2' 'version' } finally { Remove-Item $root -Recurse -Force }
}

Test-Case 'VERSION different from tauri.conf.json: refused' {
    $root = New-Checkout '1.4.2' '1.4.1'
    try { Assert-Throws { Get-AppVersion -Root $root } 'tauri.conf.json says version 1.4.1, VERSION says 1.4.2' } finally { Remove-Item $root -Recurse -Force }
}

Test-Case 'a VERSION that is not major.minor.patch: refused' {
    $root = New-Checkout '1.4' '1.4'
    try { Assert-Throws { Get-AppVersion -Root $root } 'VERSION must be one line' } finally { Remove-Item $root -Recurse -Force }
}

Test-Case 'this checkout: VERSION and tauri.conf.json agree' {
    $version = (Get-Content (Join-Path $RepoDir 'VERSION') -Raw).Trim()
    Assert-Equal (Get-AppVersion) $version 'version'
}

if ($failures.Count) { throw "$($failures.Count) case(s) failed: $($failures -join '; ')" }
'build-script: all cases passed'
