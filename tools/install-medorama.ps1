<#
.SYNOPSIS
  Medorama with ONE VitalStream waveform viewer, nothing else, on Windows (the Windows counterpart of
  tools/install-medorama.sh).

.DESCRIPTION
  Builds Medorama (medorama/, Rust) and the viewer you choose, and copies both into
  .venv\Scripts, where Medorama finds the viewer; the choice is recorded in
  .venv\Scripts\medorama.viewer (docs/administration.md, "The waveform viewer").  Nothing
  else is built.
    -Viewer cpp    the C++ viewer vitalstream-cpp (default): CMake and Ninja into .venv, MSVC
    -Viewer rs     the Rust viewer vitalstream-rs
    -Viewer py     the Python viewer vitalstream-py and its packages
    -Viewer none   Medorama alone
  The database tools, which build databases to import and the demo's DICOM examples, go into
  .venv too: muse-py (GE MUSE ECGs) and dicom-py (DICOM studies, with pydicom); -NoTools leaves
  them out.

  -Demo also makes the demo: invented patients that `medorama init` loads into a cohort DEMO,
  for trying Medorama (off by default; docs/administration.md, "The demo").

  -NoWfdb builds Medorama and the viewer without WFDB support: no wfdb-x code in them (a WFDB
  recording is refused, not added; every other format is read; not with -Demo, whose recordings
  are WFDB records; docs/building.md, "Without WFDB").

  Prerequisites: Python 3.9 or later, and the Microsoft C++ Build Tools (workload "Desktop
  development with C++"), which Rust and C++ both need.  Rust is installed in your user account
  if cargo is missing (-NoRustup to refuse).  Licensed recording formats need their
  add-on, installed afterwards with `medorama addons install PROGRAM` (docs/adapters.md).  For
  every implementation and the tests, use tools\install.ps1 instead.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1
  powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1 -Viewer rs
  powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1 -Demo
#>
param(
  [ValidateSet('cpp', 'rs', 'py', 'none')]
  [string]$Viewer = 'cpp',
  [switch]$NoRustup,
  [switch]$NoTools,
  [switch]$Demo,
  [switch]$NoWfdb
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }

# Runs a program and stops on a non-zero exit code (PowerShell does not, for programs).
function Run {
  $exe = $args[0]
  $rest = @($args | Select-Object -Skip 1)
  & $exe @rest
  if ($LASTEXITCODE -ne 0) { throw "failed ($LASTEXITCODE): $exe $($rest -join ' ')" }
}

# The MSVC compiler and linker: returns the Visual Studio installation folder, or stops.
function Find-VisualCpp {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  $vs = if (Test-Path $vswhere) { & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 Microsoft.VisualStudio.Component.VC.Tools.ARM64 -requiresAny -property installationPath }
  if (-not $vs) {
    throw ("The Microsoft C++ Build Tools are missing (Rust needs their linker, C++ their compiler). Install them, then rerun:`n" +
      "  winget install Microsoft.VisualStudio.2022.BuildTools --override `"--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`"`n" +
      "or from https://visualstudio.microsoft.com/visual-cpp-build-tools/ (workload 'Desktop development with C++').")
  }
  return $vs
}

# --- .venv: where the programs go (and, for some viewers, their tools) ---------------------
# Whether a Python command runs and is 3.9 or later.  Its errors (the Store's `python` stub, a
# `py` without Python 3) are not this script's: PowerShell 5.1 under 'Stop' would end the script
# on the first line a program writes to stderr, even with 2>$null.
function Test-Python($c) {
  $ErrorActionPreference = 'Continue'
  & $c[0] @($c[1..($c.Length)] | Where-Object { $_ }) -c "import sys; sys.exit(sys.version_info < (3, 9))" 2>$null
  return ($LASTEXITCODE -eq 0)
}
$py = $null
foreach ($c in @(@('py', '-3'), @('python'), @('python3'))) {
  if ((Get-Command $c[0] -ErrorAction SilentlyContinue) -and (Test-Python $c)) { $py = $c; break }
}
if (-not $py) {
  throw "Python 3.9 or later not found: install it (https://www.python.org/downloads/ or 'winget install Python.Python.3.12'), then rerun."
}
$pyArgs = @($py[1..($py.Length)] | Where-Object { $_ })
Step "Environment .venv (the programs are copied into .venv\Scripts)"
if (-not (Test-Path .venv)) { Run $py[0] @pyArgs -m venv .venv }
$vpy = Join-Path $Root '.venv\Scripts\python.exe'
$vpip = Join-Path $Root '.venv\Scripts\pip.exe'
Run $vpy -m pip install --quiet --upgrade pip
$commands = ''

# --- Rust (Medorama; the Rust viewer) ---------------------------------------------------
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if ($env:Path -notlike "*$cargoBin*") { $env:Path = "$cargoBin;$env:Path" }
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  if ($NoRustup) { throw "cargo not found and -NoRustup given: install Rust (https://rustup.rs)" }
  Step "Installing Rust in your user account (rustup; %USERPROFILE%\.rustup and .cargo)"
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
  $init = Join-Path $env:TEMP 'rustup-init.exe'
  $url = "https://static.rust-lang.org/rustup/dist/$arch-pc-windows-msvc/rustup-init.exe"
  Invoke-WebRequest -UseBasicParsing $url -OutFile $init
  # the published SHA-256, from the same server
  Invoke-WebRequest -UseBasicParsing "$url.sha256" -OutFile "$init.sha256"
  $expected = ((Get-Content -Raw "$init.sha256").Trim() -split '\s+')[0]
  Remove-Item "$init.sha256"
  $actual = (Get-FileHash $init -Algorithm SHA256).Hash
  if (-not $expected -or $actual -ne $expected) {
    Remove-Item $init
    throw "rustup-init.exe's SHA-256 ($actual) is not the published one ('$expected'): not running it."
  }
  Run $init -y --profile minimal --default-toolchain stable --no-modify-path
}
$vs = Find-VisualCpp
# the builds in this clone's target\, where they are copied from (not CARGO_TARGET_DIR or a
# build.target-dir set elsewhere)
$env:CARGO_TARGET_DIR = Join-Path $Root 'target'

