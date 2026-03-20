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


def test_workspace_dev_profile_trims_backend_debug_info() -> None:
    manifest = _load_workspace_manifest()
    profiles = manifest["profile"]
    dev_packages = profiles["dev"]["package"]
    dev_fast_packages = profiles["dev-fast"]["package"]
    expected_packages = {
        "molt-backend",
        "cranelift-codegen",
        "cranelift-frontend",
        "cranelift-module",
        "cranelift-native",
        "cranelift-object",
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
        "rustpython-parser",
        "rustpython-ast",
        "rustpython-parser-core",
        "rustls",
        "rustls-webpki",
        "simdutf",
        "tungstenite",
        "unicode_names2",
        "url",
        "xz2",
        "lzma-sys",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0


def test_runtime_manifest_uses_flate2_zip_deflate_only() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    zip_dependency = runtime_manifest["dependencies"]["zip"]

    assert zip_dependency["default-features"] is False
    assert zip_dependency["features"] == ["deflate-flate2-zlib-rs"]


def test_runtime_manifest_uses_minimal_rustpython_parser_features() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    parser_dependency = runtime_manifest["dependencies"]["rustpython-parser"]

    assert parser_dependency["default-features"] is False
    assert set(parser_dependency["features"]) == {"location", "num-bigint"}


def test_runtime_manifest_declares_vfs_bundle_tar_feature() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    assert "vfs_bundle_tar" in runtime_manifest["features"]


def test_backend_manifest_gates_loop_continue_to_native_backend() -> None:
    manifest = _load_backend_manifest()
    tests = manifest.get("test", [])
    loop_continue = next(test for test in tests if test["name"] == "loop_continue")
    assert loop_continue["required-features"] == ["native-backend"]


def test_runtime_manifest_avoids_url_compile_graph_for_websocket_client() -> None:
    runtime_manifest_path = ROOT / "runtime" / "molt-runtime" / "Cargo.toml"
    with runtime_manifest_path.open("rb") as handle:
        runtime_manifest = tomllib.load(handle)

    native_deps = runtime_manifest["target"]['cfg(not(target_arch = "wasm32"))']["dependencies"]

    assert "url" not in native_deps
    assert "url" not in native_deps["tungstenite"]["features"]
