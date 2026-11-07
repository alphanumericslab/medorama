"""Tests of tools/screenshots.py: it refuses a catalog that holds more than the demo, and no call
to Chrome waits forever.  The Python standard library only:

    python3 -m unittest discover -s tools/tests

The test with Chrome runs when Chrome or Chromium is found ($MEDORAMA_TEST_CHROME, else the
usual places) and is skipped otherwise.
"""

import importlib.util
import os
import pathlib
import socket
import stat
import sys
import tempfile
import threading
import time
import unittest

sys.dont_write_bytecode = True
TOOLS = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("screenshots", TOOLS / "screenshots.py")
shots = importlib.util.module_from_spec(spec)
spec.loader.exec_module(shots)


def demo_answers():
    """What a catalog of the demo alone answers (the connection synced: one OMOP person added)."""
    tables, _ = shots.demo_patients()
    patients = [
        {"patient_id": pid, "pseudonym": f"MO-{i}", "phi": {"mrns": [["", mrn]], "last_name": last, "first_name": first, "birth_date": born}}
        for i, (pid, (mrn, last, first, born)) in enumerate(tables.items())
    ]
    patients.append({"patient_id": "GEN-000001", "pseudonym": "MO-X", "sources": ["demo_omop"],
                     "phi": {"mrns": [["", "SYN0000101"]], "last_name": "", "first_name": "", "birth_date": "1949-09-21"}})
    return {
        "sources": {"items": [{"source": "DEMO", "patients": len(tables), "recordings": 8},
                              {"source": "demo_omop", "patients": 1, "recordings": 0}]},
        "connections": {"items": [{"name": "demo_omop", "kind": "omop"}]},
        "demo/removal": {"dry_run": True, "patients": len(tables), "recordings": 8, "kept_recordings": 0, "kept_encounters": 0},
        "patients?limit=1": {"total": len(patients), "items": patients[:1]},
        "recordings?limit=1": {"total": 8, "items": []},
        "patients?limit=1000&offset=0": {"total": len(patients), "items": patients},
    }


def api_of(answers):
    def api(path):
        if path not in answers:
            raise RuntimeError(f"api/v1/{path}: HTTP 404")
        return answers[path]
    return api


class DemoOnly(unittest.TestCase):
    def test_the_demo_passes(self):
        a = demo_answers()
        shots.check_catalog(api_of(a))
        shots.check_identities(api_of(a))

    def test_another_source_is_refused(self):
        a = demo_answers()
        a["sources"]["items"].append({"source": "admissions.csv", "patients": 1})
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_no_demo_is_refused(self):
        a = demo_answers()
        a["sources"]["items"] = [{"source": "admissions.csv", "patients": 6}]
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_another_connection_is_refused(self):
        a = demo_answers()
        a["connections"]["items"].append({"name": "warehouse", "kind": "omop"})
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_patients_or_recordings_from_elsewhere_are_refused(self):
        for key, field in (("patients?limit=1", "total"), ("recordings?limit=1", "total")):
            a = demo_answers()
            a[key][field] += 1
            with self.assertRaises(shots.NotDemo, msg=key):
                shots.check_catalog(api_of(a))
        a = demo_answers()
        a["demo/removal"]["kept_recordings"] = 1
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_an_unanswered_question_is_refused(self):
        a = demo_answers()
        del a["demo/removal"]   # e.g. a user who is not an admin: 403
        with self.assertRaises(RuntimeError):
            shots.check_catalog(api_of(a))

    def test_the_demo_folder(self):
        with tempfile.TemporaryDirectory() as d:
            folder = pathlib.Path(d)
            inside = {"recordings?limit=1000&offset=0": {"total": 1, "items": [{"path": str(folder / "icu" / "r")}]}}
            # no demo.json (a folder made before it existed): refused
            with self.assertRaises(shots.NotDemo):
                shots.check_folder(api_of(inside), folder)
            # invented data alone: passes
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "synthetic_only": true}')
            shots.check_folder(api_of(inside), folder)
            # local files on the demo's patients (--examples, --dicom): refused
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "synthetic_only": false, "examples": true}')
            with self.assertRaises(shots.NotDemo):
                shots.check_folder(api_of(inside), folder)
            # a recording from elsewhere: refused
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "synthetic_only": true}')
            outside = {"recordings?limit=1000&offset=0": {"total": 1, "items": [{"path": "/data/elsewhere/r"}]}}
            with self.assertRaises(shots.NotDemo):
                shots.check_folder(api_of(outside), folder)

    def test_identities(self):
        def refused(change):
            a = demo_answers()
            change(a["patients?limit=1000&offset=0"]["items"])
            with self.assertRaises(shots.NotDemo):
                shots.check_identities(api_of(a))

        refused(lambda ps: ps[0]["phi"]["mrns"].__setitem__(0, ["", "1234567"]))       # not a SYN MRN
        refused(lambda ps: ps[0]["phi"]["mrns"].append(["lab", "SYN9999999"]))         # a second MRN
        refused(lambda ps: ps[0]["phi"].__setitem__("last_name", "Smith"))             # renamed
        refused(lambda ps: ps[0].__setitem__("patient_id", "P123"))                    # not the demo's
        refused(lambda ps: ps[0].pop("phi"))                                           # Privacy Mode on
        refused(lambda ps: ps[-1].__setitem__("sources", ["warehouse"]))               # another database's person
        refused(lambda ps: ps[-1]["phi"].__setitem__("birth_date", "1950-01-01"))      # not the OMOP person

        a = demo_answers()   # fewer listed than the total
        listed = a["patients?limit=1000&offset=0"]
        listed["items"].pop()
        a[f"patients?limit=1000&offset={len(listed['items'])}"] = {"total": listed["total"], "items": []}
        with self.assertRaises(shots.NotDemo):
            shots.check_identities(api_of(a))


