"""Tests of tools/screenshots.py: it refuses a Medorama connected to more than the demo, and no call
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


def demo_answers(folder="/demo"):
    """What a Medorama connected to the demo alone answers: its databases, managed by the admin,
    and their patients (the persons tools/make-demo-data.py invents)."""
    patients = [
        {"patient_id": p["patient_id"], "pseudonym": f"MO-{i}", "sources": ["demo"],
         "phi": {"mrns": [["DEMO", p["mrn"]]] if p["mrn"] else [], "last_name": p["last_name"], "first_name": p["first_name"],
                 "birth_date": p["birth_date"]}}
        for i, p in enumerate(shots.demo_patients())
    ]
    files = {"demo": "demo.sqlite", "demo_north": "north/north.sqlite", "demo_omop": "omop/omop.sqlite",
             "demo_registry": "registry/registry.sqlite", "demo_muse": "muse/ecg_metadata.sqlite", "demo_secure": "secure/secure.sqlite"}
    return {
        "databases": {"items": [
            {"name": n, "kind": "omop" if n == "demo_omop" else "views" if n in ("demo_registry", "demo_muse") else "dbb", "permission": "manage",
             "url": f"sqlite:{folder}/{f}"} for n, f in files.items()
        ]},
        "patients?limit=1": {"total": len(patients), "items": patients[:1]},
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
        # the small demo: its two databases, its eight persons
        small = shots.demo_patients(False)
        self.assertEqual(len(small), 8)

    def test_another_database_is_refused(self):
        a = demo_answers()
        a["databases"]["items"].append({"name": "warehouse", "kind": "omop", "permission": "manage", "url": "postgres://x"})
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_a_database_outside_the_folder_is_refused(self):
        a = demo_answers()
        a["databases"]["items"][0]["url"] = "sqlite:/data/elsewhere/demo.sqlite"
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a), "/demo")
        shots.check_catalog(api_of(demo_answers()), "/demo")

    def test_a_database_not_managed_is_refused(self):
        a = demo_answers()
        a["databases"]["items"][1]["permission"] = "view"
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_patients_from_elsewhere_are_refused(self):
        a = demo_answers()
        a["patients?limit=1"]["total"] = len(shots.demo_patients()) + 1
        with self.assertRaises(shots.NotDemo):
            shots.check_catalog(api_of(a))

    def test_an_unanswered_question_is_refused(self):
        a = demo_answers()
        del a["databases"]   # e.g. not signed in
        with self.assertRaises(RuntimeError):
            shots.check_catalog(api_of(a))

    def test_the_demo_folder(self):
        with tempfile.TemporaryDirectory() as d:
            folder = pathlib.Path(d)
            inside = {"recordings?limit=1000&offset=0": {"total": 1, "items": [{"path": str(folder / "icu" / "r")}]}}
            # no demo.json (a folder made before it existed): refused
            with self.assertRaises(shots.NotDemo):
                shots.check_folder(api_of(inside), folder)
            # invented data alone: passes (and a folder made before "invented" was written)
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "invented": true, "library": false}')
            shots.check_folder(api_of(inside), folder)
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "synthetic_only": true}')
            shots.check_folder(api_of(inside), folder)
            # made with the demo library (transformed, checked, publishable): passes
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "invented": false, "library": true, "examples": false, "dicom": false}')
            shots.check_folder(api_of(inside), folder)
            # the library and local files: refused
            (folder / "demo.json").write_text('{"made_by": "tools/make-demo-data.py", "invented": false, "library": true, "dicom": true}')
            with self.assertRaises(shots.NotDemo):
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

        refused(lambda ps: ps[0]["phi"]["mrns"].__setitem__(0, ["DEMO", "1234567"]))   # not a SYN MRN
        refused(lambda ps: ps[0]["phi"]["mrns"].append(["lab", "SYN9999999"]))         # a second MRN
        refused(lambda ps: ps[0]["phi"]["mrns"].append(["DEMO", "SYN0000002"]))        # another person's MRN too
        refused(lambda ps: ps[0]["phi"].__setitem__("last_name", "Smith"))             # renamed
        refused(lambda ps: ps[0].__setitem__("patient_id", "P123"))                    # not the demo's
        refused(lambda ps: ps[0].pop("phi"))                                           # Privacy Mode on
        refused(lambda ps: ps[-1]["phi"].__setitem__("birth_date", "1950-01-01"))      # not the OMOP person
        refused(lambda ps: ps[-1]["phi"].__setitem__("mrns", []))                       # an OMOP person without its MRN

        # the same MRN in another letter case (a second hospital's, linked ignoring case): passes
        a = demo_answers()
        a["patients?limit=1000&offset=0"]["items"][0]["phi"]["mrns"].append(["DEMO", "syn0000001"])
        shots.check_identities(api_of(a))

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
