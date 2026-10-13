#Requires -Version 7
<#
.SYNOPSIS
  Builds the AgentPlaza Windows MSI and zip for one architecture.

.DESCRIPTION
  -Stage  the staged tree (bin\ and share\ beneath it)
  -Version  x.y.z
  -Arch  x64 or arm64
  -Out  output directory for the .msi and .zip

  If WINDOWS_CERT_PFX_BASE64 and WINDOWS_CERT_PASSWORD are set, the exes are signed
  before packaging and the MSI is signed after it is built. Otherwise signing is skipped.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Stage,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory)][string]$Out
)

$ErrorActionPreference = 'Stop'

$Stage = (Resolve-Path -LiteralPath $Stage).Path
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$Out = (Resolve-Path -LiteralPath $Out).Path
$wxs = Join-Path $PSScriptRoot 'agentplaza.wxs'
$msi = Join-Path $Out "AgentPlaza-$Version-windows-$Arch.msi"
$zip = Join-Path $Out "AgentPlaza-$Version-windows-$Arch.zip"

# Ensure the WiX v5 CLI is available.
$dotnetTools = Join-Path $env:USERPROFILE '.dotnet\tools'
$env:PATH = "$dotnetTools;$env:PATH"
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    Write-Output 'Installing WiX Toolset 5.0.2 (dotnet global tool)'
    dotnet tool install --global wix --version 5.0.2
    if ($LASTEXITCODE -ne 0) { throw "dotnet tool install wix failed (exit $LASTEXITCODE)" }
}
if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    throw 'wix is not on PATH after installation'
}

$signing = [bool]($env:WINDOWS_CERT_PFX_BASE64 -and $env:WINDOWS_CERT_PASSWORD)
$pfx = $null
$signtool = $null
$timestamp = 'http://timestamp.digicert.com'

try {
    if ($signing) {
        $pfx = Join-Path ([IO.Path]::GetTempPath()) ("agentplaza-sign-" + [guid]::NewGuid().ToString('N') + '.pfx')
        [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))

        $signtool = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
            Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
            Select-Object -First 1 -ExpandProperty FullName
        if (-not $signtool) { throw 'signtool.exe not found under Windows Kits 10 bin' }

        function Invoke-Sign([string[]]$Files) {
            $signArgs = @('sign', '/fd', 'SHA256', '/tr', $timestamp, '/td', 'SHA256', '/f', $pfx, '/p', $env:WINDOWS_CERT_PASSWORD) + $Files
            & $signtool @signArgs
            if ($LASTEXITCODE -ne 0) { throw "signtool sign failed (exit $LASTEXITCODE)" }
        }

        $exes = @(Get-ChildItem -Path $Stage -Recurse -Filter '*.exe' | Select-Object -ExpandProperty FullName)
        if ($exes.Count -eq 0) { throw "no .exe files found under $Stage to sign" }
        Write-Output "Signing $($exes.Count) executable(s)"
        Invoke-Sign $exes
    }
    else {
        Write-Output 'Authenticode signing skipped: WINDOWS_CERT_PFX_BASE64 / WINDOWS_CERT_PASSWORD not set (unsigned build)'
    }

    # MSI
    Write-Output "Building $msi"
    wix build $wxs -arch $Arch -d "Version=$Version" -d "StageDir=$Stage" -o $msi
    if ($LASTEXITCODE -ne 0) { throw "wix build failed (exit $LASTEXITCODE)" }
    # WiX also writes the MSI's debug symbols (.wixpdb) beside it: not a deliverable.
    Remove-Item -LiteralPath ([IO.Path]::ChangeExtension($msi, '.wixpdb')) -Force -ErrorAction SilentlyContinue

    if ($signing) {
        Write-Output "Signing $msi"
        Invoke-Sign @($msi)
    }

    # Zip: the stage contents at the archive root (bin\, share\)
    Write-Output "Building $zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
    Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $zip -CompressionLevel Optimal
}
finally {
    if ($pfx -and (Test-Path -LiteralPath $pfx)) { Remove-Item -LiteralPath $pfx -Force }
}

Write-Output 'Produced:'
Get-ChildItem -LiteralPath $Out -File | Where-Object { $_.Name -like "AgentPlaza-$Version-windows-$Arch.*" } |
    ForEach-Object { Write-Output ("  {0}  ({1:N0} bytes)" -f $_.FullName, $_.Length) }
