from __future__ import annotations

from tests.compiler_identity_helper import (
    compiler_build_admission,
    stub_compiler_admission,
    write_compiler_lock,
)

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
RUNTIME_SOURCE_CLOSURE = importlib.import_module("molt.cli.runtime_source_closure")


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
    assert (
        CACHE_KEYS._cache_payload_digest(payload)
        == hashlib.sha256(canonical_bytes).hexdigest()
    )


@pytest.mark.parametrize(
    "project", ["_cache_ir_payload_ir", "_cache_backend_payload_ir"]
)
def test_cache_identity_preserves_return_abi_without_payload_returns(
    project: str,
) -> None:
    projection = getattr(CACHE_KEYS, project)
    digests = set()
    for return_abi in ("void", "value"):
        function = {
            "name": "function",
            "params": [],
            "return_abi": return_abi,
            "ops": [{"kind": "ret_void"}],
        }
        payload = projection({"functions": [function]})
        assert payload["functions"][0]["return_abi"] == return_abi
        digests.add(CACHE_KEYS._cache_payload_digest(payload))
    assert len(digests) == 2


@pytest.fixture
def isolated_compiler_source(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Path:
    source = tmp_path / "runtime" / "molt-backend" / "src" / "lib.rs"
    source.parent.mkdir(parents=True)
    write_compiler_lock(tmp_path)
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(tmp_path))
    stub_compiler_admission(monkeypatch)
    source.write_text("pub fn marker() -> u8 { 1 }\n", encoding="utf-8")

    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        lambda root, backend_features: [source],
    )
    monkeypatch.setattr(
        RUNTIME_SOURCE_CLOSURE, "runtime_source_paths", lambda root, **_kwargs: []
    )
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


def test_cache_key_ignores_compiler_source_mtime_only_changes_in_process(
    isolated_compiler_source: Path,
) -> None:
    first = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    stat = isolated_compiler_source.stat()
    next_mtime_ns = stat.st_mtime_ns + 5_000_000
    os.utime(isolated_compiler_source, ns=(next_mtime_ns, next_mtime_ns))

    second = CACHE_KEYS._cache_key(_tiny_ir(), "native", None, "variant")

    assert second == first


def test_cache_fingerprint_threads_selected_backend_and_runtime_features(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stub_compiler_admission(monkeypatch)
    root = tmp_path / "repo"
    backend_source = root / "runtime" / "molt-backend-native" / "src" / "lib.rs"
    runtime_source = root / "runtime" / "molt-runtime" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True)
    write_compiler_lock(root)
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

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        backend_source_paths,
    )
    monkeypatch.setattr(
        RUNTIME_SOURCE_CLOSURE, "runtime_source_paths", runtime_source_paths
    )

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
    stub_compiler_admission(monkeypatch)
    root = tmp_path / "repo"
    backend_source = root / "runtime" / "molt-backend-native" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True)
    write_compiler_lock(root)
    backend_source.write_text("pub fn backend_marker() {}\n", encoding="utf-8")

    def runtime_source_paths(*args: object, **kwargs: object) -> list[Path]:
        raise AssertionError("runtime sources are not backend object cache inputs")

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        lambda source_root, backend_features: [backend_source],
    )
    monkeypatch.setattr(
        RUNTIME_SOURCE_CLOSURE, "runtime_source_paths", runtime_source_paths
    )

    assert CACHE_FINGERPRINTS._cache_fingerprint(
        backend_features=("native-backend",),
        include_runtime_sources=False,
    )


