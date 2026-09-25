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
from typing import Mapping, Sequence, cast


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
from molt import file_locks  # noqa: E402
from molt import disk_capacity  # noqa: E402
from molt.exact_json import read_exact  # noqa: E402
from molt.python_environment_identity import python_capture_authority_paths  # noqa: E402
from tools import proof_plan  # noqa: E402
from tools.proof_queue_pkg import (  # noqa: E402
    command_admission as admission,
    command_identity,
    cargo_cache_custody,
    cargo_output_environment,
    cargo_output_layout,
    custody_cas,
    execution_custody,
    execution_environment as environment,
    execution_receipt_details,
    process_image_capture,
    state,
    supervisor_custody as supervisor,
    toolchain_capture,
)


def _run_supervisor_with_transcripts(
    command: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    result_path: Path,
    result: dict[str, object],
    execution_deadline: float,
    timeout_seconds: float,
    shutdown_reserve: float,
) -> int:
    """Publish opened output custody before launch and retain the owned wait."""
    paths = command_identity.execution_transcript_paths(result_path)
    with (
        paths["stdout"].open("xb") as stdout_handle,
        paths["stderr"].open("xb") as stderr_handle,
    ):
        result["live_command_transcript"] = {
            "stdout": command_identity.opened_transcript_identity(
                paths["stdout"], stdout_handle
            ),
            "stderr": command_identity.opened_transcript_identity(
                paths["stderr"], stderr_handle
            ),
        }
        result["phase"] = "command"
        # Only these exclusive opened files may acquire this execution nonce.
        # This is mutable observation custody, not a terminal content receipt.
        supervisor._atomic_json(result_path, result)
        remaining = execution_deadline - time.monotonic() - shutdown_reserve
        if remaining <= 0:
            raise subprocess.TimeoutExpired(command, timeout_seconds)
        process = admission._COMMANDS.start_owned(
            command, cwd=cwd, env=env, stdout=stdout_handle, stderr=stderr_handle
        )
        result["command_started"] = True
        try:
            supervisor._atomic_json(result_path, result)
        finally:
            # Publication errors must not abandon a successfully launched child.
            returncode = admission._COMMANDS.wait_owned(
                process, timeout=remaining, terminate_timeout=shutdown_reserve
            )
        stdout_handle.flush()
        stderr_handle.flush()
        os.fsync(stdout_handle.fileno())
        os.fsync(stderr_handle.fileno())
    return returncode


def _supervisor_build_environment(
    execution_env: Mapping[str, str],
    *,
    target: Path,
    external_placement: bool = False,
) -> dict[str, str]:
    """Keep bootstrap outputs external and Unix startup sockets bounded."""
    build_env = dict(execution_env)
    build_env["CARGO_TARGET_DIR"] = str(target.resolve(strict=True))
    if os.name != "nt" and external_placement:
        wrappers = (
            build_env.get(name, "")
            for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")
        )
        if any(
            Path(wrapper).name in {"sccache", "sccache.exe"} for wrapper in wrappers
        ):
            temporary = build_env.get("TMPDIR", "")
            # Refuse an already-impossible directory prefix. The wrapper owns
            # its filename suffix; do not invent a second socket allocator.
            path_limit = 108 if sys.platform.startswith("linux") else 104
            if temporary and len(os.fsencode(temporary)) + 1 >= path_limit:
                raise ValueError(
                    "declared Cargo output TMPDIR cannot fit the enabled sccache "
                    "POSIX startup socket; select a shorter --cargo-output-root "
                    "or explicitly configure a non-sccache compiler wrapper; "
                    "no temporary-directory fallback is permitted"
                )
    if os.name != "nt" and not external_placement:
        # sccache creates its server-startup notification socket below TMPDIR.
        # A proof run's result-root TMPDIR can exceed sockaddr_un.sun_path even
        # though its Cargo target is valid. /tmp is the established POSIX socket
        # authority; this override is private to supervisor provisioning and
        # deliberately preserves the selected wrappers and SCCACHE_DIR.
        socket_temp_root = Path("/tmp").resolve(strict=True)
        for name in ("TMPDIR", "TMP", "TEMP"):
            build_env[name] = str(socket_temp_root)
    return build_env


LLVM_RELEASE_MANIFEST = "config/llvm_toolchain_releases.toml"


