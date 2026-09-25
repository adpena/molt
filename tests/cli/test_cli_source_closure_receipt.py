from __future__ import annotations

from collections import Counter
import hashlib
import json
from pathlib import Path

import pytest

from molt.cli import cache_fingerprints as fingerprints
from molt.cli import compiler_metadata
from molt.cli import python_source_closure as graph
from molt.cli.module_source import PythonSourceSnapshot
from molt.cli.python_import_resolution import LocalPythonModuleResolver


pytestmark = pytest.mark.usefixtures("isolated_molt_cache")


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path.resolve()


def test_receipt_uses_captured_python_and_manifest_bytes(tmp_path, monkeypatch):
    seed = _write(tmp_path / "entry.py", "VALUE = 1\n")
    manifest = _write(
        tmp_path / graph._DYNAMIC_IMPORT_MANIFEST,
        "schema_version = 1\nsource = []\n",
    )
    original = {seed: seed.read_bytes(), manifest: manifest.read_bytes()}
    capture = PythonSourceSnapshot.capture
    loads = graph.tomllib.loads

    def capture_then_edit(path):
        snapshot = capture(path)
        path.write_text("VALUE = 2\n", encoding="utf-8")
        return snapshot

    def parse_then_edit(text):
        payload = loads(text)
        manifest.write_text("not valid toml !!!", encoding="utf-8")
        return payload

    monkeypatch.setattr(PythonSourceSnapshot, "capture", capture_then_edit)
    monkeypatch.setattr(graph.tomllib, "loads", parse_then_edit)
    receipt = graph.local_python_import_closure(tmp_path, (seed,))
    expected = hashlib.sha256()
    for path in receipt.paths:
        expected.update(path.relative_to(tmp_path).as_posix().encode())
        expected.update(b"\0")
        expected.update(original[path])
        expected.update(b"\0")
    assert receipt.content_digest == expected.hexdigest()
    assert dict(receipt.source_sha256) == {
        path: hashlib.sha256(content).hexdigest() for path, content in original.items()
    }
    assert receipt.source_bytes == sum(map(len, original.values()))
    with pytest.raises(TypeError):
        receipt.source_sha256[seed] = "changed"


def test_unchanged_request_cache_is_not_republished(tmp_path, monkeypatch):
    seed = _write(tmp_path / "entry.py", "import helper\n")
    helper = _write(tmp_path / "helper.py", "VALUE = 1\n")
    publications = []
    write = graph._atomic_write_text

    def record(path, text):
        publications.append(path)
        write(path, text)

    monkeypatch.setattr(graph, "_atomic_write_text", record)
    first = graph.local_python_import_closure(tmp_path, (seed,))
    assert graph.local_python_import_closure(tmp_path, (seed,)) == first
    assert len(publications) == 1
    helper.write_text("VALUE = 2\n", encoding="utf-8")
    assert graph.local_python_import_closure(tmp_path, (seed,)) != first
    assert len(publications) == 2
    helper.unlink()
    graph.local_python_import_closure(tmp_path, (seed,))
    assert len(publications) == 3
    payload = json.loads((graph.python_source_closure_cache_path(tmp_path)).read_text())
    assert set(payload["entries"]) == {"entry.py"}


def test_cache_keys_do_not_resolve_or_grant_source_authority(tmp_path, monkeypatch):
    source = _write(tmp_path / "source.py", "VALUE = 1\n")
    cache = graph.python_source_closure_cache_path(tmp_path)
    _write(
        cache,
        json.dumps(
            {
                "schema_version": graph._GRAPH_CACHE_SCHEMA_VERSION,
                "entries": {
                    "source.py": {},
                    "../outside.py": {},
                    "/outside.py": {},
                    "C:/outside.py": {},
                    "nested/../source.py": {},
                    "./source.py": {},
                    "nested\\source.py": {},
                    "vanished.py": {},
                },
            }
        ),
    )

    def forbidden(*args, **kwargs):
        raise AssertionError("persisted cache keys must not resolve filesystem paths")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "resolve", forbidden)
        entries, pruned = graph._read_graph_cache(tmp_path)
    assert entries == {source.name: {}}
    assert pruned


def test_resolver_does_not_canonicalize_missing_candidates(tmp_path, monkeypatch):
    resolver = LocalPythonModuleResolver((tmp_path,))

    def forbidden(*args, **kwargs):
        raise AssertionError("missing import candidate must stop at stat")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "resolve", forbidden)
        result = resolver.source_for_module("missing.child")
    assert result is None


