"""Explicit synthetic receipt inputs; native execution is supplied by integration tests.

The synthetic fixture replaces only runner._COMMANDS.run for exact issued native
verify requests. Python validators, CAS bytes, policy binding, stream hashing and
custody digests remain real. These inputs are never product/native proof receipts.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping
from functools import partial, wraps
import inspect
import hashlib
import json
from pathlib import Path
import subprocess
import sqlite3
import sys
from types import SimpleNamespace
from typing import Protocol

import pytest

from molt.exact_json import canonical_json_sha256
from molt.python_environment_location import (
    PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
    validate_python_environment_location,
)
from tests.python_environment_test_support import (
    realized_environment_manifest,
    runtime_identity_manifest,
)
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    custody_cas,
    cargo_output_layout,
    cli,
    guarded_execution,
    supervisor_generation,
    state,
    execution_custody,
    execution_environment,
    execution_receipt_details,
    process_image_capture,
    runner,
    supervisor_custody,
    toolchain_capture,
)


class _VerifiedSupervisorRefusal(Exception):
    """The unavailable branch has proved its complete prelaunch contract."""


def assert_supervisor_refusal(
    returncode: int, result_path: Path, *, queue_terminal: bool = False
) -> bool:
    """Recognize only a typed native refusal, never unrelated launch failures."""
    record = json.loads(result_path.read_text(encoding="utf-8"))
    capability = record.get("supervisor_capability")
    if capability is None:
        return False
    envelope = record["envelope"]
    mode = (
        "leaf"
        if envelope["process_closure"]["descendants"] == "forbidden"
        else "declared-tree"
    )
    with pytest.raises(supervisor_custody.SupervisorCapabilityUnavailable) as refusal:
        supervisor_custody.decode_supervisor_capability(capability, mode=mode)
    assert returncode == 2
    assert record["phase"] == "failed"
    assert record["command_started"] is False
    assert record["error"] == f"SupervisorCapabilityUnavailable: {refusal.value}"
    assert capability["available"] is False
    assert isinstance(capability["reason"], str) and capability["reason"]
    if queue_terminal:
        context = record["receipt_context"]
        assert context["schema"] == state.UNATTESTED_RECEIPT_CONTEXT_SCHEMA
        assert context["status"] == "non-evidence"
        assert context["queue_terminal"]["status"] == "failed"
        assert context["queue_terminal"]["command_returncode"] is None
        assert context["queue_terminal"]["execution_error"] == record["error"]
        assert "terminal_evidence_sha256" not in context
        assert "execution_custody_sha256" not in context
        assert "process_supervisor" not in context
        assert "command_transcript" not in context
    else:
        assert "receipt_context" not in record
    assert "command_returncode" not in record
    assert "live_command_transcript" not in record
    assert "cargo_cache_publication" not in record
    for path in (
        *command_identity.execution_transcript_paths(result_path).values(),
        result_path.with_suffix(".supervisor-policy.json"),
        result_path.with_suffix(".supervisor-receipt.json"),
    ):
        assert not path.exists(), path
    assert not list(result_path.parent.glob("*.events.*.jsonl"))
    return True


@pytest.fixture
def proof_queue_execution_capability(monkeypatch: pytest.MonkeyPatch) -> None:
    """Keep real execution; adapt only the test's post-call expectation.

    Refusal replaces success-only assertions with the shared refusal contract.
    Queue calls finish terminal publication before inspection. No host-name
    selection or successful synthetic execution is involved.
    """
    execute = guarded_execution.execute_guarded_request
    queue_main = cli.main
    queue_depth = 0

    def execute_request(request_path: Path) -> int:
        returncode = execute(request_path)
        if queue_depth == 0:
            request = json.loads(request_path.read_text(encoding="utf-8"))
            if assert_supervisor_refusal(returncode, Path(request["result_path"])):
                raise _VerifiedSupervisorRefusal
        return returncode

    def queue_call(argv: list[str]) -> int:
        nonlocal queue_depth
        queue_depth += 1
        try:
            returncode = queue_main(argv)
        finally:
            queue_depth -= 1
        if "--db" not in argv:
            return returncode
        database = Path(argv[argv.index("--db") + 1])
        if not database.exists():
            return returncode
        with sqlite3.connect(database) as connection:
            connection.row_factory = sqlite3.Row
            rows = list(connection.execute("SELECT * FROM proof_runs"))
        refused = False
        for row in rows:
            _, result_path = command_identity.execution_record_paths(
                Path(row["log_path"])
            )
            if not result_path.exists():
                continue
            if assert_supervisor_refusal(
                row["returncode"], result_path, queue_terminal=True
            ):
                assert returncode == 2
                assert row["status"] == "failed"
                context = json.loads(row["receipt_context_json"])
                record = json.loads(result_path.read_text(encoding="utf-8"))
                assert context == record["receipt_context"]
                refused = True
        if refused:
            assert all(row["status"] != "passed" for row in rows)
            raise _VerifiedSupervisorRefusal
        return returncode

    monkeypatch.setattr(guarded_execution, "execute_guarded_request", execute_request)
    monkeypatch.setattr(cli, "main", queue_call)


def capability_aware_proof_execution(test: Callable) -> Callable:
    """Preserve the original body on available hosts; prove refusals otherwise."""

    @wraps(test)
    def run_test(*args: object, **kwargs: object) -> None:
        kwargs.pop("proof_queue_execution_capability")
        try:
            test(*args, **kwargs)
        except _VerifiedSupervisorRefusal:
            return

    signature = inspect.signature(test)
    run_test.__signature__ = signature.replace(
        parameters=[
            *signature.parameters.values(),
            inspect.Parameter(
                "proof_queue_execution_capability", inspect.Parameter.KEYWORD_ONLY
            ),
        ]
    )
    return run_test


class ReceiptCustodyFactory(Protocol):
    def __call__(
        self,
        directory: Path,
        toolchains: dict[str, object],
        *,
        nonce: str = "a" * 64,
        descendants: str = "forbidden",
        environment: dict[str, str] | None = None,
    ) -> tuple[dict[str, object], dict[str, object], dict[str, object]]: ...


def synthetic_python_toolchain(directory: Path) -> dict[str, object]:
    """Use validated environment/location shapes and real owned fixture bytes."""
    prefix = directory / "synthetic-python"
    prefix.mkdir(exist_ok=True)
    executable = prefix / "python-fixture.bin"
    executable.write_bytes(b"synthetic interpreter image; never executed\n")
    executable = executable.resolve(strict=True)
    environment = realized_environment_manifest(runtime_identity_manifest(), [])
    location_material = {
        "schema": PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "prefix": str(prefix.resolve()),
        "selected_executable": str(executable),
        "base_executable": str(executable),
        "roots": [str(prefix.resolve())],
        "external_roots": [],
        "file_paths": [str(executable)],
    }
    location = validate_python_environment_location(
        {
            **location_material,
            "identity_sha256": canonical_json_sha256(location_material),
        }
    )
    image = process_image_capture.capture_image("python", executable)
    material = {
        "schema": "molt.proof-python-toolchain.v3",
        "identity_kind": "synthetic-test-input",
        "location": location,
        "environment": environment,
        "file_custody": [
            {
                "path": str(executable),
                "size": executable.stat().st_size,
                "sha256": image["sha256"],
            }
        ],
        "process_images": [image],
        "inventory_profile": {"total_s": 0.0},
    }
    return {**material, "identity_sha256": canonical_json_sha256(material)}


@pytest.fixture
def synthetic_receipt_custody(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> ReceiptCustodyFactory:
    """No launch/build: fake only exact native verification of issued fixtures."""
    binary = tmp_path / "synthetic-supervisor.bin"
    binary.write_bytes(b"synthetic supervisor image; never executed\n")
    required_environment = {"MOLT_TEST_SUPERVISOR_REQUIRED": "1"}
    issued: dict[str, tuple[tuple[str, ...], bytes, bytes]] = {}

    def issue(command: list[str]) -> None:
        assert command[1:3] == ["run", "--policy"] and command[4] == "--receipt"
        policy_path, receipt_path = Path(command[3]), Path(command[5])
        policy = json.loads(policy_path.read_text(encoding="utf-8"))
        events = b'{"kind":"synthetic-test-event"}\n'
        event_digest = hashlib.sha256(events).hexdigest()
        event_path = receipt_path.with_name(
            f"{receipt_path.name}.events.{event_digest}.jsonl"
        )
        event_path.write_bytes(events)
        receipt = {
            "schema": supervisor_custody.SUPERVISOR_RECEIPT_SCHEMA,
            "complete": True,
            "state": "COMPLETE",
            "root_exit_code": 0,
            "nonce_sha256": hashlib.sha256(policy["nonce"].encode()).hexdigest(),
            "event_log": {
                "schema": supervisor_custody.SUPERVISOR_EVENT_LOG_SCHEMA,
                "file": event_path.name,
                "count": 1,
                "bytes": len(events),
                "sha256": hashlib.sha256(events).hexdigest(),
            },
        }
        supervisor_custody._atomic_json(receipt_path, receipt)
        verify = (command[0], "verify", *command[2:])
        issued[str(receipt_path)] = (
            verify,
            policy_path.read_bytes(),
            receipt_path.read_bytes(),
        )

    def verify(command, *, check, capture_output, text):
        assert check is False and capture_output is True and text is True
        assert len(command) == 6 and command[1:3] == ["verify", "--policy"]
        assert command[4] == "--receipt"
        expected = issued.get(command[5])
        assert expected is not None, "unissued synthetic native verification request"
        exact, policy_bytes, receipt_bytes = expected
        assert tuple(command) == exact
        unchanged = (Path(command[3]).read_bytes(), Path(command[5]).read_bytes()) == (
            policy_bytes,
            receipt_bytes,
        )
        policy = json.loads(policy_bytes)
        capability = {
            "schema": supervisor_custody.SUPERVISOR_CAPABILITY_SCHEMA,
            "platform": {"win32": "windows", "darwin": "macos"}.get(
                sys.platform, sys.platform
            ),
            "mode": policy["mode"],
            "backend": "synthetic-test-backend",
            "available": True,
            "pre_entry_exec_authority": True,
            "pre_entry_process_create_authority": True,
            "recursive_descendant_authority": True,
            "reason": None,
            "required_environment": required_environment,
        }
        return subprocess.CompletedProcess(
            command,
            0 if unchanged else 1,
            json.dumps({"capability": capability}),
            "",
        )

    monkeypatch.setattr(runner, "_COMMANDS", SimpleNamespace(run=verify))
    return partial(
        publish_receipt_custody,
        supervisor_binary=binary,
        execute_supervisor=issue,
        required_environment=required_environment,
    )


def publish_receipt_custody(
    directory: Path,
    toolchains: dict[str, object],
    *,
    supervisor_binary: Path,
    execute_supervisor: Callable[[list[str]], None],
    required_environment: Mapping[str, str] | None = None,
    nonce: str = "a" * 64,
    descendants: str = "forbidden",
    environment: dict[str, str] | None = None,
) -> tuple[dict[str, object], dict[str, object], dict[str, object]]:
    summaries, artifact, telemetry = toolchain_capture.publish_capture(
        directory / "custody-cas", toolchains
    )
    verification = toolchain_capture.verify_capture(
        artifact, workers=1, cas_root=directory / "custody-cas"
    )
    binary_artifact = custody_cas.put_file(
        directory / "custody-cas",
        supervisor_binary,
        logical_name=supervisor_binary.name,
        executable=True,
    ).as_dict()
    binary = Path(str(binary_artifact["path"])).resolve(strict=True)
    command = [str(binary), "capability", "leaf"]
    root_role, fixed_images = supervisor_custody._supervisor_fixed_images(
        toolchains,
        {},
        command,
    )
    policy = {
        "schema": "molt.proof-process-closure.v2",
        "nonce": nonce,
        "mode": "leaf" if descendants == "forbidden" else "declared-tree",
        "cwd": str(directory.resolve()),
        "command": command,
        "environment": supervisor_custody.bind_required_environment(
            environment or {}, required_environment or {}
        ),
        "root_role": root_role,
        "fixed_images": fixed_images,
        "derived_roots": [],
    }
    policy_path = directory / "synthetic-supervisor-policy.json"
    receipt_path = directory / "synthetic-supervisor-receipt.json"
    supervisor_custody._atomic_json(policy_path, policy)
    execute_supervisor(
        [
            str(binary),
            "run",
            "--policy",
            str(policy_path),
            "--receipt",
            str(receipt_path),
        ]
    )
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    event_artifact = supervisor_custody._publish_supervisor_event_artifact(
        receipt_path=receipt_path,
        receipt=receipt,
        cas_root=directory / "custody-cas",
    )
    capture = {
        "schema": "molt.proof-toolchain-custody.v1",
        "artifact": artifact,
        "verification": verification,
        "telemetry": {"capture": telemetry},
    }
    toolchain_custody = {
        "capture_semantic_sha256": artifact["semantic_sha256"],
        "verification_identity_sha256": verification["identity_sha256"],
        "identical": True,
    }
    provision_target = cargo_output_layout.CargoOutputLayout.create(
        result_root=directory
    ).supervisor_target
    generation_inputs = {"profile": "release", "synthetic_fixture": True}
    generation = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": supervisor_generation.GENERATION_SCHEMA,
        "inputs": generation_inputs,
        "input_sha256": canonical_json_sha256(generation_inputs),
        "build_target_dir": str(provision_target),
        "binary": {
            "sha256": binary_artifact["sha256"],
            "size_bytes": binary_artifact["size_bytes"],
            "name": binary.name,
        },
        "freshness_authority": "cargo-build-locked",
    }
    supervisor = {
        "schema": "molt.proof-process-supervision.v1",
        "binary": command_identity._file_identity(binary),
        "binary_artifact": binary_artifact,
        "policy": command_identity._file_identity(policy_path),
        "receipt": receipt,
        "receipt_file": command_identity._file_identity(receipt_path),
        "event_artifact": event_artifact,
        "supervisor_returncode": 0,
        "required_environment": dict(required_environment or {}),
        "provision_telemetry": {
            "schema": supervisor_generation.PROVISION_SCHEMA,
            "build_target_dir": str(provision_target),
            "build_output_sha256": binary_artifact["sha256"],
            "build_output_size_bytes": binary_artifact["size_bytes"],
            "generation_artifact": custody_cas.put_json(
                directory / "custody-cas", generation
            ).as_dict(),
        },
    }
    return summaries, toolchain_custody, {"capture": capture, "supervisor": supervisor}


def synthetic_live_custody(directory: Path) -> dict[str, object]:
    raw = {
        "schema": execution_custody.LIVE_CUSTODY_RECEIPT_SCHEMA,
        "watch_roots": 0,
        "events": [],
        "apparatus_events": [],
        "errors": [],
        "state": "DRAINED",
        "lifecycle": ["CREATED", "ARMED", "DRAINING", "DRAINED"],
        "stable": True,
    }
    raw["identity_sha256"] = execution_custody.live_custody_identity_sha256(
        events=[],
        apparatus_events=[],
        errors=[],
        state=raw["state"],
        lifecycle=["CREATED", "ARMED", "DRAINING", "DRAINED"],
    )
    return supervisor_custody._publish_live_custody_receipt(
        raw, cas_root=directory / "custody-cas"
    )


def assert_execution_context_rejects_substitutions(
    tmp_path: Path,
    custody_factory: ReceiptCustodyFactory,
) -> None:
    execution_path = tmp_path / "run.execution.json"
    stdout_path = execution_path.with_suffix(".stdout.bin")
    stderr_path = execution_path.with_suffix(".stderr.bin")
    stdout_path.write_bytes(b"1 passed in 0.01s\n")
    stderr_path.write_bytes(b"")
    transcript = {
        "stdout": command_identity._transcript_identity(stdout_path),
        "stderr": command_identity._transcript_identity(stderr_path),
    }
    transcript["identity_sha256"] = hashlib.sha256(
        json.dumps(transcript, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    envelope = command_admission.envelope_for_command([sys.executable, "-c", "pass"])
    synthetic_toolchains, compact_custody, v3 = custody_factory(
        tmp_path,
        {"python": synthetic_python_toolchain(tmp_path)},
        environment={"MOLT_TEST_VALUE": "alpha"},
    )
    supervisor_policy_path = Path(str(v3["supervisor"]["policy"]["path"]))
    supervisor_policy = json.loads(supervisor_policy_path.read_text(encoding="utf-8"))
    required_environment = v3["supervisor"]["required_environment"]
    assert isinstance(required_environment, dict)
    context: dict[str, object] = {
        "run_id": "run-one",
        "execution_nonce_sha256": hashlib.sha256(("a" * 64).encode()).hexdigest(),
        "command_envelope": envelope,
        "command_transcript": transcript,
        "toolchains": synthetic_toolchains,
        "toolchain_custody": compact_custody,
        "toolchain_capture": v3["capture"],
        "live_input_custody": synthetic_live_custody(tmp_path),
        "child_process_custody": {
            "policy": {"descendants": "forbidden", "derived_roots": []},
            "receipt": {
                "broker_complete": True,
                "events": [],
                "errors": [],
                "violations": [],
            },
        },
        "process_supervisor": v3["supervisor"],
        "exact_command_sha256": hashlib.sha256(
            json.dumps(supervisor_policy["command"], separators=(",", ":")).encode()
        ).hexdigest(),
        "execution_environment": {
            "prelaunch": {
                "passed_names": sorted(
                    supervisor_policy["environment"], key=str.casefold
                ),
                **(
                    {
                        "supervisor_owned_names": sorted(
                            required_environment or {}, key=str.casefold
                        )
                    }
                    if required_environment
                    else {}
                ),
                "identity_sha256": "e" * 64,
                "canonical_values_sha256": (
                    execution_environment._canonical_environment_sha256(
                        supervisor_policy["environment"]
                    )
                ),
            },
            "postcompletion_identity_sha256": "e" * 64,
            "identical": True,
            "executable_inputs": {
                "prelaunch": {},
                "postcompletion_sha256": supervisor_custody._canonical_payload_sha256(
                    {}
                ),
                "identical": True,
            },
        },
        "custody_authorities": {
            "prelaunch": [],
            "postcompletion_sha256": supervisor_custody._canonical_payload_sha256([]),
            "identical": True,
        },
        "derived_root_custody": {"prelaunch": [], "policy_roots": []},
        "source_custody": {"row_cwd": supervisor_policy["cwd"]},
    }
    context = execution_receipt_details.compact_context(
        context, cas_root=tmp_path / "custody-cas"
    )
    context["execution_custody_sha256"] = supervisor_custody.execution_custody_sha256(
        context, run_id="run-one", returncode=0
    )
    runner._validated_execution_context(
        context,
        execution_path=execution_path,
        envelope=envelope,
        run_id="run-one",
        execution_nonce="a" * 64,
        returncode=0,
    )
    supervisor_record = v3["supervisor"]
    original_required_environment = supervisor_record["required_environment"]
    supervisor_record["required_environment"] = {
        "MOLT_SUBSTITUTED_SUPERVISOR_REQUIRED": "1"
    }
    context["execution_custody_sha256"] = supervisor_custody.execution_custody_sha256(
        context, run_id="run-one", returncode=0
    )
    with pytest.raises(ValueError, match="required-environment metadata mismatch"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    supervisor_record["required_environment"] = original_required_environment
    environment_prelaunch = context["execution_environment"]["prelaunch"]
    assert isinstance(environment_prelaunch, dict)
    original_supervisor_owned_names = environment_prelaunch.get(
        "supervisor_owned_names"
    )
    environment_prelaunch["supervisor_owned_names"] = [
        "MOLT_SUBSTITUTED_SUPERVISOR_REQUIRED"
    ]
    context["execution_custody_sha256"] = supervisor_custody.execution_custody_sha256(
        context, run_id="run-one", returncode=0
    )
    with pytest.raises(ValueError, match="environment ownership metadata mismatch"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    if original_supervisor_owned_names is None:
        environment_prelaunch.pop("supervisor_owned_names")
    else:
        environment_prelaunch["supervisor_owned_names"] = (
            original_supervisor_owned_names
        )
    context["execution_custody_sha256"] = supervisor_custody.execution_custody_sha256(
        context, run_id="run-one", returncode=0
    )
    receipt_path = Path(str(supervisor_record["receipt_file"]["path"]))
    original_receipt_bytes = receipt_path.read_bytes()
    original_policy_bytes = supervisor_policy_path.read_bytes()
    receipt = supervisor_record["receipt"]
    original_exit = receipt["root_exit_code"]
    receipt["root_exit_code"] = 73
    supervisor_custody._atomic_json(receipt_path, receipt)
    supervisor_record["receipt_file"] = command_identity._file_identity(receipt_path)
    with pytest.raises(ValueError, match="receipt failed independent verification"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    receipt["root_exit_code"] = original_exit
    # Native publication and Python JSON formatting need not use identical
    # bytes. Restore the issued artifacts, not equivalent reserializations;
    # later substitutions must start from the same content-bound authority.
    custody_cas.atomic_write_bytes(receipt_path, original_receipt_bytes)
    supervisor_record["receipt_file"] = command_identity._file_identity(receipt_path)
    supervisor_policy["environment"]["MOLT_TEST_VALUE"] = "substituted"
    supervisor_custody._atomic_json(supervisor_policy_path, supervisor_policy)
    v3["supervisor"]["policy"] = command_identity._file_identity(supervisor_policy_path)
    with pytest.raises(ValueError, match="policy binding is invalid"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    supervisor_policy["environment"]["MOLT_TEST_VALUE"] = "alpha"
    custody_cas.atomic_write_bytes(supervisor_policy_path, original_policy_bytes)
    v3["supervisor"]["policy"] = command_identity._file_identity(supervisor_policy_path)
    event_artifact = v3["supervisor"]["event_artifact"]
    event_artifact["count"] = int(event_artifact["count"]) + 1
    with pytest.raises(ValueError, match="event artifact binding is invalid"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    event_artifact["count"] = int(event_artifact["count"]) - 1
    stdout_path.write_bytes(b"substituted\n")
    with pytest.raises(ValueError, match="transcript content substitution"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
    stdout_path.write_bytes(b"1 passed in 0.01s\n")
    context["execution_custody_sha256"] = "0" * 64
    with pytest.raises(ValueError, match="custody digest mismatch"):
        runner._validated_execution_context(
            context,
            execution_path=execution_path,
            envelope=envelope,
            run_id="run-one",
            execution_nonce="a" * 64,
            returncode=0,
        )
