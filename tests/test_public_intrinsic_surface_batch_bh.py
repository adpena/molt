from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/tkinter/__init__.py",
    "src/molt/stdlib/tkinter/__main__.py",
    "src/molt/stdlib/tkinter/_support.py",
    "src/molt/stdlib/tkinter/colorchooser.py",
    "src/molt/stdlib/tkinter/commondialog.py",
    "src/molt/stdlib/tkinter/constants.py",
    "src/molt/stdlib/tkinter/dialog.py",
    "src/molt/stdlib/tkinter/dnd.py",
    "src/molt/stdlib/tkinter/filedialog.py",
    "src/molt/stdlib/tkinter/font.py",
    "src/molt/stdlib/tkinter/messagebox.py",
    "src/molt/stdlib/tracemalloc.py",
    "src/molt/stdlib/tty.py",
    "src/molt/stdlib/venv/__init__.py",
    "src/molt/stdlib/wave.py",
    "src/molt/stdlib/webbrowser.py",
    "src/molt/stdlib/wsgiref/__init__.py",
    "src/molt/stdlib/wsgiref/headers.py",
    "src/molt/stdlib/wsgiref/simple_server.py",
    "src/molt/stdlib/wsgiref/util.py",
]

INTRINSIC_GLOBALS_RE = re.compile(
    r"require_intrinsic\([^)]*,\s*globals\(\)\s*\)", re.DOTALL
)


def test_tranche_files_do_not_bind_intrinsics_via_globals() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert INTRINSIC_GLOBALS_RE.search(text) is None, rel_path
