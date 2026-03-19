from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
MODULE_PATHS = [
    ROOT / "src/molt/stdlib/codeop.py",
    ROOT / "src/molt/stdlib/collections/abc.py",
    ROOT / "src/molt/stdlib/importlib/_abc.py",
    ROOT / "src/molt/stdlib/importlib/abc.py",
    ROOT / "src/molt/stdlib/importlib/readers.py",
    ROOT / "src/molt/stdlib/importlib/simple.py",
    ROOT / "src/molt/stdlib/importlib/util.py",
    ROOT / "src/molt/stdlib/importlib/resources/__init__.py",
    ROOT / "src/molt/stdlib/importlib/resources/_adapters.py",
    ROOT / "src/molt/stdlib/importlib/resources/_common.py",
    ROOT / "src/molt/stdlib/importlib/resources/_functional.py",
    ROOT / "src/molt/stdlib/importlib/resources/_itertools.py",
    ROOT / "src/molt/stdlib/importlib/resources/_legacy.py",
    ROOT / "src/molt/stdlib/importlib/resources/abc.py",
    ROOT / "src/molt/stdlib/importlib/resources/readers.py",
    ROOT / "src/molt/stdlib/importlib/resources/simple.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_collections.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_functools.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_itertools.py",
    ROOT / "src/molt/stdlib/importlib/metadata/_meta.py",
]


def test_public_intrinsic_surface_batch_az_avoids_globals_injection() -> None:
    for path in MODULE_PATHS:
        source = path.read_text()
        for line in source.splitlines():
            if "require_intrinsic(" in line:
                assert "globals()" not in line, path
