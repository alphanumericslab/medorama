"""Tests of tools/license-accept.sh, the installers' license prompt: the terms are shown, nothing
goes on unless the user types y or yes (or the installer is told to accept), and the acceptance
is recorded so that it is asked once.  The Python standard library only (Linux and macOS):

    python3 -m unittest discover -s tools/tests
"""

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
# the installers' way of calling it, after their options are read
SCRIPT = 'set -euo pipefail; ROOT="$1"; ACCEPT_LICENSE="$2"; . "$ROOT/tools/license-accept.sh"; accept_license; echo CONTINUES'


@unittest.skipIf(os.name == 'nt', 'the shell installers are for Linux and macOS')
class LicenseAccept(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.env = {**os.environ, 'HOME': self.home.name}
        self.env.pop('XDG_CONFIG_HOME', None)
        self.env.pop('MEDORAMA_ACCEPT_LICENSE', None)
        self.record = pathlib.Path(self.home.name, '.config', 'medorama', 'license-accepted')

    def tearDown(self):
        self.home.cleanup()

    def run_piped(self, accept='0', **env):
        """Without a terminal (stdin is not one), as in a script or CI."""
        return subprocess.run(['bash', '-c', SCRIPT, 'bash', str(ROOT), accept], env={**self.env, **env},
                              stdin=subprocess.DEVNULL, capture_output=True, text=True)

    def run_terminal(self, answer):
        """In a terminal, answering the question with answer; returns (output, exit code)."""
        import pty
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe('bash', ['bash', '-c', SCRIPT, 'bash', str(ROOT), '0'], self.env)
        out = b''
        while b'[no]: ' not in out:
            out += os.read(fd, 4096)
        os.write(fd, (answer + '\n').encode())
        while True:
            try:
                chunk = os.read(fd, 4096)
            except OSError:
                break
            if not chunk:
                break
            out += chunk
        _, status = os.waitpid(pid, 0)
        return out.decode(), os.waitstatus_to_exitcode(status)

    def test_without_a_terminal_nothing_goes_on(self):
        r = self.run_piped()
        self.assertEqual(r.returncode, 1)
        self.assertIn('PolyForm Noncommercial License 1.0.0', r.stdout)
        self.assertIn('--accept-license', r.stdout)
        self.assertNotIn('CONTINUES', r.stdout)
        self.assertFalse(self.record.exists())

    def test_the_option_or_the_variable_accepts_and_is_recorded(self):
        for accept, env in (('1', {}), ('0', {'MEDORAMA_ACCEPT_LICENSE': 'YES'})):
            r = self.run_piped(accept, **env)
            self.assertEqual(r.returncode, 0, r.stdout)
            self.assertIn('CONTINUES', r.stdout)
            text = self.record.read_text()
            self.assertIn('license=PolyForm-Noncommercial-1.0.0', text)
            self.assertIn('accepted_by=option', text)
            self.record.unlink()

    def test_asked_once_until_the_terms_change(self):
        self.run_piped('1')
        r = self.run_piped()
        self.assertEqual(r.returncode, 0)
        self.assertIn('License terms accepted on', r.stdout)
        self.assertNotIn('PolyForm', r.stdout)
        # a record of other terms is asked again
        self.record.write_text(self.record.read_text().replace('notice_sha256=', 'notice_sha256=0'))
        self.assertEqual(self.run_piped().returncode, 1)

    def test_in_a_terminal_only_y_or_yes_goes_on(self):
        for answer, accepted in (('y', True), ('Yes', True), ('YES', True), ('n', False), ('', False), ('sure', False)):
            out, code = self.run_terminal(answer)
            self.assertEqual((code, 'CONTINUES' in out, self.record.exists()), (0, True, True) if accepted else (1, False, False), answer)
            if accepted:
                self.assertIn('accepted_by=prompt', self.record.read_text())
                self.record.unlink()


if __name__ == '__main__':
    unittest.main()
