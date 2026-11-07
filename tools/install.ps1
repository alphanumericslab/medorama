<#
.SYNOPSIS
  Medorama and the VitalStream viewers, for development: everything into this clone on Windows
  (no administrator rights needed for the programs themselves).

.DESCRIPTION
  The Windows counterpart of tools/install.sh.  Without -Py/-Rs/-Cpp: all of it.
    1. .venv\ with the Python packages, editable: the Python viewer vitalstream_py and the
       database tools muse_py and dicom_py, with the test tools.
    2. Rust (-Rs): installs rustup in your user account if cargo is missing (-NoRustup to
       refuse), builds the workspace (medorama, vitalstream-rs; copies in .venv\Scripts).
    3. C++ (-Cpp): installs CMake and Ninja into .venv, builds vitalstream-cpp and its tests
       with MSVC (copy in .venv\Scripts).
    4. Enables the repository's git hooks (privacy checks before each commit).
  -Demo also makes the demo: invented patients that `medorama init` loads into a cohort DEMO,
  for trying Medorama (off by default; docs/administration.md, "The demo").

  Prerequisites: Python 3.9 or later (python.org installer or `winget install Python.Python.3.12`)
  and, for Rust and C++, the Microsoft C++ Build Tools (workload "Desktop development with C++").
  See docs/building.md.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\install.ps1
  powershell -ExecutionPolicy Bypass -File tools\install.ps1 -Py
  powershell -ExecutionPolicy Bypass -File tools\install.ps1 -Cpp
  powershell -ExecutionPolicy Bypass -File tools\install.ps1 -Demo
#>
param(
  [switch]$Py,           # the Python packages only (unless -Rs or -Cpp is given too)
  [switch]$Rs,           # Python + Rust
  [switch]$Cpp,          # Python + C++
  [switch]$NoRustup,     # do not install Rust if cargo is missing
  [switch]$Demo          # also make the demo (invented patients that `medorama init` loads)
)
if (-not ($Py -or $Rs -or $Cpp)) { $Rs = $true; $Cpp = $true }

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }

# Runs a program and stops on a non-zero exit code (PowerShell does not, for programs).
# A simple function (no param block), so that arguments such as -m reach the program.
function Run {
  $exe = $args[0]
  $rest = @($args | Select-Object -Skip 1)
  & $exe @rest
  if ($LASTEXITCODE -ne 0) { throw "failed ($LASTEXITCODE): $exe $($rest -join ' ')" }
}

# --- Python -----------------------------------------------------------------------------
# Whether a Python command runs and is 3.9 or later.  Its errors (the Store's `python` stub, a
# `py` without Python 3) are not this script's: PowerShell 5.1 under 'Stop' would end the script
# on the first line a program writes to stderr, even with 2>$null.
function Test-Python($c) {
  $ErrorActionPreference = 'Continue'
  & $c[0] @($c[1..($c.Length)] | Where-Object { $_ }) -c "import sys; sys.exit(sys.version_info < (3, 9))" 2>$null
  return ($LASTEXITCODE -eq 0)
}
# the Python command ($python, not $py: PowerShell names ignore case, and -Py is a switch)
$python = $null
foreach ($c in @(@('py', '-3'), @('python'), @('python3'))) {
  if ((Get-Command $c[0] -ErrorAction SilentlyContinue) -and (Test-Python $c)) { $python = $c; break }
}
if (-not $python) {
  throw "Python 3.9 or later not found: install it (https://www.python.org/downloads/ or 'winget install Python.Python.3.12'), then rerun."
}
$pyArgs = @($python[1..($python.Length)] | Where-Object { $_ })

Step "Python environment (.venv) and packages"
if (-not (Test-Path .venv)) { Run $python[0] @pyArgs -m venv .venv }
$vpy = Join-Path $Root '.venv\Scripts\python.exe'
$vpip = Join-Path $Root '.venv\Scripts\pip.exe'
Run $vpy -m pip install --quiet --upgrade pip
Run $vpip install --quiet -e "./viewers/vitalstream/python/vitalstream[test,wfdb]" -e ./viewers/ecg/python/muse -e ./viewers/pacs/python/dicom --config-settings editable_mode=compat
Run $vpip install --quiet pytest httpx

