#!/usr/bin/env python3
"""How fast the three VitalStream viewers index and read recordings, through their HTTP API.

For each recording, a viewer of its own is started with an empty index folder; the recording is
indexed three times (the first `open`, then twice with `reindex`), opened again once indexed,
and read as the page reads it: the whole recording at 1200 points per channel, 20 windows of
10 minutes and 20 of 10 seconds (raw samples) at random places, every channel, and the vital
signs. The memory of the viewer and of the programs it starts (index workers, adapters) is
sampled throughout.

    python3 tools/bench-viewers.py make-data /tmp/bench          # synthetic recordings (2.5 GB)
    python3 tools/bench-viewers.py run --viewer rs --out results.json \\
        wfdb_day=/tmp/bench/wfdb_day/day.hea edf_day=/tmp/bench/edf_day/day.edf ...
    python3 tools/bench-viewers.py run --viewer py --out results.json ...   # the same names
    python3 tools/bench-viewers.py report results.json

`run` takes NAME=PATH pairs: a WFDB header, a folder of records, an EDF or BDF file, or a file
an adapter reads (`--adapters DIR`). Results are keyed by NAME, so that the report shows no
file names; the dataset fields it keeps are counts, formats and durations. Indexes are made in
a temporary folder and removed: nothing is written beside the recordings. `report` says, for
each NAME, whether every viewer answered the same data.

The viewers are those of this clone, built for release: `target/release/vitalstream-rs`,
`viewers/vitalstream/cpp/build/release/bin/vitalstream-cpp`, and the Python package run from
`viewers/vitalstream/python/vitalstream` (`--command` names another program). Memory is read
with `ps` (Linux and macOS). Standard library only (Python 3.9 or later).
"""

import argparse
import array
import hashlib
import http.client
import json
import math
import os
import random
import shutil
import signal
import statistics
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VIEWERS = {
    "rs": ([os.path.join(ROOT, "target", "release", "vitalstream-rs")], None),
    "cpp": ([os.path.join(ROOT, "viewers", "vitalstream", "cpp", "build", "release", "bin", "vitalstream-cpp")], None),
    "py": ([sys.executable, "-m", "vitalstream_py"], os.path.join(ROOT, "viewers", "vitalstream", "python", "vitalstream")),
}
WINDOWS = (("10 min", 600.0), ("10 s", 10.0))

# ---------------------------------------------------------------------------------------------
# Synthetic recordings
# ---------------------------------------------------------------------------------------------

FS, BLOCK_S, DAY_S = 250, 60, 86400
SIGNALS = ["I", "II", "III", "V", "ART", "PAP", "PLETH", "RESP"]


def block():
    """60 s of 8 signals at 250 Hz, frame by frame: sines of whole cycles in 60 s, so that the
    block repeats without a seam."""
    n = FS * BLOCK_S
    out = array.array("h", bytes(2 * n * len(SIGNALS)))
    for k in range(len(SIGNALS)):
        f, slow = (66 + 4 * k) / BLOCK_S, 12 / BLOCK_S
        for i in range(n):
            t = i / FS
            out[i * len(SIGNALS) + k] = int(400 * math.sin(2 * math.pi * f * t) + 30 * math.sin(2 * math.pi * slow * t))
    return out


def le(a):
    if sys.byteorder == "big":
        a = array.array(a.typecode, a)
        a.byteswap()
    return a.tobytes()


