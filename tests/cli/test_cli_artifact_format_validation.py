from __future__ import annotations

from pathlib import Path

import molt.cli as cli
from molt.exact_json import canonical_json_sha256
from tests.cli.native_link_test_support import static_archive_bytes


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
    fingerprint = {
        "hash": canonical_json_sha256("artifact-format"),
        "rustc": "rustc-test",
    }
    stored = {"version": 3, **fingerprint}
    runtime_lib.write_bytes(static_archive_bytes(b"object"))
    assert cli._artifact_needs_rebuild(runtime_lib, fingerprint, stored) is False
    runtime_lib.write_bytes(b"runtime")

    assert cli._artifact_needs_rebuild(runtime_lib, fingerprint, stored) is True
