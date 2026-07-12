[CmdletBinding()]
param(
    [Parameter()]
    [string] $SdkRoot = $env:MVCAM_COMMON_RUNENV
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "This script only generates bindings for Windows x64."
}

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "This script requires a 64-bit Windows operating system."
}

if ([string]::IsNullOrWhiteSpace($SdkRoot)) {
    throw @"
No MVS SDK development directory was provided.
Set MVCAM_COMMON_RUNENV or pass -SdkRoot with a directory containing Includes\MvCameraControl.h.
"@
}

$SdkRoot = [IO.Path]::GetFullPath($SdkRoot)
$IncludePath = Join-Path -Path $SdkRoot -ChildPath "Includes"
$HeaderPath = Join-Path -Path $IncludePath -ChildPath "MvCameraControl.h"

if (-not (Test-Path -LiteralPath $HeaderPath -PathType Leaf)) {
    throw "MvCameraControl.h was not found at '$HeaderPath'."
}

$BindgenCommand = Get-Command bindgen -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $BindgenCommand) {
    throw @"
bindgen was not found on PATH.
Install the latest CLI with: cargo install bindgen-cli --locked
"@
}

$BindgenVersion = (& $BindgenCommand.Source --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Failed to query the bindgen version."
}

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$OutputPath = Join-Path -Path $RepositoryRoot -ChildPath "mvs-sdk-sys\src\bindings.rs"
$TemporaryPath = "$OutputPath.$([Guid]::NewGuid().ToString('N')).tmp"

$BindgenArguments = @(
    "--output"
    $TemporaryPath
    "--allowlist-function"
    "MV_CC_.*"
    "--allowlist-function"
    "MV_GIGE_.*"
    "--allowlist-function"
    "MV_USB_.*"
    "--allowlist-function"
    "MV_CAML_.*"
    "--allowlist-function"
    "MV_GENTL_.*"
    "--allowlist-function"
    "MV_XML_.*"
    "--allowlist-function"
    "MV_SetLogPath"
    "--allowlist-function"
    "MV_SetLogLevel"
    "--allowlist-type"
    "MV_.*"
    "--allowlist-type"
    "_MV_.*"
    "--allowlist-type"
    "Mv.*"
    "--allowlist-type"
    "_Mv.*"
    "--allowlist-var"
    "MV_.*"
    "--allowlist-var"
    "INFO_MAX_BUFFER_SIZE"
    "--allowlist-var"
    "MAX_EVENT_NAME_SIZE"
    "--allowlist-var"
    "MAX_STRING_.*"
    "--allowlist-var"
    "PIXEL_.*"
    "--blocklist-file"
    ".*MvObsoleteInterfaces\.h"
    "--blocklist-file"
    ".*ObsoleteCamParams\.h"
    "--with-derive-default"
    "--no-prepend-enum-name"
    "--no-layout-tests"
    "--no-doc-comments"
    $HeaderPath
    "--"
    "-I$IncludePath"
    "--target=x86_64-pc-windows-msvc"
)

Write-Host "Generating Windows x64 bindings with $BindgenVersion..."
Write-Host "Header: $HeaderPath"

try {
    & $BindgenCommand.Source @BindgenArguments
    if ($LASTEXITCODE -ne 0) {
        throw "bindgen failed with exit code $LASTEXITCODE."
    }

    if (-not (Test-Path -LiteralPath $TemporaryPath -PathType Leaf)) {
        throw "bindgen completed without creating '$TemporaryPath'."
    }

    if ((Get-Item -LiteralPath $TemporaryPath).Length -eq 0) {
        throw "bindgen generated an empty bindings file."
    }

    Move-Item -LiteralPath $TemporaryPath -Destination $OutputPath -Force
    Write-Host "Updated: $OutputPath"
}
finally {
    if (Test-Path -LiteralPath $TemporaryPath) {
        Remove-Item -LiteralPath $TemporaryPath -Force
    }
}
