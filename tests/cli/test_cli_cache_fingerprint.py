from __future__ import annotations

import importlib
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
    monkeypatch.setattr(CACHE_FINGERPRINTS, "_runtime_source_paths", lambda root: [])
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


def _write_crate(root: Path, name: str, manifest: str, lib_text: str = "") -> Path:
    crate_root = root / "runtime" / name
    src = crate_root / "src" / "lib.rs"
    src.parent.mkdir(parents=True, exist_ok=True)
    src.write_text(lib_text or f"pub const MARKER: &str = {name!r};\n", encoding="utf-8")
    (crate_root / "Cargo.toml").write_text(manifest, encoding="utf-8")
    return src


def _write_backend_identity_fixture(root: Path, *, include_wasm: bool = True) -> dict[str, Path]:
    root.mkdir(parents=True, exist_ok=True)
    (root / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
    (root / "Cargo.lock").write_text("# lock\n", encoding="utf-8")
    wasm_dep = (
        'molt-backend-wasm = { path = "../molt-backend-wasm", optional = true, '
        'default-features = false }\n'
        if include_wasm
        else ""
    )
    wasm_feature = (
        'wasm-backend = ["dep:molt-backend-wasm", '
        '"molt-backend-wasm/wasm-backend"]\n'
        if include_wasm
        else 'wasm-backend = []\n'
    )
    backend = _write_crate(
        root,
        "molt-backend",
        "[package]\nname = \"molt-backend\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\" }\n"
        "molt-tir = { path = \"../molt-tir\" }\n"
        "molt-backend-native = { path = \"../molt-backend-native\", optional = true, default-features = false }\n"
        "molt-backend-rust = { path = \"../molt-backend-rust\", optional = true, default-features = false }\n"
        "molt-backend-luau = { path = \"../molt-backend-luau\", optional = true, default-features = false }\n"
        f"{wasm_dep}"
        "[features]\n"
        "default = [\"native-backend\"]\n"
        "native-backend = [\"dep:molt-backend-native\", \"molt-backend-native/native-backend\"]\n"
        "rust-backend = [\"dep:molt-backend-rust\", \"molt-backend-rust/rust-backend\"]\n"
        "luau-backend = [\"dep:molt-backend-luau\", \"molt-backend-luau/luau-backend\"]\n"
        f"{wasm_feature}",
    )
    native = _write_crate(
        root,
        "molt-backend-native",
        "[package]\nname = \"molt-backend-native\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\" }\n"
        "molt-tir = { path = \"../molt-tir\" }\n"
        "molt-codegen-abi = { path = \"../molt-codegen-abi\" }\n"
        "[features]\ndefault = []\nnative-backend = []\nllvm = []\n",
    )
    wasm = _write_crate(
        root,
        "molt-backend-wasm",
        "[package]\nname = \"molt-backend-wasm\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\", default-features = false }\n"
        "molt-tir = { path = \"../molt-tir\", default-features = false }\n"
        "molt-codegen-abi = { path = \"../molt-codegen-abi\" }\n"
        "[features]\ndefault = []\nwasm-backend = [\"molt-ir/wasm-backend\", \"molt-tir/wasm-backend\"]\n",
    )
    _write_crate(
        root,
        "molt-backend-rust",
        "[package]\nname = \"molt-backend-rust\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\" }\n"
        "molt-tir = { path = \"../molt-tir\" }\n"
        "[features]\ndefault = []\nrust-backend = []\n",
    )
    _write_crate(
        root,
        "molt-backend-luau",
        "[package]\nname = \"molt-backend-luau\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\" }\n"
        "molt-tir = { path = \"../molt-tir\" }\n"
        "[features]\ndefault = []\nluau-backend = []\n",
    )
    _write_crate(
        root,
        "molt-tir",
        "[package]\nname = \"molt-tir\"\nversion = \"0.1.0\"\n"
        "[dependencies]\n"
        "molt-ir = { path = \"../molt-ir\" }\n"
        "molt-passes = { path = \"../molt-passes\" }\n"
        "[features]\ndefault = []\nwasm-backend = [\"molt-ir/wasm-backend\"]\n",
    )
    _write_crate(
        root,
        "molt-ir",
        "[package]\nname = \"molt-ir\"\nversion = \"0.1.0\"\n"
        "[features]\ndefault = []\nwasm-backend = []\n",
    )
    _write_crate(
        root,
        "molt-passes",
        "[package]\nname = \"molt-passes\"\nversion = \"0.1.0\"\n",
    )
    _write_crate(
        root,
        "molt-codegen-abi",
        "[package]\nname = \"molt-codegen-abi\"\nversion = \"0.1.0\"\n",
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


def test_backend_source_fingerprint_tracks_selected_leaf_sources(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    sources = _write_backend_identity_fixture(root)

    wasm_first = _backend_source_fingerprint(root, ("wasm-backend",))
    native_first = _backend_source_fingerprint(root, ("native-backend",))

    sources["wasm"].write_text(
        "pub const WASM_MARKER: &str = \"changed-wasm-leaf\";\n",
        encoding="utf-8",
    )

    wasm_second = _backend_source_fingerprint(root, ("wasm-backend",))
    native_after_wasm = _backend_source_fingerprint(root, ("native-backend",))
    assert wasm_second != wasm_first
    assert native_after_wasm == native_first

    sources["native"].write_text(
        "pub const NATIVE_MARKER: &str = \"changed-native-leaf\";\n",
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
