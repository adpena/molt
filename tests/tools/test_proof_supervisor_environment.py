"""Native launch requirements stay singular and enter every environment receipt."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from tests import proof_queue_owned_roots
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    execution_environment,
    guarded_execution,
    supervisor_custody,
    supervisor_generation,
)
from tests.proof_queue_custody_test_support import (
    assert_supervisor_refusal,
    capability_aware_proof_execution,
)


def test_leaf_requires_pre_entry_process_creation_authority() -> None:
    capability = _capability({})
    capability["mode"] = "leaf"
    capability["pre_entry_process_create_authority"] = False
    with pytest.raises(ValueError, match="lacks process custody"):
        supervisor_custody.decode_supervisor_capability(capability, mode="leaf")


def test_typed_event_artifact_binds_image_size_and_content(tmp_path: Path) -> None:
    receipt = tmp_path / "receipt.json"
    image_path = tmp_path / "observed-image"
    image_digest = "a" * 64
    payload = {
        "sequence": 1,
        "process_id": 7,
        "stable_process_id": "test:7",
        "event": {
            "kind": "exec",
            "image": {
                "path": str(image_path),
                "sha256": image_digest,
                "size_bytes": 11,
            },
        },
    }
    data = json.dumps(payload).encode() + b"\n"
    digest = hashlib.sha256(data).hexdigest()
    event_path = receipt.with_name(f"{receipt.name}.events.{digest}.jsonl")
    event_path.write_bytes(data)
    descriptor = {
        "schema": supervisor_custody.SUPERVISOR_EVENT_LOG_SCHEMA,
        "file": event_path.name,
        "bytes": len(data),
        "count": 1,
        "sha256": digest,
    }
    path, images = supervisor_custody._verified_supervisor_event_artifact(
        receipt_path=receipt, descriptor=descriptor, collect_images=True
    )
    assert path == event_path.resolve()
    assert images == [(str(image_path), image_digest, 11)]
    event_path.write_bytes(data.replace(b'"size_bytes": 11', b'"size_bytes": 12'))
    with pytest.raises(ValueError, match="identity changed"):
        supervisor_custody._verified_supervisor_event_artifact(
            receipt_path=receipt, descriptor=descriptor, collect_images=True
        )


def test_source_authority_includes_shared_protocol() -> None:
    root = Path(__file__).resolve().parents[2]
    assert (root / "tools/proof_supervisor/protocol.json").resolve() in (
        supervisor_custody.source_authority_paths(root)
    )


def _capability(required: object) -> dict[str, object]:
    return {
        "schema": supervisor_custody.SUPERVISOR_CAPABILITY_SCHEMA,
        "platform": {"win32": "windows", "darwin": "macos"}.get(
            sys.platform, sys.platform
        ),
        "mode": "declared-tree",
        "backend": "test-native-backend",
        "available": True,
        "pre_entry_exec_authority": True,
        "pre_entry_process_create_authority": True,
        "recursive_descendant_authority": True,
        "reason": None,
        "required_environment": required,
    }


def _read_capability(
    monkeypatch: pytest.MonkeyPatch, payload: object
) -> dict[str, str]:
    def capture(
        command: tuple[str, ...], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        assert command == ("supervisor.bin", "capability", "declared-tree")
        assert kwargs["timeout"] == 30.0
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    monkeypatch.setattr(supervisor_custody.command_identity, "_run_captured", capture)
    return supervisor_custody.required_execution_environment(
        binary=Path("supervisor.bin"), mode="declared-tree", cwd=Path.cwd(), env={}
    )


@pytest.mark.parametrize("mode", ["leaf", "declared-tree"])
def test_unavailable_capability_refuses_before_execution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cargo_output_implementation_source: Path,
    mode: str,
) -> None:
    """Inject the unavailable report, not a successful execution or receipt."""
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path, source=Path(__file__), nodeid=f"unavailable-capability::{mode}"
    )
    repo = tmp_path / "repo"
    repo.mkdir()
    marker = tmp_path / "must-not-start"
    command = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
    if mode == "declared-tree":
        command = ["git", "status"]
    output_root = tmp_path / "o"
    output_root.mkdir()
    metadata = tmp_path / "metadata"
    metadata.mkdir()
    envelope = command_admission.envelope_for_command(
        command, cargo_output_root=str(output_root)
    )
    assert (envelope["process_closure"]["descendants"] == "forbidden") == (
        mode == "leaf"
    )
    result_path = metadata / "refused.execution.json"
    request_path = tmp_path / "request.json"
    request_path.write_text(
        json.dumps(
            {
                "schema": command_admission.EXECUTION_SCHEMA,
                "run_id": "unavailable-capability",
                "execution_nonce": "a" * 64,
                "env_override_names": [],
                "command": command,
                "envelope": envelope,
                "cwd": str(repo),
                "resource_family": "python-tests",
                "result_path": str(result_path),
                "timeout_seconds": 30.0,
            }
        ),
        encoding="utf-8",
    )
    capability = {
        **_capability({}),
        "platform": "macos",
        "mode": mode,
        "backend": "macos-endpoint-security",
        "available": False,
        "pre_entry_exec_authority": False,
        "pre_entry_process_create_authority": False,
        "recursive_descendant_authority": False,
        "reason": (
            "Endpoint Security entitlement and privileged helper "
            "are not available in this binary"
        ),
    }

    def capability_probe(command: tuple[str, ...], **kwargs: object):
        assert command[1:] == ("capability", mode)
        return subprocess.CompletedProcess(command, 0, json.dumps(capability), "")

    def provision(*, cwd: Path, env: dict[str, str]):
        supervisor_custody.required_execution_environment(
            binary=Path("supervisor.bin"), mode=mode, cwd=cwd, env=env
        )
        pytest.fail("an unavailable capability must never finish provisioning")

    def forbidden(*args: object, **kwargs: object):
        pytest.fail("an unavailable capability must never start the command")

    monkeypatch.setattr(command_identity, "_run_captured", capability_probe)
    monkeypatch.setattr(supervisor_custody, "sys", SimpleNamespace(platform="darwin"))
    monkeypatch.setattr(supervisor_generation, "provision", provision)
    monkeypatch.setattr(
        guarded_execution, "_run_supervisor_with_transcripts", forbidden
    )
    returncode = guarded_execution.execute_guarded_request(request_path)
    assert assert_supervisor_refusal(returncode, result_path)
    assert not marker.exists()
    assert (
        json.loads(result_path.read_text(encoding="utf-8"))["supervisor_capability"]
        == capability
    )


@pytest.mark.parametrize("required", [{}, {"_NO_DEBUG_HEAP": "1"}])
def test_native_capability_owns_platform_launch_environment(
    monkeypatch: pytest.MonkeyPatch, required: dict[str, str]
) -> None:
    assert _read_capability(monkeypatch, _capability(required)) == required


@pytest.mark.parametrize(
    "mutation", ["started", "reason", "context", "stdout", "policy", "available"]
)
def test_refusal_assertions_reject_partial_or_dishonest_execution(
    tmp_path: Path, mutation: str
) -> None:
    command = [sys.executable, "-c", "pass"]
    capability = {
        **_capability({}),
        "mode": "leaf",
        "available": False,
        "pre_entry_exec_authority": False,
        "pre_entry_process_create_authority": False,
        "recursive_descendant_authority": False,
        "reason": "kernel process closure unavailable",
    }
    record = {
        "envelope": command_admission.envelope_for_command(command),
        "phase": "failed",
        "command_started": False,
        "supervisor_capability": capability,
        "error": str(supervisor_custody.SupervisorCapabilityUnavailable.__name__)
        + ": native supervisor launch capability unavailable: "
        + capability["reason"],
    }
    result_path = tmp_path / "execution.json"
    if mutation == "started":
        record["command_started"] = True
    elif mutation == "reason":
        record["error"] = "unrelated launch failure"
    elif mutation == "context":
        record["receipt_context"] = {"source_custody": {"evidence_eligible": True}}
    elif mutation == "stdout":
        result_path.with_suffix(".stdout.bin").write_bytes(b"partial execution")
    elif mutation == "policy":
        result_path.with_suffix(".supervisor-policy.json").write_text(
            "{}", encoding="utf-8"
        )
    elif mutation == "available":
        capability["available"] = True
    result_path.write_text(json.dumps(record), encoding="utf-8")
    with pytest.raises((AssertionError, ValueError)):
        assert_supervisor_refusal(2, result_path)


def test_capability_adapter_does_not_hide_available_host_assertions() -> None:
    @capability_aware_proof_execution
    def failing_execution_assertion() -> None:
        assert False, "available-host assertion remains load-bearing"

    with pytest.raises(AssertionError, match="available-host assertion"):
        failing_execution_assertion(proof_queue_execution_capability=None)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("schema", "molt.proof-supervisor-capability.v1"),
        ("platform", "foreign-platform"),
        ("mode", "leaf"),
        ("backend", ""),
        ("available", 1),
        ("available", False),
        ("pre_entry_exec_authority", False),
        ("recursive_descendant_authority", False),
        ("reason", []),
        ("unknown", "field"),
        ("required_environment", None),
        ("required_environment", []),
        ("required_environment", {"_NO_DEBUG_HEAP": True}),
        ("required_environment", {"_NO_DEBUG_HEAP": ""}),
        ("required_environment", {"_NO_DEBUG_HEAP": "1\n"}),
        ("required_environment", {"_NO_DEBUG_HEAP": "1\0"}),
        ("required_environment", {"_no_debug_heap": "1"}),
        ("required_environment", {"NOT=AN_ENV_NAME": "1"}),
    ],
)
def test_native_launch_capability_rejects_untyped_or_foreign_data(
    monkeypatch: pytest.MonkeyPatch, field: str, value: object
) -> None:
    payload = _capability({})
    payload[field] = value
    with pytest.raises(ValueError, match="native supervisor"):
        _read_capability(monkeypatch, payload)


def test_required_launch_environment_is_captured_and_cannot_be_overridden() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    selected, contract = execution_environment._deterministic_execution_environment(
        {"PATH": "host-tools", "UNCLASSIFIED": "discard"},
        override_names=[],
        required_environment=required,
    )
    assert selected == {"PATH": "host-tools", **required}
    assert contract["passed_names"] == ["_NO_DEBUG_HEAP", "PATH"]
    assert contract["supervisor_owned_names"] == ["_NO_DEBUG_HEAP"]
    assert contract["omitted_names"] == ["UNCLASSIFIED"]
    authority = execution_environment._execution_environment_authority(
        selected,
        applied_cargo_policies=(),
        fingerprint_key=b"x" * 32,
        contract=contract,
    )
    assert authority["canonical_values_sha256"] == (
        execution_environment._canonical_environment_sha256(selected)
    )
    assert (
        authority["variables"]["_NO_DEBUG_HEAP"]["class"] == "supervisor-owned-launch"
    )
    assert authority["variables"]["_NO_DEBUG_HEAP"]["redacted"] is True
    for name in ("_NO_DEBUG_HEAP", "_no_debug_heap"):
        with pytest.raises(ValueError, match="cannot be overridden"):
            execution_environment._deterministic_execution_environment(
                {name: "1"}, override_names=[name], required_environment=required
            )


def test_inventory_and_execution_share_canonical_binding() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    assert supervisor_custody.bind_required_environment(
        {"_no_debug_heap": "1", "PATH": "tools"}, required
    ) == {"PATH": "tools", **required}
    with pytest.raises(ValueError, match="conflicts"):
        supervisor_custody.bind_required_environment({"_no_debug_heap": "0"}, required)
    with pytest.raises(ValueError, match="case-ambiguous"):
        supervisor_custody.bind_required_environment(
            {"_NO_DEBUG_HEAP": "1", "_no_debug_heap": "1"}, required
        )


def test_no_platform_requirement_does_not_admit_windows_environment() -> None:
    selected, contract = execution_environment._deterministic_execution_environment(
        {"_NO_DEBUG_HEAP": "1", "PATH": "tools"},
        override_names=[],
        required_environment={},
    )
    assert selected == {"PATH": "tools"}
    assert contract["omitted_names"] == ["_NO_DEBUG_HEAP"]
    assert "supervisor_owned_names" not in contract


def test_native_requirement_is_cross_bound_to_receipt_policy_and_ownership() -> None:
    required = {"_NO_DEBUG_HEAP": "1"}
    supervisor_custody.validate_required_environment_binding(
        required_environment=required,
        supervisor_metadata=required,
        policy_environment={"PATH": "tools", **required},
        supervisor_owned_names=["_NO_DEBUG_HEAP"],
    )
    substitutions = (
        (
            None,
            {"PATH": "tools", **required},
            ["_NO_DEBUG_HEAP"],
            "required-environment metadata mismatch",
        ),
        (
            required,
            {"PATH": "tools", "_NO_DEBUG_HEAP": "0"},
            ["_NO_DEBUG_HEAP"],
            "required environment differs from policy",
        ),
        (
            required,
            {"PATH": "tools", **required},
            [],
            "environment ownership metadata mismatch",
        ),
    )
    for metadata, policy, owned_names, error in substitutions:
        with pytest.raises(ValueError, match=error):
            supervisor_custody.validate_required_environment_binding(
                required_environment=required,
                supervisor_metadata=metadata,
                policy_environment=policy,
                supervisor_owned_names=owned_names,
            )