def test_cache_fingerprint_custodies_backend_concurrency_feature(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    stub_compiler_admission(monkeypatch)
    root = tmp_path / "repo"
    backend_source = root / "runtime" / "molt-backend-native" / "src" / "lib.rs"
    backend_source.parent.mkdir(parents=True)
    write_compiler_lock(root)
    backend_source.write_text("pub fn backend_marker() {}\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_backend_source_paths",
        lambda source_root, backend_features: [backend_source],
    )

    default = CACHE_FINGERPRINTS._cache_fingerprint(
        backend_features=("native-backend",),
        include_runtime_sources=False,
    )
    free_threaded = CACHE_FINGERPRINTS._cache_fingerprint(
        backend_features=("native-backend", "free-threaded"),
        include_runtime_sources=False,
    )
    assert default != free_threaded


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
    (root / "Cargo.lock").write_text(
        'version = 4\n[[package]]\nname = "molt-backend"\nversion = "0.1.0"\n',
        encoding="utf-8",
    )
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
    catalog = root / "src" / "molt" / "backend_environment.json"
    catalog.parent.mkdir(parents=True, exist_ok=True)
    catalog.write_text('{"schema": 1, "diagnostic": []}\n', encoding="utf-8")
    return {"backend": backend, "native": native, "wasm": wasm, "catalog": catalog}


def _backend_source_fingerprint(root: Path, features: tuple[str, ...]) -> str:
    source_paths, lock_digest = CACHE_FINGERPRINTS._backend_source_identity_inputs(
        root, CACHE_FINGERPRINTS._backend_source_paths(root, features)
    )
    return CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths(source_paths),
        scope=f"backend-test:{','.join(features)}",
        extra_fingerprint_inputs=lock_digest,
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


@pytest.mark.parametrize("feature", ["native-backend", "wasm-backend"])
def test_backend_fingerprints_track_embedded_environment_catalog(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, feature: str
) -> None:
    stub_compiler_admission(monkeypatch)
    from molt.cli import backend_binary

    root = tmp_path / "repo"
    sources = _write_backend_identity_fixture(root)
    monkeypatch.setattr(
        backend_binary, "_compiler_clean_source_state", lambda *args: None
    )

    def binary_fingerprint(stored=None):
        return backend_binary._backend_fingerprint(
            root,
            cargo_profile="dev-fast",
            build_admission=compiler_build_admission(environment={"RUSTFLAGS": ""}),
            backend_features=(feature,),
            stored_fingerprint=stored,
        )

    source_before = _backend_source_fingerprint(root, (feature,))
    binary_before = binary_fingerprint()
    assert binary_before is not None
    sources["catalog"].write_text(
        '{"schema": 1, "diagnostic": ["MOLT_DUMP_IR"]}\n', encoding="utf-8"
    )
    assert _backend_source_fingerprint(root, (feature,)) != source_before
    binary_after = binary_fingerprint(binary_before)
    assert binary_after is not None
    assert binary_after["hash"] != binary_before["hash"]


def test_source_graphs_follow_preserved_stat_transitive_manifest_replacements(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.cli import runtime_features, runtime_source_closure

    root = tmp_path / "repo"
    _write_backend_identity_fixture(root)
    _write_crate(
        root,
        "molt-runtime",
        (
            '[package]\nname="molt-runtime"\nversion="0.1.0"\n'
            '[dependencies]\nmolt-passes={path="../molt-passes"}\n'
            '[features]\nstdlib_micro=["leaf_a"]\nleaf_a=[]\nleaf_b=[]\n'
        ),
    )
    for name in ("leaf_a", "leaf_b"):
        _write_crate(
            root, "nested/" + name, f'[package]\nname="{name}"\nversion="0.1.0"\n'
        )
    hub = root / "runtime/molt-passes/Cargo.toml"
    hub.write_text(
        hub.read_text(encoding="utf-8")
        + '[dependencies]\nleaf={path="../nested/leaf_a"}\n',
        encoding="utf-8",
    )
    runtime_manifest = root / "runtime/molt-runtime/Cargo.toml"
    changed = {path: path.stat() for path in (hub, runtime_manifest)}
    real_stat = Path.stat

    def preserved_stat(path, *, follow_symlinks=True):
        # Reproduce the Windows creation-time cache collision on every host.
        # Direct-file custody still observes real lstat/open-handle generations.
        if follow_symlinks and path in changed:
            return changed[path]
        return real_stat(path, follow_symlinks=follow_symlinks)

    monkeypatch.setattr(Path, "stat", preserved_stat)
    monkeypatch.setattr(runtime_features, "_compiler_root", lambda: root)

    def observe():
        with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
            backend = CACHE_FINGERPRINTS._backend_source_paths(root, ("wasm-backend",))
            runtime = runtime_source_closure.runtime_source_paths(root)
            runtime_key = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
                root=root,
                inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths(runtime),
                scope="runtime-manifest-replacement",
                extra_fingerprint_inputs="",
            )
            return (
                set(backend),
                set(runtime),
                runtime_features.profile_link_features("micro", target_triple=None),
                _backend_source_fingerprint(root, ("wasm-backend",)),
                runtime_key,
            )

    old_leaf, new_leaf = (
        root / "runtime/nested" / name for name in ("leaf_a", "leaf_b")
    )
    before = observe()
    assert all(old_leaf in paths and new_leaf not in paths for paths in before[:2])
    assert before[2] == frozenset({"leaf_a"})
    for path, metadata in changed.items():
        original = path.read_text(encoding="utf-8")
        replacement = original.replace('leaf_a"}', 'leaf_b"}').replace(
            'stdlib_micro=["leaf_a"]', 'stdlib_micro=["leaf_b"]'
        )
        assert len(replacement) == len(original) and replacement != original
        path.write_text(replacement, encoding="utf-8")
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    after = observe()
    assert all(new_leaf in paths and old_leaf not in paths for paths in after[:2])
    assert after[2] == frozenset({"leaf_b"})
    source = new_leaf / "src/lib.rs"
    source.write_text(
        source.read_text(encoding="utf-8") + "pub const NEW: u8 = 1;\n",
        encoding="utf-8",
    )
    source_changed = observe()
    assert source_changed[3] != after[3] and source_changed[4] != after[4]


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

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))

    first = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    cli_source.write_text("CLI_MARKER = 2\n", encoding="utf-8")

    second = CACHE_FINGERPRINTS._cache_tooling_fingerprint()

    assert second != first


