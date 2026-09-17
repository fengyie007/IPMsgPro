# ============================================================================
# IPMsgPro Build Script
# Usage: .\build.ps1 [-Config Debug|Release] [-Clean] [-Run] [-Port <port>]
#                    [-Arch x64|x86] [-SkipFrontend]
# ============================================================================

param(
    [ValidateSet("Debug", "Release")]
    [string]$Config = "Release",

    [ValidateSet("x64", "x86")]
    [string]$Arch = "x64",

    [switch]$Clean,
    [switch]$Run,
    [switch]$SkipFrontend,
    [int]$Port = 0
)

$ErrorActionPreference = "Stop"
$ProjectRoot = $PSScriptRoot

# Build directory includes architecture suffix
$BuildDir = Join-Path $ProjectRoot "build_$Arch"
$FrontendDir = Join-Path $ProjectRoot "frontend"

# Output exe name differs by architecture
if ($Arch -eq "x64") {
    $ExeName = "IPMsgPro.exe"
} else {
    $ExeName = "IPMsgPro_X86.exe"
}
$ExePath = Join-Path $BuildDir "$Config\$ExeName"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " IPMsgPro Build Script" -ForegroundColor Cyan
Write-Host " Config: $Config" -ForegroundColor Cyan
Write-Host " Arch:   $Arch" -ForegroundColor Cyan
Write-Host " Output: $ExeName" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# Step 1: Build frontend (always, unless -SkipFrontend)
if (-not $SkipFrontend) {
    Write-Host "`n[1/4] Building frontend..." -ForegroundColor Yellow
    Push-Location $FrontendDir
    try {
        if (-not (Test-Path "node_modules")) {
            npm install 2>&1 | Out-Null
        }
        npx vite build
        if ($LASTEXITCODE -ne 0) { throw "Frontend build failed" }
    } finally {
        Pop-Location
    }
    Write-Host "Frontend build OK" -ForegroundColor Green
} else {
    Write-Host "`n[1/4] Skipping frontend build (-SkipFrontend)" -ForegroundColor DarkGray
}

# Step 2: CMake configure
Write-Host "`n[2/4] CMake configure (VS2022 $Arch)..." -ForegroundColor Yellow
# VS2022 uses "Win32" for x86, "x64" for x64
$cmakeArch = if ($Arch -eq "x86") { "Win32" } else { "x64" }
$cmakeArgs = @("-B", $BuildDir, "-G", "Visual Studio 17 2022", "-A", $cmakeArch)
cmake @cmakeArgs 2>&1
if ($LASTEXITCODE -ne 0) { throw "CMake configure failed" }
Write-Host "CMake configure OK" -ForegroundColor Green

# Step 3: Build (ensure frontend resources are repacked)
Write-Host "`n[3/4] Building..." -ForegroundColor Yellow

# Repack frontend resources into .rc file to ensure latest dist is embedded
$packScript = Join-Path $ProjectRoot "TauriCPP\tools\pack_resources.py"
$resourcesRc = Join-Path $BuildDir "generated\resources.rc"
$frontendDist = Join-Path $FrontendDir "dist"
if ((Test-Path $packScript) -and (Test-Path $frontendDist)) {
    Write-Host "Repacking frontend resources..." -ForegroundColor DarkGray
    & python $packScript $frontendDist -o $resourcesRc -t "TAURI_RES" 2>&1 | Out-Null
}

if ($Clean) {
    cmake --build $BuildDir --config $Config --clean-first
} else {
    cmake --build $BuildDir --config $Config
}
if ($LASTEXITCODE -ne 0) { throw "Build failed" }
Write-Host "Build OK" -ForegroundColor Green

# Step 4: Run if requested
if ($Run) {
    Write-Host "`n[4/4] Running $ExePath..." -ForegroundColor Yellow
    if (-not (Test-Path $ExePath)) { throw "Executable not found: $ExePath" }

    $exeSize = (Get-Item $ExePath).Length / 1MB
    Write-Host "Executable: $ExePath ($([math]::Round($exeSize, 2)) MB)" -ForegroundColor Cyan

    if ($Port -gt 0) {
        Write-Host "Starting with port $Port..." -ForegroundColor Cyan
        Start-Process $ExePath -ArgumentList "--port=$Port"
    } else {
        Start-Process $ExePath
    }
    Write-Host "Launched!" -ForegroundColor Green
} else {
    Write-Host "`n[4/4] Skipping run (use -Run to launch)" -ForegroundColor DarkGray
}

Write-Host "`n========================================" -ForegroundColor Cyan
Write-Host " Build complete!" -ForegroundColor Green
Write-Host " Output: $ExePath" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
