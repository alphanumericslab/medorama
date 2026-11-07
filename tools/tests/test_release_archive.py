"""Tests of tools/release-archive.py: the same files give the same archive, byte for byte,
whenever and wherever they are packed.  The Python standard library only:

    python3 -m unittest discover -s tools/tests
"""

import importlib.util
import os
import pathlib
import sys
import tarfile
import tempfile
import unittest
import zipfile
from unittest import mock

sys.dont_write_bytecode = True
TOOLS = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("release_archive", TOOLS / "release-archive.py")
archive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive)


def folder(root, mtime):
    """A release folder (a program, a text, a subfolder) whose files carry the time mtime."""
    d = pathlib.Path(root, "medorama-v0.1.0-x86_64-unknown-linux-gnu")
    (d / "docs").mkdir(parents=True)
    (d / "medorama").write_bytes(b"\x7fELF program")
    (d / "medorama").chmod(0o775)
    (d / "README.md").write_text("Medorama\n")
    (d / "docs" / "LICENSE-wfdb-x").write_text("GPL\n")
    for p in [d, *d.rglob("*")]:
        os.utime(p, (mtime, mtime))
    return d


class Archives(unittest.TestCase):
    def pack(self, name, mtime):
        with tempfile.TemporaryDirectory() as root:
            out = pathlib.Path(root, name)
            with mock.patch.dict(os.environ, {"SOURCE_DATE_EPOCH": "1700000000"}):
                with mock.patch.object(sys, "argv", ["release-archive.py", str(out), str(folder(root, mtime))]):
                    archive.main()
            return out.read_bytes()

    def test_the_same_bytes_from_files_of_other_times(self):
        for name in ("a.tar.gz", "a.zip"):
            with self.subTest(name):
                self.assertEqual(self.pack(name, 1_600_000_000), self.pack(name, 1_650_000_000))

    def test_the_entries(self):
        with tempfile.TemporaryDirectory() as root:
            data = self.pack("a.tar.gz", 1_600_000_000)
            pathlib.Path(root, "a.tar.gz").write_bytes(data)
            with tarfile.open(pathlib.Path(root, "a.tar.gz")) as tar:
                members = tar.getmembers()
        names = [m.name for m in members]
        self.assertEqual(names, sorted(names))
        self.assertEqual({m.mtime for m in members}, {1_700_000_000})
        self.assertEqual({(m.uid, m.gid, m.uname, m.gname) for m in members}, {(0, 0, "", "")})
        modes = {m.name.split("/")[-1]: m.mode for m in members}
        self.assertEqual(modes["medorama"], 0o755)
        self.assertEqual(modes["README.md"], 0o644)

    def test_a_zip_holds_the_files(self):
        with tempfile.TemporaryDirectory() as root:
            p = pathlib.Path(root, "a.zip")
            p.write_bytes(self.pack("a.zip", 1_600_000_000))
            with zipfile.ZipFile(p) as z:
                self.assertEqual(z.read("medorama-v0.1.0-x86_64-unknown-linux-gnu/README.md"), b"Medorama\n")
                self.assertIsNone(z.testzip())


if __name__ == "__main__":
    unittest.main()
