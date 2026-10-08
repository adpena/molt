"""Content-addressed toolchain identity reuse re-proves every byte it reuses."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
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
        "CARGO": "/selected/cargo",
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
        "CARGO",
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "MOLT_CUSTOM",
    }
    probe = command_identity._probe_environment_selection(env)
    assert set(probe) == {
        "CARGO",
        "RUSTFLAGS",
        "CARGO_PROFILE_RELEASE_LTO",
        "CC_x86_64_unknown_linux_gnu",
        "PATH",
    }


@pytest.mark.parametrize("explicit", [False, True])
@pytest.mark.parametrize("selected_name", ["cargo", "custom-cargo"])
def test_cargo_capture_reuse_and_rust_link_probe_share_bound_payload(
    tmp_path, monkeypatch, explicit, selected_name
):
    from tools.proof_queue_pkg import toolchain_capture

    suffix = ".exe" if os.name == "nt" else ""
    paths = {}
    for role in ("selected", "explicit", "decoy"):
        directory = tmp_path / role
        directory.mkdir()
        path = directory / ((selected_name if role == "selected" else "cargo") + suffix)
        path.write_bytes((role + " cargo image").encode())
        path.chmod(0o755)
        paths[role] = path
    rustc = tmp_path / ("rustc" + suffix)
    rustc.write_bytes(b"independent rustc image")
    rustc.chmod(0o755)
    payload = [
        str(paths["explicit"]) if explicit else "cargo",
        "build",
        "--target",
        "wasm32-wasip1",
    ]
    command = [sys.executable, "tools/guarded_exec.py", "--", *payload]
    env = {
        "CARGO": str(paths["selected"]),
        "RUSTC": str(rustc),
        "PATH": str(paths["decoy"].parent),
    }
    envelope = command_admission.envelope_for_command(command)
    exact = command_identity._exact_command(envelope, cwd=proof_plan.ROOT, env=env)
    command_identity._bind_delegated_command(
        envelope, exact, cwd=proof_plan.ROOT, env=env
    )
    expected = paths["explicit"] if explicit else paths["selected"]
    versions = []

    def version(argv, **kwargs):
        versions.append(list(argv))
        return subprocess.CompletedProcess(argv, 0, "cargo 1.99.0\n", "")

    monkeypatch.setattr(command_identity, "_run_captured", version)
    plan = proof_plan.ProofPlan.load()
    telemetry = []
    first = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=telemetry,
    )
    second = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=telemetry,
    )
    assert first == second and first["path"] == str(expected)
    assert [row["state"] for row in telemetry] == ["miss", "hit"]
    assert versions == [[str(expected), "--version"]]
    # The Rust link capture must use that same executable for metadata/build
    # probes, even though CARGO and PATH conflict with an explicit payload.
    probes = []

    def capture(**kwargs):
        probes.append(kwargs)
        return [], {}

    monkeypatch.setattr(toolchain_capture, "capture_rust_link_process_images", capture)
    policy = next(row for row in plan.toolchain_policies if row.name == "rustc")
    command_identity._capture_tool_identity(
        policy,
        "rustc",
        envelope,
        exact,
        path=rustc,
        selected_content_path=rustc,
        probe_cwd=proof_plan.ROOT,
        policy_sha256=command_identity.canonical_json_sha256(policy.data),
        cwd=proof_plan.ROOT,
        env=env,
    )
    assert len(probes) == 1
    assert probes[0]["cargo"] == expected
    assert probes[0]["target"] == "wasm32-wasip1"
    assert probes[0]["command_argv"][0] == str(expected)
    # Content mutation defeats warm reuse rather than preserving a stale image.
    expected.write_bytes(expected.read_bytes() + b" changed")
    changed = []
    third = command_identity._tool_identity(
        plan,
        "cargo",
        envelope,
        exact,
        cwd=proof_plan.ROOT,
        env=env,
        reuse_root=tmp_path / "reuse",
        reuse_telemetry=changed,
    )
    assert changed[0]["state"] == "miss"
    assert third["executable_sha256"] != first["executable_sha256"]


def test_python_declaring_cargo_uses_path_independently_of_explicit_hook(
    tmp_path, monkeypatch
):
    selected = tmp_path / ("cargo.exe" if os.name == "nt" else "cargo")
    selected.write_bytes(b"declared dependency Cargo")
    selected.chmod(0o755)
    decoy = tmp_path / "explicit-cargo"
    decoy.write_bytes(b"explicit Cargo hook")
    decoy.chmod(0o755)
    command = [sys.executable, "-c", "pass"]
    envelope = command_admission.envelope_for_command(command)
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["path"])},
    )
    identity = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "cargo",
        envelope,
        command,
        cwd=tmp_path,
        env={"CARGO": str(decoy), "PATH": str(selected.parent)},
    )
    assert identity == {"path": str(selected)}


@pytest.mark.parametrize("primary", [False, True])
def test_cargo_rustc_environment_selector_precedence(tmp_path, monkeypatch, primary):
    suffix = ".exe" if os.name == "nt" else ""
    lower, higher = (
        tmp_path / ("cargo-rustc" + suffix),
        tmp_path / ("primary-rustc" + suffix),
    )
    for path in (lower, higher):
        path.write_bytes(b"selected physical compiler")
        path.chmod(0o755)
    env = {"CARGO_BUILD_RUSTC": str(lower)}
    if primary:
        env["RUSTC"] = str(higher)
    monkeypatch.setattr(
        command_identity,
        "_which_in_command_environment",
        lambda *args, **kwargs: pytest.fail("explicit Rust selection used PATH"),
    )
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"content_path": str(kwargs["selected_content_path"])},
    )
    command = ["cargo", "build"]
    result = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "rustc",
        command_admission.envelope_for_command(command),
        command,
        cwd=tmp_path,
        env=env,
    )
    assert result["content_path"] == str(higher if primary else lower)


def test_generator_rustfmt_capture_uses_path_independently_of_cargo_hook(
    tmp_path, monkeypatch
):
    selected = tmp_path / "path" / ("rustfmt.exe" if os.name == "nt" else "rustfmt")
    selected.parent.mkdir()
    decoy = tmp_path / "cargo-formatter"
    decoy.write_bytes(b"Cargo-only formatter")
    decoy.chmod(0o755)
    selected.write_bytes(b"selected formatter")
    selected.chmod(0o755)
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["selected_content_path"])},
    )
    command = [sys.executable, "-c", "pass"]
    value = command_identity._tool_identity(
        proof_plan.ProofPlan.load(),
        "rustfmt",
        command_admission.envelope_for_command(command),
        command,
        cwd=tmp_path,
        env={"RUSTFMT": str(decoy), "PATH": str(selected.parent)},
    )
    assert value == {"path": str(selected)}


@pytest.mark.parametrize("role", ["cargo", "rustc", "rustfmt", "git", "node"])
@pytest.mark.parametrize("delegated", [False, True])
def test_registered_tool_identity_uses_bound_payload_before_dependency_selectors(
    tmp_path, monkeypatch, role, delegated
):
    selected, decoy = tmp_path / "selected", tmp_path / "decoy"
    for directory in (selected, decoy):
        directory.mkdir()
        image = directory / (role + (".exe" if os.name == "nt" else ""))
        image.write_bytes(directory.name.encode())
        image.chmod(0o755)
    name = role + (".exe" if os.name == "nt" else "")
    executable = selected / name
    payload = [str(executable), "--version"]
    command = (
        [sys.executable, "tools/guarded_exec.py", "--", *payload]
        if delegated
        else payload
    )
    env = {"PATH": str(decoy), role.upper(): str(decoy / name)}
    envelope = command_admission.envelope_for_command(command)
    exact = command_identity._exact_command(envelope, cwd=proof_plan.ROOT, env=env)
    command_identity._bind_delegated_command(
        envelope, exact, cwd=proof_plan.ROOT, env=env
    )
    monkeypatch.setattr(
        command_identity,
        "_capture_tool_identity",
        lambda *args, **kwargs: {"path": str(kwargs["path"])},
    )
    assert command_identity._tool_identity(
        proof_plan.ProofPlan.load(), role, envelope, exact, cwd=proof_plan.ROOT, env=env
    ) == {"path": str(executable)}


@pytest.mark.parametrize("owner", ["python", "cargo", "delegated-cargo"])
def test_python_rustc_metadata_dependency_has_independent_reuse_identity(
    tmp_path, monkeypatch, owner
):
    from molt import rust_toolchain
    from tools.proof_queue_pkg import toolchain_capture

    suffix = ".exe" if os.name == "nt" else ""
    path_bin = tmp_path / "path"
    path_bin.mkdir()
    proxy, rustup, cargo = (
        path_bin / (role + suffix) for role in ("rustc", "rustup", "cargo")
    )
    primary, first, second = (
        tmp_path / (name + suffix)
        for name in ("configured-compiler", "path-component-one", "path-component-two")
    )
    for path in (proxy, rustup, cargo, primary, first, second):
        path.write_bytes(b"proxy" if path in (proxy, rustup) else path.name.encode())
        path.chmod(0o755)
    selected = [first]

    def resolve(command, **kwargs):
        assert command == [str(rustup), "which", "rustc"]
        return subprocess.CompletedProcess(command, 0, str(selected[0]) + "\n", "")

    versions = []

    def version(command, **kwargs):
        assert command[0] == str(primary)
        versions.append(command)
        return subprocess.CompletedProcess(command, 0, "rustc 1.99.0", "")

    monkeypatch.setattr(rust_toolchain.process_guard, "run_completed_command", resolve)
    monkeypatch.setattr(command_identity, "_run_captured", version)
    monkeypatch.setattr(
        toolchain_capture, "capture_rust_link_process_images", lambda **kwargs: ([], {})
    )
    command = (
        [sys.executable, "-c", "pass"] if owner == "python" else ["cargo", "build"]
    )
    if owner == "delegated-cargo":
        command = [sys.executable, "tools/guarded_exec.py", "--", *command]
    envelope = command_admission.envelope_for_command(command)
    environment = {"RUSTC": str(primary), "PATH": str(path_bin)}

    def capture():
        telemetry = []
        identity = command_identity._tool_identity(
            proof_plan.ProofPlan.load(),
            "rustc",
            envelope,
            command,
            cwd=tmp_path,
            env=environment,
            reuse_root=tmp_path / "reuse",
            reuse_telemetry=telemetry,
        )
        assert identity["path"] == identity["content_path"] == str(primary)
        assert identity["version"] == "rustc 1.99.0"
        return identity, telemetry[0]

    initial, initial_event = capture()
    warm, warm_event = capture()
    assert warm == initial and warm_event["state"] == "hit"
    assert len(versions) == 1
    dependency_paths = {
        row["path"]
        for row in initial["process_images"]
        if row["role"] == "rustc-path-metadata"
    }
    assert dependency_paths == (
        {str(proxy), str(first)} if owner == "python" else set()
    )
    selected[0] = second
    changed, event = capture()
    assert event["state"] == ("miss" if owner == "python" else "hit")
    assert (event["key_sha256"] != initial_event["key_sha256"]) is (owner == "python")
    assert changed["executable_sha256"] == initial["executable_sha256"]
    if owner == "python":
        assert {
            row["path"]
            for row in changed["process_images"]
            if row["role"] == "rustc-path-metadata"
        } == {str(proxy), str(second)}