def llvm_family_toolchains(plan: "proof_plan.ProofPlan") -> frozenset[str]:
    """Toolchain policies whose setup evidence is the LLVM release manifest."""
    names: set[str] = set()
    for policy in plan.toolchain_policies:
        evidence = policy.data.get("setup_evidence")
        if isinstance(evidence, list) and any(
            isinstance(item, str) and item.startswith(LLVM_RELEASE_MANIFEST + "::")
            for item in evidence
        ):
            names.add(policy.name)
    return frozenset(names)


TOOL_RELEASES_MANIFEST = "config/tool_releases.toml"


def tool_release_toolchains(plan: "proof_plan.ProofPlan") -> frozenset[str]:
    """Toolchain policies whose setup evidence is the pinned tool-release manifest."""
    names: set[str] = set()
    for policy in plan.toolchain_policies:
        evidence = policy.data.get("setup_evidence")
        if isinstance(evidence, list) and any(
            isinstance(item, str) and item.startswith(TOOL_RELEASES_MANIFEST + "::")
            for item in evidence
        ):
            names.add(policy.name)
    return frozenset(names)


def prefer_tool_release_prefixes(
    env: Mapping[str, str], toolchains: object, *, cwd: Path
) -> tuple[dict[str, str], dict[str, str]]:
    """Provision each declared pinned tool release and put its bin first on PATH.

    A lane that declares a tool the manifest pins (wasm-tools today) must run
    exactly that release, never whatever the ambient PATH happens to carry.
    The release is provisioned under the checkout custody toolchain root
    (idempotent, digest-verified, fail-closed) before toolchains are located,
    so the version policy sees the pinned binary first.
    """
    resolved = dict(env)
    declared = (
        {str(name) for name in toolchains} if isinstance(toolchains, list) else set()
    )
    names = sorted(declared & tool_release_toolchains(proof_plan.ProofPlan.load()))
    if not names:
        return resolved, {}
    from molt import tool_releases
    from molt.dx import checkout_custody

    toolchain_root = checkout_custody(Path(cwd), dict(resolved)).toolchain_root
    prefixes: dict[str, str] = {}
    for name in names:
        release = tool_releases.tool_release(name, state.ROOT)
        discovery = tool_releases.provision_tool(release, toolchain_root)
        prefixes[name] = str(discovery.prefix)
        bin_dir = str(discovery.executable.parent.resolve())
        entries = [
            entry
            for entry in resolved.get("PATH", "").split(os.pathsep)
            if entry and os.path.normcase(entry) != os.path.normcase(bin_dir)
        ]
        resolved["PATH"] = os.pathsep.join([bin_dir, *entries])
    return resolved, prefixes


def prefer_canonical_llvm_prefix(
    env: Mapping[str, str], toolchains: object, *, cwd: Path
) -> tuple[dict[str, str], str | None]:
    """Select declared SDK WASM roles and the independent native LLVM prefix.

    Native toolchains are located through the execution PATH, so an ambient
    system LLVM (a different point release) used to shadow the canonical SDK
    that molt.llvm_toolchain discovers under the checkout custody root and the
    proof then failed closed on the version policy. The discovery authority is
    the same one `molt doctor` reports; without a native SDK, PATH is unchanged
    and the native policy check still fails closed. A declared WASM role must
    independently resolve its SDK entrypoint before capture; lookup never
    provisions either SDK.
    """
    resolved = dict(env)
    declared = (
        {str(name) for name in toolchains} if isinstance(toolchains, list) else set()
    )
    llvm_tools = declared & llvm_family_toolchains(proof_plan.ProofPlan.load())
    if not llvm_tools:
        return resolved, None
    from molt.llvm_toolchain import discover_llvm_toolchain, resolve_wasi_sdk_tool

    if "wasm-ld" in llvm_tools:
        # Select and later attest the real SDK entrypoint. Never copy it into a
        # PATH lane or promote the WebAssembly-only SDK's native-looking tools.
        resolved["MOLT_WASM_LD"] = str(
            resolve_wasi_sdk_tool(state.ROOT, "wasm-ld", environ=resolved)
        )
    if not llvm_tools - {"wasm-ld"}:
        return resolved, None

    discovery = discover_llvm_toolchain(Path(cwd), environ=dict(resolved))
    if discovery is None:
        return resolved, None
    bin_dir = str((Path(discovery.prefix) / "bin").resolve())
    entries = [entry for entry in resolved.get("PATH", "").split(os.pathsep) if entry]
    if not entries or os.path.normcase(entries[0]) != os.path.normcase(bin_dir):
        entries = [
            bin_dir,
            *[e for e in entries if os.path.normcase(e) != os.path.normcase(bin_dir)],
        ]
    resolved["PATH"] = os.pathsep.join(entries)
    return resolved, str(discovery.prefix)