@pytest.mark.parametrize(
    "filename", ["backend_environment.py", "backend_environment.json"]
)
def test_backend_environment_tooling_changes_preserve_frontend_semantic_scope(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, filename: str
) -> None:
    root = tmp_path / "repo"
    package = root / "src" / "molt"
    frontend = package / "frontend" / "__init__.py"
    frontend.parent.mkdir(parents=True)
    frontend.write_text("FRONTEND_MARKER = 1\n", encoding="utf-8")
    (package / "__init__.py").write_text("", encoding="utf-8")
    backend_environment = package / filename
    backend_environment.write_text("{}\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        lambda *args: None,
    )
    tooling_before = CACHE_FINGERPRINTS._cache_tooling_fingerprint()
    semantic_before = CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint()
    backend_environment.write_text('{"changed": true}\n', encoding="utf-8")
    assert CACHE_FINGERPRINTS._cache_tooling_fingerprint() != tooling_before
    assert (
        CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint() == semantic_before
    )


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

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))

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

    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))

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
        "capability_manifest": molt_root / "capability_manifest.py",
        "capability_policy": molt_root / "capability_policy.py",
        "host_capabilities": molt_root / "_host_capabilities_generated.py",
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
        "capability_manifest",
        "capability_policy",
        "host_capabilities",
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
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))

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
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(root))

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
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
        scope="content-digest-test",
        extra_fingerprint_inputs="",
    )

    tracked.write_text("MARKER = 2\n", encoding="utf-8")  # same length
    os.utime(tracked, ns=(stat.st_atime_ns, stat.st_mtime_ns))

    second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
        scope="content-digest-test",
        extra_fingerprint_inputs="",
    )

    assert second != first


