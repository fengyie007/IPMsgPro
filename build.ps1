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
    $ExeName = "SpeedIpMsg.exe"
} else {
    $ExeName = "SpeedIpMsg_X86.exe"
}
$ExePath = Join-Path $BuildDir "$Config\$ExeName"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host " SpeedIpMsg Build Script" -ForegroundColor Cyan
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

# Step 2: CMake configure — auto-detect compiler environment
Write-Host "`n[2/4] CMake configure ($Arch)..." -ForegroundColor Yellow
$cmakeArch = if ($Arch -eq "x86") { "Win32" } else { "x64" }

# --- Detect VS / Build Tools ---
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$vsGenMap = @{
    "2015" = 14; "2017" = 15; "2019" = 16; "2022" = 17
    "2025" = 18; "2026" = 19; "2027" = 20
}

# 1) Try vswhere to detect full VS installation
$generator = $null
$cmakeExe = "cmake"
$useNinja = $false
$vcvarsall = $null
$buildToolsCmake = $null

if (Test-Path $vswhere) {
    $vsVersion = & $vswhere -latest -property catalog_productLineVersion 2>$null
    if ($vsVersion -match "^\d{4}$") {
        $vsMajor = $vsGenMap[$vsVersion]
        if ($vsMajor) {
            $candidate = "Visual Studio $vsMajor $vsVersion"
            $cmakeHelp = cmake --help 2>$null
            if ($cmakeHelp -match [regex]::Escape($candidate)) {
                $generator = $candidate
                Write-Host "Detected: $generator (via vswhere, system cmake)" -ForegroundColor DarkGray
            } else {
                Write-Host "Detected $candidate via vswhere, but system cmake doesn't support it" -ForegroundColor DarkGray
            }
        }
    }
}

# 2) If no generator yet, look for Build Tools vcvarsall.bat
if (-not $generator) {
    $searchPaths = @(
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\*\VC\Auxiliary\Build\vcvarsall.bat",
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\*\BuildTools\VC\Auxiliary\Build\vcvarsall.bat",
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\*\VC\Auxiliary\Build\vcvarsall.bat",
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\18\*\VC\Auxiliary\Build\vcvarsall.bat",
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\18\*\BuildTools\VC\Auxiliary\Build\vcvarsall.bat",
        "${env:ProgramFiles}\Microsoft Visual Studio\18\*\VC\Auxiliary\Build\vcvarsall.bat"
    )
    foreach ($pattern in $searchPaths) {
        $found = Get-Item $pattern -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) {
            $vcvarsall = $found.FullName
            # Check if this is VS Build Tools 18 (VS2025/2026)
            if ($vcvarsall -match "Microsoft Visual Studio\\18\\") {
                $btCmake = $vcvarsall.Replace("\VC\Auxiliary\Build\vcvarsall.bat", "\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe")
                if (Test-Path $btCmake) {
                    $buildToolsCmake = $btCmake
                    $cmakeExe = $btCmake
                    $generator = "Visual Studio 18 2026"
                    Write-Host "Found VS Build Tools v18 at: $vcvarsall" -ForegroundColor DarkGray
                    Write-Host "Using Build Tools cmake: $buildToolsCmake" -ForegroundColor DarkGray
                    Write-Host "Generator: $generator" -ForegroundColor DarkGray
                    break
                }
            }
            break
        }
    }
    if (-not $generator -and $vcvarsall) {
        Write-Host "Found vcvarsall.bat: $vcvarsall (will use ninja fallback)" -ForegroundColor DarkGray
        $useNinja = $true
    } elseif (-not $vcvarsall) {
        Write-Host "WARNING: No VS installation or Build Tools found" -ForegroundColor Yellow
    }
}

