"""Purpose: validate runtime-lowered metadata payload parsing (headers + entry points)."""

import importlib.metadata
import pathlib
import sys
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = pathlib.Path(tmp)
    site = root / "site"
    site.mkdir()
    dist = site / "demo_payload-9.8.7.dist-info"
    dist.mkdir()
    (dist / "METADATA").write_text(
        "Name: demo-payload\n"
        "Version: 9.8.7\n"
        "Summary: head\n"
        " tail\n"
        "Requires-Python: >=3.12\n"
        "Requires-Dist: dep-one>=1\n"
        "Requires-Dist: dep-two; extra == \"dev\"\n"
        "Provides-Extra: dev\n",
        encoding="utf-8",
    )
    (dist / "entry_points.txt").write_text(
        "[console_scripts]\\ndemo = demo_payload:main\\n[demo.group]\\nvalue = demo_payload:value\\n",
        encoding="utf-8",
    )

    original = list(sys.path)
    try:
        sys.path.insert(0, str(site))
        dist_obj = importlib.metadata.distribution("demo-payload")
        print("version", importlib.metadata.version("demo_payload"))
        print("name", dist_obj.metadata.get("Name"))
        print("summary_has_tail", "tail" in (dist_obj.metadata.get("Summary") or ""))
        print("requires_python", dist_obj.metadata.get("Requires-Python"))
        print("requires_all", dist_obj.metadata.get_all("Requires-Dist"))
        print("requires_prop", dist_obj.requires)
        print("extra_all", dist_obj.metadata.get_all("Provides-Extra"))
        eps = importlib.metadata.entry_points().select(group="console_scripts")
        print("console_scripts", len(eps))
        payload_eps = importlib.metadata.entry_points().select(group="demo.group")
        print("demo_group", len(payload_eps))
    finally:
        sys.path[:] = original
