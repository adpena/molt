"""Purpose: validate importlib.resources open/read helpers use intrinsic-backed file reads."""

import importlib.resources
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    pkg = root / "open_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("x = 1\n", encoding="utf-8")
    (pkg / "text.txt").write_text("hello text\n", encoding="utf-8")
    (pkg / "blob.bin").write_bytes(b"\\x00molt\\xff")

    original = list(sys.path)
    try:
        sys.path.insert(0, str(root))
        with importlib.resources.open_text("open_pkg", "text.txt") as handle:
            print("open_text", handle.read().strip())
        with importlib.resources.open_binary("open_pkg", "blob.bin") as handle:
            print("open_binary", handle.read())
        print(
            "read_text", importlib.resources.read_text("open_pkg", "text.txt").strip()
        )
        print("read_binary", importlib.resources.read_binary("open_pkg", "blob.bin"))
    finally:
        sys.path[:] = original
