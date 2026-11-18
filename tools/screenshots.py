#!/usr/bin/env python3
"""Take the documentation's screenshots from a running Medorama that holds the synthetic demo.

    python3 tools/screenshots.py --url http://127.0.0.1:8090/ --user admin --password-file FILE --demo DIR [OUT]
    python3 tools/screenshots.py --vitalstream http://127.0.0.1:8060/ [OUT]

OUT defaults to docs/images/screens. Medorama must be connected to the synthetic demo alone
(tools/make-demo-data.py, made into databases by the installers' --demo; docs/administration.md,
"The demo"): every picture is published.  The script checks this through the API before it turns
Privacy Mode off (the databases connected are the demo's, their files in the demo's folder,
every patient is one of the demo's), and again before the first picture (every patient is one of
the demo's persons, with its SYN MRN, names and birth date, as tools/make-demo-data.py makes
them), and refuses otherwise; it signs in as an admin who manages every demo database.  DIR is
the folder the demo was made in: its demo.json must say that it holds invented data and the
demo library alone (the library's pieces are transformed and checked so that they may be
published: tools/demo/README.md; a demo made with --examples or --dicom holds local files on
the demo's patients, which nothing tells apart), and every recording must lie in it.  The
pictures are in the dark theme, but for ecg.png (the ECG viewer in the light
theme, beside ecg-dark.png).

With --vitalstream, only VitalStream on its own (a viewer whose root holds this repository's
viewers/vitalstream/testdata, without a password): opening testdata/session with Open multiple
records… (vitalstream-open-multiple.png, the summary before it opens) and the session it opens
(vitalstream-session.png). Chrome or Chromium is driven headless through the DevTools protocol
($MEDORAMA_TEST_CHROME, else the usual places); the Python standard library is enough.
"""

import argparse
import base64
import json
import os
import pathlib
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
WIDTH, HEIGHT = 1600, 960


def chrome():
    if os.environ.get("MEDORAMA_TEST_CHROME"):
        return os.environ["MEDORAMA_TEST_CHROME"]
    for p in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    ]:
        if os.path.exists(p):
            return p
    for n in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"]:
        if shutil.which(n):
            return shutil.which(n)
    sys.exit("no Chrome or Chromium found (set MEDORAMA_TEST_CHROME)")


class Socket:
    """A WebSocket client, enough for the DevTools protocol (text frames, no extensions).

    What arrives is kept in a buffer, so a timeout never loses part of a message."""

    def __init__(self, url, seconds=10):
        host, _, path = url.removeprefix("ws://").partition("/")
        h, _, p = host.partition(":")
        self.s = socket.create_connection((h, int(p)), timeout=seconds)
        self.buf = bytearray()
        self.parts = b""
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(
            f"GET /{path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode()
        )
        deadline = time.monotonic() + seconds
        while b"\r\n\r\n" not in self.buf:
            self._fill(deadline)
        head, _, rest = bytes(self.buf).partition(b"\r\n\r\n")
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise RuntimeError(head.decode(errors="replace"))
        self.buf = bytearray(rest)

    def send(self, text):
        data = text.encode()
        n = len(data)
        head = bytes([0x81])
        head += bytes([0x80 | n]) if n < 126 else (bytes([0x80 | 126]) + struct.pack(">H", n) if n < 65536 else bytes([0x80 | 127]) + struct.pack(">Q", n))
        mask = os.urandom(4)
        self.s.settimeout(10)
        self.s.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _fill(self, deadline):
        left = deadline - time.monotonic()
        if left <= 0:
            raise Timeout("no answer from Chrome in time")
        self.s.settimeout(left)
        try:
            chunk = self.s.recv(1 << 16)
        except socket.timeout:
            raise Timeout("no answer from Chrome in time") from None
        if not chunk:
            raise EOFError("Chrome closed the connection")
        self.buf += chunk

    def _frame(self):
        """The next whole frame in the buffer, (header byte, payload), or None."""
        b = self.buf
        if len(b) < 2:
            return None
        n, at = b[1] & 0x7F, 2
        if n == 126:
            if len(b) < 4:
                return None
            n, at = struct.unpack(">H", b[2:4])[0], 4
        elif n == 127:
            if len(b) < 10:
                return None
            n, at = struct.unpack(">Q", b[2:10])[0], 10
        if len(b) < at + n:
            return None
        frame = (b[0], bytes(b[at:at + n]))
        del b[:at + n]
        return frame

    def recv(self, deadline):
        """The next text message; Timeout at `deadline` (time.monotonic())."""
        while True:
            frame = self._frame()
            if frame is None:
                self._fill(deadline)
                continue
            b0, payload = frame
            if b0 & 0x0F == 0x8:
                raise EOFError("Chrome closed the connection")
            if b0 & 0x0F >= 0x8:   # ping, pong: nothing to answer for DevTools
                continue
            self.parts += payload
            if b0 & 0x80:
                message, self.parts = self.parts, b""
                return message.decode()

    def close(self):
        self.s.close()


