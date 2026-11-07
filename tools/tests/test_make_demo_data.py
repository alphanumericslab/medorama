"""Tests of tools/make-demo-data.py: the demo is the same, byte for byte, whenever it is made.
The Python standard library only:

    python3 -m unittest discover -s tools/tests
"""

import importlib.util
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
TOOLS = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("make_demo_data", TOOLS / "make-demo-data.py")
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)


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


if __name__ == "__main__":
    unittest.main()
