"""Native launch requirements stay singular and enter every environment receipt."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from tests import proof_queue_custody_test_support, proof_queue_owned_roots
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
        supervisor_custody.decode_supervisor_capability(
            capability,
            mode="leaf",
            context="prelaunch",
            expected_platform={"win32": "windows", "darwin": "macos"}.get(
                sys.platform, sys.platform
            ),
        )


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
        "admission": {"state": "eligible"},
        "pre_entry_exec_authority": True,
        "pre_entry_process_create_authority": True,
        "recursive_descendant_authority": True,
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


_REAL_REFUSALS = {
    # These are source-authored planner refusals, not simulated kernel admission.
    "linux-leaf": {
        "mode": "leaf",
        "sys_platform": "linux",
        "platform": "linux",
        "backend": "ptrace-exitkill",
        "pre_entry_exec_authority": True,
        "pre_entry_process_create_authority": True,
        "recursive_descendant_authority": True,
        "admission": {
            "state": "ineligible",
            "reason": "Yama ptrace_scope=3 forbids ptrace",
        },
    },
    "macos-leaf": {
        "mode": "leaf",
        "sys_platform": "darwin",
        "platform": "macos",
        "backend": "seatbelt+ptrace",
        "pre_entry_exec_authority": False,
        "pre_entry_process_create_authority": False,
        "recursive_descendant_authority": False,
        "admission": {
            "state": "ineligible",
            "reason": "macOS leaf closure has no valid pre-entry image authority: blocked SIGTRAP can bypass the former Seatbelt/ptrace reexec observation",
        },
    },
    "macos-tree": {
        "mode": "declared-tree",
        "sys_platform": "darwin",
        "platform": "macos",
        "backend": "seatbelt+ptrace",
        "pre_entry_exec_authority": False,
        "pre_entry_process_create_authority": False,
        "recursive_descendant_authority": False,
        "admission": {
            "state": "ineligible",
            "reason": "macOS tree closure has no retained pre-entry descendant creation authority in this binary; the former Seatbelt/ptrace leaf backend also cannot enforce pre-entry image custody",
        },
    },
}


@pytest.mark.parametrize("case", sorted(_REAL_REFUSALS))
def test_ineligible_plan_refuses_before_execution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    cargo_output_implementation_source: Path,
    case: str,
) -> None:
    """Inject a real backend plan, never an execution or receipt."""
    refusal = dict(_REAL_REFUSALS[case])
    mode = str(refusal["mode"])
    tmp_path = proof_queue_owned_roots.native_case_path(
        tmp_path, source=Path(__file__), nodeid=f"ineligible-plan::{mode}"
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
                "run_id": "ineligible-plan",
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
    sys_platform = str(refusal.pop("sys_platform"))
    capability = {**_capability({}), **refusal}

    def capability_probe(command: tuple[str, ...], **kwargs: object):
        assert command[1:] == ("capability", mode)
        return subprocess.CompletedProcess(command, 0, json.dumps(capability), "")

    def provision(*, cwd: Path, env: dict[str, str]):
        supervisor_custody.required_execution_environment(
            binary=Path("supervisor.bin"), mode=mode, cwd=cwd, env=env
        )
        pytest.fail("an ineligible plan must never finish provisioning")

    def forbidden(*args: object, **kwargs: object):
        pytest.fail("an ineligible plan must never start the command")

    monkeypatch.setattr(command_identity, "_run_captured", capability_probe)
    monkeypatch.setattr(
        supervisor_custody, "sys", SimpleNamespace(platform=sys_platform)
    )
    monkeypatch.setattr(
        proof_queue_custody_test_support, "sys", SimpleNamespace(platform=sys_platform)
    )
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
    "mutation", ["started", "reason", "context", "stdout", "policy", "eligibility"]
)
def test_refusal_assertions_reject_partial_or_dishonest_execution(
    tmp_path: Path, mutation: str
) -> None:
    command = [sys.executable, "-c", "pass"]
    capability = {
        **_capability({}),
        "mode": "leaf",
        "admission": {
            "state": "ineligible",
            "reason": "kernel process closure unavailable",
        },
        "pre_entry_exec_authority": False,
        "pre_entry_process_create_authority": False,
        "recursive_descendant_authority": False,
    }
    record = {
        "envelope": command_admission.envelope_for_command(command),
        "phase": "failed",
        "command_started": False,
        "supervisor_capability": capability,
        "error": str(supervisor_custody.SupervisorPrelaunchRefused.__name__)
        + ": native supervisor prelaunch refused: "
        + capability["admission"]["reason"],
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
    elif mutation == "eligibility":
        capability["admission"] = {"state": "eligible"}
    result_path.write_text(json.dumps(record), encoding="utf-8")
    with pytest.raises((AssertionError, ValueError)):
        assert_supervisor_refusal(2, result_path)


def test_capability_adapter_does_not_hide_eligible_host_assertions() -> None:
    @capability_aware_proof_execution
    def failing_execution_assertion() -> None:
        assert False, "eligible-host assertion remains load-bearing"

    with pytest.raises(AssertionError, match="eligible-host assertion"):
        failing_execution_assertion(proof_queue_execution_capability=None)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("schema", "molt.proof-supervisor-capability.v3"),
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


@pytest.mark.parametrize(
    "admission",
    [
        None,
        {"state": "available"},
        {"state": "eligible", "reason": "mixed state"},
        {"state": "ineligible"},
        {"state": "ineligible", "reason": "   "},
        {"state": "ineligible", "reason": "x" * 1537},
        {
            "state": "admitted",
            "root_stable_process_id": "root",
            "root_create_sequence": True,
            "initial_image_sequence": 2,
        },
        {
            "state": "admitted",
            "root_stable_process_id": "root",
            "root_create_sequence": 3,
            "initial_image_sequence": 2,
        },
        {
            "state": "admitted",
            "root_stable_process_id": "root",
            "root_create_sequence": 0,
            "initial_image_sequence": 2,
        },
        {
            "state": "admitted",
            "root_stable_process_id": "",
            "root_create_sequence": 1,
            "initial_image_sequence": 2,
        },
        {
            "state": "admitted",
            "root_stable_process_id": "root",
            "root_create_sequence": 1,
            "initial_image_sequence": 2**64,
        },
    ],
)
def test_terminal_admission_rejects_malformed_or_mixed_states(
    admission: object,
) -> None:
    capability = {**_capability({}), "admission": admission}
    with pytest.raises(ValueError, match="native supervisor admission"):
        supervisor_custody.decode_supervisor_capability(
            capability,
            mode="declared-tree",
            context="verified_terminal",
            expected_platform={"win32": "windows", "darwin": "macos"}.get(
                sys.platform, sys.platform
            ),
        )


@pytest.mark.parametrize(
    ("admission", "terminal", "complete"),
    [
        (
            {"state": "ineligible", "reason": "native launch forbidden"},
            "REJECTED",
            False,
        ),
        ({"state": "eligible"}, "INCOMPLETE", False),
        (
            {
                "state": "admitted",
                "root_stable_process_id": "fixture:root",
                "root_create_sequence": 1,
                "initial_image_sequence": 2,
            },
            "INCOMPLETE",
            False,
        ),
        (
            {
                "state": "admitted",
                "root_stable_process_id": "fixture:root",
                "root_create_sequence": 1,
                "initial_image_sequence": 2,
            },
            "COMPLETE",
            True,
        ),
    ],
)
def test_verified_terminal_integrity_does_not_imply_complete_execution(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    admission: dict[str, object],
    terminal: str,
    complete: bool,
) -> None:
    capability = {**_capability({}), "admission": admission}
    receipt = {
        "schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA,
        "capability": capability,
        "state": terminal,
        "complete": complete,
    }
    path = tmp_path / "receipt.json"
    path.write_text(json.dumps(receipt), encoding="utf-8")
    policy = tmp_path / "policy.json"
    policy.write_bytes(b' {"fixture":"policy bytes"}\n')

    # Only native verification is faked: its successful integrity result must
    # not turn an honest incomplete/refusal receipt into execution success.
    def verify(command, **kwargs):
        assert command == (
            "supervisor.bin",
            "verify",
            "--policy",
            str(policy),
            "--receipt",
            str(path),
        )
        assert kwargs["text"] is False
        raw = path.read_bytes()
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "receipt_sha256": hashlib.sha256(raw).hexdigest(),
                    "receipt_bytes": len(raw),
                    "policy_input_sha256": hashlib.sha256(
                        policy.read_bytes()
                    ).hexdigest(),
                    "policy_input_bytes": policy.stat().st_size,
                    "native_custody_valid": True,
                    "journal_coverage_valid": True,
                }
            ).encode("utf-8"),
            b"",
        )

    monkeypatch.setattr(command_identity, "_run_captured", verify)
    validated = supervisor_custody._validated_supervisor_receipt(
        binary=Path("supervisor.bin"),
        policy_path=policy,
        receipt_path=path,
        cwd=tmp_path,
        env={},
    )
    assert validated == receipt
    assert supervisor_custody.supervisor_receipt_is_complete(validated) is complete
    if admission["state"] == "admitted":
        with pytest.raises(ValueError, match="context"):
            supervisor_custody.decode_supervisor_capability(
                capability,
                mode="declared-tree",
                context="prelaunch",
                expected_platform={"win32": "windows", "darwin": "macos"}.get(
                    sys.platform, sys.platform
                ),
            )


def test_complete_flag_cannot_promote_an_eligible_plan() -> None:
    receipt = {"capability": _capability({}), "state": "COMPLETE", "complete": True}
    assert not supervisor_custody.supervisor_receipt_is_complete(receipt)


@pytest.mark.parametrize("replace_before_verify", [False, True])
def test_verified_receipt_retains_exact_bytes_across_path_replacement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    replace_before_verify: bool,
) -> None:
    original = {
        "schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA,
        "capability": _capability({}),
        "state": "INCOMPLETE",
        "complete": False,
        "error": "independent failure: \u00e9",
    }
    replacement = {
        **original,
        "capability": {
            **original["capability"],
            "admission": {
                "state": "admitted",
                "root_stable_process_id": "forged:root",
                "root_create_sequence": 1,
                "initial_image_sequence": 2,
            },
        },
        "state": "COMPLETE",
        "complete": True,
    }
    path = tmp_path / "receipt.json"
    # Noncanonical legal whitespace and UTF-8 distinguish raw bytes from a
    # JSON reserialization; the retained receipt is an honest failed run.
    raw = ("  \n" + json.dumps(original, ensure_ascii=False, indent=3) + "\n").encode()
    path.write_bytes(raw)
    substituted = json.dumps(replacement).encode()
    observed: list[bytes] = []

    policy = tmp_path / "policy.json"
    policy.write_bytes(b' {"fixture":"policy bytes"}\n')

    def verify(command, **kwargs):
        assert kwargs["text"] is False
        assert command[-2:] == ("--receipt", str(path))
        if replace_before_verify:
            path.write_bytes(substituted)
        verified_bytes = path.read_bytes()
        observed.append(verified_bytes)
        if not replace_before_verify:
            path.write_bytes(substituted)
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {
                    "receipt_sha256": hashlib.sha256(verified_bytes).hexdigest(),
                    "receipt_bytes": len(verified_bytes),
                    "policy_input_sha256": hashlib.sha256(
                        policy.read_bytes()
                    ).hexdigest(),
                    "policy_input_bytes": policy.stat().st_size,
                    "native_custody_valid": True,
                    "journal_coverage_valid": True,
                }
            ).encode("utf-8"),
            b"",
        )

    monkeypatch.setattr(command_identity, "_run_captured", verify)
    kwargs = dict(
        binary=tmp_path / "never-executed",
        policy_path=tmp_path / "policy.json",
        receipt_path=path,
        cwd=tmp_path,
        env={},
    )
    if replace_before_verify:
        with pytest.raises(ValueError, match="verified different receipt bytes"):
            supervisor_custody._validated_supervisor_receipt(**kwargs)
        assert observed == [substituted]
    else:
        retained = supervisor_custody._validated_supervisor_receipt(**kwargs)
        assert observed == [raw]
        assert retained == original
        assert not supervisor_custody.supervisor_receipt_is_complete(retained)
    assert json.loads(path.read_bytes()) == replacement


@pytest.mark.parametrize(
    "response",
    [
        None,
        b"{}",
        "{}",
        "[]",
        '{"receipt_sha256":"0","receipt_bytes":1}',
        '{"receipt_sha256":"' + "0" * 64 + '","receipt_bytes":true}',
        '{"receipt_sha256":"' + "0" * 64 + '","receipt_bytes":-1}',
        '{"receipt_sha256":"' + "0" * 64 + '","receipt_bytes":1.0}',
        '{"receipt_sha256":"' + "0" * 64 + '","receipt_bytes":1,"receipt_bytes":2}',
        '{"receipt_sha256":"' + "0" * 64 + '","receipt_bytes":NaN}',
    ],
)
def test_native_verification_requires_one_typed_raw_byte_identity(
    tmp_path: Path, response: object
) -> None:
    from molt.exact_json import capture_exact

    path = tmp_path / "receipt.json"
    path.write_bytes(b' {"fixture": true}\n')
    identity, _ = capture_exact(path, max_bytes=64 * 1024, label="test receipt")
    with pytest.raises(ValueError, match="verification"):
        supervisor_custody.validate_supervisor_receipt_verification(
            response, receipt_identity=identity, policy_identity=identity
        )


@pytest.mark.parametrize("field", ["receipt_sha256", "receipt_bytes"])
def test_native_verification_cannot_substitute_content_or_byte_count(
    tmp_path: Path, field: str
) -> None:
    from molt.exact_json import capture_exact

    path = tmp_path / "receipt.json"
    raw = b' {"fixture": "raw whitespace"}\n'
    path.write_bytes(raw)
    identity, _ = capture_exact(path, max_bytes=64 * 1024, label="test receipt")
    response = {
        "receipt_sha256": hashlib.sha256(raw).hexdigest(),
        "receipt_bytes": len(raw),
        "policy_input_sha256": hashlib.sha256(raw).hexdigest(),
        "policy_input_bytes": len(raw),
        "native_custody_valid": True,
        "journal_coverage_valid": True,
    }
    assert (
        supervisor_custody.validate_supervisor_receipt_verification(
            json.dumps(response), receipt_identity=identity, policy_identity=identity
        )
        == response
    )
    response[field] = "0" * 64 if field == "receipt_sha256" else len(raw) + 1
    with pytest.raises(ValueError, match="verified different receipt bytes"):
        supervisor_custody.validate_supervisor_receipt_verification(
            json.dumps(response), receipt_identity=identity, policy_identity=identity
        )


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


@pytest.mark.parametrize(
    "field,value",
    [
        ("policy_input_sha256", "0" * 64),
        ("policy_input_bytes", True),
        ("policy_input_bytes", 1),
        ("policy_input_sha256", None),
        ("native_custody_valid", False),
        ("journal_coverage_valid", False),
    ],
)
def test_native_verification_binds_policy_bytes_and_typed_custody(
    tmp_path, field, value
):
    from molt.exact_json import capture_exact

    policy = tmp_path / "policy.json"
    receipt = tmp_path / "receipt.json"
    policy.write_bytes(b'  {"authority":"policy"}\n')
    receipt.write_bytes(b'{"authority":"receipt"}\n')
    policy_identity, _ = capture_exact(policy, max_bytes=1024, label="policy fixture")
    receipt_identity, _ = capture_exact(
        receipt, max_bytes=1024, label="receipt fixture"
    )
    response = {
        "receipt_sha256": receipt_identity.sha256,
        "receipt_bytes": receipt_identity.size,
        "policy_input_sha256": policy_identity.sha256,
        "policy_input_bytes": policy_identity.size,
        "native_custody_valid": True,
        "journal_coverage_valid": True,
    }
    supervisor_custody.validate_supervisor_receipt_verification(
        json.dumps(response),
        receipt_identity=receipt_identity,
        policy_identity=policy_identity,
    )
    response[field] = value
    with pytest.raises(ValueError):
        supervisor_custody.validate_supervisor_receipt_verification(
            json.dumps(response),
            receipt_identity=receipt_identity,
            policy_identity=policy_identity,
        )


def test_policy_publication_counts_real_utf8_escapes_and_refuses_before_encoding(
    tmp_path, monkeypatch
):
    from molt.exact_json import encode_exact

    payload = {
        "schema": supervisor_custody.SUPERVISOR_POLICY_SCHEMA,
        "nonce": 'é\n\\"',
        "mode": "leaf",
        "cwd": str(tmp_path),
        "command": ["command", "𐐀\u0000"],
        "environment": {"K": "é\t"},
        "root_role": "root",
        "fixed_images": [],
        "derived_roots": [],
    }
    target = tmp_path / "policy.json"
    supervisor_custody.publish_supervisor_policy(target, payload)
    assert target.read_bytes() == encode_exact(payload, indent=None)
    prior = target.read_bytes()
    payload["nonce"] = "x" * (
        supervisor_custody.SUPERVISOR_BUDGETS["nonce_utf8_bytes"] + 1
    )

    def forbidden_encode(*args, **kwargs):
        raise AssertionError("oversize policy reached whole-document encoding")

    monkeypatch.setattr(supervisor_custody, "encode_exact", forbidden_encode)
    with pytest.raises(ValueError, match="nonce_utf8_bytes"):
        supervisor_custody.publish_supervisor_policy(target, payload)
    assert target.read_bytes() == prior