def execute_guarded_request(request_path: Path) -> int:
    """Run identity, preflight, proof, and completion custody under one guard."""
    request = read_exact(
        request_path, max_bytes=16 * 1024 * 1024, label="proof execution request"
    )
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
    effective_cwd = admission._execution_source_paths(envelope, cwd=cwd)
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
    cargo_cache: cargo_cache_custody.CargoCacheLease | None = None
    try:
        inherited_env = dict(os.environ)
        output_layout = cargo_output_layout.CargoOutputLayout.for_envelope(
            envelope, result_root=result_path.parent, source_root=effective_cwd
        )
        if output_layout.declaration is not None:
            output_layout.validate_environment(inherited_env)
            cargo_command = admission._nested_command(command) or command
            environment._require_cargo_build_tool_environment_context(
                cargo_command,
                outputs=cargo_output_environment.CargoOutputEnvironment.for_envelope(
                    envelope
                ),
                cwd=cwd,
                env=inherited_env,
            )
            for name in cargo_output_environment.TEMPORARY_VARIABLE_NAMES:
                inherited_env[name] = str(output_layout.temporary)
            inherited_env["PYTHONPYCACHEPREFIX"] = str(
                output_layout.temporary / "pycache"
            )
        requested_cargo_target = inherited_env.get("CARGO_TARGET_DIR")
        applied_cargo_policies: tuple[str, ...] = ()
        if "cargo" in envelope.get("toolchains", []):
            output_layout.admit_target_path()
            result["disk_capacity_admission"] = disk_capacity.require_build_capacity(
                output_layout.capacity_paths()
                if output_layout.declaration is not None
                else (result_path.parent,),
                env=inherited_env,
            ).as_dict()
            if output_layout.declaration is not None:
                custody_cas._durable_makedirs(output_layout.temporary)
            inherited_env, applied_cargo_policies = normalize_cargo_environment(
                inherited_env
            )
            # A stable selection-only path avoids putting this run's nonce in
            # toolchain discovery. No proof command runs here; after immutable
            # input capture the cache authority selects an exclusively owned,
            # empty or content-verified generation.
            selection_target = output_layout.selection
            custody_cas._canonical_root(selection_target, create=True)
            inherited_env["CARGO_TARGET_DIR"] = str(
                selection_target.resolve(strict=True)
            )
        # Output scratch is run-owned and external to source custody.
        run_scratch = output_layout.scratch(execution_nonce)
        run_scratch.mkdir(parents=True, exist_ok=False)
        inherited_env[supervisor.PROOF_SCRATCH_ROOT_ENV] = str(
            run_scratch.resolve(strict=True)
        )
        inherited_env, _llvm_prefix = prefer_canonical_llvm_prefix(
            inherited_env, envelope.get("toolchains", []), cwd=cwd
        )
        inherited_env, _tool_release_prefixes = prefer_tool_release_prefixes(
            inherited_env, envelope.get("toolchains", []), cwd=cwd
        )
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
        # Provisioning belongs before the custody snapshot.  No tool may change
        # after its bytes become the authority consumed by the proof command.
        from tools.proof_queue_pkg import policy

        preflight = policy._ensure_run_toolchain_preflight(
            repo_root=cwd, resource_family=str(request["resource_family"])
        )
        if preflight:
            raise ValueError("toolchain preflight failed: " + "; ".join(preflight))
        # The result custody root is already proven external to the admitted
        # source tree.  It is therefore the single authority for the reusable
        # supervisor build as well; inherited Cargo target state must not move
        # control-plane output back under proof source custody.
        supervisor_target = output_layout.supervisor_target
        source_root = effective_cwd.resolve(strict=True)
        supervisor_target = Path(os.path.abspath(supervisor_target))
        if supervisor_target == source_root or supervisor_target.is_relative_to(
            source_root
        ):
            raise ValueError("native proof supervisor target overlaps admitted source")
        supervisor_target.mkdir(parents=True, exist_ok=True)
        supervisor_build_env = _supervisor_build_environment(
            execution_env,
            target=supervisor_target,
            external_placement=output_layout.declaration is not None,
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
        supervisor_required_environment = supervisor.required_execution_environment(
            binary=supervisor_binary,
            mode=(
                "leaf"
                if process_closure.get("descendants") == "forbidden"
                else "declared-tree"
            ),
            cwd=cwd,
            env=execution_env,
        )
        execution_env, environment_contract = (
            environment._deterministic_execution_environment(
                inherited_env,
                override_names=[
                    *[str(name) for name in override_names],
                    *sorted(canonical_env),
                ],
                required_environment=supervisor_required_environment,
            )
        )
        execution_env, environment_contract = (
            environment._bind_cargo_build_tool_environment(
                envelope, execution_env, environment_contract, cwd=cwd
            )
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
        pre_identities = [executable_pre]
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
            raise ValueError("proof command input has unavailable content identity")
        pre_source = environment._git_snapshot(effective_cwd, execution_env)
        environment.validate_typed_source_root(envelope, pre_source)
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
        if output_layout.declaration is not None:
            output_layout.validate(protected_roots=located_roots)
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
        passed_names = environment_contract["passed_names"]
        override_names_contract = environment_contract["override_names"]
        assert isinstance(passed_names, list)
        assert isinstance(override_names_contract, list)
        passed_names = cast(list[str], passed_names)
        override_names_contract = cast(list[str], override_names_contract)
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
            Path(cargo_cache_custody.__file__).resolve(strict=True),
            Path(cargo_output_environment.__file__).resolve(strict=True),
            Path(cargo_output_layout.__file__).resolve(strict=True),
            Path(custody_cas.__file__).resolve(strict=True),
            Path(execution_receipt_details.__file__).resolve(strict=True),
            Path(command_identity.__file__).resolve(strict=True),
            Path(environment.__file__).resolve(strict=True),
            Path(file_locks.__file__).resolve(strict=True),
            Path(supervisor.__file__).resolve(strict=True),
            supervisor_binary,
        ]
        if any(name in envelope.get("toolchains", []) for name in ("python", "cargo")):
            custody_authority_paths.extend(python_capture_authority_paths())
        custody_authority_paths.extend(
            supervisor.source_authority_paths(admission._REPO_ROOT)
        )
        if python_has_payload:
            custody_authority_paths.extend(
                (
                    admission._PYTHON_CUSTODY_BOOTSTRAP.resolve(strict=True),
                    Path(execution_custody.__file__)
                    .with_name("python_child_custody.py")
                    .resolve(strict=True),
                )
            )
        if "node" in envelope.get("toolchains", []):
            custody_authority_paths.extend(
                Path(execution_custody.__file__).with_name(name).resolve(strict=True)
                for name in (
                    "node_child_custody.cjs",
                    "node_child_custody_worker.cjs",
                )
            )
        if envelope.get("typed_command") is not None:
            custody_authority_paths.extend(
                (
                    admission._REPO_ROOT
                    / "tools"
                    / "proof_queue_pkg"
                    / "target_derived_toolchains.py",
                    admission._REPO_ROOT
                    / "src"
                    / "molt"
                    / "cli"
                    / "source_extension_invocation.py",
                    admission._REPO_ROOT
                    / "src"
                    / "molt"
                    / "source_extension_link_inputs.py",
                    admission._REPO_ROOT
                    / "src"
                    / "molt"
                    / "cli"
                    / "source_extension_link_inputs.py",
                    admission._REPO_ROOT
                    / "src"
                    / "molt"
                    / "cli"
                    / "source_extension_compiler_inputs.py",
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
            source_root=source_root_path,
            hash_workers=plan.inventory_hash_workers,
            located_toolchains=policy_identities,
        )
        for name, identity in toolchains_full.items():
            assert isinstance(identity, Mapping)
            command_identity._validate_toolchain_identity(plan, name, identity)
        toolchains, capture_ref, capture_telemetry = toolchain_capture.publish_capture(
            result_path.parent / "custody-cas", toolchains_full
        )
        source_content = None
        source_content_telemetry = None
        verify_source_content = None
        if (
            "cargo" in envelope.get("toolchains", [])
            and process_closure.get("descendants") != "forbidden"
        ):
            cargo_outputs = (
                cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
            )
            source_content, source_content_telemetry, verify_source_content = (
                environment.capture_source_content(
                    source_root=Path(source_root_raw),
                    env=execution_env,
                    overlays=(),
                    cas_root=result_path.parent / "custody-cas",
                    hash_workers=plan.inventory_hash_workers,
                )
            )
            cargo_cache = cargo_cache_custody.acquire(
                cargo_output_root=output_layout.declaration,
                cargo_output_lifetime=admission.validated_cargo_output_lifetime(
                    envelope
                ),
                result_root=result_path.parent,
                source_root=Path(source_root_raw),
                toolchains=toolchains_full,
                command=execution_command,
                outputs=cargo_outputs,
                env=execution_env,
                requested_target=requested_cargo_target,
                run_id=run_id,
                execution_nonce_sha256=hashlib.sha256(
                    execution_nonce.encode()
                ).hexdigest(),
                timeout_s=execution_deadline - time.monotonic() - shutdown_reserve,
                source_snapshot=pre_source,
                source_content=source_content,
            )
            result["cargo_cache"] = cargo_cache.provenance
            supervisor._atomic_json(result_path, result)
            execution_env, environment_contract = (
                environment._cargo_output_environment_contract(
                    cargo_cache.environment,
                    environment_contract,
                    outputs=cargo_outputs,
                    target=cargo_cache.target,
                )
            )
            passed_names = cast(list[str], environment_contract["passed_names"])
            override_names_contract = cast(
                list[str], environment_contract["override_names"]
            )
            print(
                "cargo_target_selection="
                + json.dumps(cargo_cache.provenance, sort_keys=True),
                flush=True,
            )
        derived_root_provenance = supervisor._derived_root_provenance(
            descendants=process_closure.get("descendants"),
            env=execution_env,
            source_root=source_root_path,
            result_path=result_path,
            cargo_cache=cargo_cache.provenance if cargo_cache is not None else None,
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
        child_policy = execution_custody.child_policy(envelope, toolchains_full)
        child_event_server = execution_custody.ChildCustodyEventServer(
            expected_child_runtime, child_policy
        )
        execution_env[execution_custody.CHILD_POLICY_ENV] = json.dumps(
            child_policy, sort_keys=True, separators=(",", ":")
        )
        execution_env.update(child_event_server.environment())
        published_custody_names = [
            execution_custody.CHILD_POLICY_ENV,
            execution_custody.CHILD_ENDPOINT_ENV,
            execution_custody.CHILD_TOKEN_ENV,
        ]
        if envelope.get("typed_command") is not None:
            execution_env["MOLT_PROOF_SOURCE_ROOT"] = str(source_root_path)
            published_custody_names.append("MOLT_PROOF_SOURCE_ROOT")
            captured_extension = toolchains_full.get("source-extension")
            if (
                not isinstance(captured_extension, Mapping)
                or "link_inputs" not in captured_extension
            ):
                raise ValueError(
                    "typed producer has no captured source-extension link inputs"
                )
            link_inputs_name = command_identity.SOURCE_EXTENSION_LINK_INPUTS_ENV
            execution_env[link_inputs_name] = json.dumps(
                captured_extension["link_inputs"], sort_keys=True, separators=(",", ":")
            )
            published_custody_names.append(link_inputs_name)
        for name in published_custody_names:
            if name not in passed_names:
                passed_names.append(name)
            if name not in override_names_contract:
                override_names_contract.append(name)
        passed_names.sort(key=str.casefold)
        override_names_contract.sort(key=str.casefold)
        custody_session.bind_child_server(child_event_server)
        if cargo_cache is not None:
            # Check the actual final environment before any command runs. The
            # parent repeats this same check against the sealed native policy.
            cargo_cache_custody.validate_prelaunch(
                cargo_cache.provenance,
                cargo_output_root=output_layout.declaration,
                cargo_output_lifetime=admission.validated_cargo_output_lifetime(
                    envelope
                ),
                cas_root=result_path.parent / "custody-cas",
                command=execution_command,
                outputs=cargo_outputs,
                env=execution_env,
                toolchains=toolchains_full,
                source_root=str(source_root_raw),
                source_snapshot=pre_source,
                source_content=source_content,
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
            toolchains=toolchains_full,
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
                "content": (
                    {"prelaunch": source_content, "telemetry": source_content_telemetry}
                    if source_content is not None
                    else None
                ),
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
                "required_environment": supervisor_required_environment,
            },
            "custody_authorities": {"prelaunch": custody_authorities_pre},
            "live_input_custody": {
                "state": custody_session.state,
                "watch_roots": len(live_watch_specs),
            },
        }
        result.update(
            {
                "receipt_context": context,
                "exact_command_sha256": context["exact_command_sha256"],
            }
        )
        transcript_paths = command_identity.execution_transcript_paths(result_path)
        stdout_path = transcript_paths["stdout"]
        stderr_path = transcript_paths["stderr"]
        for transcript_path in (stdout_path, stderr_path):
            try:
                transcript_path.unlink()
            except FileNotFoundError:
                pass
        custody_session.mark_running()
        supervisor_started = time.perf_counter()
        supervisor_returncode = _run_supervisor_with_transcripts(
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
            result_path=result_path,
            result=result,
            execution_deadline=execution_deadline,
            timeout_seconds=float(timeout_seconds),
            shutdown_reserve=shutdown_reserve,
        )
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
        context["command_transcript"] = transcript
        command_identity.validate_structured_test_counts(
            envelope, transcript, returncode=int(completed.returncode)
        )
        custody_session.mark_verifying()
        post_source = environment._git_snapshot(effective_cwd, execution_env)
        if verify_source_content is not None:
            verify_source_content()
            context["source_custody"]["content"].update(
                {
                    "postcompletion": source_content,
                    "identical": True,
                }
            )
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
        context = execution_receipt_details.compact_context(
            context, cas_root=result_path.parent / "custody-cas"
        )
        result["receipt_context"] = context
        telemetry = capture_context["telemetry"]
        assert isinstance(telemetry, dict)
        # Reserve the fixed-width custody digest before measuring so telemetry
        # reports the final serialized context size, not a pre-digest estimate.
        context["execution_custody_sha256"] = "0" * 64
        for _iteration in range(2):
            telemetry["receipt_context_bytes"] = len(
                json.dumps(context, sort_keys=True, separators=(",", ":")).encode()
            )
        if (
            int(telemetry["receipt_context_bytes"])
            > execution_receipt_details.CONTEXT_LIMIT_BYTES
        ):
            raise ValueError("compact proof receipt context exceeds 64 KiB")
        context["execution_custody_sha256"] = supervisor.execution_custody_sha256(
            context,
            run_id=run_id,
            returncode=int(completed.returncode),
        )
        result["phase"] = "complete"
        supervisor._atomic_json(result_path, result)
        if cargo_cache is not None:
            result["cargo_cache_publication"] = cargo_cache.publish(result)
            supervisor._atomic_json(result_path, result)
        return int(completed.returncode)
    except BaseException as exc:
        if isinstance(exc, toolchain_capture.RustLinkCaptureError):
            try:
                artifact = custody_cas.put_json(
                    result_path.parent / "custody-cas",
                    {
                        "schema": custody_cas.ARTIFACT_SCHEMA,
                        "kind": exc.diagnostic["schema"],
                        "diagnostic": exc.diagnostic,
                    },
                ).as_dict()
                result["rust_link_capture_failure"] = {
                    "unit": exc.diagnostic["unit"],
                    "phase": exc.diagnostic["phase"],
                    "artifact": artifact,
                }
                print(
                    "Rust linker capture diagnostic: " + str(artifact["path"]),
                    file=sys.stderr,
                )
            except Exception as diagnostic_exc:
                # Keep the primary probe failure; failed evidence publication
                # is an additional visible failure, never a successful capture.
                result["rust_link_capture_failure"] = {
                    "unit": exc.diagnostic["unit"],
                    "phase": exc.diagnostic["phase"],
                    "publication_error": f"{type(diagnostic_exc).__name__}: {diagnostic_exc}",
                }
                print(
                    "Rust linker capture diagnostic publication failed: "
                    + str(result["rust_link_capture_failure"]["publication_error"]),
                    file=sys.stderr,
                )
        if isinstance(exc, disk_capacity.DiskCapacityError):
            result["disk_capacity_admission"] = dict(exc.diagnostic)
        if isinstance(exc, cargo_cache_custody.CargoInputClosureUnproven):
            result["cargo_cache_admission"] = exc.diagnostic
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
    finally:
        if cargo_cache is not None:
            try:
                cargo_cache.close()
            except BaseException as exc:
                result["cargo_cache_lifecycle_error"] = f"{type(exc).__name__}: {exc}"
                print(
                    f"Cargo generation closure failed: {type(exc).__name__}: {exc}",
                    file=sys.stderr,
                )
                raise
            finally:
                outcome = cargo_cache.publication_outcome
                if (
                    outcome != result.get("cargo_cache_publication")
                    or "cargo_cache_lifecycle_error" in result
                ):
                    if outcome is not None:
                        result["cargo_cache_publication"] = dict(outcome)
                    supervisor._atomic_json(result_path, result)


def _main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args(argv)
    return execute_guarded_request(Path(args.request))


if __name__ == "__main__":
    raise SystemExit(_main())
