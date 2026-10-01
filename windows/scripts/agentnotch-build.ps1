#Requires -Version 7.2
<#
.SYNOPSIS
    Builds Agent Notch for Windows: the hook, the app and its NSIS installer.

.DESCRIPTION
    What agentnotch-windows.yml runs, and what a Windows PC runs to build from source
    (DESIGN-WIN §6.1 step 12, §6.5). It needs Rust, Node 22 and, for the installer, the
    network (the bundler fetches NSIS once).

        windows\scripts\agentnotch-build.ps1 -Out out
        windows\scripts\agentnotch-build.ps1 -Out out -UpdaterPubkey <key>   # release.yml only
        windows\scripts\agentnotch-build.ps1 -CheckSigning                   # after a build

    Writes into -Out:
        AgentNotch-<VERSION>-Setup.exe   the installer, per user, no administrator
        windows-release-info.env         what the release notes need to know: the file's name,
                                         whether it is code-signed, whether it updates itself

    A build without -UpdaterPubkey carries no update key and no feed: a copy built from
    source never updates itself, and the doctor says so. Only release.yml passes the key,
    which it derives from the update seed in a job of its own; this script never sees the
    seed.

    Code signing (Authenticode) is off until its secrets exist. With them in the
    environment the bundler signs the app, the hook, the uninstaller, the installer's
    plugins and the installer itself through bundle.windows.signCommand, which is this
    script again (-SignFile). One of:

        WINDOWS_SIGN_PFX_BASE64, WINDOWS_SIGN_PFX_PASSWORD
            a code signing certificate with its private key, as a .pfx
        WINDOWS_SIGN_AZURE_ENDPOINT, WINDOWS_SIGN_AZURE_ACCOUNT, WINDOWS_SIGN_AZURE_PROFILE,
        WINDOWS_SIGN_AZURE_TENANT_ID, WINDOWS_SIGN_AZURE_CLIENT_ID, WINDOWS_SIGN_AZURE_CLIENT_SECRET
            Azure Artifact Signing (formerly Trusted Signing): all six
        WINDOWS_SIGN_TIMESTAMP_URL
            optional, for a certificate: the RFC 3161 timestamp server
            (default http://timestamp.digicert.com; "none" signs without a timestamp)

    Without any of them the installer is unsigned and windows-release-info.env says
    AUTHENTICODE=unsigned. Half a set is an error, never a silently unsigned release.
    The signing variables are taken out of the environment before anything is compiled
    (every dependency's build script runs then) and put back only for the bundling
    step, which compiles nothing.

    -CheckSigning proves that path on a machine with no certificate: it makes a throwaway
    self-signed one, bundles the app that was just built with it, checks every signature
    the bundler was meant to make, and puts everything back.
#>
[CmdletBinding(DefaultParameterSetName = 'Build')]
param(
    # Where the installer goes; relative to the current folder.
    [Parameter(ParameterSetName = 'Build')]
    [string]$Out = 'out',

    # The Tauri updater's public key. Release builds only: it turns updates on.
    [Parameter(ParameterSetName = 'Build')]
    [string]$UpdaterPubkey = '',

    # The bundler's signCommand: signs one file with what the build prepared.
    [Parameter(ParameterSetName = 'Sign', Mandatory)]
    [string]$SignFile,

    # Bundles the built app once more with a throwaway certificate and checks the signatures.
    [Parameter(ParameterSetName = 'CheckSigning', Mandatory)]
    [switch]$CheckSigning
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$WindowsDir = Split-Path -Parent $PSScriptRoot
$RepoDir = Split-Path -Parent $WindowsDir
$AppDir = Join-Path $WindowsDir 'codenotch'
$ToolsDir = Join-Path $WindowsDir 'tools'
$HookExe = Join-Path $WindowsDir 'target\hook\release\agentnotch-hook.exe'
$BundleDir = Join-Path $WindowsDir 'target\release\bundle\nsis'

# The one feed installed copies read; release.yml publishes latest.json there.
$FeedUrl = 'https://github.com/rivantmedia/agentnotch/releases/latest/download/latest.json'

# Azure's signing plug-in for signtool, pinned by hash: it runs with the signing credentials.
$AzureClientVersion = '1.0.128'
$AzureClientUrl = "https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/$AzureClientVersion/microsoft.artifactsigning.client.$AzureClientVersion.nupkg"
$AzureClientSha512 = '98f06a691f4fc2fa22f19dcf8556733e98607fbef91a312c453b9b0798cc9088dae0acb36e389b552a11b4d2320324785b8541c2b51091a724c05bc5df5cbf95'
$AzureTimestampUrl = 'http://timestamp.acs.microsoft.com'
$DefaultTimestampUrl = 'http://timestamp.digicert.com'

$SigningVariables = @(
    'WINDOWS_SIGN_PFX_BASE64', 'WINDOWS_SIGN_PFX_PASSWORD', 'WINDOWS_SIGN_TIMESTAMP_URL',
    'WINDOWS_SIGN_AZURE_ENDPOINT', 'WINDOWS_SIGN_AZURE_ACCOUNT', 'WINDOWS_SIGN_AZURE_PROFILE',
    'WINDOWS_SIGN_AZURE_TENANT_ID', 'WINDOWS_SIGN_AZURE_CLIENT_ID', 'WINDOWS_SIGN_AZURE_CLIENT_SECRET'
)
# What a -SignFile run reads. Set only around the bundling step.
$SignStateVariables = @(
    'AGENTNOTCH_SIGN_METHOD', 'AGENTNOTCH_SIGN_TOOL', 'AGENTNOTCH_SIGN_THUMBPRINT',
    'AGENTNOTCH_SIGN_TIMESTAMP', 'AGENTNOTCH_SIGN_DLIB', 'AGENTNOTCH_SIGN_METADATA',
    'AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_CLIENT_SECRET'
)

function Get-ScratchDir {
    # The runner's own temporary folder on CI (emptied after the job), else the user's.
    $base = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $dir = Join-Path $base 'agentnotch-build'
    New-Item -ItemType Directory -Force $dir | Out-Null
    $dir
}

function Invoke-Tool {
    # A native command whose failure stops the build. Its output goes to the console, never
    # into the caller's result (a function's output is everything it writes).
    param(
        [Parameter(Mandatory)][string]$File,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory,
        [switch]$Quiet
    )
    if ($WorkingDirectory) { Push-Location $WorkingDirectory }
    try {
        if ($Quiet) { & $File @Arguments | Out-Null } else { & $File @Arguments | Out-Host }
        if ($LASTEXITCODE -ne 0) {
            throw "$([IO.Path]::GetFileName($File)) $($Arguments | Select-Object -First 1) failed with exit code $LASTEXITCODE"
        }
    } finally {
        if ($WorkingDirectory) { Pop-Location }
    }
}

function Get-AppVersion {
    # VERSION is the app's only version source; tauri.conf.json must say the same
    # (check-seams.sh holds the two together, and so does this). -Root is another checkout
    # laid out like this one (the tests use a scratch folder).
    param([string]$Root = $RepoDir)
    $version = (Get-Content (Join-Path $Root 'VERSION') -Raw).Trim()
    if ($version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
        throw "VERSION must be one line of major.minor.patch numbers, not '$version'"
    }
    $conf = Get-Content (Join-Path $Root 'windows/codenotch/tauri.conf.json') -Raw | ConvertFrom-Json
    if ($conf.version -ne $version) {
        throw "tauri.conf.json says version $($conf.version), VERSION says $version (Scripts/bump-version.sh changes both)"
    }
    $version
}

function New-BuildOverlay {
    # The config laid over tauri.conf.json for this build (DESIGN-WIN §6.5): always the
    # version; in a release build the update key, the feed, and the rule that a signature
    # names the version it was made for.
    param([Parameter(Mandatory)][string]$Version, [string]$UpdaterPubkey = '')
    $overlay = [ordered]@{ version = $Version }
    if ($UpdaterPubkey) {
        $overlay.plugins = [ordered]@{
            updater = [ordered]@{
                pubkey               = $UpdaterPubkey
                endpoints            = @($FeedUrl)
                requireSignedVersion = $true
                windows              = [ordered]@{ installMode = 'passive' }
            }
        }
    }
    $overlay
}

function New-SignOverlay {
    # bundle.windows.signCommand in its object form, so no path is ever split on a space.
    # The bundler replaces "%1" with each file it signs.
    param([Parameter(Mandatory)][string]$ScriptPath)
    [ordered]@{
        bundle = [ordered]@{
            windows = [ordered]@{
                signCommand = [ordered]@{
                    cmd  = 'pwsh'
                    args = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
                        '-File', $ScriptPath, '-SignFile', '%1')
                }
            }
        }
    }
}

function Write-Json {
    param([Parameter(Mandatory)]$Value, [Parameter(Mandatory)][string]$Path)
    # No byte order mark: the Tauri CLI reads the file as plain JSON.
    $Value | ConvertTo-Json -Depth 8 | Set-Content -Path $Path -Encoding utf8NoBOM
}

function Get-SigningPlan {
    # Which way of signing the given variables ask for: 'none', 'certificate' or 'azure'.
    # Half a set, or both sets, is refused: a release must never be unsigned by accident.
    param([Parameter(Mandatory)][hashtable]$Variables)
    $value = { param($name) if ($Variables.ContainsKey($name)) { [string]$Variables[$name] } else { '' } }
    $certificate = @('WINDOWS_SIGN_PFX_BASE64', 'WINDOWS_SIGN_PFX_PASSWORD')
    $azure = @('WINDOWS_SIGN_AZURE_ENDPOINT', 'WINDOWS_SIGN_AZURE_ACCOUNT', 'WINDOWS_SIGN_AZURE_PROFILE',
        'WINDOWS_SIGN_AZURE_TENANT_ID', 'WINDOWS_SIGN_AZURE_CLIENT_ID', 'WINDOWS_SIGN_AZURE_CLIENT_SECRET')
    $certificateSet = @($certificate | Where-Object { & $value $_ })
    $azureSet = @($azure | Where-Object { & $value $_ })
    if ($certificateSet.Count -and $azureSet.Count) {
        throw 'Both a certificate (WINDOWS_SIGN_PFX_*) and Azure signing (WINDOWS_SIGN_AZURE_*) are set; keep one.'
    }
    if ($azureSet.Count) {
        $missing = @($azure | Where-Object { $_ -notin $azureSet })
        if ($missing.Count) { throw "Azure signing needs all six WINDOWS_SIGN_AZURE_* values; missing: $($missing -join ', ')" }
        if ((& $value 'WINDOWS_SIGN_AZURE_ENDPOINT') -notmatch '^https://[a-z0-9.-]+/?$') {
            throw 'WINDOWS_SIGN_AZURE_ENDPOINT must be the account''s https address, like https://eus.codesigning.azure.net'
        }
        return @{
            Method       = 'azure'
            Endpoint     = & $value 'WINDOWS_SIGN_AZURE_ENDPOINT'
            Account      = & $value 'WINDOWS_SIGN_AZURE_ACCOUNT'
            Profile      = & $value 'WINDOWS_SIGN_AZURE_PROFILE'
            TenantId     = & $value 'WINDOWS_SIGN_AZURE_TENANT_ID'
            ClientId     = & $value 'WINDOWS_SIGN_AZURE_CLIENT_ID'
            ClientSecret = & $value 'WINDOWS_SIGN_AZURE_CLIENT_SECRET'
        }
    }
    if ($certificateSet.Count) {
        # A .pfx may have an empty password, so only the file itself is required.
        if (-not (& $value 'WINDOWS_SIGN_PFX_BASE64')) {
            throw 'WINDOWS_SIGN_PFX_PASSWORD is set without WINDOWS_SIGN_PFX_BASE64.'
        }
        $timestamp = & $value 'WINDOWS_SIGN_TIMESTAMP_URL'
        if (-not $timestamp) { $timestamp = $DefaultTimestampUrl }
        if ($timestamp -ne 'none' -and $timestamp -notmatch '^https?://\S+$') {
            throw "WINDOWS_SIGN_TIMESTAMP_URL must be a web address or 'none', not '$timestamp'"
        }
        return @{
            Method      = 'certificate'
            PfxBase64   = & $value 'WINDOWS_SIGN_PFX_BASE64'
            PfxPassword = & $value 'WINDOWS_SIGN_PFX_PASSWORD'
            Timestamp   = $timestamp
        }
    }
    if (& $value 'WINDOWS_SIGN_TIMESTAMP_URL') {
        throw 'WINDOWS_SIGN_TIMESTAMP_URL is set without a certificate (WINDOWS_SIGN_PFX_BASE64).'
    }
    @{ Method = 'none' }
}

function Read-SigningVariables {
    # The signing variables, taken out of this process's environment: nothing the build
    # starts inherits them. Initialize-Signing hands the bundling step what it needs.
    $variables = @{}
    foreach ($name in $SigningVariables) {
        $current = [Environment]::GetEnvironmentVariable($name)
        if ($current) { $variables[$name] = $current }
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    $variables
}

function Find-SignTool {
    # The newest x64 signtool of the installed Windows SDKs.
    $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $found = @(Get-ChildItem -Path $kits -Filter signtool.exe -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -eq 'x64' } |
            Sort-Object { try { [version]$_.Directory.Parent.Name } catch { [version]'0.0' } } -Descending)
    if (-not $found.Count) { throw "signtool.exe was not found under $kits; install the Windows SDK." }
    $found[0].FullName
}

function Import-SigningCertificate {
    # Puts the certificate and its key in the user's store and answers its thumbprint, so
    # signtool can name it by thumbprint and the password never reaches a command line.
    param([Parameter(Mandatory)][string]$PfxBase64, [string]$PfxPassword = '')
    try {
        $bytes = [Convert]::FromBase64String(($PfxBase64 -replace '\s', ''))
    } catch {
        throw 'WINDOWS_SIGN_PFX_BASE64 is not base64.'
    }
    $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]'UserKeySet, PersistKeySet'
    try {
        $certificate = [Security.Cryptography.X509Certificates.X509Certificate2]::new($bytes, $PfxPassword, $flags)
    } catch {
        throw "The signing certificate could not be opened (a wrong WINDOWS_SIGN_PFX_PASSWORD?): $($_.Exception.Message)"
    }
    if (-not $certificate.HasPrivateKey) { throw 'The signing certificate has no private key.' }
    $store = [Security.Cryptography.X509Certificates.X509Store]::new('My', 'CurrentUser')
    $store.Open('ReadWrite')
    try { $store.Add($certificate) } finally { $store.Close() }
    $certificate
}

function Remove-SigningCertificate {
    # Out of the store again, key included where Windows lets it go. Best effort: on CI the
    # machine is thrown away; on a PC a leftover would be a usable signing key.
    param([Parameter(Mandatory)][string]$Thumbprint)
    $store = [Security.Cryptography.X509Certificates.X509Store]::new('My', 'CurrentUser')
    $store.Open('ReadWrite')
    try {
        foreach ($certificate in @($store.Certificates | Where-Object Thumbprint -EQ $Thumbprint)) {
            try {
                $key = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($certificate)
                if ($key -is [Security.Cryptography.RSACng]) { $key.Key.Delete() }
            } catch {
                Write-Verbose "the signing key stays in its container: $($_.Exception.Message)"
            }
            $store.Remove($certificate)
        }
    } finally {
        $store.Close()
    }
}

function Install-AzureSigningClient {
    # Microsoft's signtool plug-in for Artifact Signing, from NuGet, checked against the
    # pinned hash before anything of it runs.
    $dir = Join-Path (Get-ScratchDir) "artifactsigning-$AzureClientVersion"
    $dlib = Join-Path $dir 'bin\x64\Azure.CodeSigning.Dlib.dll'
    if (Test-Path $dlib) { return $dlib }
    $package = Join-Path (Get-ScratchDir) "artifactsigning-$AzureClientVersion.zip"
    Invoke-WebRequest -Uri $AzureClientUrl -OutFile $package
    $hash = (Get-FileHash -Path $package -Algorithm SHA512).Hash.ToLowerInvariant()
    if ($hash -ne $AzureClientSha512) {
        Remove-Item $package -Force
        throw "Microsoft.ArtifactSigning.Client $AzureClientVersion has SHA-512 $hash, not the pinned one."
    }
    Expand-Archive -Path $package -DestinationPath $dir -Force
    Remove-Item $package -Force
    if (-not (Test-Path $dlib)) { throw "Microsoft.ArtifactSigning.Client $AzureClientVersion holds no bin\x64\Azure.CodeSigning.Dlib.dll" }
    $dlib
}

function Initialize-Signing {
    # Makes a signing plan usable and answers what a -SignFile run must find in its
    # environment, plus what to undo afterwards.
    param([Parameter(Mandatory)][hashtable]$Plan)
    $state = @{ Method = $Plan.Method; Environment = [ordered]@{}; Thumbprint = $null; Signer = ''; Files = @() }
    if ($Plan.Method -eq 'none') { return $state }
    $state.Environment.AGENTNOTCH_SIGN_METHOD = $Plan.Method
    $state.Environment.AGENTNOTCH_SIGN_TOOL = Find-SignTool
    switch ($Plan.Method) {
        'certificate' {
            $certificate = Import-SigningCertificate -PfxBase64 $Plan.PfxBase64 -PfxPassword $Plan.PfxPassword
            $state.Thumbprint = $certificate.Thumbprint
            $state.Signer = $certificate.Subject
            $state.Environment.AGENTNOTCH_SIGN_THUMBPRINT = $certificate.Thumbprint
            $state.Environment.AGENTNOTCH_SIGN_TIMESTAMP = $Plan.Timestamp
        }
        'azure' {
            $metadata = Join-Path (Get-ScratchDir) 'artifactsigning-metadata.json'
            # Only the credentials given here are ever tried: never a managed identity, a
            # cached login or a browser.
            Write-Json -Path $metadata -Value ([ordered]@{
                    Endpoint               = $Plan.Endpoint
                    CodeSigningAccountName = $Plan.Account
                    CertificateProfileName = $Plan.Profile
                    ExcludeCredentials     = @('ManagedIdentityCredential', 'WorkloadIdentityCredential',
                        'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
                        'AzureCliCredential', 'AzurePowerShellCredential', 'AzureDeveloperCliCredential',
                        'InteractiveBrowserCredential')
                })
            $state.Files += $metadata
            $state.Signer = "Azure Artifact Signing ($($Plan.Account)/$($Plan.Profile))"
            $state.Environment.AGENTNOTCH_SIGN_DLIB = Install-AzureSigningClient
            $state.Environment.AGENTNOTCH_SIGN_METADATA = $metadata
            $state.Environment.AZURE_TENANT_ID = $Plan.TenantId
            $state.Environment.AZURE_CLIENT_ID = $Plan.ClientId
            $state.Environment.AZURE_CLIENT_SECRET = $Plan.ClientSecret
        }
    }
    $state
}

function Clear-Signing {
    param([Parameter(Mandatory)][hashtable]$State)
    foreach ($name in $SignStateVariables) { [Environment]::SetEnvironmentVariable($name, $null) }
    if ($State.Thumbprint) { Remove-SigningCertificate -Thumbprint $State.Thumbprint }
    foreach ($file in $State.Files) { Remove-Item $file -Force -ErrorAction SilentlyContinue }
}

function Invoke-SignFile {
    # The bundler's signCommand. Everything it needs was prepared by the build
    # (Initialize-Signing); a run without it has nothing to sign with and says so.
    param([Parameter(Mandatory)][string]$Path)
    $method = $env:AGENTNOTCH_SIGN_METHOD
    $tool = $env:AGENTNOTCH_SIGN_TOOL
    if (-not $method -or -not $tool) { throw '-SignFile is the bundler''s signing command; it only works inside a build.' }
    if (-not (Test-Path -LiteralPath $Path)) { throw "There is no file to sign at $Path" }
    $arguments = switch ($method) {
        'certificate' {
            $list = @('sign', '/fd', 'SHA256', '/sha1', $env:AGENTNOTCH_SIGN_THUMBPRINT, '/d', 'Agent Notch')
            if ($env:AGENTNOTCH_SIGN_TIMESTAMP -and $env:AGENTNOTCH_SIGN_TIMESTAMP -ne 'none') {
                $list += @('/tr', $env:AGENTNOTCH_SIGN_TIMESTAMP, '/td', 'SHA256')
            }
            $list
        }
        'azure' {
            @('sign', '/v', '/fd', 'SHA256', '/tr', $AzureTimestampUrl, '/td', 'SHA256', '/d', 'Agent Notch',
                '/dlib', $env:AGENTNOTCH_SIGN_DLIB, '/dmdf', $env:AGENTNOTCH_SIGN_METADATA)
        }
        default { throw "Unknown signing method '$method'" }
    }
    # Timestamp and signing services drop a request now and then; an installer is not
    # worth losing to one.
    $attempts = 4
    for ($attempt = 1; $attempt -le $attempts; $attempt++) {
        & $tool @arguments $Path
        if ($LASTEXITCODE -eq 0) { return }
        if ($attempt -lt $attempts) {
            Write-Host "signtool failed (exit $LASTEXITCODE); trying again ($attempt of $($attempts - 1))"
            Start-Sleep -Seconds (5 * $attempt)
        }
    }
    throw "signtool could not sign $Path (exit $LASTEXITCODE)"
}

function Test-Signature {
    # Whether a file carries the signature the build meant to make. A certificate is matched
    # by thumbprint (a self-signed one is not "valid" to Windows, and need not be); Azure's
    # changes every few days, so there Windows has to call the signature valid.
    param([Parameter(Mandatory)][string]$Path, [Parameter(Mandatory)][hashtable]$State)
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if (-not $signature.SignerCertificate) { return "$Path is not signed" }
    if ($State.Thumbprint) {
        if ($signature.SignerCertificate.Thumbprint -ne $State.Thumbprint) {
            return "$Path is signed by $($signature.SignerCertificate.Thumbprint), not the build's certificate"
        }
        # "UnknownError" is Windows's word for a chain it doesn't trust, which a certificate
        # of one's own always is; a signature over other bytes is a different matter.
        if ("$($signature.Status)" -in 'HashMismatch', 'NotSigned', 'NotSupportedFileFormat', 'Incompatible') {
            return "$Path has a signature Windows calls $($signature.Status)"
        }
    } elseif ($signature.Status -ne 'Valid') {
        return "$Path has a signature Windows calls $($signature.Status)"
    }
    $null
}

function Get-TauriCli {
    # The pinned Tauri CLI (windows/tools, npm ci: the lockfile's hashes decide what runs),
    # started through node so PowerShell never rewrites its "--".
    Invoke-Tool -File 'npm' -Arguments @('ci', '--no-audit', '--no-fund') -WorkingDirectory $ToolsDir
    $cli = Join-Path $ToolsDir 'node_modules\@tauri-apps\cli\tauri.js'
    if (-not (Test-Path $cli)) { throw "npm ci left no Tauri CLI at $cli" }
    $cli
}

function Invoke-Bundle {
    # Makes the installer from what is already built, and answers its path. With a signing
    # state whose method isn't 'none', the bundler signs as it goes.
    param(
        [Parameter(Mandatory)][string]$TauriCli,
        [Parameter(Mandatory)][string]$OverlayPath,
        [Parameter(Mandatory)][hashtable]$Signing
    )
    # One installer, this build's: a cached target folder may still hold an older version's.
    Remove-Item (Join-Path $BundleDir '*-setup.exe') -Force -ErrorAction SilentlyContinue
    $arguments = @($TauriCli, 'bundle', '--bundles', 'nsis', '--config', 'tauri.bundle.conf.json', '--config', $OverlayPath)
    if ($Signing.Method -ne 'none') {
        $signOverlay = Join-Path (Get-ScratchDir) 'agentnotch.sign.json'
        Write-Json -Path $signOverlay -Value (New-SignOverlay -ScriptPath $PSCommandPath)
        $arguments += @('--config', $signOverlay)
    }
    try {
        foreach ($entry in $Signing.Environment.GetEnumerator()) {
            [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value)
        }
        Invoke-Tool -File 'node' -Arguments $arguments -WorkingDirectory $AppDir
    } finally {
        foreach ($name in $SignStateVariables) { [Environment]::SetEnvironmentVariable($name, $null) }
    }
    $installers = @(Get-ChildItem (Join-Path $BundleDir '*-setup.exe'))
    if ($installers.Count -ne 1) { throw "expected one installer in $BundleDir, found $($installers.Count)" }
    $installers[0].FullName
}

function Invoke-Build {
    $version = Get-AppVersion
    $variables = Read-SigningVariables
    $plan = Get-SigningPlan -Variables $variables
    $outDir = (New-Item -ItemType Directory -Force $Out).FullName
    $name = "AgentNotch-$version-Setup.exe"
    $info = Join-Path $outDir 'windows-release-info.env'
    # A build that stops early leaves no installer behind, not even an earlier run's.
    Remove-Item (Join-Path $outDir $name), $info -Force -ErrorAction SilentlyContinue

    Write-Host "Agent Notch $version for Windows: updates $(if ($UpdaterPubkey) { 'on' } else { 'off (built from source)' }), code signing: $($plan.Method)"

    # The hook carries its C runtime (Claude Code may run it where vcruntime140.dll is
    # missing) and has its own target folder: the bundler copies resources beside the app
    # in target\release, where the hook would otherwise be copied onto itself.
    $flags = $env:RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        Invoke-Tool -File 'cargo' -WorkingDirectory $WindowsDir -Arguments @(
            'build', '--release', '--locked', '-p', 'agentnotch-hook', '--bin', 'agentnotch-hook', '--target-dir', 'target/hook')
    } finally {
        $env:RUSTFLAGS = $flags
    }
    if (-not (Test-Path $HookExe)) { throw "the hook was not built at $HookExe" }

    $overlayPath = Join-Path (Get-ScratchDir) 'agentnotch.build.json'
    Write-Json -Path $overlayPath -Value (New-BuildOverlay -Version $version -UpdaterPubkey $UpdaterPubkey)

    $tauri = Get-TauriCli
    # Compiling and bundling are two steps, so that signing credentials exist only while
    # nothing is being compiled. The app is built with the overlay (the update key is
    # compiled in); the signing command is the bundler's business alone.
    Invoke-Tool -File 'node' -WorkingDirectory $AppDir -Arguments @(
        $tauri, 'build', '--no-bundle', '--config', 'tauri.bundle.conf.json', '--config', $overlayPath, '--', '--locked')

    $signing = Initialize-Signing -Plan $plan
    try {
        $built = Invoke-Bundle -TauriCli $tauri -OverlayPath $overlayPath -Signing $signing
        $installer = Join-Path $outDir $name
        Copy-Item $built $installer
        $product = (Get-Item $installer).VersionInfo.ProductVersion
        if (-not $product -or -not $product.StartsWith($version)) {
            throw "the installer's ProductVersion is '$product', not $version"
        }
        if ($signing.Method -ne 'none') {
            # The bundler signs the hook where it lies and the installer it wrote; the app inside
            # is checked once it is installed (the smoke test).
            $problems = @($installer, $HookExe | ForEach-Object { Test-Signature -Path $_ -State $signing } | Where-Object { $_ })
            if ($problems.Count) { throw ($problems -join '; ') }
        }
    } finally {
        Clear-Signing -State $signing
    }

    @(
        "VERSION=$version"
        "INSTALLER=$name"
        "AUTHENTICODE=$(if ($signing.Method -eq 'none') { 'unsigned' } else { $signing.Method })"
        "SIGNER=$($signing.Signer)"
        "UPDATES=$(if ($UpdaterPubkey) { 'on' } else { 'off' })"
    ) | Set-Content -Path $info -Encoding utf8NoBOM
    if ($env:GITHUB_OUTPUT) {
        "version=$version", "name=$name", "authenticode=$(if ($signing.Method -eq 'none') { 'unsigned' } else { $signing.Method })" |
            Add-Content -Path $env:GITHUB_OUTPUT -Encoding utf8NoBOM
    }
    Write-Host "built $installer ($(if ($signing.Method -eq 'none') { 'not code-signed' } else { "code-signed: $($signing.Signer)" }))"
}