class Timeout(RuntimeError):
    """Chrome did not answer in time."""


class Browser:
    """Headless Chrome on a temporary profile, driven through the DevTools protocol.  Every call
    has a deadline; Chrome is stopped if starting fails."""

    START_SECONDS = 30

    def __init__(self):
        self.profile = tempfile.mkdtemp(prefix="medorama-shots-")
        self.proc = None
        self.ws = None
        try:
            self._start()
        except BaseException:
            self.close()
            raise

    def _start(self):
        args = [chrome(), "--headless=new", "--remote-debugging-port=0", f"--user-data-dir={self.profile}",
                "--no-first-run", "--no-default-browser-check", "--hide-scrollbars", f"--window-size={WIDTH},{HEIGHT}"]
        if sys.platform == "darwin":
            # no keychain prompt, and the login keychain is never touched
            args += ["--use-mock-keychain", "--password-store=basic"]
        elif sys.platform.startswith("linux"):
            args += ["--password-store=basic"]
        self.proc = subprocess.Popen(args + ["about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # Chrome picks a free port and writes it to DevToolsActivePort in the profile
        active = pathlib.Path(self.profile) / "DevToolsActivePort"
        deadline = time.monotonic() + self.START_SECONDS
        ws = None
        while ws is None:
            if self.proc.poll() is not None:
                raise RuntimeError(f"Chrome exited at start (status {self.proc.returncode})")
            if time.monotonic() > deadline:
                raise RuntimeError(f"Chrome did not start in {self.START_SECONDS} s")
            try:
                port = int(active.read_text().split()[0])
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/list", timeout=2) as r:
                    pages = json.load(r)
                ws = next(p["webSocketDebuggerUrl"] for p in pages if p["type"] == "page")
            except (OSError, ValueError, IndexError, StopIteration):
                time.sleep(0.1)
        self.ws = Socket(ws)
        self.n = 0
        self.call("Emulation.setDeviceMetricsOverride", {"width": WIDTH, "height": HEIGHT, "deviceScaleFactor": 1, "mobile": False})

    def call(self, method, params=None, seconds=60, deadline=None):
        """A DevTools command's result; Timeout when Chrome has not answered by the deadline
        (`deadline`, time.monotonic(), else `seconds` from now)."""
        deadline = deadline if deadline is not None else time.monotonic() + seconds
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": method, "params": params or {}}))
        while True:
            m = json.loads(self.ws.recv(deadline))
            if m.get("id") == self.n:   # answers to earlier, timed-out calls are skipped
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})

    def eval(self, js, seconds=30, deadline=None):
        deadline = deadline if deadline is not None else time.monotonic() + seconds
        left_ms = max(int((deadline - time.monotonic()) * 1000), 1)
        r = self.call("Runtime.evaluate", {"expression": js, "awaitPromise": True, "returnByValue": True, "timeout": left_ms},
                      deadline=deadline)
        if "exceptionDetails" in r:
            raise RuntimeError(f"{js[:80]}: {r['exceptionDetails']}")
        return r["result"].get("value")

    def until(self, js, what, seconds=30):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            try:
                if self.eval(js, deadline=end):
                    return
            except RuntimeError:
                pass
            time.sleep(0.2)
        raise RuntimeError(f"timed out waiting for {what}")

    def theme(self, dark):
        """The page's theme, by its button (the choice is remembered, as when a person clicks it)."""
        want = "dark" if dark else "light"
        if self.eval("document.documentElement.dataset.theme") != want:
            self.eval("document.querySelector('#btn-theme').click(); true")
            self.until(f"document.documentElement.dataset.theme === '{want}'", f"the {want} theme")

    def shoot(self, path, settle=1.5, only=None):
        """The page as drawn; with `only` (a selector), that element alone."""
        time.sleep(settle)
        params = {"format": "png"}
        if only:
            r = self.eval(f"(e => {{ const r = e.getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; }})(document.querySelector({json.dumps(only)}))")
            params["clip"] = {"x": max(r[0] - 8, 0), "y": max(r[1] - 8, 0), "width": r[2] + 16, "height": r[3] + 16, "scale": 1}
        data = self.call("Page.captureScreenshot", params)["data"]
        path.write_bytes(base64.b64decode(data))
        print(f"  {path}")

    def close(self):
        if self.ws is not None:
            self.ws.close()
        if self.proc is not None and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        # Chrome's helpers (its crash reporter) may still write into the profile for a moment
        # after Chrome itself has stopped: remove it until it is gone, for up to 5 s
        for _ in range(50):
            shutil.rmtree(self.profile, ignore_errors=True)
            if not os.path.exists(self.profile):
                break
            time.sleep(0.1)


