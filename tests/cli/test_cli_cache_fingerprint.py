from __future__ import annotations

import importlib
import hashlib
import json
import os
from pathlib import Path
from typing import Any

import pytest

CACHE_FINGERPRINTS = importlib.import_module("molt.cli.cache_fingerprints")
CACHE_KEYS = importlib.import_module("molt.cli.cache_keys")
COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")


def _cli_init(root: Path) -> Path:
    return root / "src" / "molt" / "cli" / "__init__.py"


def _tiny_ir() -> dict[str, Any]:
    return {
        "module": "__main__",
        "filename": "test.py",
        "ops": [],
        "functions": [],
        "classes": [],
        "constants": {},
        "imports": [],
    }


def test_cache_payload_digest_matches_canonical_json_bytes() -> None:
    payload = {
        "zeta": [{"kind": "tuple", "value": (1, 2)}],
        "alpha": {"bytes": b"abc", "set": {"b", "a"}},
    }

    canonical_bytes = json.dumps(
        payload,
        sort_keys=True,
        separators=(",", ":"),
        default=CACHE_KEYS._json_ir_default,
    ).encode("utf-8")

    assert b"".join(CACHE_KEYS._iter_cache_json_payload_bytes(payload)) == (
        canonical_bytes
    )
    assert CACHE_KEYS._cache_payload_digest(payload) == hashlib.sha256(
        canonical_bytes
    ).hexdigest()


@pytest.fixture
def isolated_compiler_source(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    source = tmp_path / "runtime" / "molt-backend" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    source.write_text("pub fn marker() -> u8 { 1 }\n", encoding="utf-8")

    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        lambda root, backend_features: [source],
    )
    monkeypatch.setattr(
        CACHE_FINGERPRINTS, "_runtime_source_paths", lambda root, **_kwargs: []
    )
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_rustc_version", lambda: "rustc-test")
    monkeypatch.setattr(
        CACHE_KEYS, "_cache_tooling_fingerprint", lambda: "tooling-test"
    )
    return source


def test_cache_key_changes_when_compiler_source_content_changes_in_process(
    isolated_compiler_source: Path,
) -> None:
    first = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    isolated_compiler_source.write_text(
        "pub fn marker() -> u8 { 2 }\n",
        encoding="utf-8",
    )

    second = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    assert second != first


def test_cache_key_changes_when_compiler_source_mtime_changes_in_process(
    isolated_compiler_source: Path,
) -> None:
    first = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    stat = isolated_compiler_source.stat()
    next_mtime_ns = stat.st_mtime_ns + 5_000_000
    os.utime(isolated_compiler_source, ns=(next_mtime_ns, next_mtime_ns))

    second = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    assert second != first


def test_cache_fingerprint_threads_selected_backend_and_runtime_features(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    backend_source = root / "runtime" / "molt-backend-native" / "src" / "lib.rs"
    runtime_source = root / "runtime" / "molt-runtime" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True)
    runtime_source.parent.mkdir(parents=True)
    backend_source.write_text("pub fn backend_marker() {}\n", encoding="utf-8")
    runtime_source.write_text("pub fn runtime_marker() {}\n", encoding="utf-8")
    seen_backend_features: list[tuple[str, ...]] = []
    seen_runtime_features: list[tuple[str, ...]] = []

    def backend_source_paths(
        source_root: Path, backend_features: tuple[str, ...]
    ) -> list[Path]:
        assert source_root == root
        seen_backend_features.append(tuple(backend_features))
        return [backend_source]

    def runtime_source_paths(source_root: Path, **kwargs: object) -> list[Path]:
        assert source_root == root
        runtime_features = kwargs.get("runtime_features")
        assert runtime_features is not None
        seen_runtime_features.append(tuple(runtime_features))  # type: ignore[arg-type]
        return [runtime_source]

    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        backend_source_paths,
    )
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_runtime_source_paths", runtime_source_paths)
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_rustc_version", lambda: "rustc-test")

    fingerprint = CACHE_FINGERPRINTS._cache_fingerprint(
        backend_features=("native-backend",),
        runtime_features=("stdlib_micro", "no-default-features"),
    )

    assert fingerprint
    assert seen_backend_features == [("native-backend",)]
    assert seen_runtime_features == [("no-default-features", "stdlib_micro")]


