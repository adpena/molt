from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/email/policy.py",
    ROOT / "src/molt/stdlib/json/__main__.py",
    ROOT / "src/molt/stdlib/json/decoder.py",
    ROOT / "src/molt/stdlib/json/encoder.py",
    ROOT / "src/molt/stdlib/json/scanner.py",
    ROOT / "src/molt/stdlib/json/tool.py",
    ROOT / "src/molt/stdlib/multiprocessing/spawn.py",
    ROOT / "src/molt/stdlib/pathlib/_abc.py",
    ROOT / "src/molt/stdlib/pathlib/_local.py",
    ROOT / "src/molt/stdlib/pathlib/_os.py",
    ROOT / "src/molt/stdlib/pathlib/types.py",
    ROOT / "src/molt/stdlib/py_compile.py",
    ROOT / "src/molt/stdlib/re/_compiler.py",
    ROOT / "src/molt/stdlib/re/_constants.py",
    ROOT / "src/molt/stdlib/reprlib.py",
    ROOT / "src/molt/stdlib/unittest/__init__.py",
    ROOT / "src/molt/stdlib/urllib/__init__.py",
    ROOT / "src/molt/stdlib/xmlrpc/__init__.py",
    ROOT / "src/molt/stdlib/xmlrpc/client.py",
    ROOT / "src/molt/stdlib/xmlrpc/server.py",
]


def test_public_intrinsic_surface_batch_ba_avoids_globals_injection() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        for line in source.splitlines():
            if "require_intrinsic(" in line:
                assert "globals()" not in line, path
