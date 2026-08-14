"""Guarded proof execution composing admitted command custody authorities."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import secrets
import subprocess
import sys
import time
from typing import Mapping, Sequence


# This file is launched by absolute path from an arbitrary proof cwd.  Establish
# the two source-layout roots before importing the owning authority modules.
_REPO_ROOT = Path(__file__).resolve().parents[2]
_PYTHON_SOURCE_ROOT = _REPO_ROOT / "src"
for _import_root in (_REPO_ROOT, _PYTHON_SOURCE_ROOT):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))
_loaded_molt = sys.modules.get("molt")
if _loaded_molt is not None and hasattr(_loaded_molt, "__path__"):
    _local_molt_root = str(_PYTHON_SOURCE_ROOT / "molt")
    if _local_molt_root not in _loaded_molt.__path__:
        _loaded_molt.__path__.insert(0, _local_molt_root)

from molt.cargo_execution_policy import normalize_cargo_environment  # noqa: E402
from tools import proof_plan  # noqa: E402
from tools.proof_queue_pkg import (  # noqa: E402
    command_admission as admission,
    command_identity,
    custody_cas,
    execution_custody,
    execution_environment as environment,
    process_image_capture,
    supervisor_custody as supervisor,
    toolchain_capture,
)


def execute_guarded_request(request_path: Path) -> int:
    """Run identity, preflight, proof, and completion custody under one guard."""
    request = json.loads(request_path.read_text(encoding="utf-8"))
    if not isinstance(request, dict):
        raise ValueError("proof execution request must be an object")
    if request.get("schema") != admission.EXECUTION_SCHEMA:
        raise ValueError("proof execution request schema mismatch")
    command = request.get("command")
    envelope = request.get("envelope")
    result_path = Path(str(request["result_path"]))
    cwd = Path(str(request["cwd"]))
    run_id = request.get("run_id")
    execution_nonce = request.get("execution_nonce")
    timeout_seconds = request.get("timeout_seconds")
    override_names = request.get("env_override_names", [])
    if not isinstance(command, list) or not isinstance(envelope, dict):
        raise ValueError("proof execution request has no typed command envelope")
    if not isinstance(run_id, str) or not run_id:
        raise ValueError("proof execution request has no run identity")
    if not isinstance(execution_nonce, str) or not re.fullmatch(
        r"[0-9a-f]{64}", execution_nonce
    ):
        raise ValueError("proof execution request has no canonical nonce")
    if (
        not isinstance(timeout_seconds, (int, float))
        or isinstance(timeout_seconds, bool)
        or not math.isfinite(float(timeout_seconds))
        or float(timeout_seconds) <= 0
    ):
        raise ValueError("proof execution request has no finite positive timeout")
    execution_deadline = time.monotonic() + float(timeout_seconds)
    shutdown_reserve = min(2.0, max(0.1, float(timeout_seconds) * 0.05))
    if not isinstance(override_names, list) or not all(
        isinstance(name, str) for name in override_names
    ):
        raise ValueError(
            "proof execution request has malformed environment override names"
        )
    command = [str(value) for value in command]
    admission.validate_envelope(envelope, command)
    execution_custody.require_enforceable_process_closure(envelope)
    effective_cwd, overlay_paths = admission._execution_source_paths(envelope, cwd=cwd)
    admission._require_external_execution_outputs(
        result_path=result_path, effective_source=effective_cwd
    )
    result: dict[str, object] = {
        "schema": admission.EXECUTION_SCHEMA,
        "run_id": run_id,
        "execution_nonce": execution_nonce,
        "envelope": envelope,
        "phase": "identity",
        "command_started": False,
    }
    custody_session: execution_custody.ExecutionCustodySession | None = None
    try:
        inherited_env = dict(os.environ)
        applied_cargo_policies: tuple[str, ...] = ()
        if "cargo" in envelope.get("toolchains", []):
            inherited_env, applied_cargo_policies = normalize_cargo_environment(
                inherited_env
            )
            # Proof-produced executables require run provenance. Ordinary developer
            # builds keep the persistent target authority, while guarded proofs use
            # a fresh target and retain shared compiler caches outside that tree.
            run_target = (
                result_path.parent / "derived" / execution_nonce / "cargo-target"
            )
            run_target.mkdir(parents=True, exist_ok=False)
            inherited_env["CARGO_TARGET_DIR"] = str(run_target.resolve(strict=True))
        canonical_env = dict(command_identity._CANONICAL_EXECUTION_ENV)
        if "node" in envelope.get("toolchains", []):
            node_hook = (
                Path(execution_custody.__file__)
                .with_name("node_child_custody.cjs")
                .resolve(strict=True)
            )
            canonical_env["NODE_OPTIONS"] = (
                f"--no-global-search-paths --require={node_hook}"
            )
        inherited_env.update(canonical_env)
        execution_env, environment_contract = (
            environment._deterministic_execution_environment(
                inherited_env,
                override_names=[
                    *[str(name) for name in override_names],
                    *sorted(canonical_env),
                ],
            )
        )
        process_closure = envelope.get("process_closure")
        if not isinstance(process_closure, Mapping):
            raise ValueError("proof command envelope has no process closure")
        derived_root_provenance = supervisor._derived_root_provenance(
            descendants=process_closure.get("descendants"),
            env=execution_env,
            source_root=effective_cwd,
            result_path=result_path,
        )
        # Provisioning belongs before the custody snapshot.  No tool may change
        # after its bytes become the authority consumed by the proof command.
        from tools.proof_queue_pkg import policy

        preflight = policy._ensure_run_toolchain_preflight(
            repo_root=cwd, resource_family=str(request["resource_family"])
        )
        if preflight:
            raise ValueError("toolchain preflight failed: " + "; ".join(preflight))
        supervisor_build_env = dict(execution_env)
        # The result custody root is already proven external to the admitted
        # source tree.  It is therefore the single authority for the reusable
        # supervisor build as well; inherited Cargo target state must not move
        # control-plane output back under proof source custody.
        supervisor_target = result_path.parent / "proof-supervisor-target"
        source_root = effective_cwd.resolve(strict=True)
        supervisor_target = Path(os.path.abspath(supervisor_target))
        if supervisor_target == source_root or supervisor_target.is_relative_to(
            source_root
        ):
            raise ValueError("native proof supervisor target overlaps admitted source")
        supervisor_target.mkdir(parents=True, exist_ok=True)
        supervisor_build_env["CARGO_TARGET_DIR"] = str(
            supervisor_target.resolve(strict=True)
        )
        built_supervisor, supervisor_provision_telemetry = (
            supervisor._provision_proof_supervisor(cwd=cwd, env=supervisor_build_env)
        )
        supervisor_binary_artifact = custody_cas.put_file(
            result_path.parent / "custody-cas",
            built_supervisor,
            logical_name=built_supervisor.name,
            executable=True,
        ).as_dict()
        supervisor_binary = Path(str(supervisor_binary_artifact["path"])).resolve(
            strict=True
        )
        environment_fingerprint_key = secrets.token_bytes(32)
        exact = command_identity._exact_command(envelope, cwd=cwd, env=execution_env)
        payload_executable_pre = command_identity._payload_executable_identity(
            envelope, exact
        )
        guarded_exec_pre, delegated_pre = command_identity._bind_delegated_command(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
        )
        executable_pre = command_identity._executable_identity(Path(exact[0]))
        overlay_pre = [command_identity._file_identity(path) for path in overlay_paths]
        pre_identities = [executable_pre, *overlay_pre]
        if payload_executable_pre is not None:
            pre_identities.append(payload_executable_pre)
        if guarded_exec_pre is not None:
            pre_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            pre_identities.append(delegated_pre)
        if not all(
            command_identity._content_identity_available(identity)
            for identity in pre_identities
        ):
            raise ValueError(
                "proof command or overlay input has unavailable content identity"
            )
        pre_source = environment._git_snapshot(effective_cwd, execution_env)
        plan = proof_plan.ProofPlan.load()
        located_roots, policy_identities, location_telemetry = (
            environment._locate_toolchain_watch_roots(
                envelope,
                exact,
                cwd=cwd,
                env=execution_env,
                supervisor_binary=supervisor_binary,
            )
        )
        child_policy = execution_custody.child_policy(envelope, policy_identities)
        python_authority = envelope.get("python")
        python_has_payload = isinstance(python_authority, Mapping) and (
            admission.parse_python_invocation(
                admission._python_invocation_argv(
                    [str(value) for value in envelope["argv"]], python_authority
                )
            ).mode
            != "terminal"
        )
        expected_child_runtime = (
            "python"
            if python_has_payload
            else (
                "node"
                if admission._basename(str(envelope["argv"][0])) in {"node", "node.exe"}
                else None
            )
        )
        child_event_server = execution_custody.ChildCustodyEventServer(
            expected_child_runtime, child_policy
        )
        execution_env[execution_custody.CHILD_POLICY_ENV] = json.dumps(
            child_policy, sort_keys=True, separators=(",", ":")
        )
        execution_env.update(child_event_server.environment())
        passed_names = environment_contract["passed_names"]
        override_names_contract = environment_contract["override_names"]
        assert isinstance(passed_names, list)
        assert isinstance(override_names_contract, list)
        for name in (
            execution_custody.CHILD_POLICY_ENV,
            execution_custody.CHILD_ENDPOINT_ENV,
            execution_custody.CHILD_TOKEN_ENV,
        ):
            if name not in passed_names:
                passed_names.append(name)
            if name not in override_names_contract:
                override_names_contract.append(name)
        passed_names.sort(key=str.casefold)
        override_names_contract.sort(key=str.casefold)
        execution_command, python_launcher_environment = (
            admission._supervised_execution_command(envelope, exact, policy_identities)
        )
        execution_env.update(python_launcher_environment)
        for name in sorted(python_launcher_environment):
            if name not in passed_names:
                passed_names.append(name)
            if name not in override_names_contract:
                override_names_contract.append(name)
        passed_names.sort(key=str.casefold)
        override_names_contract.sort(key=str.casefold)
        environment_executables_pre = (
            environment._execution_environment_executable_identities(
                execution_env, cwd=cwd
            )
        )
        process_closure = envelope.get("process_closure")
        if not isinstance(process_closure, Mapping):
            raise ValueError("proof envelope has no process-closure authority")
        platform_process_images_pre = process_image_capture.platform_auxiliary_images(
            process_closure.get("descendants")
        )
        custody_authority_paths = [
            Path(execution_custody.__file__).resolve(strict=True),
            supervisor_binary,
        ]
        supervisor_source = admission._REPO_ROOT / "tools" / "proof_supervisor"
        custody_authority_paths.extend(
            path.resolve(strict=True)
            for path in (
                supervisor_source / "build.py",
                supervisor_source / "Cargo.toml",
                supervisor_source / "Cargo.lock",
                *sorted((supervisor_source / "src").rglob("*.rs")),
            )
        )
        if python_has_payload:
            custody_authority_paths.append(
                admission._PYTHON_CUSTODY_BOOTSTRAP.resolve(strict=True)
            )
        if "node" in envelope.get("toolchains", []):
            custody_authority_paths.extend(
                Path(execution_custody.__file__).with_name(name).resolve(strict=True)
                for name in (
                    "node_child_custody.cjs",
                    "node_child_custody_worker.cjs",
                )
            )
        custody_authorities_pre = [
            command_identity._file_identity(path)
            for path in dict.fromkeys(custody_authority_paths)
        ]
        if not all(
            command_identity._content_identity_available(identity)
            for identity in custody_authorities_pre
        ):
            raise ValueError("proof custody authority has unavailable content identity")
        tracked_paths = environment._git_tracked_paths(effective_cwd, execution_env)
        source_root_raw = pre_source.get("root")
        if not isinstance(source_root_raw, str):
            raise ValueError("proof source custody has no canonical Git root")
        watch_identities: list[object] = [
            executable_pre,
            *overlay_pre,
            policy_identities,
            environment_executables_pre,
            custody_authorities_pre,
            platform_process_images_pre,
        ]
        if payload_executable_pre is not None:
            watch_identities.append(payload_executable_pre)
        if guarded_exec_pre is not None:
            watch_identities.append(guarded_exec_pre)
        if delegated_pre is not None:
            watch_identities.append(delegated_pre)
        source_root_path = Path(source_root_raw).resolve(strict=True)
        broad_roots = [
            root
            for root in located_roots
            if root != source_root_path and not root.is_relative_to(source_root_path)
        ]
        live_watch_specs = execution_custody.watch_specs(
            source_root=source_root_path,
            tracked_paths=tracked_paths,
            identities=watch_identities,
            broad_roots=broad_roots,
        )
        monitor = execution_custody.LiveCustodyMonitor(live_watch_specs)
        custody_session = execution_custody.ExecutionCustodySession(
            monitor=monitor,
            child_server=child_event_server,
        )
        custody_session.__enter__()

        platform_process_images_armed = process_image_capture.revalidate_images(
            platform_process_images_pre
        )
        if platform_process_images_armed != platform_process_images_pre:
            raise ValueError("platform process-image custody changed while arming")

        # Python's mutable package inventory is captured once after custody is
        # armed. Non-Python selection ran exactly once pre-arm so its complete
        # executable/config closure could itself be watched; only exact-path
        # content revalidation is permitted here.
        pre_source = environment._git_snapshot(effective_cwd, execution_env)
        if pre_source.get("root") != source_root_raw:
            raise ValueError("proof source root changed while live custody armed")
        executable_pre = command_identity._executable_identity(Path(exact[0]))
        payload_executable_pre = command_identity._payload_executable_identity(
            envelope, exact
        )
        overlay_pre = [command_identity._file_identity(path) for path in overlay_paths]
        guarded_exec_pre = (
            command_identity._file_identity(Path(str(guarded_exec_pre["path"])))
            if guarded_exec_pre is not None
            else None
        )
        delegated_pre = (
            command_identity._executable_identity(Path(str(delegated_pre["path"])))
            if delegated_pre is not None
            else None
        )
        _proof_python_full, toolchains_full = environment._capture_toolchains(
            envelope,
            exact,
            cwd=cwd,
            env=execution_env,
            source_root=effective_cwd,
            hash_workers=plan.inventory_hash_workers,
            located_toolchains=policy_identities,
        )
        for name, identity in toolchains_full.items():
            assert isinstance(identity, Mapping)
            command_identity._validate_toolchain_identity(plan, name, identity)
        if execution_custody.child_policy(envelope, toolchains_full) != child_policy:
            raise ValueError("toolchain closure changed while live custody armed")
        toolchains, capture_ref, capture_telemetry = toolchain_capture.publish_capture(
            result_path.parent / "custody-cas", toolchains_full
        )
        frozen = toolchain_capture.frozen_files(toolchains_full)
        uncovered = [
            row.path
            for row in frozen
            if not any(spec.owns(Path(row.path)) for spec in live_watch_specs)
        ]
        if uncovered:
            raise ValueError(
                "toolchain capture contains paths outside armed custody: "
                + ", ".join(uncovered[:3])
            )
        proof_python = toolchains.get("python")
        if proof_python is not None and not isinstance(proof_python, dict):
            raise ValueError("compact Python toolchain summary is malformed")
        supervisor_policy_path = result_path.with_suffix(".supervisor-policy.json")
        supervisor_receipt_path = result_path.with_suffix(".supervisor-receipt.json")
        for supervisor_output in (supervisor_policy_path, supervisor_receipt_path):
            try:
                supervisor_output.unlink()
            except FileNotFoundError:
                pass
        supervisor_policy = supervisor._supervisor_policy(
            envelope=envelope,
            execution_command=execution_command,
            execution_env=execution_env,
            cwd=cwd,
            nonce=execution_nonce,
            toolchains=toolchains,
            environment_executables=environment_executables_pre,
            platform_process_images=platform_process_images_pre,
        )
        supervisor._atomic_json(supervisor_policy_path, supervisor_policy)
        supervisor_policy_identity = command_identity._file_identity(
            supervisor_policy_path
        )
        custody_session.mark_captured()
        del _proof_python_full, toolchains_full, frozen
        environment_executables_pre = (
            environment._execution_environment_executable_identities(
                execution_env, cwd=cwd
            )
        )
        custody_authorities_pre = [
            command_identity._file_identity(path) for path in custody_authority_paths
        ]
        authoritative_pre_identities = [
            executable_pre,
            *overlay_pre,
            *custody_authorities_pre,
        ]
        for optional_identity in (
            payload_executable_pre,
            guarded_exec_pre,
            delegated_pre,
        ):
            if optional_identity is not None:
                authoritative_pre_identities.append(optional_identity)
        if not all(
            command_identity._content_identity_available(identity)
            for identity in authoritative_pre_identities
        ):
            raise ValueError(
                "proof execution input became unavailable after live custody armed"
            )
        python_version = "none"
        if proof_python is not None:
            match = re.match(r"(\d+\.\d+)", str(proof_python["version"]))
            if match is None:
                raise ValueError("proof Python identity has no major.minor version")
            python_version = match.group(1)
        context: dict[str, object] = {
            "schema": plan.receipt_schema,
            "authority_sha256": proof_plan._authority_sha256(plan),
            "run_id": run_id,
            "execution_nonce_sha256": hashlib.sha256(
                execution_nonce.encode()
            ).hexdigest(),
            "source_commit": pre_source.get("commit"),
            "source_tree": pre_source.get("tree"),
            "source_tree_state": "clean" if pre_source.get("clean") else "dirty",
            "environment": {
                "os": proof_plan._normalized_os(),
                "arch": proof_plan._normalized_arch(),
                "python": python_version,
            },
            "toolchains": toolchains,
            "toolchain_custody": {
                "capture_semantic_sha256": capture_ref["semantic_sha256"],
            },
            "toolchain_capture": {
                "schema": "molt.proof-toolchain-custody.v1",
                "artifact": capture_ref,
                "telemetry": {
                    "location": location_telemetry,
                    "capture": capture_telemetry,
                },
            },
            "command_envelope": envelope,
            "command_envelope_sha256": hashlib.sha256(
                json.dumps(envelope, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "exact_command_sha256": hashlib.sha256(
                json.dumps(execution_command, separators=(",", ":")).encode()
            ).hexdigest(),
            "command_executable": {"prelaunch": executable_pre},
            "payload_command_executable": (
                {"prelaunch": payload_executable_pre}
                if payload_executable_pre is not None
                else None
            ),
            "guarded_exec": (
                {"prelaunch": guarded_exec_pre}
                if guarded_exec_pre is not None
                else None
            ),
            "delegated_command_executable": (
                {"prelaunch": delegated_pre} if delegated_pre is not None else None
            ),
            "execution_environment": {
                "prelaunch": environment._execution_environment_authority(
                    execution_env,
                    applied_cargo_policies=applied_cargo_policies,
                    fingerprint_key=environment_fingerprint_key,
                    contract=environment_contract,
                ),
                "executable_inputs": {"prelaunch": environment_executables_pre},
            },
            "platform_process_custody": {
                "schema": process_image_capture.PROCESS_IMAGE_SCHEMA,
                "prelaunch": platform_process_images_pre,
                "prelaunch_sha256": supervisor._canonical_payload_sha256(
                    platform_process_images_pre
                ),
            },
            "python_interpreters": {
                "queue_control_plane": {
                    "executable": sys.executable,
                    "implementation": platform.python_implementation(),
                    "version": platform.python_version(),
                    "role": "queue-runner-and-memory-guard",
                },
                "proof_command": (
                    {
                        **{
                            key: proof_python.get(key)
                            for key in (
                                "executable",
                                "implementation",
                                "version",
                                "identity_sha256",
                            )
                        },
                        "role": "proof-command-envelope",
                    }
                    if proof_python is not None
                    else {"kind": "none", "role": "proof-command-envelope"}
                ),
            },
            "source_custody": {
                "row_cwd": str(cwd.resolve(strict=True)),
                "effective_cwd": str(effective_cwd),
                "prelaunch": pre_source,
                "overlay_inputs": {"prelaunch": overlay_pre},
            },
            "child_process_custody": {
                "policy": child_policy,
                "transport": "parent-owned-authenticated-loopback",
            },
            "derived_root_custody": {
                "prelaunch": derived_root_provenance,
                "policy_roots": [
                    {"role": row["role"], "path": row["path"]}
                    for row in derived_root_provenance
                ],
            },
            "process_supervisor": {
                "schema": "molt.proof-process-supervision.v1",
                "binary": command_identity._file_identity(supervisor_binary),
                "binary_artifact": supervisor_binary_artifact,
                "policy": supervisor_policy_identity,
                "provision_telemetry": supervisor_provision_telemetry,
            },
            "custody_authorities": {"prelaunch": custody_authorities_pre},
            "live_input_custody": {
                "state": custody_session.state,
                "watch_roots": len(live_watch_specs),
            },
        }
        result.update(
            {
                "phase": "command",
                "receipt_context": context,
                "exact_command_sha256": context["exact_command_sha256"],
            }
        )
        supervisor._atomic_json(result_path, result)
        result["command_started"] = True
        stdout_path = result_path.with_suffix(".stdout.bin")
        stderr_path = result_path.with_suffix(".stderr.bin")
        for transcript_path in (stdout_path, stderr_path):
            try:
                transcript_path.unlink()
            except FileNotFoundError:
                pass
        custody_session.mark_running()
        supervisor_started = time.perf_counter()
        with (
            stdout_path.open("xb") as stdout_handle,
            stderr_path.open("xb") as stderr_handle,
        ):
            supervisor_process = admission._COMMANDS.start_owned(
                (
                    str(supervisor_binary),
                    "run",
                    "--policy",
                    str(supervisor_policy_path),
                    "--receipt",
                    str(supervisor_receipt_path),
                ),
                cwd=cwd,
                env=execution_env,
                stdout=stdout_handle,
                stderr=stderr_handle,
            )
            supervisor_timeout = (
                execution_deadline - time.monotonic() - shutdown_reserve
            )
            if supervisor_timeout <= 0:
                raise subprocess.TimeoutExpired(
                    [str(supervisor_binary), "run"], float(timeout_seconds)
                )
            supervisor_returncode = admission._COMMANDS.wait_owned(
                supervisor_process,
                timeout=supervisor_timeout,
                terminate_timeout=shutdown_reserve,
            )
            stdout_handle.flush()
            stderr_handle.flush()
            os.fsync(stdout_handle.fileno())
            os.fsync(stderr_handle.fileno())
        supervisor_run_s = time.perf_counter() - supervisor_started
        supervisor_receipt = supervisor._validated_supervisor_receipt(
            binary=supervisor_binary,
            policy_path=supervisor_policy_path,
            receipt_path=supervisor_receipt_path,
            cwd=cwd,
            env=execution_env,
        )
        supervisor_event_artifact = supervisor._publish_supervisor_event_artifact(
            receipt_path=supervisor_receipt_path,
            receipt=supervisor_receipt,
            cas_root=result_path.parent / "custody-cas",
        )
        root_exit_code = supervisor_receipt.get("root_exit_code")
        if not isinstance(root_exit_code, int):
            root_exit_code = (
                int(supervisor_returncode) if supervisor_returncode != 0 else 2
            )
        completed = subprocess.CompletedProcess(execution_command, int(root_exit_code))
        custody_session.mark_quiescent()
        command_identity._replay_transcript(stdout_path, sys.stdout)
        command_identity._replay_transcript(stderr_path, sys.stderr)
        result["command_returncode"] = int(completed.returncode)
        process_supervisor = context["process_supervisor"]
        assert isinstance(process_supervisor, dict)
        process_supervisor.update(
            {
                "receipt": supervisor_receipt,
                "receipt_file": command_identity._file_identity(
                    supervisor_receipt_path
                ),
                "event_artifact": supervisor_event_artifact,
                "supervisor_returncode": int(supervisor_returncode),
                "run_s": supervisor_run_s,
            }
        )
        transcript = {
            "stdout": command_identity._transcript_identity(stdout_path),
            "stderr": command_identity._transcript_identity(stderr_path),
        }
        transcript["identity_sha256"] = hashlib.sha256(
            json.dumps(transcript, sort_keys=True, separators=(",", ":")).encode()
        ).hexdigest()
        if int(
            completed.returncode
        ) == 0 and command_identity._requires_structured_test_counts(envelope):
            if not any(
                isinstance(value, Mapping)
                and value.get("structured_test_output") is True
                for key, value in transcript.items()
                if key in {"stdout", "stderr"}
            ):
                raise ValueError(
                    "successful test command produced no structured test-count authority"
                )
        context["command_transcript"] = transcript
        custody_session.mark_verifying()
        post_source = environment._git_snapshot(effective_cwd, execution_env)
        overlay_post = [command_identity._file_identity(path) for path in overlay_paths]
        executable_post = command_identity._executable_identity(Path(exact[0]))
        payload_executable_post = command_identity._payload_executable_identity(
            envelope, exact
        )
        environment_executables_post = (
            environment._execution_environment_executable_identities(
                execution_env, cwd=cwd
            )
        )
        guarded_exec_post = (
            command_identity._file_identity(Path(str(guarded_exec_pre["path"])))
            if guarded_exec_pre is not None
            else None
        )
        delegated_post = (
            command_identity._executable_identity(Path(str(delegated_pre["path"])))
            if delegated_pre is not None
            else None
        )
        capture_verification = toolchain_capture.verify_capture(
            capture_ref,
            workers=plan.inventory_hash_workers,
            cas_root=result_path.parent / "custody-cas",
        )
        environment_post = environment._execution_environment_authority(
            execution_env,
            applied_cargo_policies=applied_cargo_policies,
            fingerprint_key=environment_fingerprint_key,
            contract=environment_contract,
        )
        custody_authorities_post = [
            command_identity._file_identity(path) for path in custody_authority_paths
        ]
        platform_process_images_post = process_image_capture.revalidate_images(
            platform_process_images_pre
        )
        custody_session.drain()
        session_receipt = custody_session.receipt()
        live_custody_receipt = session_receipt["live_input_custody"]
        child_custody_receipt = session_receipt["child_process_custody"]
        assert isinstance(live_custody_receipt, dict)
        assert isinstance(child_custody_receipt, dict)
        context["execution_custody_session"] = {
            "schema": session_receipt["schema"],
            "state": session_receipt["state"],
            "lifecycle": session_receipt["lifecycle"],
        }
        context["live_input_custody"] = supervisor._publish_live_custody_receipt(
            live_custody_receipt, cas_root=result_path.parent / "custody-cas"
        )
        child_process_custody = context["child_process_custody"]
        assert isinstance(child_process_custody, dict)
        child_process_custody["receipt"] = child_custody_receipt
        source_identical = pre_source == post_source
        executable_identical = executable_pre == executable_post
        payload_executable_identical = payload_executable_pre == payload_executable_post
        guarded_exec_identical = guarded_exec_pre == guarded_exec_post
        delegated_identical = delegated_pre == delegated_post
        toolchains_identical = capture_verification.get("stable") is True
        environment_pre_container = context["execution_environment"]
        assert isinstance(environment_pre_container, dict)
        environment_pre = environment_pre_container["prelaunch"]
        environment_identical = environment_pre == environment_post
        environment_executables_identical = (
            environment_executables_pre == environment_executables_post
        )
        custody_authorities_identical = (
            custody_authorities_pre == custody_authorities_post
        )
        platform_process_images_identical = (
            platform_process_images_pre == platform_process_images_post
        )
        ineligible_reasons: list[str] = []
        if not pre_source.get("available") or not post_source.get("available"):
            ineligible_reasons.append("source-unavailable")
        if not pre_source.get("clean"):
            ineligible_reasons.append("source-dirty-prelaunch")
        if not post_source.get("clean"):
            ineligible_reasons.append("source-dirty-postcompletion")
        if not source_identical:
            ineligible_reasons.append("source-snapshot-changed")
        if not executable_identical:
            ineligible_reasons.append("command-executable-changed")
        if not command_identity._content_identity_available(executable_post):
            ineligible_reasons.append("command-executable-unavailable-postcompletion")
        if not payload_executable_identical:
            ineligible_reasons.append("payload-command-executable-changed")
        if not guarded_exec_identical:
            ineligible_reasons.append("guarded-exec-changed")
        if not delegated_identical:
            ineligible_reasons.append("delegated-command-executable-changed")
        if not toolchains_identical:
            ineligible_reasons.append("toolchain-frozen-manifest-changed")
        if not environment_identical:
            ineligible_reasons.append("execution-environment-changed")
        if not environment_executables_identical:
            ineligible_reasons.append("execution-environment-executable-changed")
        if not custody_authorities_identical:
            ineligible_reasons.append("execution-custody-authority-changed")
        if not platform_process_images_identical:
            ineligible_reasons.append("platform-process-image-changed")
        if not all(
            command_identity._content_identity_available(identity)
            for identity in custody_authorities_post
        ):
            ineligible_reasons.append("execution-custody-authority-unavailable")
        if live_custody_receipt.get("stable") is not True:
            if live_custody_receipt.get("events"):
                ineligible_reasons.append("transient-input-mutation")
            if live_custody_receipt.get("errors"):
                ineligible_reasons.append("live-input-monitor-incomplete")
        if child_custody_receipt.get("broker_complete") is not True:
            ineligible_reasons.append("child-custody-broker-incomplete")
        if supervisor_receipt.get("complete") is not True:
            ineligible_reasons.append("native-process-supervision-incomplete")
        ineligible_reasons.extend(
            environment._python_editable_ineligible_reasons(
                proof_python,
                source_snapshot=pre_source,
            )
        )
        if overlay_pre != overlay_post:
            ineligible_reasons.append("overlay-input-changed")
        if not all(
            command_identity._content_identity_available(identity)
            for identity in overlay_post
        ):
            ineligible_reasons.append("overlay-input-unavailable-postcompletion")
        eligible = not ineligible_reasons
        source_custody = context["source_custody"]
        assert isinstance(source_custody, dict)
        source_custody.update(
            {
                "postcompletion": post_source,
                "identical": source_identical,
                "evidence_eligible": eligible,
                "ineligible_reasons": ineligible_reasons,
            }
        )
        overlay_inputs = source_custody["overlay_inputs"]
        assert isinstance(overlay_inputs, dict)
        overlay_inputs.update(
            {
                "postcompletion": overlay_post,
                "identical": overlay_pre == overlay_post,
            }
        )
        command_executable = context["command_executable"]
        assert isinstance(command_executable, dict)
        command_executable.update(
            {
                "postcompletion": executable_post,
                "identical": executable_identical,
            }
        )
        if payload_executable_pre is not None:
            payload_executable = context["payload_command_executable"]
            assert isinstance(payload_executable, dict)
            payload_executable.update(
                {
                    "postcompletion": payload_executable_post,
                    "identical": payload_executable_identical,
                }
            )
        if guarded_exec_pre is not None:
            guarded_exec = context["guarded_exec"]
            assert isinstance(guarded_exec, dict)
            guarded_exec.update(
                {
                    "postcompletion": guarded_exec_post,
                    "identical": guarded_exec_identical,
                }
            )
        if delegated_pre is not None:
            delegated_executable = context["delegated_command_executable"]
            assert isinstance(delegated_executable, dict)
            delegated_executable.update(
                {"postcompletion": delegated_post, "identical": delegated_identical}
            )
        toolchain_custody = context["toolchain_custody"]
        assert isinstance(toolchain_custody, dict)
        toolchain_custody.update(
            {
                "verification_identity_sha256": capture_verification.get(
                    "identity_sha256"
                ),
                "identical": toolchains_identical,
            }
        )
        capture_context = context["toolchain_capture"]
        assert isinstance(capture_context, dict)
        capture_context["verification"] = capture_verification
        environment_pre_container.update(
            {
                "postcompletion_identity_sha256": environment_post.get(
                    "identity_sha256"
                ),
                "identical": environment_identical,
            }
        )
        executable_inputs = environment_pre_container["executable_inputs"]
        assert isinstance(executable_inputs, dict)
        executable_inputs.update(
            {
                "postcompletion_sha256": supervisor._canonical_payload_sha256(
                    environment_executables_post
                ),
                "identical": environment_executables_identical,
            }
        )
        custody_authorities = context["custody_authorities"]
        assert isinstance(custody_authorities, dict)
        custody_authorities.update(
            {
                "postcompletion_sha256": supervisor._canonical_payload_sha256(
                    custody_authorities_post
                ),
                "identical": custody_authorities_identical,
            }
        )
        platform_process_custody = context["platform_process_custody"]
        assert isinstance(platform_process_custody, dict)
        platform_process_custody.update(
            {
                "postcompletion_sha256": supervisor._canonical_payload_sha256(
                    platform_process_images_post
                ),
                "identical": platform_process_images_identical,
            }
        )
        telemetry = capture_context["telemetry"]
        assert isinstance(telemetry, dict)
        # Reserve the fixed-width custody digest before measuring so telemetry
        # reports the final serialized context size, not a pre-digest estimate.
        context["execution_custody_sha256"] = "0" * 64
        for _iteration in range(2):
            telemetry["receipt_context_bytes"] = len(
                json.dumps(context, sort_keys=True, separators=(",", ":")).encode()
            )
        if int(telemetry["receipt_context_bytes"]) > 64 * 1024:
            raise ValueError("compact proof receipt context exceeds 64 KiB")
        context["execution_custody_sha256"] = supervisor.execution_custody_sha256(
            context,
            run_id=run_id,
            returncode=int(completed.returncode),
        )
        result["phase"] = "complete"
        supervisor._atomic_json(result_path, result)
        return int(completed.returncode)
    except BaseException as exc:
        if custody_session is not None and custody_session.state != "DRAINED":
            try:
                custody_session.__exit__(type(exc), exc, exc.__traceback__)
            except BaseException as cleanup_exc:
                result["custody_cleanup_error"] = (
                    f"{type(cleanup_exc).__name__}: {cleanup_exc}"
                )
        result.update(
            {
                "phase": "failed",
                "error": f"{type(exc).__name__}: {exc}",
            }
        )
        supervisor._atomic_json(result_path, result)
        print(
            f"proof command envelope failed: {type(exc).__name__}: {exc}",
            file=sys.stderr,
        )
        return 2


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args(argv)
    return execute_guarded_request(Path(args.request))


if __name__ == "__main__":
    raise SystemExit(_main())
