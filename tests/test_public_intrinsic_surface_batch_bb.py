from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/logging/config.py",
    ROOT / "src/molt/stdlib/concurrent/__init__.py",
    ROOT / "src/molt/stdlib/html/__init__.py",
    ROOT / "src/molt/stdlib/importlib/__init__.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_text.py",
    ROOT / "src/molt/stdlib/socketserver.py",
    ROOT / "src/molt/stdlib/stringprep.py",
    ROOT / "src/molt/stdlib/weakref.py",
    ROOT / "src/molt/stdlib/importlib/machinery.py",
    ROOT / "src/molt/stdlib/urllib/request.py",
    ROOT / "src/molt/stdlib/ctypes/__init__.py",
    ROOT / "src/molt/stdlib/http/cookiejar.py",
    ROOT / "src/molt/stdlib/importlib/metadata/__init__.py",
    ROOT / "src/molt/stdlib/string/__init__.py",
    ROOT / "src/molt/stdlib/typing.py",
    ROOT / "src/molt/stdlib/urllib/error.py",
    ROOT / "src/molt/stdlib/ast.py",
    ROOT / "src/molt/stdlib/secrets.py",
    ROOT / "src/molt/stdlib/textwrap.py",
    ROOT / "src/molt/stdlib/traceback.py",
]


def test_public_intrinsic_surface_batch_bb_avoids_globals_injection() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        for line in source.splitlines():
            if "require_intrinsic(" in line:
                assert "globals()" not in line, path