# ------------------------------------------------------------------ the demo, and nothing else

# the names `medorama init --demo` connects the demo's databases by (tools/make-demo-data.py's
# demo-databases.json; the small demo has the first two)
DEMO_DATABASES = {"demo", "demo_north", "demo_omop", "demo_registry", "demo_muse", "demo_secure"}


class NotDemo(Exception):
    """The catalog holds more than the synthetic demo."""


def generator():
    """tools/make-demo-data.py, as a module (not run: its functions only)."""
    import importlib.util
    spec = importlib.util.spec_from_file_location("make_demo_data", ROOT / "tools" / "make-demo-data.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def demo_patients(full=True):
    """The persons tools/make-demo-data.py invents (from its code, for the same seed): every
    person of every demo database, [{patient_id, mrn, last_name, first_name, birth_date}]."""
    return generator().identities(full)


def made(folder):
    """The demo folder's demo.json (what went in), {} when there is none."""
    try:
        return json.loads((pathlib.Path(folder).expanduser() / "demo.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def check_catalog(api, folder=None):
    """Before Privacy Mode is turned off: the databases connected are the demo's alone (their
    files in `folder`, when given), and there are no more patients than the demo's persons.
    `api(path)` answers a GET of api/v1/<path>.  NotDemo otherwise."""
    dbs = api("databases")["items"]
    names = {d.get("name") for d in dbs}
    if not names or names - DEMO_DATABASES:
        raise NotDemo(f"it is connected to databases other than the demo's: {sorted(names)}")
    for d in dbs:
        if d.get("permission") != "manage":
            raise NotDemo(f"{d.get('name')}: the user does not manage it (its file cannot be checked)")
        if folder is not None:
            path = pathlib.Path(str(d.get("url", "")).removeprefix("sqlite:"))
            try:
                path.resolve().relative_to(pathlib.Path(folder).expanduser().resolve())
            except ValueError:
                raise NotDemo(f"{d.get('name')} is not a file of the demo's folder") from None
    people = demo_patients(made(folder).get("full", True) if folder is not None else True)
    if api("patients?limit=1")["total"] > len(people):
        raise NotDemo("it holds patients that are not the demo's")


def publishable(m):
    """A demo of invented data, or made with the demo library (which may be published: its
    pieces are transformed and checked, tools/demo/README.md), and no local files."""
    if m.get("made_by") != "tools/make-demo-data.py" or m.get("examples") or m.get("dicom"):
        return False
    return m.get("invented") is True or m.get("library") is True or m.get("synthetic_only") is True


def check_folder(api, folder):
    """The demo folder `folder` holds invented data and the demo library alone (its demo.json,
    from tools/make-demo-data.py: no --examples, no --dicom), and every recording of the catalog
    lies in it.  NotDemo otherwise, and when demo.json is missing or unreadable (a folder made
    before it existed: make it again)."""
    folder = pathlib.Path(folder).expanduser().resolve()
    try:
        m = json.loads((folder / "demo.json").read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise NotDemo(f"{folder} has no readable demo.json ({e}): make the demo again with tools/make-demo-data.py")
    if not publishable(m):
        raise NotDemo(f"{folder} holds local files besides invented data ({m}): make the demo without --examples or --dicom")
    offset, seen = 0, 0
    while True:
        page = api(f"recordings?limit=1000&offset={offset}")
        for r in page["items"]:
            path = pathlib.Path(r.get("path", ""))
            try:
                path.resolve().relative_to(folder)
            except ValueError:
                raise NotDemo(f"a recording lies outside {folder}") from None
        seen += len(page["items"])
        offset += 1000
        if not page["items"] or seen >= page["total"]:
            return


def check_identities(api, full=True):
    """With Privacy Mode off, before any picture: every patient is one of the demo's persons,
    with the demo's SYN MRN (in any letter case), names and birth date.  NotDemo otherwise
    (naming patients by pseudonym)."""
    people = demo_patients(full)
    by_id = {p["patient_id"]: p for p in people if p["patient_id"]}
    by_mrn = {p["mrn"].upper(): p for p in people if p["mrn"]}
    rows, total = [], None
    while total is None or len(rows) < total:
        page = api(f"patients?limit=1000&offset={len(rows)}")
        total = page["total"]
        if not page["items"]:
            break
        rows += page["items"]
    if not rows or len(rows) != total:
        raise NotDemo("the patients could not be listed")
    for p in rows:
        who = f"patient {p.get('pseudonym')}"
        phi = p.get("phi")
        if not phi:
            raise NotDemo(f"{who}: no identity shown (Privacy Mode is still on?)")
        mrns = {m.upper() for _, m in phi.get("mrns") or []}
        if any(not m.startswith("SYN") for m in mrns):
            raise NotDemo(f"{who} has an MRN that is not a demo's SYN one")
        names = (phi.get("last_name"), phi.get("first_name"), phi.get("birth_date"))
        if len(mrns) > 1:
            raise NotDemo(f"{who} has MRNs of two demo persons")
        want = by_mrn.get(next(iter(mrns))) if mrns else by_id.get(p.get("patient_id"))
        if want is None or (want["mrn"].upper() if want["mrn"] else "") != (next(iter(mrns)) if mrns else ""):
            raise NotDemo(f"{who} is not one of the demo's")
        if names != (want["last_name"], want["first_name"], want["birth_date"]) or (p.get("patient_id") or "") != want["patient_id"]:
            raise NotDemo(f"{who} is not the demo's {want['patient_id'] or want['mrn']}")


def standalone_vitalstream(url, out):
    """VitalStream on its own, opening the synthetic session of testdata/ as one session."""
    session = str(ROOT / "viewers" / "vitalstream" / "testdata" / "session")
    b = Browser()
    try:
        b.call("Page.navigate", {"url": url.rstrip("/") + "/#theme=dark"})
        b.until("!!document.querySelector('#btn-open-many') && document.documentElement.dataset.theme === 'dark'", "VitalStream, dark")
        b.eval("document.querySelector('#btn-open-many').click(); true")
        b.until("document.querySelector('#dlg-open').open && !!document.querySelector('#browser li')", "the Open dialog")
        b.eval(f"{{ const i = document.querySelector('#path-input'); i.value = {json.dumps(session)}; document.querySelector('#path-go').click(); }} true")
        b.until(f"document.querySelector('#path-input').value === {json.dumps(session)} && !document.querySelector('#open-here').hidden"
                " && [...document.querySelectorAll('#browser li')].some(l => l.innerText.includes('abp'))", "the folder listed")
        b.eval("document.querySelector('#open-here').click(); true")
        b.until("!document.querySelector('#scan-summary').hidden", "the folder's summary")
        b.shoot(out / "vitalstream-open-multiple.png", settle=1)
        b.eval("document.querySelector('#scan-open').click(); true")
        b.until("!document.querySelector('#dlg-open').open && document.querySelector('#source').innerText.includes('session')", "the session")
        b.shoot(out / "vitalstream-session.png", settle=3)
    finally:
        b.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("out", nargs="?", default=str(ROOT / "docs/images/screens"))
    ap.add_argument("--url", default="http://127.0.0.1:8090/")
    ap.add_argument("--user", default="admin")
    ap.add_argument("--password-file")
    ap.add_argument("--vitalstream", metavar="URL", help="only VitalStream on its own, at URL")
    ap.add_argument("--demo", metavar="DIR", help="the folder the catalog's demo was made in (tools/make-demo-data.py)")
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    if a.vitalstream:
        standalone_vitalstream(a.vitalstream, out)
        return
    if not a.password_file:
        ap.error("--password-file is required (or --vitalstream)")
    if not a.demo:
        ap.error("--demo DIR is required: the folder the catalog's demo was made in (tools/make-demo-data.py)")
    password = pathlib.Path(a.password_file).read_text().strip()
    base = a.url.rstrip("/") + "/"

    b = Browser()
    try:
        b.call("Page.navigate", {"url": base})
        b.until("!!document.querySelector('#login') && !document.querySelector('#login').classList.contains('hidden')", "the sign-in form")
        b.eval(f"""document.querySelector('#login-user').value = {json.dumps(a.user)};
                   document.querySelector('#login-pass').value = {json.dumps(password)};
                   document.querySelector('#login-form').requestSubmit(); true""")
        b.until("!document.querySelector('#app').classList.contains('hidden')", "the page after signing in")

        def api(path):
            r = b.eval(f"fetch('api/v1/{path}').then(async r => ({{status: r.status, body: await r.json().catch(() => null)}}))")
            if r["status"] != 200:
                raise RuntimeError(f"api/v1/{path}: HTTP {r['status']} {(r['body'] or {}).get('detail', '')}")
            return r["body"]

        def privacy_on():
            return b.eval("document.body.classList.contains('privacy')")

        # every picture is published: the catalog must hold the synthetic demo and nothing else,
        # checked before Privacy Mode is turned off and again, by the identities, before any picture
        try:
            check_catalog(api, a.demo)
            check_folder(api, a.demo)
        except Exception as e:
            sys.exit(f"refused: {base} is not a catalog of the demo alone: {e} (tools/README.md, \"Refresh the screenshots\"; an admin signs in)")
        if privacy_on():
            b.eval("document.querySelector('#btn-privacy').click(); true")
            b.until("!document.body.classList.contains('privacy')", "Privacy Mode off")
        try:
            check_identities(api, made(a.demo).get("full", True))
        except Exception as e:
            try:
                if not privacy_on():
                    b.eval("document.querySelector('#btn-privacy').click(); true")
            except RuntimeError:
                pass
            sys.exit(f"refused: {base} is not a catalog of the demo alone: {e}")
        b.theme(True)

        def go(hash, ready, what):
            b.eval(f"location.hash = {json.dumps(hash)}; true")
            b.until(ready, what)

        # the patient with the most to show (an ICU stay, ECGs, echo, images): the most kinds of data
        patients = api("patients?limit=50")["items"]
        pid = max(patients, key=lambda p: (len(p.get("kinds", [])), p.get("items", 0), -p["id"]))["id"]

        def type_into(selector, text):
            b.eval(f"""(i => {{ i.value = {json.dumps(text)}; i.dispatchEvent(new Event('input', {{bubbles: true}})); }})
                       (document.querySelector({json.dumps(selector)})); true""")

        def open_entry(text, button):
            """Clicks the report's entry containing `text`, then its detail's button `button`."""
            b.until(f"[...document.querySelectorAll('#view-patient [data-key]')].some(x => x.innerText.includes({json.dumps(text)}))", text)
            b.eval(f"[...document.querySelectorAll('#view-patient [data-key]')].find(x => x.innerText.includes({json.dumps(text)})).click(); true")
            if button:
                b.until(f"[...document.querySelectorAll('#view-patient button, #view-patient a')].some(x => x.innerText.includes({json.dumps(button)}))", button)
                b.eval(f"[...document.querySelectorAll('#view-patient button, #view-patient a')].find(x => x.innerText.includes({json.dumps(button)})).click(); true")

        # (the report listed: one line per thing done)
        patient_ready = "document.querySelectorAll('#pp-list .it[data-key]').length > 0"

        go(f"#/patient/{pid}", patient_ready, "the patient's page")
        open_entry("12-lead ECG", None)
        b.shoot(out / "patient.png")

        # the report searched
        type_into("#pp-q", "creatinine")
        b.until("document.querySelector('#view-patient mark') !== null", "the words highlighted")
        b.shoot(out / "patient-search.png")

        # an ICU recording at a glance
        go(f"#/patient/{pid}", patient_ready, "the patient's page")
        type_into("#pp-q", "")
        open_entry("ICU monitoring", None)
        b.shoot(out / "patient-icu.png", settle=3)

        # a Holter recording and its heart rate
        holter = next((r for r in api("recordings?limit=50")["items"] if "holter" in r["path"].lower()), None)
        if holter and holter.get("patient_id") is not None:
            go(f"#/patient/{holter['patient_id']}", patient_ready, "the Holter's patient")
            open_entry("Holter recording", None)
            b.shoot(out / "patient-holter.png", settle=3)
        else:
            hid = api("patients?q=holter")["items"]
            if hid:
                go(f"#/patient/{hid[0]['id']}", patient_ready, "the Holter's patient")
                open_entry("Holter recording", None)
                b.shoot(out / "patient-holter.png", settle=3)

        # a sleep study (an EDF+ file) and its scoring
        sleep = next((r for r in api("recordings?limit=50")["items"] if r["path"].lower().endswith(".edf")), None)
        sleepers = api("patients?q=sleep")["items"] if sleep else []
        sleepers = sorted(sleepers, key=lambda p: p.get("patient_id") != "P900004")   # the first patients' study first
        if sleepers:
            go(f"#/patient/{sleepers[0]['id']}", patient_ready, "the sleep study's patient")
            open_entry("Sleep study", None)
            b.shoot(out / "patient-sleep.png", settle=3)

        go("#/patients", "document.querySelector('#view-patients').innerText.includes('MRN') || document.querySelectorAll('#view-patients tr').length > 3", "the Patient Catalog")
        b.shoot(out / "patients.png")
        go("#/databases", "document.querySelector('#db-list').innerText.includes('online')", "the databases")
        b.shoot(out / "databases.png")
        # the form that connects a database (not the server's folders)
        b.shoot(out / "connect.png", only="#db-connect")

        # a cohort, by an expression
        go("#/cohorts", "!document.querySelector('#view-cohorts').classList.contains('hidden')", "the cohorts")
        if not b.eval("!!document.querySelector('#co-criteria input[data-k=\"text\"]')"):
            b.eval("document.querySelector('#co-addexpr').click(); true")
        b.until("!!document.querySelector('#co-criteria input[data-k=\"text\"]')", "the expression's field")
        type_into('#co-criteria input[data-k="text"]', "demo.creatinine > 1.5 OR has icu")
        b.until("/^\\d+ patient/.test(document.querySelector('#co-count').innerText)", "the patients found")
        b.shoot(out / "cohorts.png", settle=2.5)

        # the viewers, from the patient's page; the ECG viewer in both themes
        for dark, name in ((False, "ecg.png"), (True, "ecg-dark.png")):
            b.theme(dark)
            go(f"#/patient/{pid}", patient_ready, "the patient's page")
            open_entry("12-lead ECG", "Open in ECG")
            b.until("location.hash.startsWith('#/ecg')", "the ECG tab")
            b.until("(() => { const d = document.querySelector('#app-ecg').contentDocument; return !!(d && d.querySelector('canvas, svg')); })()", "the ECG drawn")
            b.shoot(out / name, settle=3)

        def vitalstream(rec, label, key, name):
            """VitalStream on its own page, at the recording, `key` the span (1: 10 s, 3: 5 min)."""
            go("#/viewer?path=" + urllib.parse.quote(rec["path"], safe=""),
               f"(() => {{ const f = document.querySelector('#viewer-frame'); const d = f && f.contentDocument; return !!(d && d.querySelector('canvas') && d.body.innerText.includes({json.dumps(label)})); }})()",
               "the recording drawn")
            src = b.eval("document.querySelector('#viewer-frame').contentWindow.location.href")
            b.call("Page.navigate", {"url": src})
            b.until(f"!!document.querySelector('canvas') && document.body.innerText.includes({json.dumps(label)})", "VitalStream on its own")
            time.sleep(1)
            for t in ("keyDown", "keyUp"):
                b.call("Input.dispatchKeyEvent", {"type": t, "key": key, "code": f"Digit{key}", "text": key if t == "keyDown" else "", "windowsVirtualKeyCode": 48 + int(key)})
            b.shoot(out / name, settle=4)
            b.call("Page.navigate", {"url": base})
            b.until("!document.querySelector('#app').classList.contains('hidden')", "the page again")

        rec = next((r for r in api("recordings?limit=50")["items"] if "icu" in r["path"].lower()), None)
        if rec:
            vitalstream(rec, "II", "1", "vitalstream.png")
        if sleep:
            vitalstream(sleep, "EEG", "3", "vitalstream-sleep.png")

        go(f"#/patient/{pid}", patient_ready, "the patient's page")
        for text, name in [("Transthoracic echo", "echo"), ("CT", "pacs")]:
            try:
                open_entry(text, "Open in PACS")
                b.until("location.hash.startsWith('#/pacs')", "the PACS tab")
                b.until("(() => { const d = document.querySelector('#app-pacs').contentDocument; return !!(d && d.querySelector('canvas')); })()", "the study drawn")
                b.shoot(out / f"{name}.png", settle=3)
                go(f"#/patient/{pid}", patient_ready, "the patient's page")
            except RuntimeError as e:
                print(f"  {name}: skipped ({e})")

        # the same patient in Privacy Mode
        b.eval("document.querySelector('#btn-privacy').click(); true")
        b.until("document.body.classList.contains('privacy')", "Privacy Mode")
        go(f"#/patient/{pid}", "document.querySelector('#pp-bar') && document.querySelector('#pp-bar').innerText.includes('MO-')", "the pseudonym")
        open_entry("12-lead ECG", None)
        b.shoot(out / "privacy.png")
        b.eval("document.querySelector('#btn-privacy').click(); true")
    finally:
        b.close()


if __name__ == "__main__":
    main()
