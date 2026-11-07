"""Writes seeds/dicom/deep_sequences.dcm: a synthetic DICOM file whose sequences nest DEPTH deep
(undefined lengths, explicit VR little endian), for the dicom target to try the parser's depth
limit.  Usage: python3 deep_dicom.py [DEPTH] (default 200;
Medorama refuses sequences nested more than 32 deep)."""
import pathlib
import struct
import sys

depth = int(sys.argv[1]) if len(sys.argv) > 1 else 200
syntax = b"1.2.840.10008.1.2.1\0"  # explicit VR little endian
uid = struct.pack("<HH2sH", 2, 0x10, b"UI", len(syntax)) + syntax
meta = struct.pack("<HH2sHI", 2, 0, b"UL", 4, len(uid)) + uid
open_seq = struct.pack("<HH2sHI", 0x0008, 0x1115, b"SQ", 0, 0xFFFFFFFF)  # Referenced Series Sequence
item, item_end, seq_end = (struct.pack("<HHI", 0xFFFE, t, n) for t, n in ((0xE000, 0xFFFFFFFF), (0xE00D, 0), (0xE0DD, 0)))
name = struct.pack("<HH2sH", 0x0010, 0x0010, b"PN", 12) + b"Test^Person "
body = name + (open_seq + item) * depth + (item_end + seq_end) * depth
out = pathlib.Path(__file__).parent / "seeds" / "dicom" / "deep_sequences.dcm"
out.write_bytes(b"\0" * 128 + b"DICM" + meta + body)
print(f"{out}: {depth} levels, {out.stat().st_size} bytes")
