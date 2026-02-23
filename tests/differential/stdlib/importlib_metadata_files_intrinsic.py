"""Purpose: validate intrinsic-backed importlib.metadata files() RECORD lowering."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()

    package = site / "demo_files_pkg"
    package.mkdir()
    (package / "__init__.py").write_text("value = 7\n", encoding="utf-8")
    (package / "data.txt").write_text("payload\n", encoding="utf-8")

    dist = site / "demo_files_pkg-1.0.dist-info"
    dist.mkdir()
    (dist / "METADATA").write_text(
        "Name: demo-files-pkg\nVersion: 1.0\n",
        encoding="utf-8",
    )
    (dist / "RECORD").write_text(
        "demo_files_pkg/__init__.py,sha256=abc123,10\n"
        "demo_files_pkg/data.txt,sha256=def456,8\n"
        "demo_files_pkg-1.0.dist-info/METADATA,,\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))
        files = importlib.metadata.files("demo-files-pkg")
        print("is_none", files is None)
        entries = list(files or [])
        print("count", len(entries))
        names = sorted(str(entry) for entry in entries)
        print("has_init", "demo_files_pkg/__init__.py" in names)
        print("has_data", "demo_files_pkg/data.txt" in names)

        target = next(
            (entry for entry in entries if str(entry) == "demo_files_pkg/data.txt"),
            None,
        )
        print("target_type", type(target).__name__ if target is not None else None)

        if target is not None:
            hash_obj = getattr(target, "hash", None)
            print("hash_mode", getattr(hash_obj, "mode", None))
            print("hash_value", getattr(hash_obj, "value", None))
            print("size", getattr(target, "size", None))
            dist_obj = getattr(target, "dist", None)
            dist_name = None
            if dist_obj is not None:
                try:
                    dist_name = dist_obj.metadata["Name"]
                except Exception:
                    dist_name = getattr(dist_obj, "_name", None)
            print("dist_name", dist_name)
            try:
                text_value = target.read_text().strip()
            except Exception as exc:
                text_value = type(exc).__name__
            print("read_text", text_value)
    finally:
        sys.path[:] = original
