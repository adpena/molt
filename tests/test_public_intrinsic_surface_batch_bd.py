from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/sys.py",
    "src/molt/stdlib/subprocess.py",
    "src/molt/stdlib/pathlib/__init__.py",
    "src/molt/stdlib/lzma.py",
    "src/molt/stdlib/urllib/parse.py",
    "src/molt/stdlib/os.py",
    "src/molt/stdlib/decimal.py",
    "src/molt/stdlib/logging/__init__.py",
    "src/molt/stdlib/threading.py",
    "src/molt/stdlib/zlib.py",
    "src/molt/stdlib/csv.py",
    "src/molt/stdlib/http/client.py",
    "src/molt/stdlib/fractions.py",
    "src/molt/stdlib/array.py",
    "src/molt/stdlib/select.py",
    "src/molt/stdlib/ssl.py",
    "src/molt/stdlib/enum.py",
    "src/molt/stdlib/datetime.py",
    "src/molt/stdlib/collections/__init__.py",
    "src/molt/stdlib/socket.py",
]

INTRINSIC_GLOBALS_RE = re.compile(r"require_intrinsic\(.{0,200}?globals\(\)", re.DOTALL)


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
