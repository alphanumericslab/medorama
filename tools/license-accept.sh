# The license terms and their acceptance, shared by the installers (sourced, not run):
#   accept_license      shows tools/license-notice.txt and asks the user to accept it; stops the
#                       installer unless they type y or yes.
# Accepted without asking when ACCEPT_LICENSE=1 (the installers' --accept-license) or
# MEDORAMA_ACCEPT_LICENSE=yes, for scripts and CI; without a terminal and without either, the
# installer stops and says how to accept.  The acceptance is recorded in the user's settings
# folder (~/.config/medorama/license-accepted): it is not asked again until the terms change.
LICENSE_NOTICE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/license-notice.txt"
ACCEPT_LICENSE=${ACCEPT_LICENSE:-0}

accept_license() {
  local record="${XDG_CONFIG_HOME:-$HOME/.config}/medorama/license-accepted" hash how answer version
  hash=$( (command -v sha256sum >/dev/null && sha256sum || shasum -a 256) < "$LICENSE_NOTICE" | cut -c1-64)
  if [ -f "$record" ] && grep -qx "notice_sha256=$hash" "$record"; then
    echo "License terms accepted on $(sed -n 's/^date=//p' "$record") (LICENSE, COMMERCIAL.md)."
    return 0
  fi
  echo
  cat "$LICENSE_NOTICE"
  echo
  case "$(printf %s "${MEDORAMA_ACCEPT_LICENSE:-}" | tr '[:upper:]' '[:lower:]')" in
    y|yes) ACCEPT_LICENSE=1 ;;
  esac
  if [ "$ACCEPT_LICENSE" = 1 ]; then
    how=option
    echo "Accepted with --accept-license (or MEDORAMA_ACCEPT_LICENSE=yes)."
  elif [ -t 0 ]; then
    printf 'Do you accept these terms? Type y or yes to continue [no]: '
    read -r answer || answer=
    # letters only, in any locale: a terminal's stray carriage return, escape sequences (a
    # bracketed paste) or spaces around the answer do not refuse it
    case "$(printf %s "$answer" | LC_ALL=C tr -cd 'A-Za-z' | LC_ALL=C tr 'A-Z' 'a-z')" in
      y|yes) how=prompt ;;
      *) echo "Nothing was installed: the license terms were not accepted (the answer read: $(printf %q "$answer"))."
         echo "To accept them without the question, use --accept-license, or MEDORAMA_ACCEPT_LICENSE=yes."
         exit 1 ;;
    esac
  else
    echo "Nothing was installed: there is no terminal to ask whether you accept the license terms."
    echo "Accept them with --accept-license, or with MEDORAMA_ACCEPT_LICENSE=yes."
    exit 1
  fi
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$(dirname "$LICENSE_NOTICE")/../medorama/Cargo.toml" | head -1)
  mkdir -p "$(dirname "$record")"
  {
    echo "license=PolyForm-Noncommercial-1.0.0"
    echo "notice_sha256=$hash"
    echo "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "user=$(id -un)"
    echo "accepted_by=$how"
    echo "version=$version"
  } > "$record"
}
