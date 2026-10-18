from __future__ import annotations

import inspect
import json
import sqlite3

import pytest

import molt.cli as cli
from molt.cli import module_graph
from molt.cli import module_graph_cache

_MODULE_GRAPH_CACHE_NAMES = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION",
    "_import_scan_cache_key",
    "_read_persisted_import_scan_record",
    "_resolved_module_cache_key",
    "_write_persisted_import_scan",
)

_MODULE_GRAPH_CACHE_DEFINITIONS = (
    "_IMPORT_SCAN_CACHE_SCHEMA_VERSION =",
    "def _import_scan_cache_key(",
    "def _read_persisted_import_scan_record(",
    "def _resolved_module_cache_key(",
    "def _write_persisted_import_scan(",
)


def test_cli_module_graph_cache_authority_is_single_home() -> None:
    for name in _MODULE_GRAPH_CACHE_NAMES:
        assert hasattr(module_graph_cache, name)
        assert not hasattr(module_graph, name)
        assert not hasattr(cli, name)

    module_graph_source = inspect.getsource(module_graph)
    cli_source = inspect.getsource(cli)
    for marker in _MODULE_GRAPH_CACHE_DEFINITIONS:
        assert marker not in module_graph_source
        assert marker not in cli_source


def test_persisted_whole_graph_authority_is_removed() -> None:
    for name in (
        "_MODULE_GRAPH_CACHE_SCHEMA_VERSION",
        "_PersistedModuleGraphState",
        "_PersistedImportScan",
        "_module_graph_cache_key",
        "_module_graph_cache_path",
        "_module_graph_policy_digest",
        "_read_persisted_module_graph",
        "_write_persisted_module_graph",
    ):
        for owner in (module_graph_cache, module_graph, cli):
            assert not hasattr(owner, name)


@pytest.mark.parametrize("kind", ["import-scan", "intrinsic-facts"])
@pytest.mark.parametrize(
    ("payload", "expected"),
    [
        (b'{"value":"\\u00e9","numbers":[1,2]}', {"value": "é", "numbers": [1, 2]}),
        (b"{}", {}),
        (b"[1,2]", None),
        (b'"scalar"', None),
        (b"null", None),
        (b"{", None),
        (b"\xff", None),
        (b"[" * 2000 + b"]" * 2000, None),
    ],
)
def test_source_record_json_admission_belongs_to_graph_cache(
    tmp_path, monkeypatch, kind, payload, expected
) -> None:
    from molt.cli.source_record_store import SourceRecordStore

    store = SourceRecordStore(tmp_path / "records.sqlite")
    monkeypatch.setattr(module_graph_cache, "_source_records", lambda *_args: store)
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "g"
    )
    assert store.put(kind, "key", payload, generation="g")
    # Opaque storage retains even malformed/non-object JSON unchanged.
    assert store.get(kind, "key", generation="g", max_bytes=10000) == payload
    reader = (
        module_graph_cache._read_import_scan_payload
        if kind == "import-scan"
        else module_graph_cache._read_intrinsic_facts_payload
    )
    assert reader(tmp_path, tmp_path / "source.py", "key") == expected
    assert reader(tmp_path, tmp_path / "source.py", "absent") is None


def test_json_record_producers_preserve_compact_ascii_blob_encoding(
    tmp_path, monkeypatch
) -> None:
    from molt.cli.source_record_store import SourceRecordStore
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    store = SourceRecordStore(tmp_path / "records.sqlite")
    monkeypatch.setattr(module_graph_cache, "_source_records", lambda *_args: store)
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "g"
    )
    source = tmp_path / "café.py"
    source.write_bytes(b"raise ImportError\n")
    # This actual producer publishes both its source import scan and its facts.
    module_graph_cache._stdlib_intrinsic_source_facts(
        tmp_path, "café", source, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    with sqlite3.connect(store.path) as connection:
        rows = connection.execute(
            "SELECT kind, typeof(payload), payload FROM records ORDER BY kind"
        ).fetchall()
    assert [row[0] for row in rows] == ["import-scan", "intrinsic-facts"]
    for kind, storage_type, payload in rows:
        assert storage_type == "blob"
        assert b"caf\\u00e9" in payload
        assert b"\xc3\xa9" not in payload
        assert b'": ' not in payload
        decoded = json.loads(payload)
        assert isinstance(decoded, dict)
        if kind == "import-scan":
            assert decoded["module_name"] == "café"
            assert decoded["compiler_fingerprint"] == "g"
        else:
            assert set(decoded) == {"identity", "facts"}
