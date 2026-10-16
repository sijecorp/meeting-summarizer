# Builds llama-server for Windows x64 (Vulkan), llama.cpp, ggml and the C runtime linked in, as the
# Tauri sidecar src-tauri\binaries\llama-server-x86_64-pc-windows-msvc.exe (M6.5 plan 5.1, architecture
# 11.3). CI runs this same script. Run it in the x64 MSVC developer environment with Ninja and the
# Vulkan SDK, like the CI build. The commit is scripts\llama.cpp.rev and the CMake options
# scripts\llama-server.flags. The binary is cached outside the repository per commit, target and
# options, so another worktree only copies it. MS_LLAMA_SERVER_CACHE overrides the cache folder (CI).
# The build itself runs in <MS_LLAMA_SERVER_WORK, default C:\t>\lls, removed after a successful build:
# ggml-vulkan's nested vulkan-shaders-gen build is deep, and under %LOCALAPPDATA% its paths come near
# MSVC's 260-character limit (C1041, the reason CI moves CARGO_TARGET_DIR).
# No pwsh-only syntax, so PowerShell 7 (pwsh, what CI runs) or Windows PowerShell 5.1 (not tested).
# macOS: scripts/build-llama-server.sh.
$ErrorActionPreference = 'Stop'

$triple = 'x86_64-pc-windows-msvc'
$root = Split-Path -Parent $PSScriptRoot

# Native commands do not throw on a non-zero exit code. @(): always an array, even for one argument.
function Invoke-Checked {
    $cmd = $args[0]
    $rest = @($args | Select-Object -Skip 1)
    & $cmd @rest
    if ($LASTEXITCODE) { throw "$cmd $rest failed with exit code $LASTEXITCODE" }
}

$missing = @()
if (-not (Get-Command git -ErrorAction SilentlyContinue)) { $missing += 'git: winget install Git.Git' }
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) { $missing += 'cmake: winget install Kitware.CMake' }
if (-not (Get-Command ninja -ErrorAction SilentlyContinue)) { $missing += 'ninja: winget install Ninja-build.Ninja' }
# VsDevCmd (and msvc-dev-cmd in CI) sets VSCMD_ARG_TGT_ARCH. Without it, the x64 compiler lives in
# ...\Hostx64\x64\cl.exe. Not the banner: it is localized (Korean Visual Studio).
$cl = Get-Command cl -ErrorAction SilentlyContinue
$x64 = if ($env:VSCMD_ARG_TGT_ARCH) { $env:VSCMD_ARG_TGT_ARCH -eq 'x64' } else { $cl -and $cl.Source -match '\\x64\\cl\.exe$' }
if (-not ($cl -and $x64)) {
    $missing += 'cl.exe for x64 (MSVC): winget install Microsoft.VisualStudio.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive", then run this in "x64 Native Tools Command Prompt for VS" (start pwsh there) or after Launch-VsDevShell.ps1 -Arch amd64 -HostArch amd64'
}
if (-not $env:VULKAN_SDK) {
    $missing += 'Vulkan SDK (VULKAN_SDK): winget install KhronosGroup.VulkanSDK, then open a new terminal'
} elseif (-not (Test-Path "$env:VULKAN_SDK\Bin\glslc.exe")) {
    $missing += "glslc: not in $env:VULKAN_SDK\Bin, reinstall the Vulkan SDK"
}
if ($missing.Count) {
    Write-Host 'Missing for the llama-server build:'
    $missing | ForEach-Object { Write-Host "  $_" }
    exit 1
}

$rev = ((Get-Content "$root\scripts\llama.cpp.rev" | Where-Object { $_ -notmatch '^#' }) -join '').Trim()
if ($rev -notmatch '^[0-9a-f]{40}$') { throw "scripts\llama.cpp.rev must hold one full commit SHA, not '$rev'" }
$flagsFile = "$root\scripts\llama-server.flags"
$flags = @(Get-Content $flagsFile | ForEach-Object {
        $scope, $opt = $_ -split '\s+', 2
        if ($scope -eq '*' -or $scope -eq $triple) { "-D$opt" }
    })
$hash = (Get-FileHash -Algorithm SHA256 $flagsFile).Hash.Substring(0, 12).ToLower()

$cacheRoot = if ($env:MS_LLAMA_SERVER_CACHE) { $env:MS_LLAMA_SERVER_CACHE } else { "$env:LOCALAPPDATA\meeting-summarizer\llama-server" }
$cached = Join-Path $cacheRoot "$rev-$triple-$hash\llama-server.exe"
if (Test-Path $cached) {
    Write-Host "Using the cached build $cached"
} else {
    # Only the lls subfolder is removed, never the folder MS_LLAMA_SERVER_WORK names.
    # ponytail: one work folder per machine, so two worktrees building at the same moment collide.
    # Rare (a build happens once per commit and options); give each run its own folder if it bites.
    $workRoot = if ($env:MS_LLAMA_SERVER_WORK) { $env:MS_LLAMA_SERVER_WORK } else { "$env:SystemDrive\t" }
    $work = Join-Path $workRoot 'lls'
    if (Test-Path $work) { Remove-Item -Recurse -Force $work }
    Invoke-Checked git init -q "$work\src"
    Invoke-Checked git -C "$work\src" fetch -q --depth 1 https://github.com/ggml-org/llama.cpp $rev
    Invoke-Checked git -C "$work\src" checkout -q FETCH_HEAD
    Write-Host "+ cmake -S $work\src -B $work\build -G Ninja $flags"
    Invoke-Checked cmake -S "$work\src" -B "$work\build" -G Ninja @flags
    $cmakeCache = Get-Content "$work\build\CMakeCache.txt"
    $cmakeCache -match '^(GGML_(NATIVE|AVX2|FMA|F16C|BMI2|AVX512|VULKAN|OPENMP)|BUILD_SHARED_LIBS|LLAMA_OPENSSL|CMAKE_MSVC_RUNTIME_LIBRARY):'
    # The CPU baseline must reach ggml, or the binary dies on office PCs without the runner's AVX-512.
    if ($cmakeCache -notcontains 'GGML_NATIVE:BOOL=OFF' -or $cmakeCache -notcontains 'GGML_AVX2:BOOL=ON' -or $cmakeCache -contains 'GGML_AVX512:BOOL=ON') {
        throw "llama.cpp's CMake cache does not have GGML_NATIVE=OFF, GGML_AVX2=ON and GGML_AVX512 off"
    }
    Invoke-Checked cmake --build "$work\build" --target llama-server
    New-Item -ItemType Directory -Force (Split-Path $cached) | Out-Null
    Copy-Item "$work\build\bin\llama-server.exe" "$cached.tmp"
    Move-Item -Force "$cached.tmp" $cached
    Remove-Item -Recurse -Force $work
}

# Stamp last: an interrupted copy leaves no stamp, and src-tauri\build.rs refuses a release build then.
# Copy, then move over: build.rs never sees a half-written exe.
$out = "$root\src-tauri\binaries\llama-server-$triple"
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue "$out.stamp"
Copy-Item -Force $cached "$out.exe.tmp"
Move-Item -Force "$out.exe.tmp" "$out.exe"
[IO.File]::WriteAllText("$out.stamp", "rev=$rev`nflags=$hash`n")
Write-Host "Sidecar: $out.exe (llama.cpp $rev, flags $hash)"
# GitHub's pwsh wrapper exits with $LASTEXITCODE, which the last native command left.
exit 0