def wfdb_header(name, file, fmt, frames, sums, days):
    lines = [f"{name} {len(SIGNALS)} {FS} {frames} 08:00:00 01/03/2024"]
    for k, s in enumerate(SIGNALS):
        c = (sums[k] * days * (DAY_S // BLOCK_S)) & 0xFFFF
        lines.append(f"{file} {fmt} 200/mV 16 0 0 {c - 0x10000 if c >= 0x8000 else c} 0 {s}")
    return "\n".join(lines) + "\n"


def edf_header(magic, reserved, records, phys, dig):
    def f(s, n):
        return s.ljust(n)[:n].encode("ascii")
    ns = len(SIGNALS)
    h = (magic + f("X X X X", 80) + f("Startdate X X X X", 80) + f("01.03.24", 8) + f("08.00.00", 8)
         + f(str(256 * (ns + 1)), 8) + f(reserved, 44) + f(str(records), 8) + f("1", 8) + f(str(ns), 4))
    for vals, n in ((["ECG " + s for s in SIGNALS], 16), ([""] * ns, 80), (["uV"] * ns, 8), ([phys[0]] * ns, 8),
                    ([phys[1]] * ns, 8), ([dig[0]] * ns, 8), ([dig[1]] * ns, 8), ([""] * ns, 80), ([str(FS)] * ns, 8),
                    ([""] * ns, 32)):
        h += b"".join(f(v, n) for v in vals)
    return h


def make_data(out):
    """One day of 8 signals at 250 Hz as WFDB, as WFDB with a MATLAB signal file, as EDF and as
    BDF, and three days as WFDB."""
    b = block()
    ns = len(SIGNALS)
    sums = [sum(b[k::ns]) for k in range(ns)]
    reps = DAY_S // BLOCK_S
    frames = le(b)
    for d in ("wfdb_day", "mat_day", "edf_day", "bdf_day", "wfdb_3days"):
        os.makedirs(os.path.join(out, d), exist_ok=True)

    def write(path, head, body, times):
        with open(path, "wb") as f:
            f.write(head)
            for _ in range(times):
                f.write(body)
    write(os.path.join(out, "wfdb_day", "day.dat"), b"", frames, reps)
    with open(os.path.join(out, "wfdb_day", "day.hea"), "w") as f:
        f.write(wfdb_header("day", "day.dat", "16", DAY_S * FS, sums, 1))
    # MATLAB level 4: a matrix of 8 rows (one per signal), stored column by column, which is
    # frame by frame; its samples start at byte 24
    write(os.path.join(out, "mat_day", "day.mat"), struct.pack("<5I", 30, ns, DAY_S * FS, 0, 4) + b"val\0", frames, reps)
    with open(os.path.join(out, "mat_day", "day.hea"), "w") as f:
        f.write(wfdb_header("day", "day.mat", "16+24", DAY_S * FS, sums, 1))
    write(os.path.join(out, "wfdb_3days", "days.dat"), b"", frames, 3 * reps)
    with open(os.path.join(out, "wfdb_3days", "days.hea"), "w") as f:
        f.write(wfdb_header("days", "days.dat", "16", 3 * DAY_S * FS, sums, 3))
    # EDF and BDF: data records of 1 s, each signal's 250 samples in turn
    recs = bytearray()
    recs24 = bytearray()
    for r in range(BLOCK_S):
        for k in range(ns):
            v = b[r * FS * ns + k:(r + 1) * FS * ns:ns]
            recs += le(v)
            for x in v:
                recs24 += (x * 256 & 0xFFFFFF).to_bytes(3, "little")
    write(os.path.join(out, "edf_day", "day.edf"),
          edf_header(b"0       ", "", DAY_S, ("-32768", "32767"), ("-32768", "32767")), bytes(recs), reps)
    write(os.path.join(out, "bdf_day", "day.bdf"),
          edf_header(b"\xffBIOSEMI", "24BIT", DAY_S, ("-8388608", "8388607"), ("-8388608", "8388607")), bytes(recs24), reps)
    for d, p in (("wfdb_day", "day.hea"), ("mat_day", "day.hea"), ("edf_day", "day.edf"), ("bdf_day", "day.bdf"),
                 ("wfdb_3days", "days.hea")):
        print(f"{d}={os.path.join(out, d, p)}")


# ---------------------------------------------------------------------------------------------
# Measuring
# ---------------------------------------------------------------------------------------------

def tree_rss(pgid):
    """Resident memory of the processes of a group, in bytes."""
    out = subprocess.run(["ps", "-A", "-o", "pgid=,rss="], capture_output=True, text=True).stdout
    rows = (line.split() for line in out.splitlines() if line.strip())
    return sum(int(r) * 1024 for g, r in rows if int(g) == pgid)


class Peak(threading.Thread):
    def __init__(self, pgid):
        super().__init__(daemon=True)
        self.pgid, self.peak, self.stop = pgid, 0, False

    def run(self):
        while not self.stop:
            self.peak = max(self.peak, tree_rss(self.pgid))
            time.sleep(0.02)


class Viewer:
    """A viewer on 127.0.0.1, without a password, in a process group of its own."""

    def __init__(self, cmd, cwd, port, cache, roots, adapters, home, log):
        args = cmd + ["--host", "127.0.0.1", "--port", str(port), "--no-auth", "--no-browser", "--cache-dir", cache]
        if adapters:
            args += ["--adapters-dir", adapters]
        for r in roots:
            args += ["--root", r]
        self.port, self.c = port, None
        self.p = subprocess.Popen(args, cwd=cwd, env=dict(os.environ, HOME=home), stdout=log, stderr=log,
                                  start_new_session=True)
        t = time.time()
        while True:
            try:
                if self.req("GET", "/api/v1/config")[0] == 200:
                    return
            except OSError:
                self.c = None
            if self.p.poll() is not None or time.time() - t > 60:
                raise RuntimeError(f"the viewer did not start: {' '.join(cmd)}")
            time.sleep(0.1)

    def req(self, method, path, body=None):
        for attempt in (0, 1):
            if self.c is None:
                self.c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=900)
            try:
                h = {"Host": f"127.0.0.1:{self.port}"}
                data = None
                if body is not None:
                    data = json.dumps(body).encode()
                    h["Content-Type"] = "application/json"
                self.c.request(method, path, body=data, headers=h)
                r = self.c.getresponse()
                return r.status, r.read()
            except (http.client.HTTPException, OSError):
                self.c.close()
                self.c = None
                if attempt:
                    raise
        raise AssertionError

    def get(self, path):
        st, body = self.req("GET", path)
        if st != 200:
            raise RuntimeError(f"{path.split('?')[0]}: {st} {body[:200]!r}")
        return body

    def close(self):
        try:
            os.killpg(self.p.pid, signal.SIGTERM)
            self.p.wait(10)
        except (OSError, subprocess.TimeoutExpired):
            try:
                os.killpg(self.p.pid, signal.SIGKILL)
            except OSError:
                pass


def timed(f):
    t = time.perf_counter()
    r = f()
    return time.perf_counter() - t, r


def digest(body):
    """An answer's content, its numbers to 6 significant digits (each viewer prints numbers its
    own way)."""
    def norm(v):
        if isinstance(v, float):
            return float(f"{v:.6g}")
        if isinstance(v, list):
            return [norm(x) for x in v]
        if isinstance(v, dict):
            return {k: norm(x) for k, x in sorted(v.items())}
        return v
    return hashlib.sha256(json.dumps(norm(json.loads(body)), sort_keys=True).encode()).hexdigest()[:16]


def index(v, path, reindex):
    """Seconds from the request until the dataset is ready, its key and the answer."""
    t0 = time.perf_counter()
    st, b = v.req("POST", "/api/v1/open", {"path": path, "reindex": reindex})
    if st != 200:
        raise RuntimeError(f"open: {st} {b[:200]!r}")
    key = json.loads(b)["key"]
    while True:
        st, b = v.req("GET", f"/api/v1/datasets/{key}")
        d = json.loads(b)
        if d.get("state") == "ready":
            return time.perf_counter() - t0, key, d["dataset"]
        if st != 200 or d.get("state") in ("error", "cancelled"):
            raise RuntimeError(str(d.get("error") or d)[:300])
        time.sleep(0.01)


def measure(v, path, peak, repeat):
    runs = []
    for i in range(repeat):
        dt, key, ds = index(v, path, i > 0)
        runs.append(dt)
    out = {"index_s": statistics.median(runs), "index_runs": runs, "peak_bytes": peak.peak,
           "format": ds.get("format"), "description": ds.get("description") if ds.get("kind") == "stp" else None,
           "channels": len(ds["channels"]), "pieces": len(ds["pieces"]), "samples": sum(p["n"] for p in ds["pieces"]),
           "vitals": len(ds["vitals"]), "alarms": len(ds.get("alarms") or []),
           "markers": (ds.get("markers") or {}).get("count", 0)}
    a, z = ds["t_range"]
    out["span_s"] = z - a
    ch = ",".join(str(i) for i in range(len(ds["channels"])))

    def reopen():
        v.req("POST", "/api/v1/open", {"path": path})
        return v.get(f"/api/v1/datasets/{key}")
    out["reopen_s"] = statistics.median(timed(reopen)[0] for _ in range(5))
    digests = {}
    times = []
    for _ in range(5):
        dt, body = timed(lambda: v.get(f"/api/v1/datasets/{key}/window?t0={a!r}&t1={z!r}&px=1200&ch={ch}"))
        times.append(dt)
    out["overview_s"], digests["overview"] = statistics.median(times), digest(body)
    rnd = random.Random(7)
    for name, w in WINDOWS:
        times, dg = [], hashlib.sha256()
        for _ in range(20):
            s = a + rnd.random() * max(0.0, z - a - w)
            dt, body = timed(lambda: v.get(f"/api/v1/datasets/{key}/window?t0={s!r}&t1={s + w!r}&px=1200&ch={ch}"))
            times.append(dt)
            dg.update(digest(body).encode())
        out[f"window_{name}_s"] = statistics.median(times)
        digests[name] = dg.hexdigest()[:16]
    if ds["vitals"]:
        names = urllib.parse.quote("|".join(x["name"] for x in ds["vitals"]))
        times = []
        for _ in range(5):
            dt, body = timed(lambda: v.get(f"/api/v1/datasets/{key}/vitals?t0={a!r}&t1={z!r}&px=1200&names={names}"))
            times.append(dt)
        out["vitals_s"], digests["vitals"] = statistics.median(times), digest(body)
    out["digests"] = digests
    out["after_reads_bytes"] = tree_rss(v.p.pid)
    return out


def run(a):
    cmd, cwd = VIEWERS[a.viewer]
    if a.command:
        cmd, cwd = a.command.split(), None
    items = []
    for pair in a.recordings:
        name, sep, path = pair.partition("=")
        if not sep or not name or not path:
            sys.exit(f"bench-viewers: NAME=PATH, not {pair!r}")
        path = os.path.realpath(path)
        items.append((name, path, path if os.path.isdir(path) else os.path.dirname(path)))
    label = a.label or a.viewer
    results = {}
    with tempfile.TemporaryDirectory(prefix="bench-viewers-") as work:
        home, cache = os.path.join(work, "home"), os.path.join(work, "index")
        os.makedirs(home)
        with open(os.path.join(work, "viewer.log"), "w") as log:
            for name, path, root in items:
                shutil.rmtree(cache, ignore_errors=True)
                v = Viewer(cmd, cwd, a.port, cache, sorted({r for _, _, r in items}), a.adapters, home, log)
                base = tree_rss(v.p.pid)
                peak = Peak(v.p.pid)
                peak.start()
                try:
                    r = measure(v, path, peak, a.repeat)
                except (RuntimeError, OSError, ValueError, KeyError) as e:
                    r = {"error": f"{type(e).__name__}: {e}"[:300]}
                finally:
                    peak.stop = True
                    v.close()
                r["bytes"] = sum(os.path.getsize(os.path.join(dp, f)) for dp, _, fs in os.walk(root) for f in fs) \
                    if os.path.isdir(path) else _record_bytes(path)
                r["startup_bytes"] = base
                results[name] = r
                if "error" in r:
                    print(f"[{label}] {name}: {r['error']}", flush=True)
                else:
                    print(f"[{label}] {name}: index {r['index_s']:.2f} s, overview {r['overview_s'] * 1e3:.0f} ms, "
                          f"10 min {r['window_10 min_s'] * 1e3:.0f} ms, 10 s {r['window_10 s_s'] * 1e3:.0f} ms, "
                          f"peak {r['peak_bytes'] / 1e6:.0f} MB", flush=True)
    every = {}
    if os.path.exists(a.out):
        with open(a.out) as f:
            every = json.load(f)
    every[label] = {"viewer": a.viewer, "adapters": a.adapters, "results": results}
    with open(a.out, "w") as f:
        json.dump(every, f, indent=1)


def _record_bytes(path):
    """A recording's size: a WFDB header's folder entries of the same record, or the file."""
    if not path.lower().endswith(".hea"):
        return os.path.getsize(path)
    d, stem = os.path.dirname(path), os.path.splitext(os.path.basename(path))[0]
    return sum(os.path.getsize(os.path.join(d, f)) for f in os.listdir(d) if os.path.splitext(f)[0] == stem)


def report(a):
    with open(a.results) as f:
        every = json.load(f)
    labels = list(every)
    names = list(dict.fromkeys(n for lab in labels for n in every[lab]["results"]))
    print("| Recording | Viewer | Size | Index | Open again | Overview | 10 min | 10 s | Peak memory |")
    print("|---|---|---|---|---|---|---|---|---|")
    for n in names:
        for lab in labels:
            r = every[lab]["results"].get(n)
            if r is None:
                continue
            if "error" in r:
                print(f"| {n} | {lab} | | {r['error']} | | | | | |")
                continue
            print(f"| {n} | {lab} | {r['bytes'] / 1e6:.0f} MB | {r['index_s']:.2f} s | {r['reopen_s'] * 1e3:.0f} ms | "
                  f"{r['overview_s'] * 1e3:.0f} ms | {r['window_10 min_s'] * 1e3:.0f} ms | {r['window_10 s_s'] * 1e3:.0f} ms | "
                  f"{r['peak_bytes'] / 1e6:.0f} MB |")
    print()
    for n in names:
        got = [every[lab]["results"][n].get("digests") for lab in labels if n in every[lab]["results"]]
        got = [g for g in got if g]
        same = all(g == got[0] for g in got)
        print(f"{n}: {'the same data from every viewer' if same else 'THE VIEWERS ANSWERED DIFFERENT DATA'} ({len(got)})")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="what", required=True)
    m = sub.add_parser("make-data", help="write the synthetic recordings into a folder")
    m.add_argument("folder")
    r = sub.add_parser("run", help="measure one viewer on recordings given as NAME=PATH")
    r.add_argument("recordings", nargs="+", metavar="NAME=PATH")
    r.add_argument("--viewer", required=True, choices=sorted(VIEWERS))
    r.add_argument("--command", help="the viewer's program and its first arguments, in place of this clone's")
    r.add_argument("--adapters", help="an adapter folder (the viewers' --adapters-dir), for the files adapters read")
    r.add_argument("--label", help="the results' name in --out (default: the viewer)")
    r.add_argument("--out", required=True, help="a JSON file; results under other labels are kept")
    r.add_argument("--repeat", type=int, default=3, help="indexing runs per recording (default 3; the median is kept)")
    r.add_argument("--port", type=int, default=8391)
    p = sub.add_parser("report", help="print the results as a Markdown table")
    p.add_argument("results")
    a = ap.parse_args(argv)
    if a.what == "make-data":
        make_data(a.folder)
    elif a.what == "run":
        run(a)
    else:
        report(a)


if __name__ == "__main__":
    main()