def test_cache_fingerprint_can_exclude_runtime_implementation_sources(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    backend_source = root / "runtime" / "molt-backend-native" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True)
    backend_source.write_text("pub fn backend_marker() {}\n", encoding="utf-8")

    def runtime_source_paths(*args: object, **kwargs: object) -> list[Path]:
        raise AssertionError("runtime sources are not backend object cache inputs")

    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        lambda source_root, backend_features: [backend_source],
    )
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_runtime_source_paths", runtime_source_paths)
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_rustc_version", lambda: "rustc-test")

    assert CACHE_FINGERPRINTS._cache_fingerprint(
        backend_features=("native-backend",),
        include_runtime_sources=False,
    )


def _write_crate(root: Path, name: str, manifest: str, lib_text: str = "") -> Path:
    crate_root = root / "runtime" / name
    src = crate_root / "src" / "lib.rs"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_text(
        lib_text or f"pub const MARKER: &str = {name!r};\n", encoding="utf-8"
    )
    (crate_root / "Cargo.toml").write_text(manifest, encoding="utf-8")
    return src


def _write_backend_identity_fixture(
    root: Path, *, include_wasm: bool = True
) -> dict[str, Path]:
    root.mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
    (root / "Cargo.lock").write_text("# lock\n", encoding="utf-8")
    wasm_dep = (
        'molt-backend-wasm = { path = "../molt-backend-wasm", optional = true, '
        "default-features = false }\n"
        if include_wasm
        else ""
    )
    wasm_feature = (
        'wasm-backend = ["dep:molt-backend-wasm", "molt-backend-wasm/wasm-backend"]\n'
        if include_wasm
        else "wasm-backend = []\n"
    )
    backend = _write_crate(
        root,
        "molt-backend",
        '[package]\nname = "molt-backend"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir" }\n'
        'molt-tir = { path = "../molt-tir" }\n'
        'molt-backend-native = { path = "../molt-backend-native", optional = true, default-features = false }\n'
        'molt-backend-rust = { path = "../molt-backend-rust", optional = true, default-features = false }\n'
        'molt-backend-luau = { path = "../molt-backend-luau", optional = true, default-features = false }\n'
        f"{wasm_dep}"
        "[features]\n"
        'default = ["native-backend"]\n'
        'native-backend = ["dep:molt-backend-native", "molt-backend-native/native-backend"]\n'
        'rust-backend = ["dep:molt-backend-rust", "molt-backend-rust/rust-backend"]\n'
        'luau-backend = ["dep:molt-backend-luau", "molt-backend-luau/luau-backend"]\n'
        f"{wasm_feature}",
    )
    native = _write_crate(
        root,
        "molt-backend-native",
        '[package]\nname = "molt-backend-native"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir" }\n'
        'molt-tir = { path = "../molt-tir" }\n'
        'molt-codegen-abi = { path = "../molt-codegen-abi" }\n'
        "[features]\ndefault = []\nnative-backend = []\nllvm = []\n",
    )
    wasm = _write_crate(
        root,
        "molt-backend-wasm",
        '[package]\nname = "molt-backend-wasm"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir", default-features = false }\n'
        'molt-tir = { path = "../molt-tir", default-features = false }\n'
        'molt-codegen-abi = { path = "../molt-codegen-abi" }\n'
        '[features]\ndefault = []\nwasm-backend = ["molt-ir/wasm-backend", "molt-tir/wasm-backend"]\n',
    )
    _write_crate(
        root,
        "molt-backend-rust",
        '[package]\nname = "molt-backend-rust"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir" }\n'
        'molt-tir = { path = "../molt-tir" }\n'
        "[features]\ndefault = []\nrust-backend = []\n",
    )
    _write_crate(
        root,
        "molt-backend-luau",
        '[package]\nname = "molt-backend-luau"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir" }\n'
        'molt-tir = { path = "../molt-tir" }\n'
        "[features]\ndefault = []\nluau-backend = []\n",
    )
    _write_crate(
        root,
        "molt-tir",
        '[package]\nname = "molt-tir"\nversion = "0.1.0"\n'
        "[dependencies]\n"
        'molt-ir = { path = "../molt-ir" }\n'
        'molt-passes = { path = "../molt-passes" }\n'
        '[features]\ndefault = []\nwasm-backend = ["molt-ir/wasm-backend"]\n',
    )
    _write_crate(
        root,
        "molt-ir",
        '[package]\nname = "molt-ir"\nversion = "0.1.0"\n'
        "[features]\ndefault = []\nwasm-backend = []\n",
    )
    _write_crate(
        root,
        "molt-passes",
        '[package]\nname = "molt-passes"\nversion = "0.1.0"\n',
    )
    _write_crate(
        root,
        "molt-codegen-abi",
        '[package]\nname = "molt-codegen-abi"\nversion = "0.1.0"\n',
    )
    return {"backend": backend, "native": native, "wasm": wasm}


