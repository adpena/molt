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
        "rustpython-parser",
        "rustpython-ast",
        "rustpython-parser-core",
        "rustls",
        "rustls-webpki",
        "tungstenite",
        "url",
    }

    for packages in (dev_packages, dev_fast_packages):
        assert expected_packages <= packages.keys()
        for package in expected_packages:
            assert packages[package]["debug"] == 0
