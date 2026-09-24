"""Queue submission and memory-guarded proof execution lifecycle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import sqlite3
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Mapping, TextIO, TypeGuard

from molt.dx import bind_repo_src_pythonpath, development_artifact_env
from molt import disk_capacity
from molt.exact_json import ExactJsonError, loads_exact, read_exact, write_exact
from tools.command_execution import CommandExecutor
from tools.memory_guard_core import repro_context as guard_repro_context
from tools.memory_guard_core.process_custody import GuardInfrastructureFailure
from tools.memory_guard import INFRASTRUCTURE_RETURN_CODE
from molt.memory_guard_paths import pytest_guard_summary_dir
from tools.proof_queue_pkg import (
    command_admission,
    command_identity,
    cargo_cache_custody,
    cargo_output_environment,
    cargo_output_lifecycle,
    cargo_output_layout,
    execution_custody,
    execution_environment as environment_authority,
    execution_receipt_details,
    supervisor_custody,
    custody,
    custody_cas,
    evidence,
    policy,
    process_image_capture,
    scheduling,
    state,
    toolchain_capture,
)
from tools.proof_queue_pkg import diagnostic_engine, diagnostic_model


_COMMANDS = CommandExecutor.for_file(__file__)


def _is_receipt_object(value: object) -> TypeGuard[dict[str, object]]:
    return isinstance(value, dict) and all(isinstance(key, str) for key in value)


def _is_receipt_object_list(value: object) -> TypeGuard[list[dict[str, object]]]:
    return isinstance(value, list) and all(_is_receipt_object(item) for item in value)


def _is_string_list(value: object) -> TypeGuard[list[str]]:
    return isinstance(value, list) and all(isinstance(item, str) for item in value)


def _file_receipt_identity(path: Path) -> dict[str, object]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            size += len(chunk)
            digest.update(chunk)
    return {"path": str(path), "size_bytes": size, "sha256": digest.hexdigest()}


def _validated_guard_outcome(
    summary_json: Path,
    *,
    guarded_command: list[str],
    returncode: int,
    child_returncode: int,
) -> tuple[dict[str, object], GuardInfrastructureFailure | None]:
    payload = loads_exact(summary_json.read_text(encoding="utf-8"))
    if not _is_receipt_object(payload):
        raise ValueError("memory-guard receipt is not an object")
    if payload.get("command") != guarded_command:
        raise ValueError("memory-guard receipt command substitution detected")
    if (
        type(payload.get("returncode")) is not int
        or payload["returncode"] != returncode
    ):
        raise ValueError("memory-guard receipt return-code substitution detected")
    if (
        type(payload.get("child_returncode")) is not int
        or payload["child_returncode"] != child_returncode
    ):
        raise ValueError("memory-guard receipt child return-code substitution detected")
    failure = GuardInfrastructureFailure.from_payload(
        payload.get("infrastructure_failure")
    )
    if failure is None and returncode != child_returncode:
        raise ValueError("guard/command return-code custody mismatch")
    return payload, failure


def _validated_guard_receipt(
    summary_json: Path,
    *,
    guarded_command: list[str],
    returncode: int,
    child_returncode: int,
    run_id: str,
    execution_nonce: str,
    guard_pid: int,
) -> dict[str, object]:
    payload, failure = _validated_guard_outcome(
        summary_json,
        guarded_command=guarded_command,
        returncode=returncode,
        child_returncode=child_returncode,
    )
    expected_returncode = (
        INFRASTRUCTURE_RETURN_CODE
        if failure is not None and child_returncode == 0
        else child_returncode
    )
    if returncode != expected_returncode:
        raise ValueError("guard/command return-code custody mismatch")
    incident = payload.get("incident")
    infrastructure_incident = (
        failure is not None
        and _is_receipt_object(incident)
        and incident.get("reason") == "infrastructure_error"
        and incident.get("child_returncode") == child_returncode
        and incident.get("infrastructure_failure") == failure.json_payload()
    )
    dirty_terminal_fields = {
        "violation": payload.get("violation"),
        "timed_out": payload.get("timed_out"),
        "exit_signal": payload.get("exit_signal"),
        "guard_signal": payload.get("guard_signal"),
        "incident": payload.get("incident"),
        "orphaned_process_groups": payload.get("orphaned_process_groups"),
    }
    if (
        dirty_terminal_fields["violation"] is not None
        or dirty_terminal_fields["timed_out"] is not False
        or dirty_terminal_fields["exit_signal"] is not None
        or dirty_terminal_fields["guard_signal"] is not None
        or (
            dirty_terminal_fields["incident"] is not None
            and not infrastructure_incident
        )
        or (failure is not None and not infrastructure_incident)
        or dirty_terminal_fields["orphaned_process_groups"] not in ([], ())
    ):
        raise ValueError(
            "memory-guard receipt does not prove a clean terminal outcome: "
            + json.dumps(dirty_terminal_fields, sort_keys=True)
        )
    windows_cleanup = payload.get("windows_job_cleanup")
    if os.name == "nt" and not _is_receipt_object(windows_cleanup):
        raise ValueError("memory-guard Windows job cleanup is missing")
    if _is_receipt_object(windows_cleanup):
        after_cleanup = windows_cleanup.get("after")
        if (
            windows_cleanup.get("completed") is not True
            or not _is_receipt_object(after_cleanup)
            or after_cleanup.get("active_processes") != 0
            or windows_cleanup.get("terminated_remaining_processes") is not False
            or windows_cleanup.get("remaining_processes") != []
        ):
            raise ValueError("memory-guard Windows job cleanup is incomplete")
    sampling = payload.get("sampling_telemetry")
    if (
        not _is_receipt_object(sampling)
        or sampling.get("enforcement_complete") is not True
        or not isinstance(sampling.get("attempts"), int)
        or sampling.get("attempts") != sampling.get("successes")
        or sampling.get("transient_failures") != 0
    ):
        raise ValueError("memory-guard sampling enforcement is incomplete")
    temporary_artifacts = payload.get("temporary_artifacts")
    closure = (
        temporary_artifacts.get("closure")
        if _is_receipt_object(temporary_artifacts)
        else None
    )
    if (
        not _is_receipt_object(closure)
        or closure.get("schema") != "molt.guard-scratch-closure.v1"
        or closure.get("closed") is not True
        or closure.get("direct_child_reaped") is not True
    ):
        raise ValueError("memory-guard descendant closure is incomplete")
    receipt = _file_receipt_identity(summary_json)
    receipt["child_returncode"] = child_returncode
    receipt["infrastructure_failure"] = (
        None if failure is None else failure.json_payload()
    )
    receipt["identity_sha256"] = hashlib.sha256(
        json.dumps(
            {
                **receipt,
                "run_id": run_id,
                "execution_nonce": execution_nonce,
                "guard_pid": guard_pid,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    return receipt


def _record_cargo_generation_terminal(
    *,
    execution_record: Mapping[str, object],
    execution_path: Path,
    run_id: str,
    execution_nonce: str,
    receipt_context: Mapping[str, object] | None,
    process_cleanup_safe: bool,
) -> dict[str, object] | None:
    """Project parent-validated terminal custody into the generation owner."""
    provenance = execution_record.get("cargo_cache")
    if not _is_receipt_object(provenance):
        return None  # No generation was acquired, including prelaunch refusal.
    nonce_hash = hashlib.sha256(execution_nonce.encode()).hexdigest()
    if (
        provenance.get("generation_run_id") != run_id
        or provenance.get("execution_nonce_sha256") != nonce_hash
    ):
        raise ValueError("Cargo generation owner differs from execution identity")
    context = dict(receipt_context or {})
    envelope = context.get("command_envelope")
    lifetime = (
        command_admission.validated_cargo_output_lifetime(envelope)
        if isinstance(envelope, Mapping)
        else "retain"
    )
    if provenance.get("cargo_output_lifetime", "retain") != lifetime:
        raise ValueError("Cargo generation output lifetime differs from parent receipt")
    output_root = (
        envelope.get("cargo_output_root") if isinstance(envelope, Mapping) else None
    )
    if not cargo_output_layout.same_root(
        provenance.get("cargo_output_root"), output_root
    ):
        raise ValueError("Cargo generation output root differs from parent receipt")
    cargo_output_layout.validate_root(output_root)
    process_supervisor = None
    if process_cleanup_safe:
        derived = context.get("derived_root_custody")
        prelaunch = derived.get("prelaunch") if _is_receipt_object(derived) else None
        if not isinstance(prelaunch, list) or provenance not in prelaunch:
            raise ValueError(
                "Cargo generation differs from validated derived output custody"
            )
        process_supervisor = context.get("process_supervisor")
    terminal = {
        "schema": cargo_cache_custody.TERMINAL_RECEIPT_SCHEMA,
        "cargo_output_lifetime": lifetime,
        **({"cargo_output_root": output_root} if output_root is not None else {}),
        "run_id": run_id,
        "execution_nonce_sha256": nonce_hash,
        "input_sha256": provenance.get("input_sha256"),
        "generation_id": provenance.get("generation_id"),
        "target": provenance.get("path"),
        "command_returncode": execution_record.get("command_returncode"),
        "queue_terminal": context.get("queue_terminal"),
        "command_started": execution_record.get("command_started"),
        "cargo_cache_publication": execution_record.get("cargo_cache_publication"),
        "execution_custody_sha256": context.get("execution_custody_sha256"),
        "guard_receipt": context.get("guard_receipt"),
        "process_supervisor": process_supervisor,
        "process_cleanup_safe": process_cleanup_safe,
        "execution_error": execution_record.get("error"),
    }
    return cargo_cache_custody.record_terminal_receipt(
        result_root=execution_path.parent,
        provenance=provenance,
        terminal_receipt=terminal,
    )


def _finalize_execution_receipt(
    *,
    execution_record: dict[str, object] | None,
    execution_path: Path,
    run_id: str,
    execution_nonce: str,
    receipt_context: dict[str, object] | None,
    process_cleanup_safe: bool,
    status: str,
    returncode: int | None,
    execution_error: str | None,
    guard_infrastructure_failure: GuardInfrastructureFailure | None = None,
) -> tuple[supervisor_custody.QueueTerminalOutcome, dict[str, object]]:
    """Choose one final queue result before binding any terminal projection.

    Command execution and proof eligibility are separate facts. The child owns
    command_returncode; this parent owns queue_terminal. A cleanly terminated
    command can be ineligible proof while its unsealed outputs remain safely
    reclaimable. Publication failures propagate without retrying or resealing a
    generation under a different result.
    """
    context = dict(
        receipt_context
        or state._unattested_receipt_context(
            status="non-evidence",
            phase="guarded command envelope",
            reason=execution_error or "guarded execution produced no receipt context",
        )
    )
    source = context.get("source_custody")
    if guard_infrastructure_failure is not None:
        status = (
            "non-evidence"
            if execution_record is not None
            and execution_record.get("command_returncode") == 0
            else "failed"
        )
        returncode = 2
        infrastructure_error = "memory guard infrastructure error: " + "; ".join(
            guard_infrastructure_failure.details
        )
        execution_error = (
            f"{execution_error}; {infrastructure_error}"
            if execution_error
            else infrastructure_error
        )
    if (
        process_cleanup_safe
        and status == "passed"
        and not (_is_receipt_object(source) and source.get("evidence_eligible") is True)
    ):
        status, returncode = "non-evidence", 2
        execution_error = execution_error or (
            "source custody is not evidence eligible between prelaunch "
            "and postcompletion"
        )
    command_rc = (
        execution_record.get("command_returncode") if execution_record else None
    )
    if command_rc is not None and type(command_rc) is not int:
        raise ValueError("terminal execution has an invalid command return code")
    outcome: supervisor_custody.QueueTerminalOutcome = {
        "schema": supervisor_custody.QUEUE_TERMINAL_SCHEMA,
        "status": status,
        "returncode": returncode,
        "command_returncode": command_rc,
        "execution_error": execution_error,
    }
    supervisor_custody.validate_queue_terminal(outcome)
    context["queue_terminal"] = outcome
    # Neither a child-supplied digest nor a stale parent projection is authority.
    context.pop("terminal_evidence_sha256", None)
    context.pop("cargo_generation_lifecycle", None)
    if execution_record is not None:
        generation = _record_cargo_generation_terminal(
            execution_record=execution_record,
            execution_path=execution_path,
            run_id=run_id,
            execution_nonce=execution_nonce,
            receipt_context=context,
            process_cleanup_safe=process_cleanup_safe,
        )
        if generation is not None:
            context["cargo_generation_lifecycle"] = generation
    if process_cleanup_safe:
        if type(returncode) is not int:
            raise ValueError("validated execution has no final queue return code")
        context["terminal_evidence_sha256"] = (
            supervisor_custody.terminal_evidence_sha256(
                context, run_id=run_id, returncode=returncode
            )
        )
    if execution_record is not None:
        # Preserve the child's command result; persist the same final context
        # that the caller will commit to the queue database.
        execution_record["receipt_context"] = context
        supervisor_custody._atomic_json(execution_path, execution_record)
    return outcome, context


def _validated_execution_context(
    context: dict[str, object],
    *,
    execution_path: Path,
    envelope: dict[str, object],
    run_id: str,
    execution_nonce: str,
    returncode: int,
) -> None:
    if context.get("run_id") != run_id:
        raise ValueError("guarded receipt context run identity mismatch")
    expected_nonce_hash = hashlib.sha256(execution_nonce.encode()).hexdigest()
    if context.get("execution_nonce_sha256") != expected_nonce_hash:
        raise ValueError("guarded receipt context nonce substitution detected")
    if context.get("command_envelope") != envelope:
        raise ValueError("guarded receipt context command substitution detected")
    requested_toolchains = envelope.get("toolchains")
    captured_toolchains = context.get("toolchains")
    custody = context.get("toolchain_custody")
    if (
        not isinstance(requested_toolchains, list)
        or not requested_toolchains
        or not all(isinstance(name, str) and name for name in requested_toolchains)
    ):
        raise ValueError("guarded command envelope has no toolchain authority")
    if (
        not _is_receipt_object(captured_toolchains)
        or set(captured_toolchains) != set(requested_toolchains)
        or any(
            not _is_receipt_object(identity)
            or not isinstance(identity.get("identity_sha256"), str)
            for identity in captured_toolchains.values()
        )
    ):
        raise ValueError("guarded receipt toolchain closure is incomplete")
    wire_context = context
    receipt_size = len(
        json.dumps(wire_context, sort_keys=True, separators=(",", ":")).encode()
    )
    if receipt_size > execution_receipt_details.CONTEXT_LIMIT_BYTES:
        raise ValueError(
            "guarded receipt context exceeds the 64 KiB compactness ceiling"
        )
    context = execution_receipt_details.expand_context(
        wire_context, cas_root=execution_path.parent / "custody-cas"
    )
    capture = context.get("toolchain_capture")
    artifact = capture.get("artifact") if _is_receipt_object(capture) else None
    verification = capture.get("verification") if _is_receipt_object(capture) else None
    telemetry = capture.get("telemetry") if _is_receipt_object(capture) else None
    capture_telemetry = (
        telemetry.get("capture") if _is_receipt_object(telemetry) else None
    )
    if (
        not _is_receipt_object(capture)
        or capture.get("schema") != "molt.proof-toolchain-custody.v1"
        or not _is_receipt_object(artifact)
        or not _is_receipt_object(verification)
        or verification.get("schema") != "molt.proof-toolchain-verification.v1"
        or verification.get("stable") is not True
        or verification.get("capture_semantic_sha256")
        != artifact.get("semantic_sha256")
        or not _is_receipt_object(capture_telemetry)
        or capture_telemetry.get("full_capture_count") != 1
    ):
        raise ValueError("guarded receipt has no compact single-capture authority")
    if (
        not _is_receipt_object(custody)
        or custody.get("identical") is not True
        or custody.get("capture_semantic_sha256") != artifact.get("semantic_sha256")
        or custody.get("verification_identity_sha256")
        != verification.get("identity_sha256")
    ):
        raise ValueError("guarded receipt toolchain closure is not stable")
    cas_root = execution_path.parent / "custody-cas"
    custody_cas.verify_ref(artifact, expected_root=cas_root)
    capture_payload = toolchain_capture.load_capture(artifact, cas_root=cas_root)
    full_toolchains = capture_payload.get("toolchains")
    if (
        not _is_receipt_object(full_toolchains)
        or toolchain_capture.compact_toolchains(full_toolchains) != captured_toolchains
    ):
        raise ValueError(
            "guarded receipt compact toolchains disagree with full capture"
        )
    reverified_capture = toolchain_capture.verify_capture(
        artifact, workers=1, cas_root=cas_root
    )
    verification_authority_fields = {
        "schema",
        "capture_semantic_sha256",
        "verified_file_count",
        "bytes_hashed",
        "mismatches",
        "stable",
        "identity_sha256",
    }
    if any(
        reverified_capture.get(field) != verification.get(field)
        for field in verification_authority_fields
    ):
        raise ValueError("guarded receipt toolchain verification is not reproducible")
    live_custody = context.get("live_input_custody")
    if (
        not _is_receipt_object(live_custody)
        or live_custody.get("schema") != execution_custody.LIVE_CUSTODY_RECEIPT_SCHEMA
        or live_custody.get("stable") is not True
    ):
        raise ValueError("guarded receipt has no stable live input custody")
    live_event_artifact = live_custody.get("event_artifact")
    if not _is_receipt_object(live_event_artifact):
        raise ValueError("guarded receipt has no durable live-custody event authority")
    live_event_payload = custody_cas.read_ref(
        live_event_artifact, expected_root=cas_root
    )
    live_events = live_event_payload.get("events")
    live_apparatus_events = live_event_payload.get("apparatus_events")
    live_errors = live_event_payload.get("errors")
    live_lifecycle = live_custody.get("lifecycle")
    if (
        live_event_payload.get("kind") != "live-input-custody-events"
        or not isinstance(live_events, list)
        or not isinstance(live_apparatus_events, list)
        or not isinstance(live_errors, list)
        or not isinstance(live_lifecycle, list)
        or live_custody.get("event_count") != len(live_events)
        or live_custody.get("error_count") != len(live_errors)
        or live_custody.get("identity_sha256")
        != execution_custody.live_custody_identity_sha256(
            events=live_events,
            apparatus_events=live_apparatus_events,
            errors=live_errors,
            state=live_custody.get("state"),
            lifecycle=live_lifecycle,
        )
    ):
        raise ValueError("guarded receipt live-custody event binding is invalid")
    child_custody = context.get("child_process_custody")
    closure = envelope.get("process_closure")
    child_policy = (
        child_custody.get("policy") if _is_receipt_object(child_custody) else None
    )
    child_receipt = (
        child_custody.get("receipt") if _is_receipt_object(child_custody) else None
    )
    if (
        not _is_receipt_object(child_custody)
        or not _is_receipt_object(closure)
        or not _is_receipt_object(child_policy)
        or child_policy.get("descendants") != closure.get("descendants")
        or not _is_receipt_object(child_receipt)
        or child_receipt.get("broker_complete") is not True
    ):
        raise ValueError("guarded receipt has no complete child-process custody")
    platform_custody = context.get("platform_process_custody")
    platform_applicable = (
        sys.platform == "win32" and closure.get("descendants") == "declared-toolchains"
    )
    platform_images: list[dict[str, object]] = []
    if platform_custody is not None or platform_applicable:
        platform_prelaunch = (
            platform_custody.get("prelaunch")
            if _is_receipt_object(platform_custody)
            else None
        )
        if (
            not _is_receipt_object(platform_custody)
            or platform_custody.get("schema")
            != process_image_capture.PROCESS_IMAGE_SCHEMA
            or not _is_receipt_object_list(platform_prelaunch)
            or platform_custody.get("identical") is not True
            or platform_custody.get("prelaunch_sha256")
            != supervisor_custody._canonical_payload_sha256(platform_prelaunch)
            or platform_custody.get("postcompletion_sha256")
            != supervisor_custody._canonical_payload_sha256(platform_prelaunch)
        ):
            raise ValueError("guarded receipt has no stable platform process custody")
        platform_images = process_image_capture.revalidate_images(platform_prelaunch)
        if platform_images != platform_prelaunch:
            raise ValueError(
                "guarded receipt platform process custody is not reproducible"
            )
        if platform_applicable != bool(platform_images):
            raise ValueError(
                "guarded receipt platform process custody applicability mismatch"
            )
    supervisor = context.get("process_supervisor")
    supervisor_receipt = (
        supervisor.get("receipt") if _is_receipt_object(supervisor) else None
    )
    supervisor_binary = (
        supervisor.get("binary") if _is_receipt_object(supervisor) else None
    )
    supervisor_binary_artifact = (
        supervisor.get("binary_artifact") if _is_receipt_object(supervisor) else None
    )
    supervisor_policy = (
        supervisor.get("policy") if _is_receipt_object(supervisor) else None
    )
    supervisor_receipt_file = (
        supervisor.get("receipt_file") if _is_receipt_object(supervisor) else None
    )
    supervisor_event_artifact = (
        supervisor.get("event_artifact") if _is_receipt_object(supervisor) else None
    )
    if (
        not _is_receipt_object(supervisor)
        or supervisor.get("schema") != "molt.proof-process-supervision.v1"
        or supervisor.get("supervisor_returncode") != 0
        or not _is_receipt_object(supervisor_receipt)
        or supervisor_receipt.get("schema") != "molt.proof-process-closure-receipt.v3"
        or supervisor_receipt.get("complete") is not True
        or supervisor_receipt.get("state") != "COMPLETE"
        or not _is_receipt_object(supervisor_binary)
        or not _is_receipt_object(supervisor_binary_artifact)
        or not _is_receipt_object(supervisor_policy)
        or not _is_receipt_object(supervisor_receipt_file)
        or not _is_receipt_object(supervisor_event_artifact)
    ):
        raise ValueError("guarded receipt has no complete native process supervisor")
    binary_path = Path(str(supervisor_binary.get("path")))
    policy_path = Path(str(supervisor_policy.get("path")))
    receipt_path = Path(str(supervisor_receipt_file.get("path")))
    if (
        command_identity._file_identity(binary_path) != supervisor_binary
        or command_identity._file_identity(policy_path) != supervisor_policy
        or command_identity._file_identity(receipt_path) != supervisor_receipt_file
    ):
        raise ValueError("native process supervisor authority changed after execution")
    custody_cas.verify_file_ref(supervisor_binary_artifact, expected_root=cas_root)
    if (
        supervisor_binary_artifact.get("path") != supervisor_binary.get("path")
        or supervisor_binary_artifact.get("sha256") != supervisor_binary.get("sha256")
        or supervisor_binary_artifact.get("size_bytes")
        != supervisor_binary.get("size_bytes")
        or supervisor_binary_artifact.get("executable") is not True
    ):
        raise ValueError(
            "native process supervisor has no durable executable authority"
        )
    event_log = supervisor_receipt.get("event_log")
    durable_event = supervisor_event_artifact.get("artifact")
    if not _is_receipt_object(event_log) or not _is_receipt_object(durable_event):
        raise ValueError("native process supervisor has no durable event authority")
    custody_cas.verify_file_ref(durable_event, expected_root=cas_root)
    if (
        supervisor_event_artifact.get("schema")
        != "molt.proof-process-event-artifact.v1"
        or supervisor_event_artifact.get("count") != event_log.get("count")
        or supervisor_event_artifact.get("bytes") != event_log.get("bytes")
        or supervisor_event_artifact.get("sha256") != event_log.get("sha256")
        or durable_event.get("sha256") != event_log.get("sha256")
        or durable_event.get("size_bytes") != event_log.get("bytes")
        or durable_event.get("executable") is not False
    ):
        raise ValueError("native process supervisor event artifact binding is invalid")
    try:
        policy_payload = read_exact(
            policy_path,
            max_bytes=16 * 1024 * 1024,
            label="native proof supervisor policy",
        )
        receipt_payload = read_exact(
            receipt_path,
            max_bytes=16 * 1024 * 1024,
            label="native proof supervisor receipt",
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError("native process supervisor authority is unreadable") from exc
    expected_mode = (
        "leaf" if closure.get("descendants") == "forbidden" else "declared-tree"
    )
    policy_command = (
        policy_payload.get("command") if _is_receipt_object(policy_payload) else None
    )
    fixed_images = (
        policy_payload.get("fixed_images")
        if _is_receipt_object(policy_payload)
        else None
    )
    policy_environment = (
        policy_payload.get("environment")
        if _is_receipt_object(policy_payload)
        else None
    )
    policy_derived_roots = (
        policy_payload.get("derived_roots")
        if _is_receipt_object(policy_payload)
        else None
    )
    source_custody = context.get("source_custody")
    execution_environment = context.get("execution_environment")
    environment_prelaunch = (
        execution_environment.get("prelaunch")
        if _is_receipt_object(execution_environment)
        else None
    )
    derived_root_custody = context.get("derived_root_custody")
    derived_root_prelaunch = (
        derived_root_custody.get("prelaunch")
        if _is_receipt_object(derived_root_custody)
        else None
    )
    executable_inputs = (
        execution_environment.get("executable_inputs")
        if _is_receipt_object(execution_environment)
        else None
    )
    environment_executables = (
        executable_inputs.get("prelaunch")
        if _is_receipt_object(executable_inputs)
        else None
    )
    custody_authorities = context.get("custody_authorities")
    custody_authorities_prelaunch = (
        custody_authorities.get("prelaunch")
        if _is_receipt_object(custody_authorities)
        else None
    )
    expected_root_role = None
    expected_fixed_images = None
    passed_names = (
        environment_prelaunch.get("passed_names")
        if _is_receipt_object(environment_prelaunch)
        else None
    )
    if (
        _is_receipt_object(full_toolchains)
        and _is_receipt_object(environment_executables)
        and _is_string_list(policy_command)
        and policy_command
    ):
        expected_root_role, expected_fixed_images = (
            supervisor_custody._supervisor_fixed_images(
                full_toolchains,
                environment_executables,
                policy_command,
                platform_images,
            )
        )
    expected_derived_roots = None
    if _is_receipt_object(policy_environment):
        expected_derived_roots = supervisor_custody._supervisor_derived_roots(
            descendants=closure.get("descendants"),
            env={str(key): str(value) for key, value in policy_environment.items()},
        )
    if (
        receipt_payload != supervisor_receipt
        or not _is_receipt_object(policy_payload)
        or policy_payload.get("schema") != "molt.proof-process-closure.v2"
        or policy_payload.get("nonce") != execution_nonce
        or policy_payload.get("mode") != expected_mode
        or not _is_receipt_object(source_custody)
        or policy_payload.get("cwd") != source_custody.get("row_cwd")
        or not _is_string_list(policy_command)
        or not policy_command
        or hashlib.sha256(
            json.dumps(policy_command, separators=(",", ":")).encode()
        ).hexdigest()
        != context.get("exact_command_sha256")
        or not Path(str(policy_command[0])).is_absolute()
        or not _is_receipt_object(policy_environment)
        or not _is_receipt_object(environment_prelaunch)
        or not _is_receipt_object(execution_environment)
        or execution_environment.get("identical") is not True
        or execution_environment.get("postcompletion_identity_sha256")
        != environment_prelaunch.get("identity_sha256")
        or not _is_receipt_object(executable_inputs)
        or executable_inputs.get("identical") is not True
        or executable_inputs.get("postcompletion_sha256")
        != supervisor_custody._canonical_payload_sha256(environment_executables)
        or not _is_receipt_object(custody_authorities)
        or not isinstance(custody_authorities_prelaunch, list)
        or custody_authorities.get("identical") is not True
        or custody_authorities.get("postcompletion_sha256")
        != supervisor_custody._canonical_payload_sha256(custody_authorities_prelaunch)
        or not _is_string_list(passed_names)
        or set(policy_environment) != set(passed_names)
        or environment_authority._canonical_environment_sha256(
            {str(key): str(value) for key, value in policy_environment.items()}
        )
        != environment_prelaunch.get("canonical_values_sha256")
        or not isinstance(fixed_images, list)
        or policy_payload.get("root_role") != expected_root_role
        or fixed_images != expected_fixed_images
        or policy_derived_roots != expected_derived_roots
        or not _is_receipt_object(derived_root_custody)
        or not isinstance(derived_root_prelaunch, list)
        or derived_root_custody.get("policy_roots") != policy_derived_roots
        or any(
            not _is_receipt_object(row)
            or row.get("run_owned") is not True
            or (
                row.get("schema") != cargo_cache_custody.SCHEMA
                and (
                    row.get("initial_entry_count") != 0
                    or row.get("initial_manifest_sha256")
                    != supervisor_custody._canonical_payload_sha256([])
                )
            )
            for row in derived_root_prelaunch
        )
        or [
            {"role": row.get("role"), "path": row.get("path")}
            for row in derived_root_prelaunch
            if _is_receipt_object(row)
        ]
        != policy_derived_roots
        or not any(
            _is_receipt_object(image)
            and os.path.normcase(os.path.abspath(str(image.get("path"))))
            == os.path.normcase(os.path.abspath(str(policy_command[0])))
            and image.get("sha256")
            == command_identity._hash_file(Path(str(policy_command[0])))
            for image in fixed_images
        )
        or supervisor_receipt.get("nonce_sha256") != expected_nonce_hash
    ):
        raise ValueError("native process supervisor policy binding is invalid")
    for derived in derived_root_prelaunch:
        if derived.get("schema") == cargo_cache_custody.SCHEMA:
            if (
                derived.get("generation_run_id") != run_id
                or derived.get("execution_nonce_sha256") != expected_nonce_hash
            ):
                raise ValueError(
                    "Cargo generation provenance differs from admitted run/nonce"
                )
            source_snapshot = source_custody.get("prelaunch")
            source_content = source_custody.get("content")
            if not isinstance(source_snapshot, Mapping) or not isinstance(
                source_content, Mapping
            ):
                raise ValueError(
                    "Cargo cache requires independent source content custody"
                )
            cargo_cache_custody.validate_prelaunch(
                derived,
                cargo_output_root=envelope.get("cargo_output_root"),
                cargo_output_lifetime=command_admission.validated_cargo_output_lifetime(
                    envelope
                ),
                cas_root=execution_path.parent / "custody-cas",
                command=policy_command,
                outputs=cargo_output_environment.CargoOutputEnvironment.for_envelope(
                    envelope
                ),
                env={str(key): str(value) for key, value in policy_environment.items()},
                toolchains=full_toolchains,
                source_root=str(source_snapshot.get("root")),
                source_snapshot=source_snapshot,
                source_content=source_content.get("prelaunch"),
            )
    if envelope.get("cargo_output_root") is not None:
        output_layout = cargo_output_layout.CargoOutputLayout.for_envelope(
            envelope,
            result_root=execution_path.parent,
            source_root=Path(str(source_custody.get("row_cwd"))),
        )
        output_layout.validate(
            protected_roots=[
                Path(item.path).parent
                for item in toolchain_capture.frozen_files(full_toolchains)
            ]
        )
        if policy_environment.get(supervisor_custody.PROOF_SCRATCH_ROOT_ENV) != str(
            output_layout.scratch(execution_nonce)
        ):
            raise ValueError("proof scratch differs from admitted Cargo output layout")
        provision = supervisor.get("provision_telemetry")
        if not isinstance(provision, Mapping) or provision.get(
            "build_target_dir"
        ) != str(output_layout.supervisor_target):
            raise ValueError(
                "supervisor build output differs from admitted Cargo output layout"
            )
    verified_supervisor = _COMMANDS.run(
        [
            str(binary_path),
            "verify",
            "--policy",
            str(policy_path),
            "--receipt",
            str(receipt_path),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if verified_supervisor.returncode != 0:
        raise ValueError(
            "native process supervisor receipt failed independent verification"
        )
    try:
        verification_payload = loads_exact(verified_supervisor.stdout)
    except (ExactJsonError, json.JSONDecodeError) as exc:
        raise ValueError(
            "native process supervisor verification response is not exact JSON"
        ) from exc
    verification_capability = (
        verification_payload.get("capability")
        if _is_receipt_object(verification_payload)
        else None
    )
    try:
        verified_required_environment = supervisor_custody.decode_supervisor_capability(
            verification_capability, mode=expected_mode
        )
    except ValueError as exc:
        raise ValueError(
            "native process supervisor verification capability is invalid"
        ) from exc
    supervisor_custody.validate_required_environment_binding(
        required_environment=verified_required_environment,
        supervisor_metadata=supervisor.get("required_environment"),
        policy_environment=policy_environment,
        supervisor_owned_names=environment_prelaunch.get("supervisor_owned_names", []),
    )
    expected_custody = supervisor_custody.execution_custody_sha256(
        wire_context,
        run_id=run_id,
        returncode=returncode,
    )
    if context.get("execution_custody_sha256") != expected_custody:
        raise ValueError("guarded execution custody digest mismatch")
    transcript = context.get("command_transcript")
    if not _is_receipt_object(transcript):
        raise ValueError("guarded receipt context has no command transcript")
    for stream_name, expected_path in command_identity.execution_transcript_paths(
        execution_path
    ).items():
        expected = transcript.get(stream_name)
        if not _is_receipt_object(expected) or expected.get("path") != str(
            expected_path
        ):
            raise ValueError(
                f"guarded {stream_name} transcript path substitution detected"
            )
        actual = command_identity._transcript_identity(expected_path)
        if expected != actual:
            raise ValueError(
                f"guarded {stream_name} transcript content substitution detected"
            )
    transcript_material = {name: transcript[name] for name in ("stdout", "stderr")}
    transcript_digest = hashlib.sha256(
        json.dumps(
            transcript_material,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    ).hexdigest()
    if transcript.get("identity_sha256") != transcript_digest:
        raise ValueError("guarded command transcript digest mismatch")
    command_identity.validate_structured_test_counts(
        envelope, transcript, returncode=returncode
    )


def _write_execution_request(
    *,
    row: sqlite3.Row,
    command: list[str],
    repo_root: Path,
    resource_family: str,
    run_id: str,
    env_override_names: list[str],
    log_path: Path,
    summary_path: Path,
    timeout_seconds: float,
) -> tuple[Path, Path, dict[str, object], str]:
    try:
        envelope = loads_exact(str(row["command_envelope_json"]))
    except (TypeError, ValueError) as exc:
        raise ValueError("proof row command envelope is not exact JSON") from exc
    if not _is_receipt_object(envelope):
        raise ValueError("proof row command envelope is malformed")
    command_admission.validate_envelope(envelope, command)
    request_path, result_path = command_identity.execution_record_paths(log_path)
    execution_nonce = uuid.uuid4().hex + uuid.uuid4().hex
    for stale in (
        request_path,
        result_path,
        *command_identity.execution_transcript_paths(result_path).values(),
        summary_path,
    ):
        try:
            stale.unlink()
        except FileNotFoundError:
            pass
    request = {
        "schema": command_admission.EXECUTION_SCHEMA,
        "command": command,
        "envelope": envelope,
        "cwd": str(repo_root),
        "resource_family": resource_family,
        "result_path": str(result_path),
        "run_id": run_id,
        "execution_nonce": execution_nonce,
        "env_override_names": sorted(env_override_names, key=str.casefold),
        "timeout_seconds": timeout_seconds,
    }
    write_exact(request_path, request)
    return request_path, result_path, envelope, execution_nonce


def _read_execution_record(path: Path) -> dict[str, object]:
    try:
        payload = read_exact(
            path, max_bytes=16 * 1024 * 1024, label="guarded proof execution record"
        )
    except (OSError, UnicodeDecodeError, json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(
            f"guarded proof execution record is unavailable: {exc}"
        ) from exc
    if (
        not _is_receipt_object(payload)
        or payload.get("schema") != command_admission.EXECUTION_SCHEMA
    ):
        raise ValueError("guarded proof execution record schema mismatch")
    return payload


def _wait_for_guard_completion_or_stale(
    conn: sqlite3.Connection,
    *,
    run_id: str,
    proc: subprocess.Popen[str],
    log: TextIO,
    start: float,
) -> tuple[str, int | None, float]:
    while True:
        try:
            rc = int(proc.wait(timeout=custody.PROOF_QUEUE_ACTIVE_POLL_SECONDS))
        except subprocess.TimeoutExpired:
            row = state._row_by_run_id(conn, run_id)
            if row is None:
                continue
            if row["status"] not in state.RUNNING:
                elapsed = time.monotonic() - start
                if proc.poll() is None:
                    print(
                        "proof_queue noticed externally terminal row "
                        f"status={row['status']} while guard_pid={proc.pid} "
                        "was still live; terminating queue-owned guard",
                        file=log,
                        flush=True,
                    )
                    custody._terminate_queue_owned_guard_process(
                        proc, log, run_id=run_id
                    )
                return str(row["status"]), row["returncode"], elapsed
            diagnostics = diagnostic_engine._run_diagnostics(row)
            if not diagnostic_model._diagnostics_have_terminal_stale_signal(
                diagnostics
            ):
                continue
            diagnostic_summary = diagnostic_model._format_diagnostic_summary(
                diagnostics
            )
            print(
                "\nproof_queue stale-running terminalization "
                f"diagnosis={diagnostic_summary}",
                file=log,
                flush=True,
            )
            if diagnostics:
                evidence = diagnostics[0].get("evidence")
                if isinstance(evidence, str) and evidence.strip():
                    print(f"evidence={evidence}", file=log, flush=True)
                artifacts = diagnostic_model._diagnostic_artifacts(diagnostics)
                if artifacts:
                    print(f"artifacts={', '.join(artifacts)}", file=log, flush=True)
            guard_rc = custody._terminate_queue_owned_guard_process(
                proc, log, run_id=run_id
            )
            if guard_rc is not None:
                print(
                    f"proof_queue stale terminalization guard_exit_code={guard_rc}",
                    file=log,
                    flush=True,
                )
            elapsed = time.monotonic() - start
            return "stale", custody.PROOF_QUEUE_STALE_EXIT_CODE, elapsed
        else:
            elapsed = time.monotonic() - start
            status = "passed" if rc == 0 else "failed"
            return status, rc, elapsed


def _queue_one(
    args: argparse.Namespace,
    *,
    logical_id: str,
    reason: str,
    command: list[str],
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str],
    initial_notes: list[str] | None = None,
    depends_on: list[str] | None = None,
    edge_kind: str = state.DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
    policy_error: str | None = None,
    cargo_output_lifetime: str = "retain",
    cargo_output_root: str | None = None,
) -> tuple[int, str | None]:
    if not command:
        raise SystemExit("proof command is empty")
    if cargo_output_lifetime != "retain" or cargo_output_root is not None:
        try:
            command_admission.envelope_for_command(
                command,
                cargo_output_lifetime=cargo_output_lifetime,
                cargo_output_root=cargo_output_root,
            )
        except ValueError as exc:
            print(str(exc), file=sys.stderr)
            return 2, None
    secret_error = environment_authority.command_secret_policy_error(command)
    env_error = policy._proof_env_policy_error(env_overrides)
    if secret_error is not None or env_error is not None:
        print(secret_error or env_error, file=sys.stderr)
        return 2, None
    if not initial_notes:
        print(
            "queued proof submissions require at least one append-only note; "
            "pass --note or use a named lane with built-in notes",
            file=sys.stderr,
        )
        return 2, None
    db = state._db_path(args)
    logs_root = state._logs_root(args)
    repo_root = state._repo_root(args)
    conn = state._connect(db)
    for parent_run_id in depends_on or []:
        if state._edge_kind_requires_local_parent(edge_kind) and not state._run_exists(
            conn, parent_run_id
        ):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    if edge_kind not in state.EDGE_KINDS:
        allowed = ", ".join(sorted(state.EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    scheduling._refresh_blocked_queued_runs(args, conn)
    maturity = scheduling._lane_maturity_admission(
        conn=conn,
        repo_root=repo_root,
        logical_id=logical_id,
        resource_family=resource_family,
        depends_on=depends_on or (),
    )
    if not maturity.allow:
        print(f"lane maturity refused {logical_id}: {maturity.reason}", file=sys.stderr)
        return 2, None
    run_id = f"{state._compact_utc()}-{state._slug(logical_id)}-{uuid.uuid4().hex[:16]}"
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = logs_root / f"{run_id}.log"
    active = scheduling._admit_run(
        conn,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        command=command,
        cargo_output_lifetime=cargo_output_lifetime,
        cargo_output_root=cargo_output_root,
        cwd=repo_root,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=scopes,
        env_overrides=env_overrides,
        log_path=log_path,
        summary_json=logs_root / f"{run_id}.memory_guard.json",
    )
    if active is not None:
        print(
            f"contention key {contention_key!r} already has active run(s):",
            file=sys.stderr,
        )
        for row in active:
            print(f"- {row['status']} {row['run_id']} {row['reason']}", file=sys.stderr)
        return 2, None
    try:
        for parent_run_id in depends_on or []:
            state._insert_edge(
                conn,
                parent_run_id=parent_run_id,
                child_run_id=run_id,
                kind=edge_kind,
                note=edge_note,
            )
        for note in initial_notes or []:
            state._insert_note(
                conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND
            )
    except Exception as exc:
        rc = evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="submission metadata",
        )
        return rc, run_id
    policy_error = (
        policy_error
        or policy._proof_command_policy_error(command)
        or policy._proof_env_policy_error(env_overrides)
    )
    if policy_error is not None:
        rc = _record_policy_rejection(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            policy_error=policy_error,
        )
        return rc, run_id
    evidence._write_queued_submission_log(
        log_path,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        repo_root=repo_root,
        command=command,
        resource_family=resource_family,
        contention_key=contention_key,
        scopes=scopes,
        env_overrides=env_overrides,
        depends_on=depends_on or [],
    )
    if initial_notes or depends_on:
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="submission projection",
        )
    print(f"queued {run_id}")
    return 0, run_id


def _claim_detached_run(
    conn: sqlite3.Connection,
    run_id: str,
    *,
    queue_size: int,
) -> tuple[sqlite3.Row | None, str | None]:
    """Atomically move one queued row into launched custody."""
    conn.row_factory = sqlite3.Row
    if conn.in_transaction:
        conn.commit()
    conn.execute("BEGIN IMMEDIATE")
    try:
        row = conn.execute(
            "SELECT * FROM proof_runs WHERE run_id = ?",
            (run_id,),
        ).fetchone()
        if row is None:
            conn.rollback()
            raise SystemExit(f"unknown proof run {run_id!r}")
        if row["status"] != "queued":
            conn.rollback()
            return None, f"proof run {run_id!r} is {row['status']}, not queued"

        active_count = int(
            conn.execute(
                f"SELECT COUNT(*) FROM proof_runs "
                f"WHERE status IN ({state.LAUNCHED_SQL_STATUSES})"
            ).fetchone()[0]
        )
        if active_count >= queue_size:
            conn.rollback()
            return (
                None,
                f"queue capacity full active={active_count} queue_size={queue_size}",
            )

        active = scheduling._active_contention_conflicts(
            conn,
            resource_family=str(row["resource_family"]),
            contention_key=str(row["contention_key"]),
            command=scheduling._row_command(row),
            existing_run_id=run_id,
        )
        if active:
            conn.rollback()
            return (
                None,
                f"waiting {run_id} "
                + scheduling._format_active_contention_conflicts(active).replace(
                    "\n", "; "
                ),
            )

        now = state._utc_now()
        updated = conn.execute(
            """
            UPDATE proof_runs
            SET status = 'dispatched', started_at = ?
            WHERE run_id = ? AND status = 'queued'
            """,
            (now, run_id),
        )
        if updated.rowcount != 1:
            conn.rollback()
            return None, f"proof run {run_id!r} was claimed by another scheduler"
    except sqlite3.IntegrityError:
        conn.rollback()
        return None, f"proof run {run_id!r} could not be claimed atomically"
    except BaseException:
        conn.rollback()
        raise
    state._commit_with_locked_retry(conn)
    return state._row_by_run_id(conn, run_id), None


def _dispatch_detached_runner(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_id: str,
    timeout: float,
) -> tuple[int, Path] | None:
    claimed, skip_reason = _claim_detached_run(
        conn,
        run_id,
        queue_size=state._configured_queue_size(getattr(args, "queue_size", None)),
    )
    if claimed is None:
        if skip_reason:
            print(skip_reason)
        return None
    try:
        pid, runner_log = custody._launch_detached_runner(
            args, run_id=run_id, timeout=timeout
        )
    except Exception:
        state._update_run(
            conn,
            run_id,
            status="failed",
            returncode=2,
            finished_at=state._utc_now(),
            elapsed_s=0.0,
        )
        raise
    row = state._row_by_run_id(conn, run_id)
    if row is not None:
        log_path = Path(str(row["log_path"]))
        log_path.parent.mkdir(parents=True, exist_ok=True)
        with log_path.open("a", encoding="utf-8") as log:
            print("\n--- proof_queue detached dispatch ---", file=log)
            print("status=dispatched", file=log)
            print(f"runner_pid={pid}", file=log)
            print(f"runner_log={runner_log}", file=log)
    return pid, runner_log


def _record_policy_rejection(
    args: argparse.Namespace,
    conn: sqlite3.Connection,
    *,
    run_id: str,
    logical_id: str,
    reason: str,
    repo_root: Path,
    command: list[str],
    log_path: Path,
    policy_error: str,
) -> int:
    now = state._utc_now()
    state._update_run(
        conn,
        run_id,
        status="failed",
        returncode=2,
        started_at=now,
        finished_at=now,
        elapsed_s=0.0,
        receipt_context_json=json.dumps(
            state._unattested_receipt_context(
                status="not-executed",
                phase="command policy rejection",
                reason=policy_error,
            ),
            sort_keys=True,
        ),
    )
    evidence._write_failed_run_log(
        log_path,
        run_id=run_id,
        logical_id=logical_id,
        reason=reason,
        repo_root=repo_root,
        command=command,
        lines=[policy_error],
    )
    print(f"rejected {run_id} rc=2")
    print(policy_error, file=sys.stderr)
    print(f"log: {log_path}")
    if state._notes_for_run_ids(conn, [run_id]).get(run_id):
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="policy rejection projection",
        )
    return 2


def _run_one(
    args: argparse.Namespace,
    *,
    logical_id: str,
    reason: str,
    command: list[str],
    resource_family: str,
    contention_key: str,
    scopes: list[str],
    env_overrides: dict[str, str],
    timeout: float,
    initial_notes: list[str] | None = None,
    depends_on: list[str] | None = None,
    edge_kind: str = state.DEFAULT_EDGE_KIND,
    edge_note: str | None = None,
    policy_error: str | None = None,
    existing_run_id: str | None = None,
    existing_log_path: Path | None = None,
    existing_summary_json: Path | None = None,
    cargo_output_lifetime: str = "retain",
    cargo_output_root: str | None = None,
) -> int:
    if not command:
        raise SystemExit("proof command is empty")
    if existing_run_id is None and (
        cargo_output_lifetime != "retain" or cargo_output_root is not None
    ):
        try:
            command_admission.envelope_for_command(
                command,
                cargo_output_lifetime=cargo_output_lifetime,
                cargo_output_root=cargo_output_root,
            )
        except ValueError as exc:
            print(str(exc), file=sys.stderr)
            return 2
    secret_error = environment_authority.command_secret_policy_error(command)
    env_error = policy._proof_env_policy_error(env_overrides)
    if secret_error is not None or env_error is not None:
        print(secret_error or env_error, file=sys.stderr)
        return 2
    db = state._db_path(args)
    logs_root = state._logs_root(args)
    repo_root = state._repo_root(args)
    conn = state._connect(db)
    for parent_run_id in depends_on or []:
        if not state._run_exists(conn, parent_run_id):
            raise SystemExit(f"unknown parent proof run {parent_run_id!r}")
    maturity = scheduling._lane_maturity_admission(
        conn=conn,
        repo_root=repo_root,
        logical_id=logical_id,
        resource_family=resource_family,
        depends_on=depends_on or (),
    )
    if not maturity.allow:
        print(f"lane maturity refused {logical_id}: {maturity.reason}", file=sys.stderr)
        return 2
    if edge_kind not in state.EDGE_KINDS:
        allowed = ", ".join(sorted(state.EDGE_KINDS))
        raise SystemExit(f"unknown proof edge kind {edge_kind!r}; allowed: {allowed}")
    active = scheduling._active_contention_conflicts(
        conn,
        resource_family=resource_family,
        contention_key=contention_key,
        command=command,
        existing_run_id=existing_run_id,
    )
    if active:
        scheduling._print_active_contention_conflicts(active)
        return 2
    suffix = uuid.uuid4().hex[:16]
    run_id = (
        existing_run_id or f"{state._compact_utc()}-{state._slug(logical_id)}-{suffix}"
    )
    logs_root.mkdir(parents=True, exist_ok=True)
    log_path = existing_log_path or logs_root / f"{run_id}.log"
    summary_json = existing_summary_json or logs_root / f"{run_id}.memory_guard.json"
    inserted_run = existing_run_id is None
    try:
        cargo_output_lifecycle.resume_declared_successes(db)
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
        print(
            f"Cargo output recovery could not inspect pending finalizations: {exc}",
            file=sys.stderr,
        )
    if existing_run_id is None:
        scheduling._insert_run(
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            command=command,
            cargo_output_lifetime=cargo_output_lifetime,
            cargo_output_root=cargo_output_root,
            cwd=repo_root,
            resource_family=resource_family,
            contention_key=contention_key,
            scopes=scopes,
            env_overrides=env_overrides,
            log_path=log_path,
            summary_json=summary_json,
        )
    if inserted_run:
        try:
            for parent_run_id in depends_on or []:
                state._insert_edge(
                    conn,
                    parent_run_id=parent_run_id,
                    child_run_id=run_id,
                    kind=edge_kind,
                    note=edge_note,
                )
            for note in initial_notes or []:
                state._insert_note(
                    conn, run_id=run_id, body=note, kind=state.SUBMISSION_NOTE_KIND
                )
        except Exception as exc:
            return evidence._fail_preexecution_run(
                args,
                conn,
                run_id=run_id,
                logical_id=logical_id,
                reason=reason,
                repo_root=repo_root,
                command=command,
                log_path=log_path,
                exc=exc,
                phase="submission metadata",
            )
        if initial_notes or depends_on:
            evidence._try_write_marimo_notebook(
                args,
                conn,
                run_id,
                log_path=log_path,
                phase="submission projection",
            )
    policy_error = (
        policy_error
        or policy._proof_command_policy_error(command)
        or policy._proof_env_policy_error(env_overrides)
    )
    if policy_error is not None:
        return _record_policy_rejection(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            policy_error=policy_error,
        )
    capacity_admission = None
    try:
        session_id = state._proof_session_id(resource_family, contention_key)
        admitted_row = state._row_by_run_id(conn, run_id)
        if admitted_row is None:
            raise ValueError("admitted proof run disappeared before launch")
        admitted_envelope = loads_exact(admitted_row["command_envelope_json"])
        if not isinstance(admitted_envelope, dict):
            raise ValueError("admitted command envelope must be an object")
        command_admission.validate_envelope(admitted_envelope, command)
        requested_toolchains = admitted_envelope.get("toolchains")
        if not _is_string_list(requested_toolchains):
            raise ValueError("admitted command has malformed toolchain names")
        uses_cargo = "cargo" in requested_toolchains
        output_layout = cargo_output_layout.CargoOutputLayout.for_envelope(
            admitted_envelope,
            result_root=logs_root,
            source_root=repo_root
            if admitted_envelope.get("cargo_output_root") is not None
            else None,
        )
        output_layout.validate_environment({**os.environ, **env_overrides})
        if uses_cargo:
            # The queue owns Cargo and derived output below the result root.
            # Reject before environment provisioning, capture, or child launch.
            capacity_admission = disk_capacity.require_build_capacity(
                output_layout.capacity_paths()
                if output_layout.declaration is not None
                else (logs_root,),
                env={**os.environ, **env_overrides},
            ).as_dict()
        env = development_artifact_env(
            repo_root,
            os.environ,
            session_prefix=f"proof-{resource_family}",
            session_id=session_id,
            create_dirs=uses_cargo and output_layout.declaration is None,
        )
        if not uses_cargo:
            # A non-Cargo proof owns no Cargo artifact lane.  The native proof
            # supervisor has its separate result-root target, so retaining a
            # repo-local session target here only dirties admitted source.
            env.pop("CARGO_TARGET_DIR", None)
        proof_tmp = (logs_root / "tmp").resolve()
        payload_tmp = (
            output_layout.temporary
            if output_layout.declaration is not None
            else proof_tmp
        )
        if output_layout.declaration is not None:
            custody_cas._durable_makedirs(payload_tmp)
        env.update(
            {
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(proof_tmp / "memory_guard"),
                "PYTHONPYCACHEPREFIX": str(payload_tmp / "pycache"),
                "TEMP": str(payload_tmp),
                "TMP": str(payload_tmp),
                "TMPDIR": str(payload_tmp),
            }
        )
        bind_repo_src_pythonpath(repo_root, env)
        env["MOLT_PROOF_QUEUE"] = "1"
        env["MOLT_PROOF_QUEUE_DB"] = str(db)
        env["MOLT_PROOF_QUEUE_RUN_ID"] = run_id
        env.update(env_overrides)
        if guard_repro_context._command_requests_test_custody(
            command,
            cwd=repo_root,
            root=repo_root,
        ):
            pytest_root = pytest_guard_summary_dir(repo_root, env)
            env["MOLT_PYTEST_CURRENT_TEST_FILE"] = str(
                pytest_root / f"{state._slug(run_id)}_current-test.json"
            )
        row = state._row_by_run_id(conn, run_id)
        if row is None:
            raise ValueError(f"proof run {run_id!r} disappeared before execution")
        request_path, execution_path, envelope, execution_nonce = (
            _write_execution_request(
                row=row,
                command=command,
                repo_root=repo_root,
                resource_family=resource_family,
                run_id=run_id,
                env_override_names=list(env_overrides),
                log_path=log_path,
                summary_path=summary_json,
                timeout_seconds=timeout,
            )
        )
        memory_limits = custody._proof_queue_memory_limits(env_overrides)
        poll_interval = str(memory_limits.poll_interval)
        env[custody.MEMORY_GUARD_POLL_SEC_ENV] = poll_interval
        guarded_command = [
            sys.executable,
            str(state.ROOT / "tools" / "proof_queue_pkg" / "guarded_execution.py"),
            "--request",
            str(request_path),
        ]
        wrapped = custody._memory_guard_command(
            command=guarded_command,
            summary_json=summary_json,
            timeout=timeout,
            limits=memory_limits,
        )
    except Exception as exc:
        return evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="execution environment setup",
        )
    start = time.monotonic()
    started_at = state._utc_now()
    state._update_run(conn, run_id, status="running", started_at=started_at)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        log = log_path.open("a", encoding="utf-8")
        if log.tell() > 0:
            print("\n--- proof_queue command execution ---", file=log)
        print(f"proof_queue run_id={run_id}", file=log)
        print(f"logical_id={logical_id}", file=log)
        print(f"reason={reason}", file=log)
        print(f"cwd={repo_root}", file=log)
        print("memory_guard_prefix=MOLT_PROOF_QUEUE", file=log)
        print(f"command={shlex.join(command)}", file=log)
        if env_overrides:
            print(
                "env_override_names="
                + json.dumps(sorted(env_overrides, key=str.casefold)),
                file=log,
            )
        print(f"proof_session_id={session_id}", file=log)
        if capacity_admission is not None:
            print(
                "disk_capacity_admission="
                + json.dumps(capacity_admission, sort_keys=True),
                file=log,
            )
        print(f"requested_cargo_target_dir={env.get('CARGO_TARGET_DIR', '')}", file=log)
        print(
            "command_envelope=" + json.dumps(envelope, sort_keys=True),
            file=log,
        )
        print(f"memory_guard_poll_sec={poll_interval}", file=log)
        print(f"memory_guard_summary_json={summary_json}", file=log)
        print(f"memory_guard_command={shlex.join(wrapped)}", file=log)
        print("", file=log, flush=True)
        proc = custody._launch_queued_command(
            wrapped,
            cwd=repo_root,
            env=env,
            stdout=log,
        )
    except Exception as exc:
        try:
            log.close()
        except NameError:
            pass
        return evidence._fail_preexecution_run(
            args,
            conn,
            run_id=run_id,
            logical_id=logical_id,
            reason=reason,
            repo_root=repo_root,
            command=command,
            log_path=log_path,
            exc=exc,
            phase="process launch",
        )
    try:
        state._update_run(
            conn,
            run_id,
            guard_pid=proc.pid,
            guard_identity=custody._process_identity(proc.pid),
        )
        status, rc, elapsed = _wait_for_guard_completion_or_stale(
            conn,
            run_id=run_id,
            proc=proc,
            log=log,
            start=start,
        )
        rc_text = "?" if rc is None else str(rc)
        print(
            f"\nproof_queue finished status={status} exit_code={rc_text} "
            f"elapsed={elapsed:.3f}s",
            file=log,
        )
    finally:
        log.close()
    receipt_context: dict[str, object] | None = None
    execution_error: str | None = None
    execution_record: dict[str, object] | None = None
    process_cleanup_safe = False
    guard_infrastructure_failure: GuardInfrastructureFailure | None = None
    try:
        execution_record = _read_execution_record(execution_path)
        if execution_record.get("run_id") != run_id:
            raise ValueError("guarded execution record run identity mismatch")
        if execution_record.get("execution_nonce") != execution_nonce:
            raise ValueError("guarded execution record nonce mismatch")
        if execution_record.get("envelope") != envelope:
            raise ValueError("guarded execution record changed the admitted envelope")
        raw_context = execution_record.get("receipt_context")
        if _is_receipt_object(raw_context):
            receipt_context = raw_context
        if execution_record.get("phase") == "complete":
            command_rc = execution_record.get("command_returncode")
            if type(command_rc) is not int or type(rc) is not int:
                raise ValueError("guard/command return-code custody is incomplete")
            _, guard_infrastructure_failure = _validated_guard_outcome(
                summary_json,
                guarded_command=guarded_command,
                returncode=rc,
                child_returncode=command_rc,
            )
            if receipt_context is None:
                raise ValueError("complete guarded execution has no receipt context")
            _validated_execution_context(
                receipt_context,
                execution_path=execution_path,
                envelope=envelope,
                run_id=run_id,
                execution_nonce=execution_nonce,
                returncode=command_rc,
            )
            guard_receipt = _validated_guard_receipt(
                summary_json,
                guarded_command=guarded_command,
                returncode=rc,
                child_returncode=command_rc,
                run_id=run_id,
                execution_nonce=execution_nonce,
                guard_pid=proc.pid,
            )
            receipt_context["guard_receipt"] = guard_receipt
            process_cleanup_safe = True
        elif status == "passed":
            raise ValueError(
                "memory guard passed without a complete command execution record"
            )
        if isinstance(execution_record.get("error"), str):
            execution_error = str(execution_record["error"])
    except Exception as exc:
        execution_error = f"{type(exc).__name__}: {exc}"
        if status == "passed":
            status = "failed"
            rc = 2
    try:
        outcome, receipt_context = _finalize_execution_receipt(
            execution_record=execution_record,
            execution_path=execution_path,
            run_id=run_id,
            execution_nonce=execution_nonce,
            receipt_context=receipt_context,
            process_cleanup_safe=process_cleanup_safe,
            status=status,
            returncode=rc,
            execution_error=execution_error,
            guard_infrastructure_failure=guard_infrastructure_failure,
        )
        status = str(outcome["status"])
        rc = outcome["returncode"]
        execution_error = outcome["execution_error"]
    except Exception as exc:
        # A partial publication is retained, never rebound under a second
        # result. Without the exact terminal context no consumer may reclaim it.
        detail = f"terminal custody publication failed: {type(exc).__name__}: {exc}"
        execution_error = f"{execution_error}; {detail}" if execution_error else detail
        status, rc = "failed", 2
        receipt_context = dict(
            state._unattested_receipt_context(
                status="non-evidence",
                phase="terminal custody publication",
                reason=execution_error,
            )
        )
    if execution_error:
        with log_path.open("a", encoding="utf-8") as terminal_log:
            print(
                f"proof_queue execution custody: {execution_error}", file=terminal_log
            )
    state._update_run(
        conn,
        run_id,
        status=status,
        returncode=rc,
        finished_at=state._utc_now(),
        elapsed_s=elapsed,
        receipt_context_json=json.dumps(receipt_context, sort_keys=True),
    )
    disposition_failed = False
    try:
        disposition = cargo_output_lifecycle.finalize_declared_success(db, run_id)
        if disposition is not None:
            print(
                f"Cargo output disposition: {json.dumps(disposition, sort_keys=True)}"
            )
    except (OSError, ValueError, RuntimeError, sqlite3.Error) as exc:
        disposition_failed = True
        detail = f"Cargo output finalization failed (persisted proof result unchanged): {type(exc).__name__}: {exc}"
        print(detail, file=sys.stderr)
        with log_path.open("a", encoding="utf-8") as terminal_log:
            print(detail, file=terminal_log)
    if state._notes_for_run_ids(conn, [run_id]).get(run_id):
        evidence._try_write_marimo_notebook(
            args,
            conn,
            run_id,
            log_path=log_path,
            phase="completion projection",
        )
    rc_text = "?" if rc is None else str(rc)
    print(f"{status} {run_id} rc={rc_text} elapsed={elapsed:.1f}s")
    print(f"log: {log_path}")
    return (
        2
        if disposition_failed
        else (rc if rc is not None else custody.PROOF_QUEUE_STALE_EXIT_CODE)
    )
