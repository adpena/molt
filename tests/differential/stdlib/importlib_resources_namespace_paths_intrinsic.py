"""Purpose: validate intrinsic-backed namespace package path discovery in importlib.resources."""

import importlib.resources
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    base_one = root / "base_one"
    base_two = root / "base_two"
    (base_one / "nsdemo" / "pkg").mkdir(parents=True)
    (base_two / "nsdemo" / "pkg").mkdir(parents=True)
    (base_one / "regdemo").mkdir(parents=True)
    (base_one / "regdemo" / "__init__.py").write_text("x = 1\n", encoding="utf-8")

    original = list(sys.path)
    try:
        sys.path[:] = [str(base_one), str(base_two)]
        traversable = importlib.resources.files("nsdemo.pkg")
        resolved = pathlib.Path(traversable.__fspath__())
        print("name", traversable.name)
        print("is_dir", traversable.is_dir())
        print("tail", resolved.parts[-2:])
        reg = importlib.resources.files("regdemo")
        reg_root = pathlib.Path(reg.__fspath__())
        print("reg_name", reg.name)
        print("reg_tail", reg_root.parts[-1:])
    finally:
        sys.path[:] = original
