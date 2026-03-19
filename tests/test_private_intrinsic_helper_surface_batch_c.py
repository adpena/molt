from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/_interpreters.py",
    "src/molt/stdlib/_io.py",
    "src/molt/stdlib/_ios_support.py",
    "src/molt/stdlib/_json.py",
    "src/molt/stdlib/_locale.py",
    "src/molt/stdlib/_lsprof.py",
    "src/molt/stdlib/_lzma.py",
    "src/molt/stdlib/_markupbase.py",
    "src/molt/stdlib/_md5.py",
    "src/molt/stdlib/_msi.py",
    "src/molt/stdlib/_multibytecodec.py",
    "src/molt/stdlib/_multiprocessing.py",
    "src/molt/stdlib/_opcode.py",
    "src/molt/stdlib/_opcode_metadata.py",
    "src/molt/stdlib/_osx_support.py",
    "src/molt/stdlib/_overlapped.py",
    "src/molt/stdlib/_pickle.py",
    "src/molt/stdlib/_posixshmem.py",
    "src/molt/stdlib/_posixsubprocess.py",
    "src/molt/stdlib/_py_abc.py",
]


def test_tranche_files_drop_stale_require_intrinsic_helper() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert 'globals().pop("_require_intrinsic", None)' in text, rel_path
