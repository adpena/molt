from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/email/message.py",
    "src/molt/stdlib/io.py",
    "src/molt/stdlib/zipfile/__init__.py",
    "src/molt/stdlib/hmac.py",
    "src/molt/stdlib/quopri.py",
    "src/molt/stdlib/dataclasses.py",
    "src/molt/stdlib/json/__init__.py",
    "src/molt/stdlib/tempfile.py",
    "src/molt/stdlib/unicodedata.py",
    "src/molt/stdlib/dbm/dumb.py",
    "src/molt/stdlib/gzip.py",
    "src/molt/stdlib/urllib/response.py",
    "src/molt/stdlib/configparser.py",
    "src/molt/stdlib/argparse.py",
    "src/molt/stdlib/inspect.py",
    "src/molt/stdlib/re/__init__.py",
    "src/molt/stdlib/shutil.py",
    "src/molt/stdlib/tarfile.py",
    "src/molt/stdlib/bz2.py",
    "src/molt/stdlib/concurrent/futures/__init__.py",
]


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        for line in text.splitlines():
            if "require_intrinsic(" in line:
                assert "globals()" not in line, rel_path
