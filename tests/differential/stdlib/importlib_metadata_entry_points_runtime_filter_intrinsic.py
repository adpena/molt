"""Purpose: validate runtime-filtered importlib.metadata entry points selectors."""

import importlib.metadata
import pathlib
import sys
import tempfile


def _sorted_triplets(entry_points):
    return sorted((ep.group, ep.name, ep.value) for ep in entry_points)


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()

    dist_a = site / "runtime_filter_a-1.0.dist-info"
    dist_a.mkdir()
    (dist_a / "METADATA").write_text(
        "Name: runtime-filter-a\nVersion: 1.0\n", encoding="utf-8"
    )
    (dist_a / "entry_points.txt").write_text(
        "[demo.plugins]\nalpha = pkg_a.plugin:alpha\nbeta = pkg_a.plugin:beta\n",
        encoding="utf-8",
    )

    dist_b = site / "runtime_filter_b-1.0.dist-info"
    dist_b.mkdir()
    (dist_b / "METADATA").write_text(
        "Name: runtime-filter-b\nVersion: 1.0\n", encoding="utf-8"
    )
    (dist_b / "entry_points.txt").write_text(
        "[demo.plugins]\nalpha = pkg_b.plugin:alpha\ngamma = pkg_b.plugin:gamma\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    patched = False
    original_select = None
    try:
        sys.path.insert(0, str(site))

        if hasattr(
            importlib.metadata, "_MOLT_IMPORTLIB_METADATA_ENTRY_POINTS_FILTER_PAYLOAD"
        ):
            original_select = importlib.metadata.EntryPoints.select

            def _guard_runtime_selector_select(self, **params):
                if params and set(params).issubset({"group", "name", "value"}):
                    raise RuntimeError("python_select_called_for_runtime_selectors")
                return original_select(self, **params)

            importlib.metadata.EntryPoints.select = _guard_runtime_selector_select
            patched = True

        runtime_filtered = importlib.metadata.entry_points(
            group="demo.plugins",
            name="alpha",
            value="pkg_b.plugin:alpha",
        )
        print("runtime_filtered", _sorted_triplets(runtime_filtered))

        value_filtered = importlib.metadata.entry_points(value="pkg_a.plugin:beta")
        print("value_filtered", _sorted_triplets(value_filtered))

        try:
            importlib.metadata.entry_points(group="demo.plugins", unsupported="x")
        except Exception as exc:  # noqa: BLE001 - differential type check
            print("unsupported_selector", type(exc).__name__)
    finally:
        if patched and original_select is not None:
            importlib.metadata.EntryPoints.select = original_select
        sys.path[:] = original