Step "Building Medorama (medorama)"
if ($NoWfdb -and $Demo) { throw "-Demo needs WFDB support (the demo's recordings are WFDB records): leave out -NoWfdb or -Demo" }
$noWfdbArgs = if ($NoWfdb) { @('--no-default-features') } else { @() }
Run cargo build --release --locked -p medorama @noWfdbArgs
Copy-Item 'target\release\medorama.exe' '.venv\Scripts\medorama.exe' -Force
$commands = 'medorama'

# --- the viewer ----------------------------------------------------------------------------
switch ($Viewer) {
  'cpp' {
    Step "CMake and Ninja (into .venv)"
    Run $vpip install --quiet 'cmake>=3.24' ninja
    $env:Path = "$(Join-Path $Root '.venv\Scripts');$env:Path"
    Step "Building the C++ viewer (vitalstream-cpp)"
    Import-Module (Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
    $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
    Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments "-arch=$arch -host_arch=$arch" | Out-Null
    # CMake keeps the compiler a build folder was first configured with: start afresh when
    # that is not this cl.exe.
    $cache = 'viewers\vitalstream\cpp\build\viewer\CMakeCache.txt'
    if (Test-Path $cache) {
      $line = Select-String -Path $cache -Pattern '^CMAKE_CXX_COMPILER:[A-Z]*=(.*)$' | Select-Object -First 1
      $cached = if ($line) { $line.Matches[0].Groups[1].Value } else { '' }
      $now = (Get-Command cl.exe -ErrorAction SilentlyContinue).Source
      $same = $cached -and $now -and (Test-Path $cached) -and ((Resolve-Path $cached).Path -ieq (Resolve-Path $now).Path)
      if (-not $same) {
        Write-Host "viewers\vitalstream\cpp\build\viewer was configured with '$cached'; now '$now': removing it to start afresh"
        Remove-Item -Recurse -Force 'viewers\vitalstream\cpp\build\viewer'
      }
    }
    Push-Location viewers\vitalstream\cpp
    try {
      # the whole file system as the viewer's root, and its password: Medorama's own rules apply
      Run cmake --preset viewer -DVS_ROOT= -DVS_PASSWORD=ON "-DVS_WFDB=$(if ($NoWfdb) { 'OFF' } else { 'ON' })"
      Run cmake --build --preset viewer
    } finally { Pop-Location }
    Copy-Item 'viewers\vitalstream\cpp\build\viewer\bin\vitalstream-cpp.exe' '.venv\Scripts\vitalstream-cpp.exe' -Force
    $commands += ' vitalstream-cpp'
  }
  'rs' {
    Step "Building the Rust viewer (vitalstream-rs)"
    Remove-Item Env:VITALSTREAM_ROOT -ErrorAction SilentlyContinue   # the whole file system as its root
    $rsArgs = if ($NoWfdb) { @('--no-default-features', '--features', 'password') } else { @() }
    Run cargo build --release --locked -p vitalstream-rs @rsArgs
    Copy-Item 'target\release\vitalstream-rs.exe' '.venv\Scripts\vitalstream-rs.exe' -Force
    $commands += ' vitalstream-rs'
  }
  'py' {
    Step "The Python viewer (vitalstream-py) and its packages"
    Remove-Item Env:CONDA_PREFIX, Env:VITALSTREAM_ROOT, Env:VITALSTREAM_PASSWORD -ErrorAction SilentlyContinue
    $package = './viewers/vitalstream/python/vitalstream' + $(if ($NoWfdb) { '' } else { '[wfdb]' })
    Run $vpip install --quiet -e $package --config-settings editable_mode=compat
    $commands += ' vitalstream-py'
  }
  'none' { }
}

# --- the database tools (muse-py, dicom-py; pydicom) ------------------------------------------
if (-not $NoTools) {
  Step "The database tools (muse-py, dicom-py)"
  Run $vpip install --quiet -e ./viewers/ecg/python/muse -e ./viewers/pacs/python/dicom --config-settings editable_mode=compat
  $commands += ' muse-py dicom-py'
}

# --- the demo (-Demo): invented patients, loaded by `medorama init` (docs\administration.md) --
$demoDir = Join-Path $Root '.venv\share\medorama\demo'
$demoFile = Join-Path $Root '.venv\Scripts\medorama.demo'
if ($Demo) {
  Step "The demo: invented patients and their files, in .venv\share\medorama\demo"
  # tools\make-demo-data.py writes its WFDB records with wfdb-x (the version the Python viewer
  # pins) and NumPy: installed when .venv lacks them (offline: docs\building.md)
  & $vpy -c "import importlib.util, sys; sys.exit(0 if all(importlib.util.find_spec(m) for m in ('numpy', 'wfdbx')) else 1)"
  if ($LASTEXITCODE -ne 0) {
    $wfdbx = (Select-String -Path 'viewers\vitalstream\python\vitalstream\pyproject.toml' -Pattern '"(wfdb-x @ [^"]*)"').Matches[0].Groups[1].Value
    Run $vpip install --quiet numpy $wfdbx
  }
  Run $vpy tools\make-demo-data.py $demoDir
  # `medorama init` loads the demo from the folder named here (UTF-8 without a byte-order mark)
  [System.IO.File]::WriteAllText($demoFile, $demoDir)
} elseif (Test-Path $demoFile) {
  # without -Demo, new catalogs start empty (a catalog that holds the demo keeps it)
  Remove-Item $demoFile
}

# The viewer Medorama starts (it reads this file next to itself; `viewer` in medorama.toml
# overrides it): the choice made here, even if other viewers are installed too.
Set-Content -Path '.venv\Scripts\medorama.viewer' -Value $Viewer -Encoding ascii

# --- git hooks (privacy checks before each commit) ------------------------------------------
# only in a clone of this repository (also a worktree), never in a repository around it
function Set-Hooks {
  $ErrorActionPreference = 'Continue'   # git's messages on stderr are not errors here
  $top = & git rev-parse --show-toplevel 2>$null
  if ($LASTEXITCODE -ne 0 -or -not $top -or
      (Resolve-Path $top).Path.TrimEnd('\', '/') -ine (Resolve-Path $Root).Path.TrimEnd('\', '/')) { return }
  $cloneHooks = & git config --local --get core.hooksPath 2>$null
  if ($cloneHooks -and $cloneHooks -ne 'tools/git-hooks') {
    Write-Warning ("core.hooksPath is '$cloneHooks' in this clone, so the privacy checks of tools\git-hooks do not run: " +
      "set it with 'git config core.hooksPath tools/git-hooks', or call tools/git-hooks/check-files from your hooks (PRIVACY.md)")
    return
  }
  $anyHooks = & git config --get core.hooksPath 2>$null
  if ($anyHooks -and $anyHooks -ne 'tools/git-hooks') {
    Write-Host "note: this clone now uses tools/git-hooks, not the hooks of your global core.hooksPath ('$anyHooks')"
  }
  & git config core.hooksPath tools/git-hooks
  if ($LASTEXITCODE -ne 0) { throw 'failed: git config core.hooksPath tools/git-hooks' }
}
if (Get-Command git -ErrorAction SilentlyContinue) { Set-Hooks }

# every program copied or installed
foreach ($c in $commands.Split(' ', [StringSplitOptions]::RemoveEmptyEntries)) {
  if (-not (Test-Path ".venv\Scripts\$c.exe")) { throw ".venv\Scripts\$c.exe is missing" }
}

Step "Done"
Write-Host "Installed in .venv\Scripts: $commands"
Write-Host "Next, in each new PowerShell window:  .venv\Scripts\Activate.ps1"
if ($Demo) {
  Write-Host "  medorama init --results DIR      (once; docs\administration.md): it loads the demo, the cohort DEMO"
} else {
  Write-Host "  medorama init --results DIR      (once; docs\administration.md)"
}
Write-Host "  medorama serve                   (http://127.0.0.1:8090)"
Write-Host "Licensed formats: medorama addons install PROGRAM   (docs\adapters.md)"
