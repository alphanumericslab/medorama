# Install

Copy-and-paste installation of Medorama and of each viewer on Linux, macOS and Windows. Every
example starts from nothing and ends with the program running. Everything is built from a clone
of the repository, in your user account; administrator rights are needed only to install a
compiler, once.

- [Which one do I need?](#which-one-do-i-need)
- [Medorama](#medorama)
- [VitalStream, on its own](#vitalstream-on-its-own)
- [The ECG viewer, on its own](#the-ecg-viewer-on-its-own)
- [The PACS viewer, on its own](#the-pacs-viewer-on-its-own)
- [Next steps](#next-steps)

## Which one do I need?

| You want to | Install | Needs |
|---|---|---|
| Bring patients' data from several databases together, build cohorts, and open their recordings, ECGs and images, for a team | [Medorama](#medorama), which includes all the viewers | Git, Python, a C compiler; Rust is installed for you |
| Read long physiological recordings (ICU monitoring, Holter, sleep studies) on one computer or server | [VitalStream](#vitalstream-on-its-own) | The same, or Python alone for its Python version |
| Read 12-lead ECGs | Nothing: [open the ECG viewer](https://medorama.org/viewers/ecg/) in a browser. To serve the ECGs of a server, [`muse-py serve`](#the-ecg-viewer-on-its-own) | Python, for `muse-py serve` |
| Read DICOM studies (echo, angiography, CT, MR, X-ray) | Nothing: [open the PACS viewer](https://medorama.org/viewers/pacs/) in a browser. To serve the studies of a server, [`dicom-py serve`](#the-pacs-viewer-on-its-own) | Python, for `dicom-py serve` |

The ECG and PACS viewers read the files you open in your browser: the files never leave your
computer, even on the website.

## Medorama

The installer builds Medorama, one VitalStream waveform viewer (here the Rust viewer, which needs
nothing beyond Rust) and the database tools into a `.venv` folder inside the clone. `--demo`
adds six invented patients for trying it; leave it out for a catalog that starts empty, ready
for real data. The first build takes a few minutes.

### Linux

On Debian or Ubuntu (on Fedora or RHEL: `sudo dnf install git python3 gcc`):

```bash
sudo apt install git python3 python3-venv build-essential    # once: Git, Python and a C compiler
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
tools/install-medorama.sh --viewer rs --demo                 # Medorama, the Rust viewer and the demo
source .venv/bin/activate                                    # in every new terminal
medorama init --results ~/medorama-results                   # once: creates the catalog; asks for the admin password
medorama serve                                               # then open http://127.0.0.1:8090 and sign in as admin
```

### macOS

```bash
xcode-select --install                                       # once: Git, Python and the compilers
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
tools/install-medorama.sh --viewer rs --demo
source .venv/bin/activate                                    # in every new terminal
medorama init --results ~/medorama-results
medorama serve                                               # then open http://127.0.0.1:8090
```

### Windows 10 and 11

In PowerShell. Open a new PowerShell window after the `winget` lines, so that Git and Python are
found:

```powershell
winget install Git.Git
winget install Python.Python.3.12
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then:

```powershell
git clone https://github.com/alphanumericslab/medorama.git; cd medorama
powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1 -Viewer rs -Demo
.venv\Scripts\Activate.ps1                                   # in every new PowerShell window
medorama init --results $HOME\medorama-results
medorama serve                                               # then open http://127.0.0.1:8090
```

If PowerShell answers "running scripts is disabled on this system" for `Activate.ps1`, run
`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` once.

### Options

| Linux and macOS | Windows | Effect |
|---|---|---|
| `--viewer cpp` (the default), `rs`, `py`, `none` | `-Viewer` | The waveform viewer to build with Medorama. The C++ viewer also needs a C++20 compiler (`g++` on Linux; the Command Line Tools on macOS; the Build Tools above on Windows). |
| `--demo` | `-Demo` | Adds the demo's invented patients ([Get started](getting-started.md) tours them and shows how to remove them). |
| `--no-tools` | `-NoTools` | Leaves out the database tools `muse-py` and `dicom-py`. |
| `--no-wfdb` | `-NoWfdb` | Builds without WFDB support, with no wfdb-x code ([Without WFDB](building.md#without-wfdb)); not with the demo. |

To serve Medorama to other people, read [Administration](administration.md#deployment) before
going further: it covers HTTPS, institutional sign-in, PostgreSQL, services and backups.

## VitalStream, on its own

VitalStream reads WFDB, EDF and BDF recordings, and other formats through add-ons. It has three
versions with the same page and features: `vitalstream-cpp` (C++), `vitalstream-rs` (Rust) and
`vitalstream-py` (Python). The examples install the Rust version; change `--viewer rs` to
`--viewer cpp` or `--viewer py` for the others. The Python version needs only Python, and no
compiler.

### Linux and macOS

Install the prerequisites as for [Medorama](#linux) (`sudo apt install git python3 python3-venv
build-essential` on Debian or Ubuntu, `xcode-select --install` on macOS), then:

```bash
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
viewers/vitalstream/install.sh --viewer rs                   # installs ~/.local/bin/vitalstream-rs
vitalstream-rs                                               # opens http://127.0.0.1:8070 in your browser
```

If the installer says `Add ~/.local/bin to PATH`, run the program as `~/.local/bin/vitalstream-rs`,
or add `export PATH="$HOME/.local/bin:$PATH"` to your shell's startup file.

### Windows 10 and 11

Install the prerequisites as for [Medorama](#windows-10-and-11), then in PowerShell:

```powershell
git clone https://github.com/alphanumericslab/medorama.git; cd medorama
powershell -ExecutionPolicy Bypass -File viewers\vitalstream\install.ps1 -Viewer rs
& "$env:LOCALAPPDATA\vitalstream\bin\vitalstream-rs.exe"    # opens http://127.0.0.1:8070
```

For the C++ version (`-Viewer cpp`), run the installer from a "Developer PowerShell for VS".

### Options

| Linux and macOS | Windows | Effect |
|---|---|---|
| `--viewer cpp` (the default), `rs`, `py` | `-Viewer` | The version to build. |
| `--root DIR` | `-Root DIR` | The folder the viewer may open at most, fixed in the program (default: the home folder of whoever runs it). |
| `--no-password` | `-NoPassword` | Builds it without password support. |
| `--no-wfdb` | `-NoWfdb` | Builds it without WFDB support, with no wfdb-x code. |

On a computer that other people can reach, set a password once with `vitalstream-rs
--set-password`. [VitalStream's README](../viewers/vitalstream/README.md#deploy) shows how to
run it on a server, through an SSH tunnel or behind an HTTPS proxy, and as a service.

## The ECG viewer, on its own

The ECG viewer draws 12-lead ECGs (WFDB, GE MUSE XML, DICOM, EDF+, PDF and pictures) on standard
paper. It needs no installation: [open it on the website](https://medorama.org/viewers/ecg/), or
open `viewers/ecg/index.html` from a clone. To read the ECGs stored on a computer or server
without copying them, serve them with `muse-py serve`, which needs only Python.

### Linux and macOS

```bash
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
python3 -m venv ~/.venvs/ecg && source ~/.venvs/ecg/bin/activate
pip install ./viewers/ecg/python/muse
muse-py serve --root ~/ecgs                                  # opens http://127.0.0.1:8080/browse
```

### Windows 10 and 11

```powershell
git clone https://github.com/alphanumericslab/medorama.git; cd medorama
py -m venv $HOME\.venvs\ecg; & $HOME\.venvs\ecg\Scripts\Activate.ps1
pip install .\viewers\ecg\python\muse
muse-py serve --root $HOME\ecgs                              # opens http://127.0.0.1:8080/browse
```

`--root` is the folder served (default: your home folder). On a shared computer, set a password
first with `muse-py serve --set-password`. Medorama's installer already puts `muse-py` into its
`.venv`. The [ECG viewer's README](../viewers/ecg/README.md#run-it) describes every way to run it.

## The PACS viewer, on its own

The PACS viewer plays DICOM studies: echo and angiography loops, stacks of CT and MR slices,
radiographs and structured reports. It needs no installation:
[open it on the website](https://medorama.org/viewers/pacs/), or open `viewers/pacs/index.html`
from a clone. To read the studies stored on a computer or server without copying them, serve
them with `dicom-py serve`, which needs Python.

### Linux and macOS

```bash
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
python3 -m venv ~/.venvs/pacs && source ~/.venvs/pacs/bin/activate
pip install ./viewers/pacs/python/dicom
dicom-py serve --root ~/studies                              # opens http://127.0.0.1:8081/browse
```

### Windows 10 and 11

```powershell
git clone https://github.com/alphanumericslab/medorama.git; cd medorama
py -m venv $HOME\.venvs\pacs; & $HOME\.venvs\pacs\Scripts\Activate.ps1
pip install .\viewers\pacs\python\dicom
dicom-py serve --root $HOME\studies                          # opens http://127.0.0.1:8081/browse
```

As for the ECG viewer, `--root` is the folder served and `dicom-py serve --set-password` sets a
password; Medorama's installer already puts `dicom-py` into its `.venv`. The
[PACS viewer's README](../viewers/pacs/README.md#run-it) describes every way to run it.

## Next steps

| | |
|---|---|
| [Get started](getting-started.md) | A guided tour of Medorama with the demo, and how to remove the demo. |
| [Building and installing](building.md) | Every build option, machines without administrator rights or internet access, updating and uninstalling, and troubleshooting. |
| [Administration](administration.md) | Running Medorama for other people: settings, PostgreSQL, imports, users, the audit log, deployment and backups. |
| [User's manual](user-manual) | The complete manual, installation included. |
