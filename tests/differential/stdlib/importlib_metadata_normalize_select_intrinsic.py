"""Purpose: validate runtime-lowered importlib.metadata name normalization/select payload."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()
    dist = site / "demo.select_pkg-1.0.dist-info"
    dist.mkdir()
    (dist / "METADATA").write_text(
        "Name: demo.select_pkg\nVersion: 1.0\n", encoding="utf-8"
    )
    (dist / "entry_points.txt").write_text(
        "[console_scripts]\ndemo-select = demo_select:main\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))
        print("version_dash", importlib.metadata.version("demo-select-pkg"))
        print("version_mixed", importlib.metadata.version("demo__select...pkg"))
        selected = importlib.metadata.entry_points(
            group="console_scripts", name="demo-select"
        )
        print("selected", len(selected))
        if selected:
            print("selected_value", selected["demo-select"].value)
        try:
            importlib.metadata.entry_points(bad="value")
        except Exception as exc:  # noqa: BLE001 - differential type check
            print("unexpected_kw", type(exc).__name__)
    finally:
        sys.path[:] = original
