"""Tests of tools/make-demo-data.py: the demo is the same, byte for byte, for the same seed; it
holds every feature tools/README.md lists ("The demo's features"); it holds no identifier-like
string of a local file; the demo library and the demo stay within their sizes.

    python3 -B -m unittest discover -s tools/tests

The Excel, feature-table and library tests need the standard library only; the others make the
demo, with NumPy and wfdb-x (Medorama's environment: .venv/bin/python -B -m unittest discover -s
tools/tests), and are skipped without them.
"""

import hashlib
import importlib.util
import json
import pathlib
import re
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
TOOLS = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("make_demo_data", TOOLS / "make-demo-data.py")
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)
LIBRARY = TOOLS / "demo" / "library"

try:
    import numpy  # noqa: F401
    import wfdbx  # noqa: F401
    CAN_MAKE = True
except ImportError:
    CAN_MAKE = False

# strings no demo file may hold: the names of local folders of real files, of real recordings'
# formats, of public collections' patient identifiers
FORBIDDEN = [b"sample_data", b"sample-ecg", b"sample-dicom", b".Stp", b".stp", b"LIDC-IDRI-0", b"PANCREAS_0",
             b"KiTS-0", b"case_00", b"C3N-", b"TCGA-"]


def make(out, *args):
    with mock.patch.object(sys, "argv", ["make-demo-data.py", str(out), *args]), mock.patch("builtins.print"):
        demo.main()


