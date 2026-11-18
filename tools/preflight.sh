# What building Medorama needs, checked before anything is built (tools/install-medorama.sh,
# tools/install.sh): Python 3.9 or later ($PY); for a Rust build (`preflight rust`), make and a Perl
# with IPC::Cmd too (OpenSSL's build).
# Every problem is told at once, with what to do; without administrator rights, a Python from uv
# and a relocatable Perl replace an old Python and a minimal Perl (docs/install.md, "Linux without
# administrator rights").  Sourced: `preflight` exits when something is missing.
preflight() {
  local bad=0 noadmin="docs/install.md, \"Linux without administrator rights (an old Python, a minimal Perl)\""
  if ! command -v "$PY" >/dev/null || ! "$PY" -c 'import sys; sys.exit(sys.version_info < (3, 9))' 2>/dev/null; then
    bad=1
    echo "Python 3.9 or later is needed ($("$PY" -V 2>&1 || echo "$PY not found"))."
    echo "  With administrator rights: install python3 (3.9 or later). Without, a Python from uv:"
    echo "    command -v uv || { curl -LsSf https://astral.sh/uv/install.sh | sh; source \$HOME/.local/bin/env; }"
    echo "    uv python install 3.12"
    echo "  then run this installer again with PYTHON=\"\$(uv python find 3.12)\" before it ($noadmin)."
  fi
  # (the Python packages alone, or the C++ viewer, need neither make nor Perl)
  if [ "${1:-}" = rust ] && ! command -v make >/dev/null; then
    bad=1
    echo "make is missing (OpenSSL's build needs it): sudo dnf install make, or sudo apt install make;"
    echo "  without administrator rights, see docs/building.md, \"When the build fails\"."
  fi
  if [ "${1:-}" = rust ] && { ! command -v perl >/dev/null || ! perl -MIPC::Cmd -e 1 2>/dev/null; }; then
    bad=1
    if command -v perl >/dev/null; then
      echo "Perl lacks IPC::Cmd, which OpenSSL's build needs (a minimal Perl: RHEL 8, Rocky 8)."
    else
      echo "Perl is missing (OpenSSL's build needs it)."
    fi
    if [ "$(uname)" = Linux ]; then
      echo "  With administrator rights: sudo dnf install perl-core (or perl-IPC-Cmd), or sudo apt install perl."
      echo "  Without, a relocatable Perl in ~/opt, in this terminal:"
      echo "    mkdir -p ~/opt"
      echo "    curl -fsSL https://github.com/skaji/relocatable-perl/releases/latest/download/perl-linux-amd64.tar.xz | tar xJf - -C ~/opt"
      echo "    export PATH=~/opt/perl-linux-amd64/bin:\$PATH"
      echo "    unset PERL5LIB PERL_LOCAL_LIB_ROOT"
      echo "  then run this installer again ($noadmin)."
    fi
  fi
  [ "$bad" = 0 ] || exit 1
}