function New-ThrowawayPfx {
    # A self-signed code signing certificate that exists for one check. Windows trusts it
    # for nothing, which is the point: it proves the signing path, not an identity.
    $password = [Guid]::NewGuid().ToString('N')
    $rsa = [Security.Cryptography.RSA]::Create(2048)
    try {
        $request = [Security.Cryptography.X509Certificates.CertificateRequest]::new(
            'CN=Agent Notch signing check (throwaway)', $rsa,
            [Security.Cryptography.HashAlgorithmName]::SHA256, [Security.Cryptography.RSASignaturePadding]::Pkcs1)
        $usages = [Security.Cryptography.OidCollection]::new()
        [void]$usages.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3'))
        $request.CertificateExtensions.Add(
            [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($usages, $true))
        $request.CertificateExtensions.Add(
            [Security.Cryptography.X509Certificates.X509KeyUsageExtension]::new('DigitalSignature', $true))
        $now = [DateTimeOffset]::UtcNow
        $certificate = $request.CreateSelfSigned($now.AddMinutes(-5), $now.AddDays(1))
        $bytes = $certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pfx, $password)
    } finally {
        $rsa.Dispose()
    }
    @{ Base64 = [Convert]::ToBase64String($bytes); Password = $password }
}

function Invoke-CheckSigning {
    # The signing path, end to end, with nothing that could ever ship: a throwaway
    # certificate, the same bundling step a release runs, and every file put back.
    $version = Get-AppVersion
    $appExe = Join-Path $WindowsDir 'target\release\agentnotch.exe'
    if (-not (Test-Path $appExe) -or -not (Test-Path $HookExe)) {
        throw '-CheckSigning bundles the app that was just built: run the build first.'
    }
    [void](Read-SigningVariables)
    $sevenZip = (Get-Command 7z -ErrorAction SilentlyContinue)?.Source
    if (-not $sevenZip) { throw '-CheckSigning opens the installer with 7z, which was not found.' }

    $scratch = Join-Path (Get-ScratchDir) 'check-signing'
    Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $scratch | Out-Null
    # The bundler signs the app and the hook where they lie: keep their bytes, so the
    # unsigned build that made them stays the build a cache or a later step sees.
    $hookBackup = Join-Path $scratch 'agentnotch-hook.unsigned.exe'
    $appBackup = Join-Path $scratch 'agentnotch.unsigned.exe'
    Copy-Item $HookExe $hookBackup
    Copy-Item $appExe $appBackup
    $overlayPath = Join-Path $scratch 'agentnotch.build.json'
    Write-Json -Path $overlayPath -Value (New-BuildOverlay -Version $version)

    $pfx = New-ThrowawayPfx
    $plan = Get-SigningPlan -Variables @{
        WINDOWS_SIGN_PFX_BASE64    = $pfx.Base64
        WINDOWS_SIGN_PFX_PASSWORD  = $pfx.Password
        WINDOWS_SIGN_TIMESTAMP_URL = 'none'   # no timestamp server: the check needs no network
    }
    $signing = Initialize-Signing -Plan $plan
    try {
        $tauri = Join-Path $ToolsDir 'node_modules\@tauri-apps\cli\tauri.js'
        if (-not (Test-Path $tauri)) { $tauri = Get-TauriCli }
        $installer = Invoke-Bundle -TauriCli $tauri -OverlayPath $overlayPath -Signing $signing
        $extracted = Join-Path $scratch 'installer'
        Invoke-Tool -File $sevenZip -Arguments @('x', '-y', "-o$extracted", $installer) -Quiet
        # What the bundler signs: the app, every .exe among its resources (the hook, where
        # it lies), its own installer plugin, and the installer. (The uninstaller is signed
        # too, but only exists once the installer has run: the smoke test checks it there.)
        $inside = @('agentnotch.exe', 'agentnotch-hook.exe', 'nsis_tauri_utils.dll') | ForEach-Object {
            $file = Get-ChildItem $extracted -Recurse -Filter $_ | Select-Object -First 1
            if (-not $file) { throw "the installer holds no $_" }
            $file.FullName
        }
        $checked = @($installer, $HookExe) + $inside
        $problems = @($checked | ForEach-Object { Test-Signature -Path $_ -State $signing } | Where-Object { $_ })
        if ($problems.Count) { throw "the signing check failed: $($problems -join '; ')" }
        Write-Host "signing check: the installer, the hook (where it lies and inside), the app and the installer's plugin carry the throwaway certificate's signature"
    } finally {
        Clear-Signing -State $signing
        Copy-Item $hookBackup $HookExe -Force
        Copy-Item $appBackup $appExe -Force
        Remove-Item (Join-Path $BundleDir '*-setup.exe') -Force -ErrorAction SilentlyContinue
        Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Dot-sourced (the functions alone, for a test): nothing runs.
if ($MyInvocation.InvocationName -eq '.') { return }

switch ($PSCmdlet.ParameterSetName) {
    'Sign' { Invoke-SignFile -Path $SignFile }
    'CheckSigning' { Invoke-CheckSigning }
    default { Invoke-Build }
}
