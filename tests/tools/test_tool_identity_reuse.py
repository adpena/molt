"""Content-addressed toolchain identity reuse re-proves every byte it reuses."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import sys

import pytest

from tools import proof_plan
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    process_image_capture,
)


def _git_probe(tmp_path: Path) -> tuple[dict[str, object], list[str], dict[str, str]]:
    if shutil.which("git") is None:
        pytest.skip("git toolchain is not installed")
    command = ["git", "status"]
    envelope = command_admission.envelope_for_command(command)
    env = {
        name: value
        for name, value in os.environ.items()
        if name not in command_identity.OPERATIONAL_CARGO_NAMES
    }
    env["TMPDIR"] = str(tmp_path)
    return envelope, command, env


def _identity(
    tmp_path: Path,
    reuse_root: Path,
    *,
    env_overrides: dict[str, str] | None = None,
) -> tuple[dict[str, object], list[dict[str, object]]]:
    envelope, command, env = _git_probe(tmp_path)
    telemetry: list[dict[str, object]] = []
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "git",
        envelope,
        command,
        cwd=tmp_path,
        env={**env, **(env_overrides or {})},
        reuse_root=reuse_root,
        reuse_telemetry=telemetry,
    )
    return identity, telemetry


def test_second_capture_reuses_the_record_without_running_the_probe(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    assert [row["state"] for row in first_telemetry] == ["miss"]
    assert first_telemetry[0]["reason"] == "absent"
    record = Path(str(first_telemetry[0]["record"]))
    assert record.is_file()
    stored = json.loads(record.read_text(encoding="utf-8"))
    assert stored["schema"] == command_identity.TOOL_IDENTITY_REUSE_SCHEMA
    assert stored["identity"] == first

    def forbidden(*args: object, **kwargs: object) -> None:
        raise AssertionError("a reused identity must not run its version probe")

    monkeypatch.setattr(command_identity, "_run_captured", forbidden)
    second, second_telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert [row["state"] for row in second_telemetry] == ["hit"]
    assert second_telemetry[0]["revalidated_images"] == len(first["process_images"])


def test_reuse_misses_when_a_recorded_image_no_longer_hashes_the_same(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    record = Path(str(first_telemetry[0]["record"]))
    stored = json.loads(record.read_text(encoding="utf-8"))
    stored["identity"]["process_images"][0]["sha256"] = "0" * 64
    record.write_text(json.dumps(stored), encoding="utf-8")

    second, second_telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert second_telemetry[0]["state"] == "miss"
    assert second_telemetry[0]["reason"] == "revalidation-drift"
    # The fresh capture replaces the drifted record.
    restored = json.loads(record.read_text(encoding="utf-8"))
    assert restored["identity"] == first


def test_reuse_key_binds_resolution_environment_and_probe_cwd(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    _identity(tmp_path, reuse_root)
    _, changed_path = _identity(
        tmp_path,
        reuse_root,
        env_overrides={"PATH": os.environ["PATH"] + os.pathsep + str(tmp_path)},
    )
    assert changed_path[0]["state"] == "miss"
    assert changed_path[0]["reason"] == "absent"
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    envelope, command, env = _git_probe(tmp_path)
    telemetry: list[dict[str, object]] = []
    command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "git",
        envelope,
        command,
        cwd=other_cwd,
        env=env,
        reuse_root=reuse_root,
        reuse_telemetry=telemetry,
    )
    assert telemetry[0]["state"] == "miss"
    assert len(list(reuse_root.glob("*.json"))) == 3


def test_malformed_or_oversized_records_degrade_to_a_fresh_capture(
    tmp_path: Path,
) -> None:
    reuse_root = tmp_path / "tool-identity"
    first, first_telemetry = _identity(tmp_path, reuse_root)
    record = Path(str(first_telemetry[0]["record"]))
    record.write_text("{not json", encoding="utf-8")
    second, telemetry = _identity(tmp_path, reuse_root)
    assert second == first
    assert telemetry[0] == {
        **telemetry[0],
        "state": "miss",
        "reason": "absent",
    }
    record.write_bytes(b"{" + b" " * command_identity._MAX_REUSE_RECORD_BYTES + b"}")
    _, telemetry = _identity(tmp_path, reuse_root)
    assert telemetry[0]["state"] == "miss"


def test_revalidation_rejects_changed_bytes_and_foreign_fields(tmp_path: Path) -> None:
    image = tmp_path / ("tool.exe" if sys.platform == "win32" else "tool")
    image.write_bytes(b"#!/bin/sh\nexit 0\n")
    image.chmod(0o755)
    row = process_image_capture.capture_image("probe", image)
    material: dict[str, object] = {
        "path": str(image),
        "launcher_sha256": row["sha256"],
        "content_path": str(image),
        "executable_sha256": row["sha256"],
        "version": "probe 1.0",
        "probe_cwd": str(tmp_path),
        "policy_sha256": "a" * 64,
        "configuration_files": [],
        "process_images": [row],
    }
    material["identity_sha256"] = command_identity.hashlib.sha256(
        json.dumps(material, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    current = command_identity._reused_identity_is_current(
        "probe", material, cwd=tmp_path, env={}, command_argv=["probe"]
    )
    assert current

    foreign = {**material, "runtime": {"execPath": str(image)}}
    assert not command_identity._reused_identity_is_current(
        "probe", foreign, cwd=tmp_path, env={}, command_argv=["probe"]
    )

    tampered = dict(material)
    tampered["version"] = "probe 2.0"
    assert not command_identity._reused_identity_is_current(
        "probe", tampered, cwd=tmp_path, env={}, command_argv=["probe"]
    )

    image.write_bytes(b"#!/bin/sh\nexit 1\n")
    assert not command_identity._reused_identity_is_current(
        "probe", material, cwd=tmp_path, env={}, command_argv=["probe"]
    )


def test_compile_environment_selection_is_one_authority() -> None:
    env = {
        "RUSTFLAGS": "-C opt-level=1",
        "CARGO_TARGET_DIR": "/elsewhere",
        "CARGO_PROFILE_RELEASE_LTO": "fat",
        "CC_x86_64_unknown_linux_gnu": "clang",
        "PATH": "/usr/bin",
        "MOLT_CUSTOM": "1",
    }
    selected = command_identity.compile_environment_selection(
        env, configured_names=["MOLT_CUSTOM"]
    )
    assert set(selected) == {
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "MOLT_CUSTOM",
    }
    probe = command_identity._probe_environment_selection(env)
    assert set(probe) == {
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "PATH",
    }