def digest(folder):
    """{path: sha256} of a demo's files, but README.txt (which names its folder)."""
    return {str(p.relative_to(folder)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(folder.rglob("*")) if p.is_file() and p.name != "README.txt"}


def feature_table():
    """The keys of tools/README.md's table "The demo's features"."""
    text = (TOOLS / "README.md").read_text(encoding="utf-8")
    section = text.split("## The demo's features", 1)[1].split("\n## ", 1)[0]
    return re.findall(r"^\| `([a-z0-9-]+)` \|", section, flags=re.M)


class Workbook(unittest.TestCase):
    def test_the_same_bytes_at_another_time(self):
        rows = [["Encounter ID", "Patient ID"], ["E1", "P1"]]
        with tempfile.TemporaryDirectory() as d:
            a, b = pathlib.Path(d, "a.xlsx"), pathlib.Path(d, "b.xlsx")
            with mock.patch("time.time", return_value=1_700_000_000):
                demo.write_xlsx(a, rows)
            with mock.patch("time.time", return_value=1_800_000_000):
                demo.write_xlsx(b, rows)
            self.assertEqual(a.read_bytes(), b.read_bytes())


class Features(unittest.TestCase):
    def test_the_table_lists_every_feature(self):
        keys = feature_table()
        self.assertEqual(len(keys), len(set(keys)), "a feature listed twice in tools/README.md")
        self.assertEqual(set(keys), set(demo.FEATURES), "tools/README.md's table and make-demo-data.py's FEATURES differ")

    def test_identities_are_invented(self):
        people = demo.identities()
        self.assertGreaterEqual(len([p for p in people if p["patient_id"].startswith("P9")]), 36)
        for p in people:
            self.assertTrue(p["mrn"] == "" or p["mrn"].startswith("SYN"), p)
        self.assertEqual(demo.identities(), demo.identities(), "the same persons for the same seed")


class Library(unittest.TestCase):
    def test_sizes_and_manifest(self):
        if not (LIBRARY / "manifest.json").is_file():
            self.skipTest("no demo library in this clone")
        files = [p for p in LIBRARY.rglob("*") if p.is_file()]
        self.assertLessEqual(sum(p.stat().st_size for p in files), 30_000_000)
        for p in files:
            self.assertLess(p.stat().st_size, 5 * 1024 * 1024, p.name)
            for s in FORBIDDEN:
                self.assertNotIn(s, p.read_bytes(), f"{p.name} holds {s!r}")
        m = json.loads((LIBRARY / "manifest.json").read_text(encoding="utf-8"))
        named = {i["file"] for i in m["items"] if "file" in i}
        self.assertEqual(named, {str(p.relative_to(LIBRARY)).replace("\\", "/") for p in files if p.suffix in (".i16", ".u16")})
        for i in m["items"]:
            if "file" in i:
                n = 1
                for k in i["shape"]:
                    n *= k
                self.assertEqual((LIBRARY / i["file"]).stat().st_size, 2 * n, i["id"])

        # every text of the manifest is the library's own vocabulary: kinds, ids, files, units,
        # leads, regions, collection numbers
        def strings(v):
            if isinstance(v, str):
                yield v
            elif isinstance(v, dict):
                for x in v.values():
                    yield from strings(x)
            elif isinstance(v, list):
                for x in v:
                    yield from strings(x)
        for s in strings(m["items"]):
            self.assertRegex(s, r"^([a-z0-9_]+(-[0-9]{2}|-model)?|[a-z0-9]+/[a-z0-9]+-[0-9]{2}\.[iu]16|uV|int16|uint16|"
                                r"I|II|III|aVR|aVL|aVF|V[1-6]?|C[0-9]|chest|abdomen)$")


@unittest.skipUnless(CAN_MAKE, "NumPy and wfdb-x are needed to make the demo")
class Made(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.a, cls.b = pathlib.Path(cls.tmp.name, "a"), pathlib.Path(cls.tmp.name, "b")
        make(cls.a)
        make(cls.b)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_the_same_demo_for_the_same_seed(self):
        da, db = digest(self.a), digest(self.b)
        self.assertEqual(set(da), set(db))
        self.assertEqual([k for k in da if da[k] != db[k]], [])

    def test_another_seed_another_demo(self):
        with tempfile.TemporaryDirectory() as d:
            make(pathlib.Path(d), "--small", "--seed", "7")
            self.assertNotEqual(digest(pathlib.Path(d)).get("databases/labs.csv"), digest(self.a)["databases/labs.csv"])

    def test_every_feature(self):
        failed = [k for k, f in demo.FEATURES.items() if not f["check"](self.a)]
        self.assertEqual(failed, [])

    def test_what_went_in(self):
        m = json.loads((self.a / "demo.json").read_text(encoding="utf-8"))
        library = (LIBRARY / "manifest.json").is_file()
        self.assertEqual((m["library"], m["invented"], m["examples"], m["dicom"]), (library, not library, False, False))
        dbs = json.loads((self.a / "demo-databases.json").read_text(encoding="utf-8"))["databases"]
        self.assertEqual([d["name"] for d in dbs], ["demo", "demo_north", "demo_omop", "demo_registry", "demo_muse", "demo_secure"])
        for d in dbs:
            for step in d.get("build", []):
                for arg in step:
                    self.assertFalse(arg.startswith("@") and pathlib.Path(arg[1:]).is_absolute(), arg)

    def test_no_identifier_of_a_local_file(self):
        for p in self.a.rglob("*"):
            if not p.is_file() or p.name == "README.txt" or "actions" in p.parts:
                continue          # README.txt and the plug-in's command name this computer's folders
            b = p.read_bytes()
            for s in FORBIDDEN:
                self.assertNotIn(s, b, f"{p.relative_to(self.a)} holds {s!r}")
        # every MRN of every table is a demo's (SYN, in any letter case), every name invented
        names = set(demo.LAST) | {p[2] for p in demo.PATIENTS} | {"Mbeki", "Varga", "Aalto"}
        for table in self.a.rglob("*.csv"):
            for row in demo._rows(table.parent, table.name):
                for k, v in row.items():
                    if k and k.upper() == "MRN" and v:
                        self.assertTrue(v.upper().startswith("SYN"), (table.name, v))
                    if k == "Last Name":
                        self.assertIn(v, names, table.name)

    def test_sizes(self):
        total = sum(p.stat().st_size for p in self.a.rglob("*") if p.is_file())
        self.assertLess(total, 400_000_000)

    def test_the_small_demo(self):
        with tempfile.TemporaryDirectory() as d:
            out = pathlib.Path(d)
            make(out, "--small", "--no-library")
            m = json.loads((out / "demo.json").read_text(encoding="utf-8"))
            self.assertEqual((m["full"], m["invented"], m["library"]), (False, True, False))
            dbs = json.loads((out / "demo-databases.json").read_text(encoding="utf-8"))["databases"]
            self.assertEqual([d["name"] for d in dbs], ["demo", "demo_omop"])
            self.assertEqual(len(demo._rows(out, "databases/patients.csv")), 6)


if __name__ == "__main__":
    unittest.main()
