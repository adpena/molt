from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TRANCHE_FILES = [
    "src/molt/stdlib/__init__.py",
    "src/molt/stdlib/_bisect.py",
    "src/molt/stdlib/_collections_abc.py",
    "src/molt/stdlib/_ctypes.py",
    "src/molt/stdlib/_curses.py",
    "src/molt/stdlib/_curses_panel.py",
    "src/molt/stdlib/_datetime.py",
    "src/molt/stdlib/_dbm.py",
    "src/molt/stdlib/_decimal.py",
    "src/molt/stdlib/_elementtree.py",
    "src/molt/stdlib/_frozen_importlib.py",
    "src/molt/stdlib/_frozen_importlib_external.py",
    "src/molt/stdlib/_functools.py",
    "src/molt/stdlib/_gdbm.py",
    "src/molt/stdlib/_hashlib.py",
    "src/molt/stdlib/_heapq.py",
    "src/molt/stdlib/_hmac.py",
    "src/molt/stdlib/_imp.py",
    "src/molt/stdlib/_interpchannels.py",
    "src/molt/stdlib/_interpqueues.py",
]


def test_tranche_files_drop_stale_require_intrinsic_helper() -> None:
    for rel_path in TRANCHE_FILES:
        text = (ROOT / rel_path).read_text(encoding="utf-8")
        assert 'globals().pop("_require_intrinsic", None)' in text, rel_path
