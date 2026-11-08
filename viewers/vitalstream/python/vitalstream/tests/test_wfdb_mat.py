"""WFDB records whose signal file is a MATLAB file (.mat, docs/wfdb-collection.md, "MATLAB
signal files"), the checks of the Rust engine's tests/wfdb_mat.rs with the same messages: level 4
and level 5 files are read as WFDB reads them, from the header's byte offset, and give the
samples of the same record in a .dat file; a MAT file that does not match its header
(compressed, of doubles, transposed, of another offset or length) is refused; and a signal file
opened in place of its header says to open the header."""

import struct
from pathlib import Path

import pytest

from vitalstream_py.index import build_index, load_index
from vitalstream_py.sources import kind_of, wfdb_folder

needs_wfdb = pytest.mark.skipif(not wfdb_folder.SUPPORTED,
                                reason='WFDB records: an installation without wfdb-x refuses them')

# two signals of eight samples each: frame by frame, as a .dat file and a MAT matrix of two
# rows (MATLAB stores a matrix column by column) both hold them
SAMPLES = [(0, 800), (10, 810), (20, 790), (-15, 805), (5, 800), (30, 820), (-40, 780), (0, 800)]


def frames(order='<'):
    return b''.join(struct.pack(order + 'hh', *f) for f in SAMPLES)


def checksum(i):
    c = sum(f[i] for f in SAMPLES) & 0xffff
    return c - 0x10000 if c >= 0x8000 else c


def header(d, name, file, fmt, nsamp):
    Path(d, f'{name}.hea').write_text(
        f'{name} 2 250 {nsamp} 11:00:00 01/03/2024\n'
        f'{file} {fmt} 200/mV 16 0 0 {checksum(0)} 0 II\n'
        f'{file} {fmt} 10/mmHg 16 0 800 {checksum(1)} 0 ART\n')


def mat4(ty, rows, cols, data, be=False):
    """A level 4 MAT file: its type (MOPT), rows, columns and the name `val`, then `data`."""
    return struct.pack(('>' if be else '<') + '5I', ty, rows, cols, 0, 4) + b'val\0' + data


def mat5(rows, cols, data, small_name=False, cls=10, elem=3, version=0x0100):
    """A level 5 MAT file of one matrix (little-endian): as wfdb2mat writes it (the name in a
    full data element: the samples start at byte 192), or as MATLAB does (the name in a small
    one: at byte 184)."""
    head = bytearray(128)
    head[:10] = b'MATLAB 5.0'
    head[124:128] = struct.pack('<H', version) + b'IM'
    name = struct.pack('<I', (3 << 16) | 1) + b'val\0' if small_name else struct.pack('<II', 1, 3) + b'val\0\0\0\0\0'
    body = (struct.pack('<II', 6, 8) + bytes([cls, 0, 0, 0, 0, 0, 0, 0]) + struct.pack('<IIII', 5, 8, rows, cols)
            + name + struct.pack('<II', elem, len(data)) + data + b'\0' * (-len(data) % 8))
    return bytes(head) + struct.pack('<II', 14, len(body)) + body


def arrays_of(hea, cache):
    """The index arrays of a record, to compare two records holding the same samples."""
    _, arrays, _ = load_index(str(cache), build_index(str(hea), str(cache)))
    return {k: bytes(memoryview(v)) for k, v in sorted(arrays.items())}


def refusal(hea, cache):
    with pytest.raises(Exception) as e:
        build_index(str(hea), str(cache))
    return str(e.value)


@needs_wfdb
def test_mat_files_are_read_as_the_dat_file(tmp_path):
    d, cache = tmp_path, tmp_path / 'cache'
    (d / 'r.dat').write_bytes(frames())
    header(d, 'r', 'r.dat', '16', 8)
    # level 4 (the PhysioNet Challenge files: "16+24"), level 5 as wfdb2mat writes it ("+192")
    # and as MATLAB does ("+184"), and big-endian (format 61)
    (d / 'm4.mat').write_bytes(mat4(30, 2, 8, frames()))
    header(d, 'm4', 'm4.mat', '16+24', 8)
    (d / 'm5.mat').write_bytes(mat5(2, 8, frames()))
    header(d, 'm5', 'm5.mat', '16+192', 8)
    (d / 'ml.mat').write_bytes(mat5(2, 8, frames(), small_name=True))
    header(d, 'ml', 'ml.mat', '16+184', 8)
    (d / 'mb.mat').write_bytes(mat4(1030, 2, 8, frames('>'), be=True))
    header(d, 'mb', 'mb.mat', '61+24', 8)
    dat = arrays_of(d / 'r.hea', cache)
    for name in ('m4', 'm5', 'ml', 'mb'):
        assert arrays_of(d / f'{name}.hea', cache) == dat, name