def test_source_tree_cache_fingerprint_is_install_root_and_stat_independent(
    tmp_path: Path,
) -> None:
    first_root = tmp_path / "install-a"
    second_root = tmp_path / "install-b"
    first = first_root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    second = second_root / "src" / "molt" / "frontend" / "cfg_analysis.py"
    first.parent.mkdir(parents=True, exist_ok=True)
    second.parent.mkdir(parents=True, exist_ok=True)
    source = "MARKER = 1\n"
    first.write_text(source, encoding="utf-8")
    second.write_text(source, encoding="utf-8")
    first_stat = first.stat()
    os.utime(
        second,
        ns=(
            first_stat.st_atime_ns + 10_000_000_000,
            first_stat.st_mtime_ns + 10_000_000_000,
        ),
    )

    first_fingerprint = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=first_root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([first]),
        scope="ephemeral-install-test",
        extra_fingerprint_inputs="",
    )
    second_fingerprint = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=second_root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([second]),
        scope="ephemeral-install-test",
        extra_fingerprint_inputs="",
    )

    assert second_fingerprint == first_fingerprint


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

    monkeypatch.setattr(
        CACHE_FINGERPRINTS, "_file_content_signature", counted_signature
    )

    with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
        first = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
            root=root,
            inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
            scope="transaction-test",
            extra_fingerprint_inputs="",
        )
        second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
            root=root,
            inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
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
            inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
            scope="transaction-escape-test",
            extra_fingerprint_inputs="",
        )

    tracked.write_text("MARKER = 2\n", encoding="utf-8")
    second = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
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
        CACHE_FINGERPRINTS, "_file_content_signature", content_walk_is_forbidden
    )

    fingerprint = CACHE_FINGERPRINTS._source_tree_cache_fingerprint(
        root=root,
        inputs=CACHE_FINGERPRINTS._SourceFingerprintInputs.from_paths([tracked]),
        scope="clean-pathspec-test",
        extra_fingerprint_inputs="",
    )

    assert fingerprint
    assert seen_path_keys == [(str(tracked.resolve()),)]


