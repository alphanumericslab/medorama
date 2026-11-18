# Install

Copy-and-paste installation of Medorama and of each viewer on Linux, macOS and Windows. Every
example starts from nothing and ends with the program running. Everything is built from a clone
of the repository, in your user account; administrator rights are needed only to install a
compiler, once.

Each installer first shows Medorama's license terms and goes on only if you accept them by typing
`y` or `yes`. Medorama is free for noncommercial use (personal research and study, universities,
public research and public health organizations); commercial use needs a license from the author
([COMMERCIAL.md](../COMMERCIAL.md)). For scripts and CI, `--accept-license` (Windows:
`-AcceptLicense`) or `MEDORAMA_ACCEPT_LICENSE=yes` accepts them without asking
([building.md, License terms](building.md#license-terms)). The ECG and PACS viewers install with
`pip`, which asks nothing; the same terms apply to them.

- [Which one do I need?](#which-one-do-i-need)
- [Medorama](#medorama)
- [VitalStream, on its own](#vitalstream-on-its-own)
- [The ECG viewer, on its own](#the-ecg-viewer-on-its-own)
- [The PACS viewer, on its own](#the-pacs-viewer-on-its-own)
- [Upgrading from 0.1.0](#upgrading-from-010)
- [Next steps](#next-steps)

## Which one do I need?

| You want to | Install | Needs |
|---|---|---|
| Bring patients' data from several databases together, build cohorts, and open their recordings, ECGs and images, for a team | [Medorama](#medorama), which includes all the viewers and db-builder | Git, Python, a C compiler and Perl; Rust is installed for you |
| Read long physiological recordings (ICU monitoring, Holter, sleep studies) on one computer or server | [VitalStream](#vitalstream-on-its-own) | The same, or Python alone for its Python version |
| Read 12-lead ECGs | Nothing: [open the ECG viewer](https://medorama.org/viewers/ecg/) in a browser. To serve the ECGs of a server, [`muse-py serve`](#the-ecg-viewer-on-its-own) | Python, for `muse-py serve` |
| Read DICOM studies (echo, angiography, CT, MR, X-ray) | Nothing: [open the PACS viewer](https://medorama.org/viewers/pacs/) in a browser. To serve the studies of a server, [`dicom-py serve`](#the-pacs-viewer-on-its-own) | Python, for `dicom-py serve` |

The ECG and PACS viewers read the files you open in your browser: the files never leave your
computer, even on the website.

## Medorama

The installer builds Medorama, db-builder, one VitalStream waveform viewer (here the Rust
viewer, built with the same tools as Medorama) and the database tools into a `.venv` folder inside the
clone. `--demo` also makes the demo, six databases of invented patients, for trying it; leave it
out for a Medorama that starts with no database, ready for real data. The first build takes a few
minutes: it compiles SQLCipher and OpenSSL, which needs a C compiler and Perl.

Medorama imports nothing: it connects databases and reads them in place. After the installation
come three steps:
1. `medorama init --workspace DIR` (once) sets Medorama up: its settings, its control store, its
   master key, the first administrator, and the workspace where it keeps what it generates,
   encrypted. With `--demo`, it makes a *demo installation* instead, which connects the demo's
   databases and only demo databases; run it with its own `--home`, workspace and port, apart
   from a site's installation ([design/demo.md](design/demo.md)).
2. `medorama databases connect …` (or, in the page, Databases › **Connect a database**) connects
   each database: an OMOP database, any SQL database through views, or a SQLite file made by
   **db-builder** from tables, exports (GE MUSE, CLIF) and folders of recordings
   ([db-builder's README](../db-builder/README.md)).
3. `medorama serve` runs the page.

### Linux

On Debian or Ubuntu (on Fedora or RHEL: `sudo dnf install git python3 gcc perl`):

```bash
sudo apt install git python3 python3-venv build-essential perl   # once: Git, Python, a C compiler and Perl
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
tools/install-medorama.sh --viewer rs --demo                 # Medorama, db-builder, the Rust viewer and the demo
source .venv/bin/activate                                    # in every new terminal
medorama --home ~/medorama-demo init --workspace ~/medorama-demo-workspace --demo   # once: asks for the admin password; a demo installation
medorama --home ~/medorama-demo serve            # then open http://127.0.0.1:8095 and sign in as admin
```

### Linux without administrator rights (an old Python, a minimal Perl)

Use this when the steps above stop with one of these messages and you cannot install system
packages (no `sudo`), as on a shared server with RHEL 8 or Rocky 8:

- `Python 3.9 or later is needed (Python 3.6.8)`: the system's Python is too old;
- `Perl lacks IPC::Cmd, which OpenSSL's build needs` (or, in a build by hand,
  `Can't locate IPC/Cmd.pm in @INC` while building `openssl-sys`): the system's Perl lacks the
  modules OpenSSL's build needs.

The installers check Python, and for a Rust build `make` and Perl, before they build anything
([tools/preflight.sh](../tools/preflight.sh)), tell every problem at once, and print these steps
for what is missing; then they stop. A recent Python from [uv](https://docs.astral.sh/uv/) and a complete Perl in your home folder
replace them, for the installer alone. Run these in one terminal, in your medorama clone.

**uv**, if `uv --version` does not answer (uv's own installer, into `~/.local/bin`):

```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
source $HOME/.local/bin/env
```

**A recent Python:**

```bash
uv python install 3.12
```

**A complete Perl** (a relocatable Perl, in `~/opt`):

```bash
mkdir -p ~/opt
curl -fsSL https://github.com/skaji/relocatable-perl/releases/latest/download/perl-linux-amd64.tar.xz | tar xJf - -C ~/opt
export PATH=~/opt/perl-linux-amd64/bin:$PATH
unset PERL5LIB PERL_LOCAL_LIB_ROOT
perl -MIPC::Cmd -e 'print "IPC::Cmd ok\n"'           # should print: IPC::Cmd ok
```

**Install, with that Python:**

```bash
PYTHON="$(uv python find 3.12)" tools/install-medorama.sh --viewer rs --accept-license
source .venv/bin/activate
medorama --version
```

- `--viewer rs` builds the Rust waveform viewer. Add `--demo` for the demo, as above.
- A `BuildBackendWarning: Compat editable installs` message during "The database tools" is
  harmless.
- The Perl is needed only while building: Medorama does not use it once built. For a later
  update, run the `export PATH=…` and `unset …` lines again before the installer.

### macOS

```bash
xcode-select --install                                       # once: Git, Python, the compilers (Perl is part of macOS)
git clone https://github.com/alphanumericslab/medorama.git && cd medorama
tools/install-medorama.sh --viewer rs --demo
source .venv/bin/activate                                    # in every new terminal
medorama --home ~/medorama-demo init --workspace ~/medorama-demo-workspace --demo
medorama --home ~/medorama-demo serve            # then open http://127.0.0.1:8095
```

### Windows 10 and 11

In PowerShell. Open a new PowerShell window after the `winget` lines, so that Git, Python and
Perl are found:

```powershell
winget install Git.Git
winget install Python.Python.3.12
winget install StrawberryPerl.StrawberryPerl
winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then:

```powershell
git clone https://github.com/alphanumericslab/medorama.git; cd medorama
powershell -ExecutionPolicy Bypass -File tools\install-medorama.ps1 -Viewer rs -Demo
.venv\Scripts\Activate.ps1                                   # in every new PowerShell window
medorama --home $HOME\medorama-demo init --workspace $HOME\medorama-demo-workspace --demo
medorama --home $HOME\medorama-demo serve        # then open http://127.0.0.1:8095
```

If PowerShell answers "running scripts is disabled on this system" for `Activate.ps1`, run
`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` once.

### Your own databases

Real databases go into a site's installation, set up without `--demo`; a demo installation
refuses them, and a site's installation refuses the demo's. It starts with no database. Set it
up, then connect one as an administrator, from the page (Databases › **Connect a database**) or
from the command line, for example:

```bash
medorama init --workspace ~/medorama-workspace              # once: a site's installation, the default home
medorama databases connect omop --kind omop --url "postgres://reader@omop.example.org/cdm?sslmode=require" \
    --password-file ~/.omop-password --mrn-authority HOSP
db-builder add ~/study.sqlite patients.csv labs.xlsx --issuer HOSP --mrn-authority HOSP   # tables made into a database
medorama databases connect study --kind dbb --url sqlite:$HOME/study.sqlite
```

[Administration, Connected databases](administration.md#connected-databases) explains every
option, and [databases.md](databases.md) what a database must provide.

### Options

| Linux and macOS | Windows | Effect |
|---|---|---|
| `--viewer cpp` (the default), `rs`, `py`, `none` | `-Viewer` | The waveform viewer to build with Medorama. The C++ viewer also needs a C++20 compiler (`g++` on Linux; the Command Line Tools on macOS; the Build Tools above on Windows). |
| `--demo` | `-Demo` | Makes the demo's databases of invented patients, which `medorama init --demo` connects in a demo installation of its own ([Get started](getting-started.md) tours them and shows how to remove them). |
| `--no-tools` | `-NoTools` | Leaves out the database tools `muse-py` and `dicom-py`. |
| `--no-wfdb` | `-NoWfdb` | Builds without WFDB support, with no wfdb-x code ([Without WFDB](building.md#without-wfdb)); not with the demo. |
| `--accept-license` | `-AcceptLicense` | Accepts the license terms without asking (scripts, CI). |

To serve Medorama to other people, read [Administration](administration.md#deployment) before
going further: it covers HTTPS, institutional sign-in, grants, keys, PostgreSQL, services and
backups.

## VitalStream, on its own

VitalStream reads WFDB, EDF and BDF recordings, and other formats through add-ons. It has three
versions with the same page and features: `vitalstream-cpp` (C++), `vitalstream-rs` (Rust) and
`vitalstream-py` (Python). The examples install the Rust version; change `--viewer rs` to
`--viewer cpp` or `--viewer py` for the others. The Python version needs only Python, and no
compiler. The Rust version compiles SQLCipher and OpenSSL for its index store, so it needs Perl
as well as a C compiler; the C++ version builds SQLCipher without OpenSSL and needs no Perl.

### Linux and macOS

Install the prerequisites as for [Medorama](#linux) (`sudo apt install git python3 python3-venv
build-essential perl` on Debian or Ubuntu, `xcode-select --install` on macOS), then:

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
| `--accept-license` | `-AcceptLicense` | Accepts the license terms without asking (scripts, CI). |

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

## Upgrading from 0.1.0

Medorama 0.2.0, a pre-release, changes how Medorama holds data, so a 0.1.0 installation is not
updated in place: 0.2.0 is set up in a new home, next to the old one.

- **Medorama no longer imports data.** It connects databases: OMOP, any SQL database through
  views, or a SQLite database made with [db-builder](../db-builder/README.md) from the tables,
  exports (GE MUSE, CLIF) and folders of recordings that 0.1.0 imported. Make those databases
  again from the original files, then connect them.
- **The control store starts again.** Its schema is new, with no migration. 0.2.0 refuses a
  0.1.0 control store and a 0.1.0 `medorama.toml`, and says so.
- **A workspace is required** (`medorama init --workspace DIR`). It holds what Medorama keeps of
  each database, the recordings' indexes included, encrypted with that database's own key. The
  setting `index_dir` is gone; the recordings are indexed again.
- **Users, grants and cohorts are not carried over.** Add the users again, grant them the
  databases, and build the cohorts again.
- **Keep the 0.1.0 home** (with its audit log and seals) for as long as your retention rules
  require; 0.2.0 never writes to it.

On Linux or macOS, from the existing clone:

```bash
cd medorama && git pull
tools/install-medorama.sh --viewer rs          # the same options as before
source .venv/bin/activate
medorama init --home ~/medorama-0.2 --workspace ~/medorama-workspace   # a new home; becomes the default
db-builder new study.sqlite --timezone Europe/Paris
db-builder add study.sqlite patients.csv labs.csv --issuer HOSP --mrn-authority HOSP
db-builder recordings study.sqlite /data/icu --patients icu_patients.csv --issuer HOSP
medorama databases connect study --kind dbb --url sqlite:$PWD/study.sqlite
medorama user add alice --role analyst
medorama databases grant study alice view
medorama serve
```

[Databases](databases.md) explains each kind of database, and [db-builder](../db-builder/README.md)
every way to build one. The standalone viewers keep their settings; their indexes are now in one
store, `index.sqlite` in the cache folder (`~/.cache/vitalstream`), so the index folders 0.1.0
left there can be deleted. Every change is listed in [medorama/CHANGELOG.md](../medorama/CHANGELOG.md).

## Next steps

| | |
|---|---|
| [Get started](getting-started.md) | A guided tour of Medorama with the demo, and how to remove the demo. |
| [Building and installing](building.md) | Every build option, machines without administrator rights or internet access, updating and uninstalling, and troubleshooting. |
| [Administration](administration.md) | Running Medorama for other people: settings, connected databases, grants, keys, users, the audit log, deployment and backups. |
| [db-builder](../db-builder/README.md) | Tables, known exports and folders of recordings made into a database to connect. |
| [User's manual](user-manual/medorama-user-manual.pdf) | The complete manual, installation included. |
