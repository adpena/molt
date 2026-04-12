from __future__ import annotations

from pathlib import Path

import molt.cli as cli


def test_is_valid_static_library_artifact_checks_archive_magic(tmp_path: Path) -> None:
    valid = tmp_path / "libmolt_runtime.a"
    valid.write_bytes(b"!<arch>\nrest")
    invalid = tmp_path / "libmolt_runtime.a.bad"
    invalid.write_bytes(b"runtime")

    assert cli._is_valid_static_library_artifact(valid) is True
    assert cli._artifact_content_looks_valid(valid) is True
    assert cli._artifact_content_looks_valid(invalid) is True

    invalid_static = tmp_path / "libmolt_runtime.a"
    invalid_static.write_bytes(b"runtime")
    assert cli._is_valid_static_library_artifact(invalid_static) is False
    assert cli._artifact_content_looks_valid(invalid_static) is False


def test_artifact_needs_rebuild_for_invalid_static_library_even_with_matching_fingerprint(
    tmp_path: Path,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"runtime")
    fingerprint = {"hash": "abc", "rustc": "rustc-test"}

    assert (
        cli._artifact_needs_rebuild(runtime_lib, fingerprint, dict(fingerprint)) is True
    )


def test_artifact_newer_than_sources_rejects_invalid_static_library(
    tmp_path: Path,
) -> None:
    runtime_lib = tmp_path / "libmolt_runtime.a"
    runtime_lib.write_bytes(b"runtime")
    source = tmp_path / "source.rs"
    source.write_text("// source\n")

    assert cli._artifact_newer_than_sources(runtime_lib, [source]) is False