# 3) Configure with detected toolchain
if ($generator -and $buildToolsCmake) {
    # Use Build Tools cmake with VS2026 generator — clean build dir to avoid generator conflict
    if (Test-Path $BuildDir) { Remove-Item $BuildDir -Recurse -Force -ErrorAction SilentlyContinue }
    $cmakeArgs = @("-B", $BuildDir, "-G", $generator, "-A", $cmakeArch)
    & $buildToolsCmake @cmakeArgs 2>&1
    if ($LASTEXITCODE -ne 0) { throw "CMake configure failed (Build Tools cmake)" }
} elseif ($generator) {
    # Use system cmake with detected VS generator
    $cmakeArgs = @("-B", $BuildDir, "-G", $generator, "-A", $cmakeArch)
    cmake @cmakeArgs 2>&1
    if ($LASTEXITCODE -ne 0) { throw "CMake configure failed" }
} elseif ($useNinja) {
    # Use Ninja + MSVC via vcvarsall.bat + system cmake
    $archArg = if ($Arch -eq "x86") { "x86" } else { "x64" }
    $buildType = if ($Config -eq "Debug") { "Debug" } else { "Release" }

    # Find ninja.exe full path
    $ninjaPath = (Get-Command ninja -ErrorAction SilentlyContinue).Source
    if (-not $ninjaPath) {
        $ninjaPath = "C:\Program Files\CMake\bin\ninja.exe"
        if (-not (Test-Path $ninjaPath)) {
            throw "ninja.exe not found. Install CMake or add ninja to PATH."
        }
    }
    $ninjaDir = Split-Path $ninjaPath

    # Write temp batch file for configure
    $batFile = Join-Path $BuildDir "_configure.bat"
    @"
call "`"$vcvarsall`" $archArg" >nul 2>&1
set "PATH=$ninjaDir;%PATH%"
cmake -B "`"$BuildDir`"" -G Ninja -DCMAKE_BUILD_TYPE=$buildType -DCMAKE_MSVC_RUNTIME_LIBRARY="MultiThreaded`$<$<CONFIG:Debug>:Debug>"
"@ | Set-Content -Path $batFile -Encoding ASCII

    cmd /c $batFile 2>&1 | ForEach-Object { Write-Host $_ }
    if ($LASTEXITCODE -ne 0) { throw "CMake configure failed (ninja)" }
    Remove-Item $batFile -ErrorAction SilentlyContinue

    $env:IPMSGPRO_VCARSALL = $vcvarsall
    $env:IPMSGPRO_ARCH = $archArg
    $env:IPMSGPRO_NINJA = $ninjaPath
} else {
    throw "No supported compiler found. Install VS2022 or Build Tools."
}

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

if ($buildToolsCmake -and $generator -like "Visual Studio 18*") {
    # Build with Build Tools cmake (uses VS generator, not ninja)
    $cleanFlag = if ($Clean) { " --clean-first" } else { "" }
    $buildArgs = @("--build", $BuildDir, "--config", $Config) + $cleanFlag.Split(" ", [StringSplitOptions]::RemoveEmptyEntries)
    & $buildToolsCmake @buildArgs 2>&1 | ForEach-Object { Write-Host $_ }
} elseif ($useNinja -and $env:IPMSGPRO_VCARSALL) {
    # Ninja build needs vcvarsall environment
    $vcvarsall = $env:IPMSGPRO_VCARSALL
    $archArg = $env:IPMSGPRO_ARCH
    $ninjaDir = Split-Path $env:IPMSGPRO_NINJA
    $cleanFlag = if ($Clean) { " --clean-first" } else { "" }

    $batFile = Join-Path $BuildDir "_build.bat"
    @"
call "`"$vcvarsall`" $archArg" >nul 2>&1
set "PATH=$ninjaDir;%PATH%"
cmake --build "`"$BuildDir`"" --config $Config$cleanFlag
"@ | Set-Content -Path $batFile -Encoding ASCII

    cmd /c $batFile 2>&1 | ForEach-Object { Write-Host $_ }
    Remove-Item $batFile -ErrorAction SilentlyContinue
} else {
    if ($Clean) {
        cmake --build $BuildDir --config $Config --clean-first
    } else {
        cmake --build $BuildDir --config $Config
    }
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