def test_inaccessible_cache_hint_is_pruned_without_source_admission(
    tmp_path, monkeypatch
):
    source = _write(tmp_path / "denied.py", "VALUE = 1\n")
    _write(
        graph.python_source_closure_cache_path(tmp_path),
        json.dumps(
            {
                "schema_version": graph._GRAPH_CACHE_SCHEMA_VERSION,
                "entries": {source.name: {}},
            }
        ),
    )
    is_file = Path.is_file

    def denied(path):
        if path == source:
            raise PermissionError("inaccessible stale cache hint")
        return is_file(path)

    with monkeypatch.context() as patch:
        patch.setattr(Path, "is_file", denied)
        entries, pruned = graph._read_graph_cache(tmp_path)
    assert entries == {} and pruned


def test_resolver_rejects_escaping_and_broken_symlinks(tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    outside = _write(tmp_path / "outside.py", "VALUE = 1\n")
    try:
        (root / "escape.py").symlink_to(outside)
        (root / "broken.py").symlink_to(tmp_path / "missing.py")
    except OSError as exc:
        pytest.skip(f"symlink creation unavailable: {exc}")
    resolver = LocalPythonModuleResolver((root,))
    assert resolver.source_for_module("escape") is None
    assert resolver.source_for_module("broken") is None


def test_semantic_fingerprint_reads_python_once_without_git_and_keeps_assets(
    tmp_path, monkeypatch
):
    frontend = tmp_path / "src/molt/frontend"
    seed = _write(frontend / "entry.py", "from molt import shared\n")
    helper = _write(tmp_path / "src/molt/shared.py", "VALUE = 1\n")
    asset = _write(frontend / "layout.json", '{"value": 1}\n')
    reads = Counter()
    read_bytes = Path.read_bytes
    hash_file = fingerprints._sha256_file

    def read(path):
        reads[path] += 1
        return read_bytes(path)

    def hash_unread_source(path):
        assert path not in (seed, helper), (
            "captured Python must not be rehashed from disk"
        )
        reads[path] += 1
        return hash_file(path)

    def redundant_git_query(*args):
        raise AssertionError("captured semantic identity must not query Git")

    monkeypatch.setattr(Path, "read_bytes", read)
    monkeypatch.setattr(fingerprints, "_sha256_file", hash_unread_source)
    monkeypatch.setattr(fingerprints, "_compiler_root", lambda: tmp_path)
    monkeypatch.setattr(
        fingerprints, "_compiler_clean_pathspec_source_state", redundant_git_query
    )
    before = fingerprints._frontend_semantic_tooling_fingerprint()
    assert reads[seed] == reads[helper] == reads[asset] == 1
    asset.write_text('{"value": 2}\n', encoding="utf-8")
    after = fingerprints._frontend_semantic_tooling_fingerprint()
    assert after != before
    assert reads[seed] == reads[helper] == reads[asset] == 2


def test_captured_hashes_survive_changed_disk_bytes_through_fingerprint(
    tmp_path, monkeypatch
):
    source = _write(tmp_path / "source.py", "VALUE = 1\n")
    receipt = graph.local_python_import_closure(tmp_path, (source,))
    inputs = fingerprints._SourceFingerprintInputs(receipt.paths, receipt.source_sha256)
    monkeypatch.setattr(
        fingerprints, "_compiler_clean_pathspec_source_state", lambda *args: None
    )
    options = dict(
        root=tmp_path, inputs=inputs, scope="test", extra_fingerprint_inputs=""
    )
    before = fingerprints._source_tree_cache_fingerprint(**options)
    source.write_text("VALUE = 2\n", encoding="utf-8")
    assert fingerprints._source_tree_cache_fingerprint(**options) == before
    changed = graph.local_python_import_closure(tmp_path, (source,))
    options["inputs"] = fingerprints._SourceFingerprintInputs(
        changed.paths, changed.source_sha256
    )
    assert fingerprints._source_tree_cache_fingerprint(**options) != before


def test_canonical_fingerprint_paths_are_projected_without_resolving(
    tmp_path, monkeypatch
):
    source = _write(tmp_path / "source.py", "VALUE = 1\n")
    inputs = fingerprints._SourceFingerprintInputs.from_paths((source,))

    def forbidden(*args, **kwargs):
        raise AssertionError("canonical paths must not be admitted again")

    with monkeypatch.context() as patch:
        patch.setattr(Path, "resolve", forbidden)
        pathspecs = compiler_metadata._clean_pathspecs_for_root(
            tmp_path,
            tuple(map(str, inputs.paths)),
        )
    assert pathspecs == ("source.py",)
