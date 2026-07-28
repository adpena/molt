from __future__ import annotations

import re
from pathlib import Path

import pytest

from molt.cli import runtime_fingerprints
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


def test_artifact_selection_is_part_of_runtime_cache_source_identity(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(runtime_fingerprints, "_rustc_version", lambda: "rustc-test")
    monkeypatch.setattr(
        runtime_fingerprints,
        "_compiler_clean_pathspec_source_state",
        lambda _root, _paths: None,
    )
    monkeypatch.setattr(
        runtime_fingerprints,
        "runtime_source_paths",
        lambda _root, runtime_features=(): [],
    )
    staticlib = runtime_fingerprints._runtime_fingerprint(
        tmp_path,
        cargo_profile="release-output",
        target_triple=None,
        rustflags="",
        artifact_selection=RUNTIME_STATICLIB_ARTIFACTS,
    )
    cdylib = runtime_fingerprints._runtime_fingerprint(
        tmp_path,
        cargo_profile="release-output",
        target_triple=None,
        rustflags="",
        artifact_selection=RUNTIME_CDYLIB_ARTIFACTS,
    )
    assert staticlib is not None and cdylib is not None
    assert staticlib["meta_digest"] != cdylib["meta_digest"]
    assert staticlib["hash"] != cdylib["hash"]


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
