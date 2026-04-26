<#
.SYNOPSIS
    Downloads the CPython WASI runtime for HiveMind CodeAct.

.DESCRIPTION
    Fetches the CPython 3.12 WASI build from vmware-labs/webassembly-language-runtimes
    and installs it to ~/.hivemind/runtimes/python-wasm/.

    After running this script, HiveMind will automatically detect the runtime
    at startup (no environment variables needed).

.PARAMETER InstallDir
    Override the installation directory. Defaults to ~/.hivemind/runtimes/python-wasm.
#>
param(
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"

$RELEASE_TAG = "python/3.12.0+20231211-040d5a6"
$ASSET_NAME = "python-3.12.0-wasi-sdk-20.0.tar.gz"
$DOWNLOAD_URL = "https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/$([Uri]::EscapeDataString($RELEASE_TAG))/$ASSET_NAME"

if (-not $InstallDir) {
    $InstallDir = [IO.Path]::Combine($env:USERPROFILE, ".hivemind", "runtimes", "python-wasm")
}

Write-Host "HiveMind CodeAct - CPython WASI Runtime Installer" -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Source:  $DOWNLOAD_URL"
Write-Host "Target:  $InstallDir"
Write-Host ""

# Check if already installed
$WasmBin = [IO.Path]::Combine($InstallDir, "bin", "python.wasm")
if (Test-Path $WasmBin) {
    Write-Host "python.wasm already installed at $WasmBin" -ForegroundColor Yellow
    $response = Read-Host "Reinstall? (y/N)"
    if ($response -ne "y") {
        Write-Host "Skipped." -ForegroundColor Gray
        exit 0
    }
}

# Download
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) "hivemind-python-wasm-$(Get-Random)"
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null
$TarPath = Join-Path $TempDir $ASSET_NAME

Write-Host "Downloading ($ASSET_NAME)..." -ForegroundColor Green
try {
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile $TarPath -UseBasicParsing
} catch {
    Write-Host "Download failed: $_" -ForegroundColor Red
    exit 1
}

$SizeMB = [math]::Round((Get-Item $TarPath).Length / 1MB, 1)
Write-Host "Downloaded $SizeMB MB" -ForegroundColor Green

# Extract
$ExtractDir = Join-Path $TempDir "extracted"
New-Item -ItemType Directory -Path $ExtractDir -Force | Out-Null
Write-Host "Extracting..." -ForegroundColor Green
tar -xzf $TarPath -C $ExtractDir
if ($LASTEXITCODE -ne 0) {
    Write-Host "Extraction failed" -ForegroundColor Red
    exit 1
}

# Install to normalized layout
Write-Host "Installing to $InstallDir..." -ForegroundColor Green
$BinDir = Join-Path $InstallDir "bin"
$LibDir = Join-Path $InstallDir "lib"
New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
New-Item -ItemType Directory -Path $LibDir -Force | Out-Null

# Find the .wasm binary (may be named python-3.12.0.wasm)
$WasmFile = Get-ChildItem -Path (Join-Path $ExtractDir "bin") -Filter "python*.wasm" | Select-Object -First 1
if (-not $WasmFile) {
    Write-Host "Could not find python*.wasm in archive" -ForegroundColor Red
    exit 1
}
Copy-Item $WasmFile.FullName (Join-Path $BinDir "python.wasm") -Force

# Copy stdlib directory
$StdlibSrc = [IO.Path]::Combine($ExtractDir, "usr", "local", "lib", "python3.12")
if (Test-Path $StdlibSrc) {
    $StdlibDst = Join-Path $LibDir "python3.12"
    if (Test-Path $StdlibDst) { Remove-Item -Recurse -Force $StdlibDst }
    Copy-Item -Recurse $StdlibSrc $StdlibDst
}

# Also copy the zipped stdlib if present
$StdlibZip = [IO.Path]::Combine($ExtractDir, "usr", "local", "lib", "python312.zip")
if (Test-Path $StdlibZip) {
    Copy-Item $StdlibZip (Join-Path $LibDir "python312.zip") -Force
}

# Clean up
Remove-Item -Recurse -Force $TempDir

# Verify
$FinalWasm = Join-Path $BinDir "python.wasm"
$FinalStdlib = Join-Path $LibDir "python3.12"
if ((Test-Path $FinalWasm) -and (Test-Path $FinalStdlib)) {
    $WasmSize = [math]::Round((Get-Item $FinalWasm).Length / 1MB, 1)
    Write-Host ""
    Write-Host "Installation complete!" -ForegroundColor Green
    Write-Host ("  python.wasm: {0} ({1} MB)" -f $FinalWasm, $WasmSize) -ForegroundColor White
    Write-Host "  stdlib:      $FinalStdlib" -ForegroundColor White
    Write-Host ""
    Write-Host "HiveMind will auto-detect this runtime on next startup." -ForegroundColor Cyan
} else {
    Write-Host "Installation may be incomplete - check $InstallDir" -ForegroundColor Yellow
}
