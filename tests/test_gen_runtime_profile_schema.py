"""Drift and semantic checks for the runtime-profile schema authority."""

from __future__ import annotations

import importlib
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_runtime_profile_schema")


def test_generated_runtime_profile_schema_is_byte_exact() -> None:
    gen = _gen()
    rendered = gen.render_all(gen.load_schema())
    for path, expected in rendered.items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run "
            "`python tools/gen_runtime_profile_schema.py`"
        )


def test_manifest_drives_metric_membership_and_semantics() -> None:
    schema = _gen().load_schema()
    sections = schema["section"]
    assert [section["name"] for section in sections] == [
        "profile",
        "aux",
        "gc",
        "hot_paths",
        "deopt_reasons",
    ]
    by_metric = {
        metric["name"]: metric["semantic"]
        for section in sections
        for metric in section["metrics"]
    }
    assert by_metric["alloc_count"] == "counter"
    assert by_metric["live_objects"] == "gauge"
    assert by_metric["gc_tracked_high_water"] == "gauge"


def test_schema_rejects_duplicate_metric_authority(tmp_path: Path) -> None:
    source = (ROOT / "runtime" / "runtime_profile_schema.toml").read_text(
        encoding="utf-8"
    )
    duplicate = source.replace(
        '{ name = "call_dispatch", semantic = "counter" },',
        '{ name = "call_dispatch", semantic = "counter" },\n'
        '  { name = "call_dispatch", semantic = "counter" },',
        1,
    )
    path = tmp_path / "duplicate.toml"
    path.write_text(duplicate, encoding="utf-8")
    with pytest.raises(_gen().SchemaError, match="duplicate metric"):
        _gen().load_schema(path)
