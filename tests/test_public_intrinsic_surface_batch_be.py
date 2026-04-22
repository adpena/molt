from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/bdb.py",
    "src/molt/stdlib/cProfile.py",
    "src/molt/stdlib/calendar.py",
    "src/molt/stdlib/cmd.py",
    "src/molt/stdlib/code.py",
    "src/molt/stdlib/compileall.py",
    "src/molt/stdlib/compression/_common/_streams.py",
    "src/molt/stdlib/curses/__init__.py",
    "src/molt/stdlib/dbm/__init__.py",
    "src/molt/stdlib/dis.py",
    "src/molt/stdlib/email/__init__.py",
    "src/molt/stdlib/email/_encoded_words.py",
    "src/molt/stdlib/email/header.py",
    "src/molt/stdlib/email/quoprimime.py",
    "src/molt/stdlib/encodings/aliases.py",
    "src/molt/stdlib/ensurepip/__init__.py",
    "src/molt/stdlib/faulthandler.py",
    "src/molt/stdlib/fcntl.py",
    "src/molt/stdlib/filecmp.py",
    "src/molt/stdlib/fileinput.py",
]

INTRINSIC_GLOBALS_RE = re.compile(
    r"require_intrinsic\([^)]*,\s*globals\(\)\s*\)", re.DOTALL
)


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
