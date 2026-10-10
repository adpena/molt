from __future__ import annotations

import re
import tomllib
from pathlib import Path

import pytest

from molt.cli import runtime_native_build as runtime_build
from molt.cli.runtime_artifact_selection import (
    RUNTIME_CDYLIB_ARTIFACTS,
    RUNTIME_RLIB_ARTIFACTS,
    RUNTIME_STATICLIB_ARTIFACTS,
    RUNTIME_WASM_COMBINED_ARTIFACTS,
    RuntimeArtifactSelection,
    RuntimeCrateType,
)

ROOT = Path(__file__).resolve().parents[2]


def test_runtime_artifact_selections_are_exact_cargo_level_values() -> None:
    assert RUNTIME_RLIB_ARTIFACTS.cargo_args() == ("--crate-type", "rlib")
    assert RUNTIME_STATICLIB_ARTIFACTS.cargo_args() == (
        "--crate-type",
        "staticlib",
    )
    assert RUNTIME_CDYLIB_ARTIFACTS.cargo_args() == ("--crate-type", "cdylib")
    assert RUNTIME_WASM_COMBINED_ARTIFACTS.cargo_args() == (
        "--crate-type",
        "staticlib,cdylib",
    )
    assert not RUNTIME_WASM_COMBINED_ARTIFACTS.includes(RuntimeCrateType.RLIB)


def test_runtime_artifact_selection_rejects_empty_duplicate_and_rustc_level_use() -> (
    None
):
    with pytest.raises(ValueError, match="cannot be empty"):
        RuntimeArtifactSelection(())
    with pytest.raises(ValueError, match="cannot contain duplicates"):
        RuntimeArtifactSelection(
            (RuntimeCrateType.STATICLIB, RuntimeCrateType.STATICLIB)
        )
    command = ["cargo", "rustc", "--", "--print", "native-static-libs"]
    with pytest.raises(ValueError, match="before Cargo's -- separator"):
        RUNTIME_STATICLIB_ARTIFACTS.select_in(command)


def test_native_runtime_producer_selects_only_staticlib_before_rustc_args() -> None:
    command = runtime_build._native_runtime_cargo_command(
        cargo_profile="release-output",
        concrete_stdlib_profile="micro",
        runtime_features=(),
        builtin_features=(),
        concrete_stdlib_feature="stdlib_micro",
        target_triple=None,
    )
    separator = command.index("--")
    assert command[separator - 2 : separator] == ["--crate-type", "staticlib"]
    assert command[separator:] == ["--", "--print", "native-static-libs"]
    assert "rlib" not in command
    assert "cdylib" not in command


def test_user_facing_artifact_guidance_cannot_return_to_cargo_build() -> None:
    legacy_producer = re.compile(
        r"cargo\s+build[^\n`]*(?:-p|--package)\s+molt-runtime(?:\s|`|$)"
    )
    paths = (
        ROOT / "docs" / "DEVELOPER_GUIDE.md",
        ROOT / "docs" / "OPERATIONS.md",
        ROOT / "docs" / "architecture" / "compilation-model.md",
        ROOT / "tests" / "test_exception_constructors.py",
    )
    for path in paths:
        text = path.read_text(encoding="utf-8")
        assert legacy_producer.search(text) is None, path
    developer_guide = paths[0].read_text(encoding="utf-8")
    assert (
        "cargo rustc --release --package molt-runtime --crate-type staticlib"
        in developer_guide
    )


def test_default_rlib_build_script_has_no_retired_native_cdylib_link_lane() -> None:
    build_script = (ROOT / "runtime" / "molt-runtime" / "build.rs").read_text(
        encoding="utf-8"
    )

    assert "cargo:rustc-cdylib-link-arg" not in build_script
    assert "cargo:rustc-link-arg" not in build_script
    with (ROOT / "runtime" / "molt-runtime" / "Cargo.toml").open("rb") as manifest_file:
        manifest = tomllib.load(manifest_file)
    assert manifest["lib"]["crate-type"] == ["rlib"]
    host_example = next(
        example for example in manifest["example"] if example["name"] == "cext_host"
    )
    assert host_example["crate-type"] == ["cdylib"]
    assert host_example["required-features"] == ["cext_loader"]


@pytest.mark.parametrize(
    "selection,links",
    [
        (RUNTIME_RLIB_ARTIFACTS, False),
        (RUNTIME_STATICLIB_ARTIFACTS, False),
        (RUNTIME_CDYLIB_ARTIFACTS, True),
        (RUNTIME_WASM_COMBINED_ARTIFACTS, True),
    ],
)
def test_runtime_artifact_producer_roundtrips_through_capture_authority(
    selection, links
):
    from tools.proof_queue_pkg.command_admission import (
        parse_cargo_invocation,
        rust_link_artifact_selection,
    )

    command = ["cargo", "rustc", "--lib"]
    selection.select_in(command)
    command.extend(("--", "--print", "native-static-libs"))
    assert parse_cargo_invocation(command).crate_types == tuple(
        kind.value for kind in selection.crate_types
    )
    admitted = rust_link_artifact_selection(
        command,
        cargo=True,
        cargo_invocation=parse_cargo_invocation(command),
        unit="target",
    )
    assert admitted["cargo_crate_types"] == [
        kind.value for kind in selection.crate_types
    ]
    assert admitted["rustc_crate_types"] == []
    assert admitted["link_required"] is links
