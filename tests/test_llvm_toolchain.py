from __future__ import annotations

from pathlib import Path

import pytest

from molt.llvm_toolchain import (
    LlvmToolchainConfigError,
    llvm_sys_prefix_env_var,
    required_llvm_backend_pin,
)


ROOT = Path(__file__).resolve().parents[1]


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _write_facade(root: Path, feature_values: str) -> None:
    _write(
        root / "runtime/molt-backend/Cargo.toml",
        f"""
[package]
name = "molt-backend"
version = "0.1.0"
edition = "2024"

[features]
llvm = [{feature_values}]
""".lstrip(),
    )


def _write_native(root: Path, inkwell_features: str, llvm_sys_version: str) -> None:
    _write(
        root / "runtime/molt-backend-native/Cargo.toml",
        f"""
[package]
name = "molt-backend-native"
version = "0.1.0"
edition = "2024"

[dependencies]
llvm-sys = {{ version = "{llvm_sys_version}", optional = true }}
inkwell = {{ version = "0.9", features = [{inkwell_features}], optional = true }}
""".lstrip(),
    )


def test_required_llvm_backend_pin_matches_current_manifest() -> None:
    pin = required_llvm_backend_pin(ROOT)

    assert pin is not None
    assert pin.major == 22
    assert pin.minor == 1
    assert pin.inkwell_feature == "llvm22-1"
    assert pin.env_var == "LLVM_SYS_221_PREFIX"
    assert pin.default_release == "22.1.8"


def test_llvm_sys_prefix_env_var_uses_major_and_minor() -> None:
    assert llvm_sys_prefix_env_var(22, 1) == "LLVM_SYS_221_PREFIX"
    assert llvm_sys_prefix_env_var(19, 0) == "LLVM_SYS_190_PREFIX"


def test_required_llvm_backend_pin_follows_facade_to_native(tmp_path: Path) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")

    pin = required_llvm_backend_pin(tmp_path)

    assert pin is not None
    assert pin.major == 22
    assert pin.minor == 1
    assert pin.env_var == "LLVM_SYS_221_PREFIX"


def test_required_llvm_backend_pin_rejects_facade_without_native_route(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"dep:molt-backend-native"')
    _write_native(tmp_path, '"llvm22-1"', "221.0.1")

    with pytest.raises(LlvmToolchainConfigError, match="molt-backend-native/llvm"):
        required_llvm_backend_pin(tmp_path)


def test_required_llvm_backend_pin_rejects_conflicting_inkwell_features(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm21-1", "llvm22-1"', "221.0.1")

    with pytest.raises(LlvmToolchainConfigError, match="conflicting LLVM pins"):
        required_llvm_backend_pin(tmp_path)


def test_required_llvm_backend_pin_rejects_llvm_sys_mismatch(
    tmp_path: Path,
) -> None:
    _write_facade(tmp_path, '"molt-backend-native/llvm"')
    _write_native(tmp_path, '"llvm22-1"', "211.0.0")

    with pytest.raises(LlvmToolchainConfigError, match="does not match"):
        required_llvm_backend_pin(tmp_path)