@needs_wfdb
def test_a_mat_file_that_does_not_match_its_header_is_refused(tmp_path):
    d, cache = tmp_path, tmp_path / 'cache'
    le = frames()
    doubles = b''.join(struct.pack('<dd', *f) for f in SAMPLES)
    compressed = bytearray(mat5(2, 8, le))
    compressed[128:132] = struct.pack('<I', 15)
    cases = [
        ('compressed', bytes(compressed), '16+192', 8, 'it is compressed'),
        ('hdf5', mat5(2, 8, le, version=0x0200), '16+192', 8, 'MATLAB 7.3 (HDF5)'),
        ('doubles', mat4(0, 2, 8, doubles), '16+24', 8,
         'it holds little-endian double-precision numbers; format 16 reads little-endian 16-bit integers'),
        ('doubles5', mat5(2, 8, doubles, cls=6, elem=9), '16+192', 8, 'double-precision'),
        ('transposed', mat4(30, 8, 2, le), '16+24', 8, 'it is transposed'),
        ('offset', mat4(30, 2, 8, le), '16+20', 8, 'its samples start at byte 24; the header gives +20'),
        ('no_offset', mat4(30, 2, 8, le), '16', 8, 'its samples start at byte 24; the header gives +0'),
        ('length', mat4(30, 2, 8, le), '16+24', 10, 'it holds 8 samples per signal; the header gives 10'),
        ('short', mat4(30, 2, 8, le[:20]), '16+24', 8, 'it is shorter than its 2 × 8 matrix'),
        ('order', mat4(1030, 2, 8, le, be=True), '16+24', 8,
         'it holds big-endian 16-bit integers; format 16 reads little-endian'),
        ('format', mat4(30, 2, 8, le), '212+24', 8, 'format 212, which no MAT matrix holds'),
        ('text', b'hello, world: not a MAT file at all', '16', 8, 'not a MAT file'),
    ]
    for name, data, fmt, nsamp, why in cases:
        (d / f'{name}.mat').write_bytes(data)
        header(d, name, f'{name}.mat', fmt, nsamp)
        e = refusal(d / f'{name}.hea', cache)
        assert f'{name}.mat does not match its header {name}.hea: ' in e and why in e, (name, e)
    # in a folder of records: left out with the reason, the others read
    (d / 'good.mat').write_bytes(mat4(30, 2, 8, le))
    header(d, 'good', 'good.mat', '16+24', 8)
    meta = load_index(str(cache), build_index(str(d), str(cache)))[0]
    assert 'transposed' in meta['skipped']['transposed']


def test_a_signal_file_says_to_open_its_header(tmp_path):
    d = tmp_path
    (d / 'r.dat').write_bytes(frames())
    header(d, 'r', 'r.dat', '16', 8)
    (d / 'lone.mat').write_bytes(mat4(30, 2, 8, frames()))
    # a header of the same name that names another file
    (d / 'other.dat').write_bytes(frames())
    header(d, 'other', 'r.dat', '16', 8)

    def note(f):
        return wfdb_folder.signal_file_note(d / f)

    assert note('r.dat') == 'r.dat is a WFDB signal file: open its header, r.hea'
    assert note('lone.mat') == 'lone.mat is a WFDB signal file: open the header (.hea) that names it'
    assert note('other.dat') == 'other.dat is a WFDB signal file: open the header (.hea) that names it'
    assert note('r.hea') is None
    assert note('missing.dat') is None
    assert kind_of(d / 'r.dat') is None
    assert refusal(d / 'r.dat', d / 'cache') == 'r.dat is a WFDB signal file: open its header, r.hea'
