#Requires -Version 5.1
# Installs the latest nux release for this machine.
#   irm https://raw.githubusercontent.com/sammwyy/nux/main/install.ps1 | iex
$ErrorActionPreference = "Stop"

$Repo = "sammwyy/nux"

$Arch = switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
    "X64" { "x86_64" }
    default {
        throw "no prebuilt nux binary for architecture '$_' — only x86_64 Windows builds are published."
    }
}

$Asset = "nux-windows-$Arch.zip"
Write-Host "Detected windows/$Arch; looking up the latest release..."

$Release = Invoke-RestMethod `
    -Uri "https://api.github.com/repos/$Repo/releases/latest" `
    -Headers @{ "User-Agent" = "nux-install-script" }

$AssetInfo = $Release.assets | Where-Object { $_.name -eq $Asset }
if (-not $AssetInfo) {
    throw "no release asset named '$Asset' was found in the latest release of $Repo."
}

$TmpDir = Join-Path $env:TEMP "nux-install-$([System.Guid]::NewGuid())"
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    $ZipPath = Join-Path $TmpDir $Asset
    Write-Host "Downloading $($AssetInfo.browser_download_url)"
    Invoke-WebRequest -Uri $AssetInfo.browser_download_url -OutFile $ZipPath -UseBasicParsing
    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    $BinSrc = Get-ChildItem -Path $TmpDir -Recurse -Filter "nux.exe" | Select-Object -First 1
    if (-not $BinSrc) {
        throw "couldn't find nux.exe inside $Asset."
    }

    # Installed alongside nux's own config dir (%APPDATA%\nux), in a bin\
    # subfolder — a per-user location that needs no admin rights.
    $InstallDir = Join-Path $env:APPDATA "nux\bin"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $DestPath = Join-Path $InstallDir "nux.exe"
    Copy-Item -Path $BinSrc.FullName -Destination $DestPath -Force
    Write-Host "Installed nux to $DestPath"

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathEntries = @()
    if ($UserPath) { $PathEntries = $UserPath -split ";" }
    if ($PathEntries -notcontains $InstallDir) {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "Added $InstallDir to your user PATH (open a new terminal to use it there)"
    }
    if (($env:Path -split ";") -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    & $DestPath --version
}
finally {
    Remove-Item -Path $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}
