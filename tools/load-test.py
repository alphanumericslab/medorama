#!/usr/bin/env python3
"""Many users at once on an Medorama server: how long its pages take while they all work.

Each simulated user signs in (a session of their own) and then, without pause, asks for what
people do most: a page of the Patient Catalog, a patient's summary, journey and report, a page
of a database's rows, a cohort search.  At the end it prints, for each kind of request, how many
were made, the median, the 95th percentile and the slowest time, and the errors.

Synthetic data only: it reads what the server answers and keeps nothing.  It changes nothing
(cohort searches are not saved); each sign-in is audited, as any.

    python3 tools/load-test.py --url http://127.0.0.1:8090 --user admin --users 20 --seconds 60
    (the password is read from $MEDORAMA_PASSWORD, else asked)

Standard library only (Python 3.8 or later).
"""

import argparse
import getpass
import http.client
import json
import os
import random
import statistics
import sys
import threading
import time
import urllib.parse

API = "/api/v1/"


class Session:
    """One user's signed-in session: one connection, its cookie."""

    def __init__(self, url, user, password):
        u = urllib.parse.urlsplit(url)
        conn = http.client.HTTPSConnection if u.scheme == "https" else http.client.HTTPConnection
        self.conn = conn(u.hostname, u.port or (443 if u.scheme == "https" else 80), timeout=120)
        self.cookie = ""
        status, _ = self.ask("POST", "login", {"username": user, "password": password})
        if status != 200:
            raise SystemExit(f"signing in as {user}: HTTP {status}")

    def ask(self, method, path, body=None):
        headers = {"X-Medorama": "1", "Cookie": self.cookie}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        self.conn.request(method, API + path, body=data, headers=headers)
        r = self.conn.getresponse()
        raw = r.read()
        for k, v in r.getheaders():
            if k.lower() == "set-cookie" and not self.cookie:
                self.cookie = v.split(";", 1)[0]
        try:
            return r.status, json.loads(raw) if raw else None
        except ValueError:
            return r.status, None


def setup(s):
    """What the requests pick from: patients' numbers, databases, a lab result's name."""
    _, first = s.ask("GET", "patients?limit=100")
    total = (first or {}).get("total", 0)
    if not total:
        raise SystemExit("the catalog has no patient: import some synthetic data first")
    ids = [p["id"] for p in first["items"]]
    _, far = s.ask("GET", f"patients?limit=100&offset={max(0, total // 2)}")
    ids += [p["id"] for p in (far or {}).get("items", [])]
    _, dbs = s.ask("GET", "databases")
    dbs = [d["id"] for d in (dbs or {}).get("items", []) if d.get("rows")]
    return {"total": total, "ids": ids, "dbs": dbs}


def requests_mix(ctx):
    """The kinds of requests, and how often each is made (weights)."""
    pid = lambda: random.choice(ctx["ids"])  # noqa: E731
    mix = [
        (4, "catalog page", lambda: ("GET", f"patients?limit=100&offset={random.randrange(0, max(1, ctx['total'] - 100))}", None)),
        (1, "catalog filter", lambda: ("GET", "patients?limit=100&has=labs", None)),
        (4, "patient summary", lambda: ("GET", f"patients/{pid()}", None)),
        (3, "patient journey", lambda: ("GET", f"patients/{pid()}/journey", None)),
        (2, "patient report", lambda: ("GET", f"patients/{pid()}/report", None)),
        (1, "cohort search", lambda: ("POST", "cohorts/search", {"criteria": {"version": 2, "include": {"kind": "expression", "text": random.choice(["female AND age > 60", "has labs", "male AND age < 40"])}}, "limit": 100})),
    ]
    if ctx["dbs"]:
        mix.append((2, "database rows", lambda: ("GET", f"databases/{random.choice(ctx['dbs'])}/rows?limit=100&offset={random.randrange(0, 1000)}", None)))
    return mix


def user(args, password, ctx, until, times, errors, lock):
    try:
        s = Session(args.url, args.user, password)
    except Exception as e:  # noqa: BLE001
        with lock:
            errors.setdefault("sign-in", []).append(str(e))
        return
    mix = requests_mix(ctx)
    weights = [w for w, _, _ in mix]
    while time.monotonic() < until:
        _, kind, make = random.choices(mix, weights)[0]
        method, path, body = make()
        t = time.monotonic()
        try:
            status, _ = s.ask(method, path, body)
            ok = status < 400
        except Exception as e:  # noqa: BLE001
            status, ok = str(e), False
            s = Session(args.url, args.user, password)
        dt = time.monotonic() - t
        with lock:
            times.setdefault(kind, []).append(dt)
            if not ok:
                errors.setdefault(kind, []).append(status)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--url", default="http://127.0.0.1:8090", help="the server (default %(default)s)")
    ap.add_argument("--user", default="admin", help="the account every simulated user signs in as")
    ap.add_argument("--users", type=int, default=20, help="users at once (default %(default)s)")
    ap.add_argument("--seconds", type=float, default=60, help="how long (default %(default)s)")
    ap.add_argument("--seed", type=int, default=1, help="the random choices' seed")
    args = ap.parse_args()
    random.seed(args.seed)
    password = os.environ.get("MEDORAMA_PASSWORD") or getpass.getpass(f"{args.user}'s password: ")
    ctx = setup(Session(args.url, args.user, password))
    times, errors, lock = {}, {}, threading.Lock()
    until = time.monotonic() + args.seconds
    threads = [threading.Thread(target=user, args=(args, password, ctx, until, times, errors, lock)) for _ in range(args.users)]
    start = time.monotonic()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    took = time.monotonic() - start
    n = sum(len(v) for v in times.values())
    print(f"{args.users} users for {took:.0f} s on {ctx['total']} patients: {n} requests, {n / took:.1f} a second")
    print(f"{'request':<18}{'count':>7}{'median':>9}{'95%':>9}{'slowest':>9}  errors")
    for kind in sorted(times):
        v = sorted(times[kind])
        p95 = v[min(len(v) - 1, int(0.95 * len(v)))]
        print(f"{kind:<18}{len(v):>7}{statistics.median(v):>8.2f}s{p95:>8.2f}s{v[-1]:>8.2f}s  {len(errors.get(kind, []))}")
    for kind, e in sorted(errors.items()):
        print(f"errors, {kind}: {sorted(set(map(str, e)))[:5]}", file=sys.stderr)
    sys.exit(1 if errors else 0)


if __name__ == "__main__":
    main()
