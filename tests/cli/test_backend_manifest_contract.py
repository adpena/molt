from __future__ import annotations

from pathlib import Path
import tomllib


ROOT = Path(__file__).resolve().parents[2]


def _load_workspace_manifest() -> dict[str, object]:
    with (ROOT / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def _load_backend_manifest() -> dict[str, object]:
    with (ROOT / "runtime" / "molt-backend" / "Cargo.toml").open("rb") as handle:
        return tomllib.load(handle)


def test_backend_manifest_does_not_depend_on_obj_model() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]
    assert "molt-obj-model" not in dependencies


def test_backend_manifest_does_not_redeclare_wasmparser_in_dev_dependencies() -> None:
    manifest = _load_backend_manifest()
    dev_dependencies = manifest.get("dev-dependencies", {})
    assert "wasmparser" not in dev_dependencies


def test_backend_manifest_uses_serde_with_derive_feature() -> None:
    manifest = _load_backend_manifest()
    dependencies = manifest["dependencies"]

    # serde is required for JSON boundary, IR serialization, and TIR
    assert "serde" in dependencies
    serde_dep = dependencies["serde"]
    assert "derive" in serde_dep.get("features", [])


def test_backend_manifest_uses_minimal_cranelift_codegen_features() -> None:
    manifest = _load_backend_manifest()
    codegen_dependency = manifest["dependencies"]["cranelift-codegen"]

    assert codegen_dependency["default-features"] is False
    assert set(codegen_dependency["features"]) == {"host-arch", "std", "unwind"}


def test_backend_manifest_target_overlays_only_add_cross_isa_support() -> None:
    manifest = _load_backend_manifest()
    target_tables = manifest["target"]
    aarch64_dependency = target_tables['cfg(target_arch = "aarch64")']["dependencies"][
        "cranelift-codegen"
    ]
    x86_64_dependency = target_tables['cfg(target_arch = "x86_64")']["dependencies"][
        "cranelift-codegen"
    ]

    assert aarch64_dependency["default-features"] is False
    assert aarch64_dependency["features"] == ["x86"]
    assert x86_64_dependency["default-features"] is False
    assert x86_64_dependency["features"] == ["arm64"]


def test_workspace_dev_profile_trims_backend_debug_info() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    dev_packages = profiles["dev"]["package"]
    dev_fast_packages = profiles["dev-fast"]["package"]
    expected_packages = {
        "molt-backend",
        "cranelift-codegen",
        "cranelift-frontend",
        "gimli",
        "cranelift-module",
        "cranelift-native",
        "cranelift-object",
        "object",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0


def test_workspace_dev_profile_trims_runtime_debug_info() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    dev_packages = profiles["dev"]["package"]
    dev_fast_packages = profiles["dev-fast"]["package"]
    expected_packages = {
        "molt-runtime",
        "aws-lc-rs",
        "aws-lc-sys",
        "httparse",
        "libbz2-rs-sys",
        "proc-macro2",
        "quote",
        "rustpython-parser",
        "rustpython-ast",
        "rustpython-parser-core",
        "rustls",
        "rustls-pemfile",
        "rustls-webpki",
        "serde",
        "serde_core",
        "serde_json",
        "simdutf",
        "syn",
        "thiserror",
        "tungstenite",
        "unicode_names2",
        "url",
        "webpki-roots",
        "xz2",
        "zlib-rs",
        "lzma-sys",
        "zip",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0


def test_workspace_dev_fast_does_not_force_opt_level() -> None:
    manifest = _load_workspace_manifest()
    dev_fast_profile = manifest["profile"]["dev-fast"]

    assert "opt-level" not in dev_fast_profile


def test_runtime_manifest_uses_flate2_zip_deflate_only() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    zip_dependency = runtime_manifest["dependencies"]["zip"]

    assert zip_dependency["default-features"] is False
    assert zip_dependency["features"] == ["deflate"]


def test_runtime_manifest_uses_minimal_rustpython_parser_features() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    parser_dependency = runtime_manifest["dependencies"]["rustpython-parser"]

    assert parser_dependency["default-features"] is False
    assert set(parser_dependency["features"]) == {"location", "num-bigint"}


def test_runtime_manifest_dedupes_unicode_names2_version() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    text_manifest_path = ROOT / "runtime" / "molt-runtime-text" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)
    with text_manifest_path.open("rb") as handle:
        text_manifest = tomllib.load(handle)

    runtime_dep = runtime_manifest["dependencies"]["unicode_names2"]
    text_dep = text_manifest["dependencies"]["unicode_names2"]
    runtime_version = (
        runtime_dep["version"] if isinstance(runtime_dep, dict) else runtime_dep
    )
    text_version = text_dep["version"] if isinstance(text_dep, dict) else text_dep

    assert runtime_version == text_version == "2.0"


def test_runtime_manifest_declares_vfs_bundle_tar_feature() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    assert "vfs_bundle_tar" in runtime_manifest["features"]


def test_runtime_manifest_crate_types_include_all_link_targets() -> None:
    """Validate the runtime ships staticlib + rlib + cdylib.

    cdylib is required so ``cargo build -p molt-runtime --target wasm32-…``
    emits a stable ``.wasm`` artifact consumed by the WASM split-runtime lane.
    See ``build.rs`` for the authoritative rationale.
    """
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    crate_types = runtime_manifest["lib"]["crate-type"]

    assert "staticlib" in crate_types
    assert "rlib" in crate_types
    assert "cdylib" in crate_types


def test_backend_manifest_gates_loop_continue_to_native_backend() -> None:
    manifest = _load_backend_manifest()
    tests = manifest.get("test", [])
    loop_continue = next(test for test in tests if test["name"] == "loop_continue")
    assert loop_continue["required-features"] == ["native-backend"]


def test_runtime_manifest_avoids_url_compile_graph_for_websocket_client() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    native_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))'][
        "dependencies"
    ]

    assert "url" not in native_deps
    assert "url" not in native_deps["tungstenite"]["features"]


def test_backend_ir_model_and_passes_are_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()
    ir_rs = ROOT / "runtime" / "molt-backend" / "src" / "ir.rs"
    passes_rs = ROOT / "runtime" / "molt-backend" / "src" / "passes.rs"

    assert ir_rs.exists()
    assert passes_rs.exists()
    assert "pub struct SimpleIR" not in lib_rs
    assert "pub fn validate_simple_ir" not in lib_rs


def test_backend_native_trampoline_identity_is_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()
    native_backend_mod = (
        ROOT / "runtime" / "molt-backend" / "src" / "native_backend" / "mod.rs"
    )
    trampolines_rs = (
        ROOT / "runtime" / "molt-backend" / "src" / "native_backend" / "trampolines.rs"
    )

    assert native_backend_mod.exists()
    assert trampolines_rs.exists()
    assert "struct TrampolineKey" not in lib_rs


def test_backend_native_compile_func_is_split_out_of_lib_rs() -> None:
    lib_rs = (ROOT / "runtime" / "molt-backend" / "src" / "lib.rs").read_text()
    function_compiler_rs = (
        ROOT
        / "runtime"
        / "molt-backend"
        / "src"
        / "native_backend"
        / "function_compiler.rs"
    )

    assert function_compiler_rs.exists()
    assert "fn compile_func(" not in lib_rs
