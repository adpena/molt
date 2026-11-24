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
    from molt.cli.frontend_record_files import FrontendRecordFiles
    from molt.cli.source_record_store import SourceRecordStore

    key, absent = "a" * 24, "b" * 24
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "g"
    )
    if kind == "import-scan":
        # Import scans are immutable record files (cache frontier step 2).
        files = FrontendRecordFiles(tmp_path)
        monkeypatch.setattr(module_graph_cache, "frontend_record_files", lambda: files)
        assert files.put(kind, key, payload)
        # Opaque storage retains even malformed/non-object JSON unchanged.
        observed = files.get(kind, key, max_bytes=10000)
        assert observed is not None and observed.payload == payload
        scan_record = module_graph_cache._read_import_scan_record(key)
        candidate = None if scan_record is None else scan_record.payload
        missing = module_graph_cache._read_import_scan_record(absent)
    else:
        store = SourceRecordStore(tmp_path)
        monkeypatch.setattr(module_graph_cache, "shared_source_records", lambda: store)
        assert store.put(kind, key, payload, generation="g")
        observation = store.get(kind, key, generation="g", max_bytes=10000)
        assert observation is not None and observation.payload == payload
        record = module_graph_cache._read_intrinsic_facts_record(key)
        candidate = None if record is None else record.payload
        missing = module_graph_cache._read_intrinsic_facts_record(absent)
    assert candidate is not None
    assert module_graph_cache._decode_source_record_json(candidate) == expected
    assert missing is None


def test_json_record_producers_preserve_compact_python_text(
    tmp_path, monkeypatch
) -> None:
    from molt.cli.frontend_record_files import FrontendRecordFiles
    from molt.cli.source_record_store import SourceRecordStore
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    store = SourceRecordStore(tmp_path)
    files = FrontendRecordFiles(tmp_path)
    monkeypatch.setattr(module_graph_cache, "shared_source_records", lambda: store)
    monkeypatch.setattr(module_graph_cache, "frontend_record_files", lambda: files)
    monkeypatch.setattr(
        module_graph_cache, "_frontend_semantic_tooling_fingerprint", lambda: "g"
    )
    source = tmp_path / "café.py"
    monkeypatch.setattr(
        module_graph_cache, "_import_scan_tooling_fingerprint", lambda: "scan-g"
    )
    source.write_bytes(b"raise ImportError\n")
    # This actual producer publishes both its source import scan and its facts.
    module_graph_cache._stdlib_intrinsic_source_facts(
        "café", source, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    with sqlite3.connect(store.path) as connection:
        rows = connection.execute(
            "SELECT kind, typeof(payload), payload FROM records ORDER BY kind"
        ).fetchall()
    assert [(row[0], row[1]) for row in rows] == [("intrinsic-facts", "blob")]
    # The scan is a record file (cache frontier step 2), read from disk.
    (scan_digest,) = FrontendRecordFiles(tmp_path).addresses().values()
    scan = files.blob_path(scan_digest).read_bytes()
    for kind, payload in [("import-scan", scan), *((r[0], r[2]) for r in rows)]:
        assert "café".encode("utf-8") in payload
        assert b'": ' not in payload
        decoded = json.loads(payload)
        assert isinstance(decoded, dict)
        if kind == "import-scan":
            assert decoded["module_name"] == "café"
            assert decoded["compiler_fingerprint"] == "scan-g"
        else:
            assert set(decoded) == {"identity", "facts"}
            assert decoded["identity"]["compiler_fingerprint"] == "g"
