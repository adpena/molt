"""Purpose: validate intrinsic-backed traversable stat/listdir payload in importlib.resources."""

import importlib.resources
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    pkg = root / "payload_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("x = 1\n", encoding="utf-8")
    (pkg / "data.txt").write_text("hello\n", encoding="utf-8")

    original = list(sys.path)
    try:
        sys.path.insert(0, str(root))
        base = importlib.resources.files("payload_pkg")
        print("exists", base.exists())
        print("is_dir", base.is_dir())
        print("entries", sorted(entry.name for entry in base.iterdir()))
        print("resource", importlib.resources.is_resource("payload_pkg", "data.txt"))
    finally:
        sys.path[:] = original
