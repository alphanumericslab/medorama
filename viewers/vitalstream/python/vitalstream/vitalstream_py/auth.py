"""Optional password protection.

Set a password once with `vitalstream-py --set-password`.  It is kept as a salted scrypt hash,
with a random signing key, in an auth file readable only by you (default
~/.config/vitalstream/auth.json).  When a password is set, the page asks for it once per
browser; the login is a signed cookie that stays valid for LOGIN_DAYS, also across server
restarts.  Without a password the viewer needs no login.  The sign-ins ended by `logout` are
kept in the auth file (`revoked`: {signature: expiry}) until they would have expired, so that a
copy of a cookie stops working.

A build without password support (VITALSTREAM_PASSWORD=0 when the package is built, recorded in
the generated module _build.py; SUPPORTED is then false): no password can be set, the page never
asks for one, and the viewer refuses to start where a password is required (require_password,
which Medorama sets for the viewer it runs).  The functions are the same, so the server is the
same.
"""

import hashlib
import hmac
import json
import os
import secrets
import time

try:                                   # written by setup.py when the package is built
    from ._build import PASSWORD as SUPPORTED
except ImportError:                    # run from the source tree: with password support
    SUPPORTED = True

LOGIN_DAYS = 30
_SCRYPT = dict(n=2 ** 14, r=8, p=1, dklen=32)
_NONE = 'this viewer was built without password support'


def load(path):
    """The auth file's contents, or None when no password is set (always, in a build without
    password support)."""
    if not SUPPORTED:
        return None
    try:
        with open(os.path.expanduser(path), encoding='utf-8') as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    return data if data.get('hash') and data.get('secret') else None


def set_password(path, password):
    if not SUPPORTED:
        raise OSError(_NONE)
    path = os.path.expanduser(path)
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    salt = secrets.token_bytes(16)
    data = {'scheme': 'scrypt', 'salt': salt.hex(),
            'hash': hashlib.scrypt(password.encode(), salt=salt, **_SCRYPT).hex(),
            'secret': secrets.token_hex(32)}          # new key: earlier logins end
    _write_private(path, data)


def _write_private(path, data):
    """Write `data` (JSON) to `path` readable by its owner only, replacing it at once."""
    tmp = path + '.tmp'
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8', newline='\n') as f:
        json.dump(data, f)
    os.replace(tmp, path)
    os.chmod(path, 0o600)


def revoked(path):
    """The sign-ins ended by `logout` that have not expired (signature -> expiry)."""
    if not SUPPORTED:
        return {}
    try:
        with open(os.path.expanduser(path), encoding='utf-8') as f:
            data = json.load(f)
    except (OSError, ValueError, RecursionError):
        return {}
    r = data.get('revoked') if isinstance(data, dict) else None
    now = time.time()
    if not isinstance(r, dict):
        return {}
    return {k: e for k, e in r.items() if isinstance(e, int) and not isinstance(e, bool) and e > now}


def save_revoked(path, all_revoked):
    """Write the sign-ins ended by `logout` into the auth file (the expired ones left out), its
    other keys unchanged."""
    if not SUPPORTED:
        raise OSError(_NONE)
    path = os.path.expanduser(path)
    with open(path, encoding='utf-8') as f:
        data = json.load(f)
    if not isinstance(data, dict):
        raise OSError('not an auth file')
    now = time.time()
    data['revoked'] = {k: e for k, e in sorted(all_revoked.items()) if e > now}
    _write_private(path, data)


def clear_password(path):
    if not SUPPORTED:
        raise OSError(_NONE)
    try:
        os.remove(os.path.expanduser(path))
    except FileNotFoundError:
        pass


def check_password(auth, password):
    if not SUPPORTED:
        return False
    h = hashlib.scrypt(password.encode(), salt=bytes.fromhex(auth['salt']), **_SCRYPT).hex()
    return hmac.compare_digest(h, auth['hash'])


def make_cookie(auth, days=LOGIN_DAYS):
    if not SUPPORTED:
        return ''
    expires = int(time.time() + days * 86400)
    return f'{expires}.{_sign(auth, expires)}'


def cookie_ok(auth, value):
    if not SUPPORTED:
        return False
    try:
        expires, sig = str(value).split('.', 1)
        if not (expires.isascii() and expires.isdigit()):
            return False
        # bytes: a cookie of any characters is a wrong one, never an error
        return int(expires) > time.time() and hmac.compare_digest(sig.encode('utf-8', 'surrogatepass'),
                                                                  _sign(auth, int(expires)).encode())
    except (ValueError, AttributeError, TypeError, OverflowError):
        return False


def _sign(auth, expires):
    return hmac.new(bytes.fromhex(auth['secret']), str(expires).encode(), hashlib.sha256).hexdigest()
