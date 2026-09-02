#Requires -Version 5.1
# Installs the latest nux release for this machine.
#   irm https://raw.githubusercontent.com/sammwyy/nux/main/install.ps1 | iex
$ErrorActionPreference = "Stop"

$Repo = "sammwyy/nux"

Write-Host "detecting system..."
$Arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64" { "x86_64" }
    default {
        throw "no prebuilt nux binary for architecture '$_' - only x86_64 Windows builds are published."
    }
}
$Asset = "nux-windows-$Arch.zip"
Write-Host "  -> windows/$Arch"

Write-Host "fetching latest release..."
$Release = Invoke-RestMethod `
    -Uri "https://api.github.com/repos/$Repo/releases/latest" `
    -Headers @{ "User-Agent" = "nux-install-script" }
$AssetInfo = $Release.assets | Where-Object { $_.name -eq $Asset }
if (-not $AssetInfo) {
    throw "no release asset named '$Asset' was found for $Repo."
}

$TmpDir = Join-Path $env:TEMP "nux-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    Write-Host "downloading $Asset..."
    $ZipPath = Join-Path $TmpDir $Asset
    Invoke-WebRequest -Uri $AssetInfo.browser_download_url -OutFile $ZipPath -UseBasicParsing
    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    $BinSrc = Get-ChildItem -Path $TmpDir -Recurse -Filter "nux.exe" | Select-Object -First 1
    if (-not $BinSrc) {
        throw "couldn't find nux.exe inside $Asset."
    }

    # Alongside nux's own config dir (%APPDATA%\nux), in a bin\ subfolder -
    # a per-user location that needs no admin rights.
    $InstallDir = Join-Path $env:APPDATA "nux\bin"
    Write-Host "installing to $InstallDir..."
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $DestPath = Join-Path $InstallDir "nux.exe"
    Copy-Item -Path $BinSrc.FullName -Destination $DestPath -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = if ($UserPath) { $UserPath -split ";" } else { @() }
    $PathNote = ""
    if ($PathEntries -notcontains $InstallDir) {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        $PathNote = "added to your user PATH"
    }
    if (($env:Path -split ";") -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    Write-Host ""
    Write-Host "OK: $(& $DestPath --version) -> $InstallDir"
    if ($PathNote) { Write-Host "$PathNote (already active in this session)" }
}
finally {
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
