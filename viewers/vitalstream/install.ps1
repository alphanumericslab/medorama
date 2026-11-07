<#
.SYNOPSIS
  VitalStream on its own, without Medorama, on Windows: build one viewer and install it (the
  counterpart of viewers/vitalstream/install.sh).

.DESCRIPTION
    -Viewer cpp    the C++ viewer vitalstream-cpp (default; CMake, Ninja and MSVC: run it from a
                   "Developer PowerShell for VS")
    -Viewer rs     the Rust viewer vitalstream-rs
    -Viewer py     the Python viewer vitalstream-py
    -Prefix DIR    where it goes: DIR\bin (default %LOCALAPPDATA%\vitalstream; the Python
                   viewer's environment in DIR\venv)
    -Root DIR      the viewer's root, fixed in the program: it never opens anything outside DIR
                   (default ~, the home folder of whoever runs it)
    -NoPassword    build it without password support (no password can be set; it then refuses
                   to run where one is required, as under Medorama)
    -NoWfdb        build it without WFDB support: no wfdb-x code in it (a WFDB record or folder
                   is refused; every other format is read; docs\building.md at the repository root)
  Licensed recording formats are read through their add-on, an adapter installed into
  %LOCALAPPDATA%\medorama\adapters (docs\adapters.md at the repository root).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File viewers\vitalstream\install.ps1 -Viewer rs
#>
param(
  [ValidateSet('cpp', 'rs', 'py')][string]$Viewer = 'cpp',
  [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'vitalstream'),
  [string]$Root = '~',
  [switch]$NoPassword,
  [switch]$NoWfdb
)
$ErrorActionPreference = 'Stop'
# absolute folders (a root starting with ~ stays as it is: the home folder of whoever runs the
# viewer); CMake gets the root as it is, its backslashes included (cmake/build_paths.cmake)
$Resolve = { param($p) $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }
$Prefix = & $Resolve $Prefix
if ($Root -ne '~' -and -not $Root.StartsWith('~\') -and -not $Root.StartsWith('~/')) { $Root = & $Resolve $Root }
$Here = $PSScriptRoot
$Repo = Split-Path -Parent (Split-Path -Parent $Here)
$Password = if ($NoPassword) { 'OFF' } else { 'ON' }
$Wfdb = if ($NoWfdb) { 'OFF' } else { 'ON' }
$Bin = Join-Path $Prefix 'bin'
New-Item -ItemType Directory -Force $Bin | Out-Null
function Run {
  $exe = $args[0]
  $rest = @($args | Select-Object -Skip 1)
  & $exe @rest
  if ($LASTEXITCODE -ne 0) { throw "failed ($LASTEXITCODE): $exe $($rest -join ' ')" }
}

switch ($Viewer) {
  'cpp' {
    Push-Location (Join-Path $Here 'cpp')
    try {
      # its own build folder: Medorama's viewer (build\viewer) keeps its settings
      Run cmake --preset viewer -B build/standalone "-DVS_ROOT=$Root" "-DVS_PASSWORD=$Password" "-DVS_WFDB=$Wfdb"
      Run cmake --build build/standalone
    } finally { Pop-Location }
    Copy-Item (Join-Path $Here 'cpp\build\standalone\bin\vitalstream-cpp.exe') $Bin -Force
    $Program = 'vitalstream-cpp'
  }
  'rs' {
    Push-Location $Repo
    $env:VITALSTREAM_ROOT = $Root
    # its own target folder: the workspace's (target\release, Medorama's viewer) keeps its build;
    # --target-dir wins over a CARGO_TARGET_DIR or build.target-dir set elsewhere
    $Target = Join-Path $Repo 'target\standalone'
    try {
      # the features: password and wfdb by default; -NoPassword and -NoWfdb leave one out
      $features = @(@(if (-not $NoPassword) { 'password' }) + @(if (-not $NoWfdb) { 'wfdb' })) -join ','
      $featureArgs = if ($features) { @('--features', $features) } else { @() }
      Run cargo build --release --locked -p vitalstream-rs --target-dir $Target --no-default-features @featureArgs
    } finally { Pop-Location; Remove-Item Env:VITALSTREAM_ROOT }
    Copy-Item (Join-Path $Repo 'target\standalone\release\vitalstream-rs.exe') $Bin -Force
    $Program = 'vitalstream-rs'
  }
  'py' {
    $Venv = Join-Path $Prefix 'venv'
    Run python -m venv $Venv
    Run (Join-Path $Venv 'Scripts\python.exe') -m pip install --quiet --upgrade pip
    $env:VITALSTREAM_ROOT = $Root
    $env:VITALSTREAM_PASSWORD = if ($NoPassword) { '0' } else { '1' }
    $package = (Join-Path $Here 'python\vitalstream') + $(if ($NoWfdb) { '' } else { '[wfdb]' })
    try { Run (Join-Path $Venv 'Scripts\pip.exe') install --quiet --force-reinstall $package }
    finally { Remove-Item Env:VITALSTREAM_ROOT, Env:VITALSTREAM_PASSWORD }
    Copy-Item (Join-Path $Venv 'Scripts\vitalstream-py.exe') $Bin -Force
    $Program = 'vitalstream-py'
  }
}
# the program installed, and one that runs
if (-not (Test-Path (Join-Path $Bin "$Program.exe") -PathType Leaf)) { throw "$(Join-Path $Bin "$Program.exe") is missing" }
Write-Host "Installed $(Join-Path $Bin "$Program.exe") (root: $Root$(if ($NoPassword) { '; no password support' })$(if ($NoWfdb) { '; no WFDB support' }))"
if ($NoPassword) { Write-Host "Start it with: $Program" }
else { Write-Host "Start it with: $Program   (a password for shared machines: $Program --set-password)" }
