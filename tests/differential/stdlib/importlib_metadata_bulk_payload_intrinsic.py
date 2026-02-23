"""Purpose: validate intrinsic-backed bulk metadata payload hydration across APIs."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()

    pkg = site / "bulk_payload_pkg_unique"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("value = 42\n", encoding="utf-8")
    (pkg / "data.txt").write_text("bulk-data\n", encoding="utf-8")

    dist = site / "bulk_payload_pkg_unique-1.2.3.dist-info"
    dist.mkdir()
    (dist / "METADATA").write_text(
        "Name: bulk-payload-pkg-unique\n"
        "Version: 1.2.3\n"
        "Summary: bulk cache\n"
        " hydration fixture\n"
        "Requires-Python: >=3.12\n"
        "Requires-Dist: dep-alpha>=1\n"
        "Requires-Dist: dep-beta; extra == \"dev\"\n"
        "Provides-Extra: dev\n",
        encoding="utf-8",
    )
    (dist / "entry_points.txt").write_text(
        "[bulk_payload_group_unique]\n"
        "bulk-tool = bulk_payload_pkg_unique:main\n",
        encoding="utf-8",
    )
    (dist / "top_level.txt").write_text(
        "bulk_payload_pkg_unique\nbulk_payload_shared_pkg_unique\n",
        encoding="utf-8",
    )
    (dist / "RECORD").write_text(
        "bulk_payload_pkg_unique/__init__.py,sha256=abc123,11\n"
        "bulk_payload_pkg_unique/data.txt,sha256=def456,10\n"
        "bulk_payload_pkg_unique-1.2.3.dist-info/METADATA,,\n",
        encoding="utf-8",
    )

    dist_two = site / "bulk_payload_other_pkg_unique-0.9.dist-info"
    dist_two.mkdir()
    (dist_two / "METADATA").write_text(
        "Name: bulk-payload-other-pkg-unique\nVersion: 0.9\n",
        encoding="utf-8",
    )
    (dist_two / "top_level.txt").write_text(
        "bulk_payload_other_pkg_unique\nbulk_payload_shared_pkg_unique\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))

        dist_obj = importlib.metadata.distribution("bulk-payload-pkg-unique")
        print("distribution_name", dist_obj.metadata.get("Name"))
        print("distribution_version", dist_obj.version)
        print("version_api", importlib.metadata.version("bulk_payload_pkg_unique"))

        meta = importlib.metadata.metadata("bulk-payload-pkg-unique")
        print("metadata_name", meta.get("Name"))
        print(
            "metadata_summary_has_tail",
            "hydration fixture" in (meta.get("Summary") or ""),
        )
        print("metadata_requires_python", meta.get("Requires-Python"))
        print("metadata_requires_dist", meta.get_all("Requires-Dist"))
        print("requires_api", importlib.metadata.requires("bulk-payload-pkg-unique"))

        eps = importlib.metadata.entry_points(
            group="bulk_payload_group_unique",
            name="bulk-tool",
        )
        print("entry_points_count", len(eps))
        if eps:
            print("entry_point_value", eps["bulk-tool"].value)

        files = importlib.metadata.files("bulk-payload-pkg-unique")
        print("files_is_none", files is None)
        entries = list(files or [])
        print("files_count", len(entries))
        names = sorted(str(entry) for entry in entries)
        print("files_has_init", "bulk_payload_pkg_unique/__init__.py" in names)
        print("files_has_data", "bulk_payload_pkg_unique/data.txt" in names)
        target = next(
            (
                entry
                for entry in entries
                if str(entry) == "bulk_payload_pkg_unique/data.txt"
            ),
            None,
        )
        print(
            "files_target_hash_mode",
            getattr(getattr(target, "hash", None), "mode", None),
        )
        print(
            "files_target_hash_value",
            getattr(getattr(target, "hash", None), "value", None),
        )
        print("files_target_size", getattr(target, "size", None))
        if target is not None:
            try:
                text_value = target.read_text().strip()
            except Exception as exc:  # noqa: BLE001 - differential type check
                text_value = type(exc).__name__
            print("files_target_read_text", text_value)

        mapping = importlib.metadata.packages_distributions()
        print(
            "packages_demo",
            sorted(mapping.get("bulk_payload_pkg_unique", [])),
        )
        print(
            "packages_other",
            sorted(mapping.get("bulk_payload_other_pkg_unique", [])),
        )
        print(
            "packages_shared",
            sorted(mapping.get("bulk_payload_shared_pkg_unique", [])),
        )
    finally:
        sys.path[:] = original
