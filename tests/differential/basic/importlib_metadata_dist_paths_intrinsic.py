"""Purpose: validate intrinsic-backed importlib.metadata dist-info scanning and reads."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()
    dist = site / "demo_pkg-1.2.3.dist-info"
    dist.mkdir()
    (dist / "METADATA").write_text(
        "Name: demo-pkg\nVersion: 1.2.3\n", encoding="utf-8"
    )
    (dist / "entry_points.txt").write_text(
        "[console_scripts]\ndemo = demo_pkg:main\n", encoding="utf-8"
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))
        print("version", importlib.metadata.version("demo_pkg"))
        dist_obj = importlib.metadata.distribution("demo-pkg")
        print("metadata_name", dist_obj.metadata.get("Name"))
        entry_points = importlib.metadata.entry_points().select(
            group="console_scripts", name="demo"
        )
        print("entry_points", len(entry_points))
        if entry_points:
            print("entry_value", entry_points["demo"].value)
        text = dist_obj.read_text("METADATA") or ""
        print("read_head", text.splitlines()[0] if text else "")
    finally:
        sys.path[:] = original
