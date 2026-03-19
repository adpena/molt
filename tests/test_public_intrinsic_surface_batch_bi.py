from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/tkinter/scrolledtext.py",
    "src/molt/stdlib/tkinter/simpledialog.py",
    "src/molt/stdlib/tkinter/tix.py",
    "src/molt/stdlib/tkinter/ttk.py",
]

INTRINSIC_GLOBALS_RE = re.compile(
    r'require_intrinsic\([^)]*,\s*globals\(\)\s*\)', re.DOTALL
)


def test_remaining_tkinter_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