def _backend_source_fingerprint(root: Path, features: tuple[str, ...]) -> str:
    source_paths = CACHE_FINGERPRINTS._backend_source_paths(root, features)
    return CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        source_paths=source_paths,
        scope=f"backend-test:{','.join(features)}",
        extra_fingerprint_inputs="",
    )


def test_backend_source_fingerprint_tracks_selected_leaf_sources(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    sources = _write_backend_identity_fixture(root)

    wasm_first = _backend_source_fingerprint(root, ("wasm-backend",))
    native_first = _backend_source_fingerprint(root, ("native-backend",))

    sources["wasm"].write_text(
        'pub const WASM_MARKER: &str = "changed-wasm-leaf";\n',
        encoding="utf-8",
    )

    wasm_second = _backend_source_fingerprint(root, ("wasm-backend",))
    native_after_wasm = _backend_source_fingerprint(root, ("native-backend",))
    assert wasm_second != wasm_first
    assert native_after_wasm == native_first

    sources["native"].write_text(
        'pub const NATIVE_MARKER: &str = "changed-native-leaf";\n',
        encoding="utf-8",
    )

    native_second = _backend_source_fingerprint(root, ("native-backend",))
    assert native_second != native_after_wasm


def test_backend_source_paths_cache_tracks_manifest_dependency_edits(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    _write_backend_identity_fixture(root, include_wasm=False)

    before = {
        path.relative_to(root).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(root, ("wasm-backend",))
    }
    assert "runtime/molt-backend-wasm/src" not in before

    _write_backend_identity_fixture(root, include_wasm=True)

    after = {
        path.relative_to(root).as_posix()
        for path in CACHE_FINGERPRINTS._backend_source_paths(root, ("wasm-backend",))
    }
    assert "runtime/molt-backend-wasm/src" in after


def test_cache_tooling_fingerprint_changes_when_tooling_source_changes_in_process(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = _cli_init(root)
    frontend_source = root / "src" / "molt" / "frontend" / "__init__.py"
    cli_source.parent.mkdir(parents=True)
    frontend_source.parent.mkdir(parents=True)
    cli_source.write_text("CLI_MARKER = 1\n", encoding="utf-8")
    frontend_source.write_text("FRONTEND_MARKER = 1\n", encoding="utf-8")

    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    cli_source.write_text("CLI_MARKER = 2\n", encoding="utf-8")

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second != first


def test_cache_tooling_fingerprint_tracks_frontend_helper_modules(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = _cli_init(root)
    frontend_init = root / "src" / "molt" / "frontend" / "__init__.py"
    cfg_analysis = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tv_hooks = root / "src" / "molt" / "frontend" / "tv_hooks.py"
    type_facts = root / "src" / "molt" / "type_facts.py"
    for source in (cli_source, frontend_init, cfg_analysis, tv_hooks, type_facts):
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text(f"{source.stem.upper()}_MARKER = 1\n", encoding="utf-8")

    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    cfg_analysis.write_text("CFG_ANALYSIS_MARKER = 2\n", encoding="utf-8")
    tv_hooks.write_text("TV_HOOKS_MARKER = 2\n", encoding="utf-8")
    type_facts.write_text("TYPE_FACTS_MARKER = 2\n", encoding="utf-8")

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second != first


def test_cache_tooling_fingerprint_ignores_frontend_bytecode_cache(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    cli_source = _cli_init(root)
    frontend_init = root / "src" / "molt" / "frontend" / "__init__.py"
    pycache = (
        root
        / "src"
        / "molt"
        / "frontend"
        / "__pycache__"
        / "cfg_analysis.cpython-312.pyc"
    )
    for source in (cli_source, frontend_init, pycache):
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_bytes(b"marker-1\n")

    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    pycache.write_bytes(b"marker-2\n")

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second == first


def _write_frontend_tree(root: Path) -> dict[str, Path]:
    molt_root = root / "src" / "molt"
    sources = {
        "cli_init": molt_root / "cli" / "__init__.py",
        "frontend_init": molt_root / "frontend" / "__init__.py",
        "cfg_analysis": molt_root / "frontend" / "cfg_analysis.py",
        "tv_hooks": molt_root / "frontend" / "tv_hooks.py",
        "nested_helper": molt_root / "frontend" / "lowering" / "reducer.py",
        "type_facts": molt_root / "type_facts.py",
        "capabilities": molt_root / "capabilities.py",
    }
    for name, source in sources.items():
        source.parent.mkdir(parents=True, exist_ok=True)
        source.write_text(f"{name.upper()}_MARKER = 1\n", encoding="utf-8")
    return sources


def _rewrite_same_length_content(path: Path) -> None:
    """Rewrite a marker file with different content but forced-identical stat.

    Same-length body plus restored (atime, mtime) makes size + mtime_ns match the
    prior revision. This deterministically reproduces the coarse-mtime collision
    that a metadata-only cache key would miss, independent of host FS timestamp
    granularity.
    """

    stat = path.stat()
    original = path.read_text(encoding="utf-8")
    assert original.endswith("1\n")
    path.write_text(original[:-2] + "2\n", encoding="utf-8")
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))


@pytest.mark.parametrize(
    "edited",
    [
        "frontend_init",
        "cfg_analysis",
        "tv_hooks",
        "nested_helper",
        "type_facts",
        "capabilities",
    ],
)
def test_cache_tooling_fingerprint_tracks_each_helper_under_metadata_collision(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, edited: str
) -> None:
    # Regression guard for the P0: a same-length content edit whose stat metadata
    # (size + mtime_ns + ctime_ns) is unchanged MUST still change the fingerprint.
    # A metadata-only content-digest cache key served a stale digest here.
    root = tmp_path / "repo"
    sources = _write_frontend_tree(root)
    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    _rewrite_same_length_content(sources[edited])

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second != first, (
        f"fingerprint did not change after a same-length edit to {edited!r}; "
        "content digest is not content-complete"
    )


def test_cache_tooling_fingerprint_stable_for_unrelated_source(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # A change outside the tracked frontend/tooling closure must NOT invalidate
    # the tooling fingerprint (no over-invalidation).
    root = tmp_path / "repo"
    _write_frontend_tree(root)
    unrelated = root / "src" / "molt" / "runtime_only" / "unrelated.py"
    unrelated.parent.mkdir(parents=True, exist_ok=True)
    unrelated.write_text("UNRELATED_MARKER = 1\n", encoding="utf-8")
    monkeypatch.setattr(COMPILER_METADATA, "_COMPILER_ROOT", root)

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    unrelated.write_text("UNRELATED_MARKER = 2\n", encoding="utf-8")

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second == first


def test_source_tree_content_digest_is_not_metadata_keyed(tmp_path: Path) -> None:
    # Directly exercise the content-digest authority: two files with identical
    # stat metadata but different bytes must yield different digests.
    root = tmp_path / "repo"
    tracked = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tracked.parent.mkdir(parents=True, exist_ok=True)
    tracked.write_text("MARKER = 1\n", encoding="utf-8")
    stat = tracked.stat()

    first = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        source_paths=[tracked],
        scope="content-digest-test",
        extra_fingerprint_inputs="",
    )

    tracked.write_text("MARKER = 2\n", encoding="utf-8")  # same length
    os.utime(tracked, ns=(stat.st_atime_ns, stat.st_mtime_ns))

    second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        source_paths=[tracked],
        scope="content-digest-test",
        extra_fingerprint_inputs="",
    )

    assert second != first


def test_source_tree_fingerprint_transaction_reuses_content_complete_result(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    tracked = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tracked.parent.mkdir(parents=True, exist_ok=True)
    tracked.write_text("MARKER = 1\n", encoding="utf-8")
    calls: list[Path] = []
    original = CACHE_FINGERPRINTS._file_content_signature

    def counted_signature(path: Path) -> str:
        calls.append(path)
        return original(path)

    monkeypatch.setattr(CACHE_FINGERPRINTS, "_file_content_signature", counted_signature)

    with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
        first = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
            root=root,
            source_paths=[tracked],
            scope="transaction-test",
            extra_fingerprint_inputs="",
        )
        second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
            root=root,
            source_paths=[tracked],
            scope="transaction-test",
            extra_fingerprint_inputs="",
        )

    assert second == first
    assert calls == [tracked.resolve()]


def test_source_tree_fingerprint_transaction_does_not_escape_context(
    tmp_path: Path,
) -> None:
    root = tmp_path / "repo"
    tracked = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tracked.parent.mkdir(parents=True, exist_ok=True)
    tracked.write_text("MARKER = 1\n", encoding="utf-8")

    with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
        first = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
            root=root,
            source_paths=[tracked],
            scope="transaction-escape-test",
            extra_fingerprint_inputs="",
        )

    tracked.write_text("MARKER = 2\n", encoding="utf-8")
    second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        source_paths=[tracked],
        scope="transaction-escape-test",
        extra_fingerprint_inputs="",
    )

    assert second != first


