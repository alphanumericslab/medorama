"""Command line: `vitalstream-py` starts the server; `vitalstream-py --index PATH` only indexes.
Same options, settings keys, messages and exit codes as the Rust viewer."""

import argparse
import sys
import threading
import webbrowser

from . import __version__
from .settings import absolute, built_root, load_settings


def _fail(message):
    print(f'vitalstream-py: {message}', file=sys.stderr)
    return 1


def main(argv=None):
    ap = argparse.ArgumentParser(prog='vitalstream-py', description='Web viewer for physiological recordings '
                                 '(WFDB folders, and the formats of installed adapters).')
    ap.add_argument('--config', help='settings file (TOML); default: <viewer>/vitalstream.toml (run from a clone) '
                                     'or ~/.config/vitalstream/vitalstream.toml if present (never one in the working '
                                     'directory); keys of its [py] section apply here')
    ap.add_argument('--host', help='address to listen on (default 127.0.0.1: this machine only)')
    ap.add_argument('--port', type=int, help='port to listen on (default 8050)')
    ap.add_argument('--timezone', help='display time zone, e.g. America/New_York')
    ap.add_argument('--root', action='append', dest='data_roots',
                    help="folder the viewer may open (repeatable), inside the viewer's root (set when it was "
                         "installed); default: the viewer's root")
    ap.add_argument('--cache-dir', help='index cache folder')
    ap.add_argument('--adapters-dir', help='folder of adapters, programs that read other recording formats '
                                           '(docs/adapters.md)')
    ap.add_argument('--qc-verbose', action='store_true',
                    help='also mark the irregularities a reader repaired (data blocks put back in order)')
    ap.add_argument('--no-browser', action='store_true', help='do not open a browser window')
    ap.add_argument('--set-password', action='store_true',
                    help='set (or change) the password the page asks for, then exit')
    ap.add_argument('--clear-password', action='store_true', help='remove the password, then exit')
    ap.add_argument('--no-auth', action='store_true', help='run without asking for the password')
    ap.add_argument('--index', metavar='PATH', help='index PATH into the cache and exit')
    ap.add_argument('-V', '--version', action='version', version=f'vitalstream-py {__version__}')
    args = ap.parse_args(argv)
    try:
        settings = load_settings(args.config, host=args.host, port=args.port, timezone=args.timezone,
                                 data_roots=args.data_roots, cache_dir=args.cache_dir, adapters_dir=args.adapters_dir,
                                 qc_verbose=True if args.qc_verbose else None)
    except (OSError, ValueError, RuntimeError) as e:
        return _fail(e)
    from . import adapters, paths
    from .sources import wfdb_folder
    adapters.set_folders([settings.adapters_dir])
    wfdb_folder.set_limits(settings.limits)
    # the readers open no file outside the data roots (a signal file a header names, a link)
    paths.set_allowed_roots(settings.data_roots)
    print(f'vitalstream-py: settings from {settings.source}' if settings.source
          else 'vitalstream-py: no settings file (defaults)', file=sys.stderr)
    for r in settings.outside:
        _fail(f"{r} is outside this viewer's root ({built_root()}): left out")
    from . import auth
    if args.set_password:
        if not auth.SUPPORTED:
            return _fail('this viewer was built without password support')
        import getpass
        pw = getpass.getpass('New VitalStream password: ')
        if len(pw) < 8:
            print('Use at least 8 characters.')
            return 1
        if getpass.getpass('Repeat it: ') != pw:
            print('The two entries differ; nothing changed.')
            return 1
        try:
            auth.set_password(settings.auth_file, pw)
        except OSError as e:
            return _fail(f'{settings.auth_file}: {e}')
        print(f'Password set ({settings.auth_file}). Restart the viewer for it to take effect.')
        return 0
    if args.clear_password:
        try:
            auth.clear_password(settings.auth_file)
        except OSError as e:
            return _fail(e)
        print('Password removed.')
        return 0
    if args.index:
        from .index import build_index
        try:
            key = build_index(absolute(args.index), settings.cache_dir)
        except Exception as e:
            return _fail(e)
        print(f'indexed {args.index} -> {settings.cache_dir}/{key}/')
        return 0

    protected = not args.no_auth and auth.load(settings.auth_file) is not None
    if settings.require_password and not protected:
        return _fail('a password is required here (require_password)'
                     + ('; set one with --set-password' if auth.SUPPORTED
                        else ', and this viewer was built without password support'))
    import uvicorn
    from .server import create_app
    try:
        app = create_app(settings, protected=protected)
    except RuntimeError as e:
        return _fail(e)
    host = f'[{settings.host}]' if ':' in settings.host else settings.host   # IPv6 in brackets
    url = f'http://{host}:{settings.port}/'
    print(f'VitalStream: open {url}', flush=True)
    print(f'  root: {built_root()}; data roots: {", ".join(settings.data_roots)}; cache: {settings.cache_dir}; '
          f'time zone {settings.timezone}', flush=True)
    if not wfdb_folder.SUPPORTED:
        print('  built without WFDB support (wfdb-x is not installed): WFDB records and folders are refused', flush=True)
    if protected:
        print('  password protected (vitalstream-py --set-password to change it)', flush=True)
    elif not auth.SUPPORTED:
        print('  built without password support: anyone who can connect to this port can read the files under '
              'its root', flush=True)
    else:
        print('  no password: anyone who can connect to this port can read your files through it.\n'
              '  On a shared machine, set one once with: vitalstream-py --set-password', flush=True)
    if settings.host in ('0.0.0.0', '::') and not settings.allowed_hosts:
        print('  listening on every address: only requests to localhost are answered until the names\n'
              '  this machine is reached by are listed in allowed_hosts', flush=True)
    if not args.no_browser:
        threading.Timer(1.0, webbrowser.open, (url,)).start()
    uvicorn.run(app, host=settings.host, port=settings.port, log_level='warning')
    return 0


if __name__ == '__main__':
    sys.exit(main())