class FakeDevTools:
    """A WebSocket server that accepts the handshake, then sends what `answer(n, sock)` sends for
    the n-th message (nothing: Chrome hung)."""

    def __init__(self, answer):
        self.server = socket.create_server(("127.0.0.1", 0))
        self.port = self.server.getsockname()[1]
        self.answer = answer
        threading.Thread(target=self.run, daemon=True).start()

    def run(self):
        conn, _ = self.server.accept()
        with self.server, conn:
            head = b""
            while b"\r\n\r\n" not in head:
                head += conn.recv(1)
            conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
            n = 0
            while True:
                head = self.exact(conn, 2)
                if head is None:   # the client closed
                    return
                size = head[1] & 0x7F
                if size == 126:
                    size = int.from_bytes(self.exact(conn, 2), "big")
                self.exact(conn, 4 + size)   # the mask and the message
                n += 1
                self.answer(n, conn)

    @staticmethod
    def exact(conn, n):
        got = b""
        while len(got) < n:
            chunk = conn.recv(n - len(got))
            if not chunk:
                return None
            got += chunk
        return got

    def browser(self):
        b = object.__new__(shots.Browser)
        b.ws, b.n = shots.Socket(f"ws://127.0.0.1:{self.port}/devtools/page/x"), 0
        return b


def frame(text):
    data = text.encode()
    assert len(data) < 126
    return bytes([0x81, len(data)]) + data


class Deadlines(unittest.TestCase):
    def test_a_call_chrome_never_answers_times_out(self):
        b = FakeDevTools(lambda n, conn: None).browser()
        t = time.monotonic()
        with self.assertRaises(shots.Timeout):
            b.call("Runtime.evaluate", {"expression": "1"}, seconds=0.5)
        self.assertLess(time.monotonic() - t, 3)
        with self.assertRaises(RuntimeError):   # until() gives up at its own deadline
            b.until("new Promise(() => {})", "a promise that never settles", seconds=0.5)
        b.ws.close()

    def test_a_late_answer_is_not_taken_for_the_next(self):
        def answer(n, conn):
            if n == 1:   # the first answer comes late, split across two writes
                time.sleep(0.6)
                whole = frame('{"id": 1, "result": {"result": {"value": "late"}}}')
                conn.sendall(whole[:5])
                time.sleep(0.2)
                conn.sendall(whole[5:])
            else:
                conn.sendall(frame(f'{{"id": {n}, "result": {{"result": {{"value": "on time"}}}}}}'))
        b = FakeDevTools(answer).browser()
        with self.assertRaises(shots.Timeout):
            b.eval("1", seconds=0.3)
        self.assertEqual(b.eval("2", seconds=5), "on time")
        b.ws.close()


@unittest.skipIf(os.name == "nt", "a shell script stands in for Chrome")
class StartFails(unittest.TestCase):
    def test_chrome_is_stopped_when_it_does_not_start(self):
        with tempfile.TemporaryDirectory() as d:
            fake = pathlib.Path(d) / "chrome"
            fake.write_text("#!/bin/sh\nexec sleep 60\n")
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            old, os.environ["MEDORAMA_TEST_CHROME"] = os.environ.get("MEDORAMA_TEST_CHROME"), str(fake)
            made = []
            real_popen = shots.subprocess.Popen

            def popen(*args, **kw):
                made.append(real_popen(*args, **kw))
                return made[-1]
            shots.subprocess.Popen, shots.Browser.START_SECONDS = popen, 1
            try:
                with self.assertRaises(RuntimeError):
                    shots.Browser()
            finally:
                shots.subprocess.Popen, shots.Browser.START_SECONDS = real_popen, 30
                if old is None:
                    del os.environ["MEDORAMA_TEST_CHROME"]
                else:
                    os.environ["MEDORAMA_TEST_CHROME"] = old
            self.assertEqual(len(made), 1)
            self.assertIsNotNone(made[0].poll(), "the stand-in for Chrome is still running")


def chrome_found():
    try:
        return bool(shots.chrome())
    except SystemExit:
        return False


@unittest.skipUnless(chrome_found(), "no Chrome or Chromium")
class WithChrome(unittest.TestCase):
    def test_a_promise_that_never_settles(self):
        b = shots.Browser()
        try:
            self.assertEqual(b.eval("1 + 1"), 2)
            t = time.monotonic()
            with self.assertRaises(RuntimeError):
                b.until("new Promise(() => {})", "a promise that never settles", seconds=2)
            self.assertLess(time.monotonic() - t, 10)
            self.assertEqual(b.eval("2 + 2"), 4)   # the connection still works after the timeout
        finally:
            b.close()
        self.assertIsNotNone(b.proc.poll())
        self.assertFalse(os.path.exists(b.profile))


if __name__ == "__main__":
    unittest.main()