def test_source_tree_cache_fingerprint_uses_clean_pathspec_state(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    tracked = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    tracked.parent.mkdir(parents=True, exist_ok=True)
    tracked.write_text("MARKER = 1\n", encoding="utf-8")
    clean_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 1,
        "pathspec_digest": "paths",
        "tracked_digest": "objects",
        "tracked_entry_count": 1,
    }
    seen_path_keys: list[tuple[str, ...]] = []

    def clean_pathspec_state(
        source_root: Path, path_keys: tuple[str, ...]
    ) -> dict[str, str | int] | None:
        assert source_root == root
        seen_path_keys.append(path_keys)
        return clean_state

    def content_walk_is_forbidden(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("clean pathspec state must avoid byte-walking sources")

    CACHE_FINGERPRINTS._SOURCE_TREE_CONTENT_DIGEST_CACHE.clear()
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        clean_pathspec_state,
    )
    monkeypatch.setattr(
        CACHE_FINGERPRINTS, "_hash_source_tree_metadata", content_walk_is_forbidden
    )
    monkeypatch.setattr(
        CACHE_FINGERPRINTS, "_file_content_signature", content_walk_is_forbidden
    )

    fingerprint = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        source_paths=[tracked],
        scope="clean-pathspec-test",
        extra_fingerprint_inputs="",
    )

    assert fingerprint
    assert seen_path_keys == [(str(tracked.resolve()),)]