$commands = 'vitalstream-py muse-py dicom-py'

# The MSVC compiler and linker (Microsoft C++ Build Tools, "Desktop development with C++"):
# returns the Visual Studio installation folder, or stops with instructions.
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

# --- Rust -------------------------------------------------------------------------------
function Need-Cargo {
  $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
  if ($env:Path -notlike "*$cargoBin*") { $env:Path = "$cargoBin;$env:Path" }
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    if ($NoRustup) { throw "cargo not found and -NoRustup given: install Rust (https://rustup.rs) or use -Py" }
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
  Find-VisualCpp | Out-Null
  # the builds in this clone's target\, where they are copied from (not CARGO_TARGET_DIR or a
  # build.target-dir set elsewhere)
  $env:CARGO_TARGET_DIR = Join-Path $Root 'target'
}
if ($Rs) {
  Need-Cargo
  Step "Building the Rust workspace (medorama, vitalstream-rs)"
  Run cargo build --release --locked
  foreach ($b in 'medorama', 'vitalstream-rs') {
    Copy-Item "target\release\$b.exe" ".venv\Scripts\$b.exe" -Force
  }
  $commands += ' medorama vitalstream-rs'
}

# --- C++ --------------------------------------------------------------------------------
if ($Cpp) {
  $vs = Find-VisualCpp
  Step "CMake and Ninja (into .venv)"
  Run $vpip install --quiet 'cmake>=3.24' ninja
  $env:Path = "$(Join-Path $Root '.venv\Scripts');$env:Path"
  Step "Building the C++ viewer (vitalstream-cpp) and its tests"
  # the MSVC environment (cl.exe, link.exe, the Windows SDK) for this session
  Import-Module (Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'amd64' }
  Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments "-arch=$arch -host_arch=$arch" | Out-Null
  # CMake keeps the compiler a build folder was first configured with (or none, after an attempt
  # outside a developer shell): start afresh when that is not this cl.exe.
  $cache = 'viewers\vitalstream\cpp\build\release\CMakeCache.txt'
  if (Test-Path $cache) {
    $line = Select-String -Path $cache -Pattern '^CMAKE_CXX_COMPILER:[A-Z]*=(.*)$' | Select-Object -First 1
    $cached = if ($line) { $line.Matches[0].Groups[1].Value } else { '' }
    $now = (Get-Command cl.exe -ErrorAction SilentlyContinue).Source
    $same = $cached -and $now -and (Test-Path $cached) -and
            ((Resolve-Path $cached).Path -ieq (Resolve-Path $now).Path)
    if (-not $same) {
      Write-Host "viewers\vitalstream\cpp\build\release was configured with '$cached'; now '$now': removing it to start afresh"
      Remove-Item -Recurse -Force 'viewers\vitalstream\cpp\build\release'
    }
  }
  Push-Location viewers\vitalstream\cpp
  try {
    Run cmake --preset release
    Run cmake --build --preset release
  } finally { Pop-Location }
  Copy-Item "viewers\vitalstream\cpp\build\release\bin\vitalstream-cpp.exe" ".venv\Scripts\vitalstream-cpp.exe" -Force
  $commands += ' vitalstream-cpp'
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

# --- git hooks ---------------------------------------------------------------------------
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
Write-Host "Commands in .venv\Scripts: $commands"
Write-Host "Activate with:  .venv\Scripts\Activate.ps1   (or call .venv\Scripts\<command>.exe)"
Write-Host "Check with:     cargo test --workspace   (docs/developing.md)"
if ($Demo) { Write-Host "Then:           medorama init --results DIR   (loads the demo: the cohort DEMO), medorama serve" }