def test_lowering_dependency_graph_reuses_analysis_but_rechecks_source_bytes(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt.cli import python_source_closure as graph

    source = tmp_path / "src" / "molt" / "cli" / "module_source.py"
    source.parent.mkdir(parents=True)
    source.write_text("import molt.cli.first\n", encoding="utf-8")
    first, second = source.with_name("first.py"), source.with_name("other.py")
    first.write_text("VALUE = 1\n", encoding="utf-8")
    second.write_text("VALUE = 2\n", encoding="utf-8")
    analyzed = []
    analyze = graph.analyze_local_imports

    def record(snapshot, *args, **kwargs):
        analyzed.append(snapshot.path)
        return analyze(snapshot, *args, **kwargs)

    monkeypatch.setattr(graph, "analyze_local_imports", record)
    expected = {source, first}
    assert (
        set(CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths)
        == expected
    )
    assert set(analyzed) == expected
    analyzed.clear()
    assert (
        set(CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths)
        == expected
    )
    assert analyzed == []
    before = source.stat()
    source.write_text("import molt.cli.other\n", encoding="utf-8")
    os.utime(source, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert set(CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths) == {
        source,
        second,
    }
    assert set(analyzed) == {source, second}


def test_lowering_dependency_graph_reuse_ends_with_build_transaction(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    from molt.cli import python_source_closure as graph

    source = tmp_path / "src" / "molt" / "cli" / "module_source.py"
    source.parent.mkdir(parents=True)
    source.write_text("import molt.cli.helper\n", encoding="utf-8")
    helper = source.with_name("helper.py")
    capture = graph.LocalPythonModuleResolver.capture_source
    captured = []

    def record(resolver, path):
        captured.append(path)
        return capture(resolver, path)

    monkeypatch.setattr(graph.LocalPythonModuleResolver, "capture_source", record)
    with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
        assert CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths == (
            source,
        )
        with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
            assert CACHE_FINGERPRINTS._lowering_scope_source_closure(
                tmp_path
            ).paths == (source,)
    assert captured == [source]
    helper.write_text("VALUE = 1\n", encoding="utf-8")
    captured.clear()
    with CACHE_FINGERPRINTS._source_tree_fingerprint_transaction():
        assert set(
            CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths
        ) == {
            source,
            helper,
        }
    assert set(captured) == {source, helper}


def test_installed_python_layout_drives_both_fingerprint_scopes(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import molt.cli.python_source_closure as graph

    site_packages = tmp_path / "site-packages"
    package = site_packages / "molt"
    frontend = package / "frontend"
    frontend.mkdir(parents=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    (frontend / "__init__.py").write_text(
        "from molt.target_python import TARGET\n", encoding="utf-8"
    )
    target = package / "target_python.py"
    target.write_text("TARGET = 312\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(tmp_path))
    monkeypatch.setattr(COMPILER_METADATA, "_SRC_ROOT", site_packages)
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        lambda *args: None,
    )

    def read_only_install(*args, **kwargs):
        raise PermissionError("installed package tree is read-only")

    monkeypatch.setattr(graph, "_atomic_write_text", read_only_install)
    assert COMPILER_METADATA._compiler_python_source_root(tmp_path) == site_packages
    assert target in CACHE_FINGERPRINTS._lowering_scope_source_closure(tmp_path).paths
    assert package / "cli" in CACHE_FINGERPRINTS._frontend_tooling_source_paths(
        tmp_path
    )
    paths = CACHE_FINGERPRINTS._frontend_semantic_tooling_sources(tmp_path).paths
    assert frontend in paths and target in paths
    assert all(path.is_relative_to(site_packages) for path in paths)
    first = CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint()
    target.write_text("TARGET = 313\n", encoding="utf-8")
    second = CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint()
    assert second != first
    assert not (tmp_path / "src").exists()
    assert not (tmp_path / ".molt_cache").exists()


def test_selected_compiler_root_does_not_relocate_captured_package_layout(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    installed_sources = tmp_path / "installed" / "site-packages"
    checkout = tmp_path / "checkout"
    source = checkout / "src" / "molt" / "frontend" / "__init__.py"
    source.parent.mkdir(parents=True)
    source.write_text("VALUE = 1\n", encoding="utf-8")
    monkeypatch.setattr(COMPILER_METADATA, "_SRC_ROOT", installed_sources)
    monkeypatch.setenv("MOLT_SOURCE_ROOT", str(checkout))
    monkeypatch.setattr(
        CACHE_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        lambda *args: None,
    )

    assert COMPILER_METADATA._compiler_python_source_root(checkout) == checkout / "src"
    assert (
        COMPILER_METADATA._compiler_python_source_root(installed_sources.parent)
        == installed_sources
    )
    for paths in (
        CACHE_FINGERPRINTS._frontend_tooling_source_paths(checkout),
        CACHE_FINGERPRINTS._frontend_semantic_tooling_sources(checkout).paths,
    ):
        assert all(path.is_relative_to(checkout / "src") for path in paths)
    before = (
        CACHE_FINGERPRINTS._cache_tooling_fingerprint(),
        CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint(),
    )
    source.write_text("VALUE = 2\n", encoding="utf-8")
    after = (
        CACHE_FINGERPRINTS._cache_tooling_fingerprint(),
        CACHE_FINGERPRINTS._frontend_semantic_tooling_fingerprint(),
    )
    assert all(first != second for first, second in zip(before, after, strict=True))


# A hand-authored dependency graph independent of the production projection:
# compiler -> support -> dependency; ABI -> optional Windows dependency. The latter two
# rows belong to the same workspace, but cannot affect the compiler closure.
def _write_compiler_lock_fixture(
    root: Path,
    *,
    abi_windows: bool = False,
    checksum: str = "a" * 64,
    version: str = "1.0.0",
    source: str = "registry+https://example.invalid/index",
) -> None:
    abi_dependencies = '["windows-sys"]' if abi_windows else "[]"
    (root / "Cargo.lock").write_text(
        "version = 4\n"
        '[[package]]\nname = "molt-backend"\nversion = "0.1.0"\n'
        'dependencies = ["compiler-support"]\n'
        '[[package]]\nname = "compiler-support"\nversion = "1.0.0"\n'
        'dependencies = ["compiler-dependency"]\n'
        f'[[package]]\nname = "compiler-dependency"\nversion = "{version}"\n'
        f'source = "{source}"\nchecksum = "{checksum}"\n'
        '[[package]]\nname = "molt-lang-cpython-abi"\nversion = "0.1.0"\n'
        f"dependencies = {abi_dependencies}\n"
        '[[package]]\nname = "windows-sys"\nversion = "0.61.2"\n',
        encoding="utf-8",
    )


@pytest.mark.parametrize("feature", ["native-backend", "wasm-backend"])
@pytest.mark.parametrize("changed", ["checksum", "version", "source"])
def test_backend_lock_identity_tracks_only_reachable_dependency_content(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, feature: str, changed: str
) -> None:
    stub_compiler_admission(monkeypatch)
    from molt.cli import backend_binary

    root = tmp_path / "repo"
    _write_backend_identity_fixture(root)
    _write_compiler_lock_fixture(root)
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_compiler_root", lambda: root)
    monkeypatch.setattr(backend_binary, "_compiler_clean_source_state", lambda *_: None)

    def receipt(stored=None):
        return backend_binary._backend_fingerprint(
            root,
            cargo_profile="release",
            build_admission=compiler_build_admission(environment={"RUSTFLAGS": ""}),
            backend_features=(feature,),
            stored_fingerprint=stored,
        )

    def object_key():
        return CACHE_FINGERPRINTS._cache_fingerprint(
            backend_features=(feature,), include_runtime_sources=False
        )

    cold = receipt()
    assert cold is not None
    cold_key = object_key()
    assert receipt(cold) == cold
    assert object_key() == cold_key
    lock = root / "Cargo.lock"
    timestamps = lock.stat()
    _write_compiler_lock_fixture(root, abi_windows=True)
    os.utime(lock, ns=(timestamps.st_atime_ns, timestamps.st_mtime_ns))
    assert receipt(cold) == cold
    assert object_key() == cold_key
    changed_value = {
        "checksum": "b" * 64,
        "version": "2.0.0",
        "source": "registry+https://other.invalid/index",
    }[changed]
    _write_compiler_lock_fixture(root, abi_windows=True, **{changed: changed_value})
    os.utime(lock, ns=(timestamps.st_atime_ns, timestamps.st_mtime_ns))
    edited = receipt(cold)
    assert edited is not None and edited["hash"] != cold["hash"]
    assert object_key() != cold_key
    assert receipt(edited) == edited


@pytest.mark.parametrize("defect", ["duplicate", "missing", "ambiguous", "malformed"])
def test_compiler_lock_dependency_identity_fails_closed(
    tmp_path: Path, defect: str
) -> None:
    from molt.cli.cargo_source_closure import _cargo_locked_dependency_digest

    _write_backend_identity_fixture(tmp_path)
    _write_compiler_lock_fixture(tmp_path)
    lock = tmp_path / "Cargo.lock"
    source = lock.read_text(encoding="utf-8")
    if defect == "duplicate":
        source += '\n[[package]]\nname = "molt-backend"\nversion = "0.1.0"\n'
    elif defect == "missing":
        source = source.replace('["compiler-dependency"]', '["missing-dependency"]')
    elif defect == "ambiguous":
        source += (
            '\n[[package]]\nname = "compiler-dependency"\nversion = "1.0.0"\n'
            'source = "registry+https://other.invalid/index"\n'
        )
    else:
        source = source.replace('["compiler-dependency"]', '["compiler-dependency ("]')
    lock.write_text(source, encoding="utf-8")
    with pytest.raises(ValueError):
        _cargo_locked_dependency_digest(tmp_path, tmp_path / "runtime/molt-backend")


def test_compiler_lock_dependency_identity_resolves_same_name_version_by_source(
    tmp_path: Path,
) -> None:
    from molt.cli.cargo_source_closure import _cargo_locked_dependency_digest

    _write_backend_identity_fixture(tmp_path)
    _write_compiler_lock_fixture(tmp_path)
    lock = tmp_path / "Cargo.lock"
    source = lock.read_text(encoding="utf-8").replace(
        '["compiler-dependency"]',
        '["compiler-dependency 1.0.0 (registry+https://example.invalid/index)"]',
    )
    source += (
        '\n[[package]]\nname = "compiler-dependency"\nversion = "1.0.0"\n'
        'source = "registry+https://other.invalid/index"\n'
        f'checksum = "{"c" * 64}"\n'
    )
    lock.write_text(source, encoding="utf-8")
    before = _cargo_locked_dependency_digest(
        tmp_path, tmp_path / "runtime/molt-backend"
    )
    lock.write_text(source.replace("c" * 64, "d" * 64), encoding="utf-8")
    assert (
        _cargo_locked_dependency_digest(tmp_path, tmp_path / "runtime/molt-backend")
        == before
    )