def test_lowering_scope_source_files_cache_validates_compact_pathspec_state(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    frontend = root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    cli_source = root / "src" / "molt" / "cli" / "module_source.py"
    frontend.parent.mkdir(parents=True, exist_ok=True)
    cli_source.parent.mkdir(parents=True, exist_ok=True)
    frontend.write_text("MARKER = 1\n", encoding="utf-8")
    cli_source.write_text("MARKER = 1\n", encoding="utf-8")
    files = (str(frontend.resolve()), str(cli_source.resolve()))
    seed_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 2,
        "pathspec_digest": "seed",
        "tracked_digest": "seed-objects",
        "tracked_entry_count": 2,
    }
    full_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 2,
        "pathspec_digest": "full",
        "tracked_digest": "full-objects",
        "tracked_entry_count": 2,
    }
    compact_full_keys = CACHE_FINGERPRINTS._lowering_scope_clean_path_keys(root, files)
    seen_path_keys: list[tuple[str, ...]] = []

    def clean_pathspec_state(
        source_root: Path, path_keys: tuple[str, ...]
    ) -> dict[str, str | int] | None:
        assert source_root == root
        seen_path_keys.append(path_keys)
        if path_keys == compact_full_keys:
            return full_state
        return None

    monkeypatch.setattr(CACHE_FINGERPRINTS, "_default_molt_cache", lambda: tmp_path / "cache")
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        clean_pathspec_state,
    )

    CACHE_FINGERPRINTS._write_lowering_scope_source_files_cache(
        root,
        seed_state,
        full_state,
        files,
    )

    cached = CACHE_FINGERPRINTS._read_lowering_scope_source_files_cache(root, seed_state)

    assert cached == files
    assert seen_path_keys == [compact_full_keys]


def test_lowering_scope_source_files_cache_rejects_stale_full_state(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    root = tmp_path / "repo"
    source = root / "src" / "molt" / "cli" / "module_source.py"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text("MARKER = 1\n", encoding="utf-8")
    files = (str(source.resolve()),)
    seed_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 1,
        "pathspec_digest": "seed",
        "tracked_digest": "seed-objects",
        "tracked_entry_count": 1,
    }
    stored_full_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 1,
        "pathspec_digest": "full",
        "tracked_digest": "old-objects",
        "tracked_entry_count": 1,
    }
    current_full_state = dict(stored_full_state, tracked_digest="new-objects")

    monkeypatch.setattr(CACHE_FINGERPRINTS, "_default_molt_cache", lambda: tmp_path / "cache")
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        lambda _root, _path_keys: current_full_state,
    )

    CACHE_FINGERPRINTS._write_lowering_scope_source_files_cache(
        root,
        seed_state,
        stored_full_state,
        files,
    )

    assert CACHE_FINGERPRINTS._read_lowering_scope_source_files_cache(root, seed_state) is None
