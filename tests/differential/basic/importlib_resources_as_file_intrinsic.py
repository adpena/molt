"""Purpose: validate importlib.resources.as_file intrinsic enter/exit wiring."""

import importlib.resources
import os
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    pkg = root / "as_file_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("x = 1\n", encoding="utf-8")
    data = pkg / "data.txt"
    data.write_text("payload\n", encoding="utf-8")

    original_path = list(sys.path)
    try:
        sys.path.insert(0, str(root))
        traversable = importlib.resources.files("as_file_pkg").joinpath("data.txt")
        with importlib.resources.as_file(traversable) as resolved:
            print("resource-fspath", os.fspath(resolved).endswith("data.txt"))
            print(
                "resource-data",
                pathlib.Path(os.fspath(resolved)).read_text(encoding="utf-8").strip(),
            )
        with importlib.resources.as_file(data) as resolved:
            print("pathlike-fspath", os.fspath(resolved).endswith("data.txt"))
            print(
                "pathlike-data",
                pathlib.Path(os.fspath(resolved)).read_text(encoding="utf-8").strip(),
            )
    finally:
        sys.path[:] = original_path
