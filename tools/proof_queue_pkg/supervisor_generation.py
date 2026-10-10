"""Cargo-owned supervisor freshness and immutable executable publication.

The retained Cargo directory is a bootstrap tool cache, not payload evidence.
Every selection still runs Cargo. A generation receipt never grants permission
to skip Cargo or admit a warm payload target.
"""

from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
import os
import subprocess
import tomllib
import sys
import time
from typing import Iterator, Mapping

from molt import file_locks, file_publication
from molt.exact_json import canonical_json_sha256, loads_exact
from molt.rust_toolchain import cargo_configuration_paths
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from tools.proof_queue_pkg import cargo_output_layout, custody_cas


GENERATION_SCHEMA = "molt.proof-supervisor-generation.v1"
PROVISION_SCHEMA = "molt.proof-supervisor-provision-telemetry.v2"


TOOL_IDENTITY_REUSE_DIRNAME = "tool-identity"


def _build_inputs(
    env: Mapping[str, str],
    *,
    profile: str = "release",
    target: str | None = None,
    reuse_telemetry: list[dict[str, object]] | None = None,
) -> tuple[dict[str, object], list[StableRegularFileIdentity]]:
    # Import after supervisor_custody's protocol/source authority is initialized.
    from tools import proof_plan
    from tools.proof_queue_pkg import (
        command_admission as admission,
        command_identity,
        execution_environment,
        supervisor_custody,
        toolchain_capture,
    )

    root = admission._REPO_ROOT.resolve(strict=True)
    crate = root / "tools" / "proof_supervisor"
    command = [
        "cargo",
        "build",
        "--locked",
        "--manifest-path",
        str(crate / "Cargo.toml"),
    ]
    if profile == "release":
        command.append("--release")
    elif profile != "debug":
        raise ValueError("unsupported supervisor Cargo profile")
    if target:
        command.extend(("--target", target))
    envelope = admission.envelope_for_command(command)
    execution_environment._require_cargo_build_tool_environment_context(
        envelope,
        outputs=execution_environment.cargo_output_environment.CargoOutputEnvironment.for_envelope(
            envelope
        ),
        cwd=crate,
        env=env,
    )
    command = command_identity._exact_command(envelope, cwd=crate, env=env)
    plan = proof_plan.ProofPlan.load()
    # Probe transcripts are reused beside the shared bootstrap store; every
    # recorded image is rehashed before reuse and Cargo still owns freshness.
    reuse_root = None
    raw_target = env.get("CARGO_TARGET_DIR")
    if raw_target and Path(raw_target).is_absolute():
        reuse_root = Path(raw_target).parent / TOOL_IDENTITY_REUSE_DIRNAME
    tools = {
        name: command_identity._tool_identity(
            plan,
            name,
            envelope,
            command,
            cwd=crate,
            env=env,
            reuse_root=reuse_root,
            reuse_telemetry=reuse_telemetry,
        )
        for name in ("cargo", "rustc")
    }
    configured = execution_environment._execution_environment_executable_identities(
        env, cwd=crate
    )
    # Linker probes have transient paths and timings. Keep their captured image
    # authority, while the existing toolchain capture owns selection semantics.
    tooling = {
        name: {
            key: value
            for key, value in identity.items()
            if key not in {"link_selection", "identity_sha256"}
        }
        for name, identity in tools.items()
    }
    source_paths = supervisor_custody.source_authority_paths(root)
    config_paths = cargo_configuration_paths(crate, env)
    paths = (
        set(source_paths)
        | set(config_paths)
        | {Path(sys.executable).resolve(strict=True)}
    )
    expected = toolchain_capture.frozen_files(
        {"tools": tooling, "configured": configured}
    )
    paths.update(Path(row.path).resolve(strict=True) for row in expected)
    identities = [
        stable_regular_file_identity(path, label="supervisor build input")
        for path in sorted(paths)
    ]
    by_path = {str(item.path): item for item in identities}
    for row in expected:
        actual = by_path[str(Path(row.path).resolve(strict=True))]
        if actual.sha256 != row.sha256 or (
            row.size is not None and actual.size != row.size
        ):
            raise ValueError("supervisor build tool changed during selection")
    inputs = {
        "source_root": str(root),
        "source_paths": [str(path) for path in source_paths],
        "configuration_paths": [str(path) for path in config_paths],
        "profile": profile,
        "target": target or env.get("CARGO_BUILD_TARGET"),
        "command": command,
        "toolchains": tooling,
        "environment_executables": configured,
        # Bind every effective value, including custody transport, without
        # publishing credentials. This digest is evidence, not a cache key.
        "environment_sha256": execution_environment._canonical_environment_sha256(env),
        "entrypoints": [
            {"path": row.path, "content_path": str(Path(row.path).resolve(strict=True))}
            for row in expected
        ],
        "files": [
            {"path": str(item.path), "size_bytes": item.size, "sha256": item.sha256}
            for item in identities
        ],
    }
    inputs["cargo_inputs"] = _cargo_content_inputs(inputs, env)
    return inputs, identities


def _cargo_content_inputs(
    inputs: Mapping[str, object], env: Mapping[str, str]
) -> dict[str, str]:
    """Project compiled identity into Cargo's per-package tracked environment.

    The full caller environment stays in the generation receipt. Transport,
    observers, receipt roots and scheduling do not define compiled output.
    """
    from molt import cargo_workspace

    root = Path(str(inputs["source_root"]))
    source = root / "tools" / "proof_supervisor"
    facts = cargo_workspace.workspace_manifest_facts(source)
    crates = {source.resolve()}
    crates.update(edge.dependency_manifest.parent for edge in facts.dependencies)
    files = {row["path"]: row for row in inputs["files"]}
    configured_names = set()
    for raw_path in inputs["configuration_paths"]:
        document = tomllib.loads(Path(raw_path).read_text(encoding="utf-8"))
        configured = document.get("env", {})
        if not isinstance(configured, dict):
            raise ValueError("Cargo environment configuration must be a table")
        configured_names.update(configured)
    from tools.proof_queue_pkg import command_identity

    environment = command_identity.compile_environment_selection(
        env, configured_names=configured_names
    )
    # Tool capture includes immutable compiler/linker images. Its process-image
    # metadata and probe transport are evidence, not Cargo invalidation inputs.
    from tools.proof_queue_pkg import toolchain_capture

    tool_files = toolchain_capture.frozen_files(
        {
            "tools": inputs["toolchains"],
            "configured": inputs["environment_executables"],
        }
    )
    common = {
        "schema": "molt.proof-supervisor-cargo-input.v1",
        "profile": inputs["profile"],
        "target": inputs["target"],
        "environment": environment,
        "tools": sorted(
            {
                (str(Path(row.path).resolve(strict=True)), row.sha256)
                for row in tool_files
            }
        ),
        "configuration": [
            files[str(Path(path).resolve(strict=True))]
            for path in inputs["configuration_paths"]
        ],
        "manifests": [files[str(path)] for path in facts.input_manifests],
        "lock": files[str((source / "Cargo.lock").resolve(strict=True))],
    }
    result = {}
    for crate in sorted(crates):
        manifest = tomllib.loads((crate / "Cargo.toml").read_text(encoding="utf-8"))
        name = manifest["package"]["name"]
        key = "MOLT_CARGO_INPUT_" + name.upper().replace("-", "_")
        if key in result:
            raise ValueError("supervisor local packages alias one Cargo input key")
        paths = {
            p.resolve(strict=True) for p in (crate / "src").rglob("*") if p.is_file()
        }
        build_script = crate / "build.rs"
        if build_script.is_file():
            paths.add(build_script.resolve(strict=True))
        if crate == source.resolve():
            paths.add((source / "protocol.json").resolve(strict=True))
        result[key] = canonical_json_sha256(
            {
                **common,
                "package": name,
                "sources": [files[str(path)] for path in sorted(paths)],
            }
        )
    return result


def build_cargo(
    *, inputs: Mapping[str, object], env: Mapping[str, str]
) -> tuple[Path, subprocess.CompletedProcess[str]]:
    """The direct driver and immutable producer share one Cargo invocation."""
    from molt.disk_capacity import require_build_capacity
    from tools.proof_queue_pkg import command_identity

    crate = Path(str(inputs["source_root"])) / "tools" / "proof_supervisor"
    target_root = Path(env.get("CARGO_TARGET_DIR", "target"))
    if not target_root.is_absolute():
        target_root = crate / target_root
    target_root = file_publication.resolve_owned_path(target_root)
    output_roots = [target_root]
    if build_dir := env.get("CARGO_BUILD_BUILD_DIR"):
        build_root = Path(build_dir)
        output_roots.append(
            build_root if build_root.is_absolute() else crate / build_root
        )
    require_build_capacity(output_roots, env=env)
    command = [
        *inputs["command"],
        "--message-format=json-render-diagnostics",
        "--target-dir",
        str(target_root),
    ]
    build_env = {**env, **inputs["cargo_inputs"]}
    completed = command_identity._run_captured(
        command, cwd=crate, env=build_env, timeout=600.0
    )
    selected_target = inputs.get("target")
    if selected_target:
        target_root /= str(selected_target)
    windows = "windows" in str(selected_target) if selected_target else os.name == "nt"
    name = "molt-proof-supervisor.exe" if windows else "molt-proof-supervisor"
    binary = target_root / str(inputs["profile"]) / name
    if completed.returncode == 0 and not binary.is_file():
        raise ValueError(
            f"Cargo succeeded without expected supervisor binary: {binary}"
        )
    return binary, completed


def _verify_inputs(
    inputs: Mapping[str, object],
    identities: list[StableRegularFileIdentity],
    env: Mapping[str, str],
) -> None:
    from tools.proof_queue_pkg import supervisor_custody

    root = Path(str(inputs["source_root"]))
    if inputs["source_paths"] != [
        str(path) for path in supervisor_custody.source_authority_paths(root)
    ]:
        raise ValueError("supervisor source membership changed during provisioning")
    if inputs["configuration_paths"] != [
        str(path)
        for path in cargo_configuration_paths(root / "tools" / "proof_supervisor", env)
    ]:
        raise ValueError("supervisor Cargo configuration changed during provisioning")
    for row in inputs["entrypoints"]:
        if str(Path(row["path"]).resolve(strict=True)) != row["content_path"]:
            raise ValueError(
                "supervisor build tool entrypoint changed during provisioning"
            )
    for identity in identities:
        verify_stable_regular_file_identity(identity, label="supervisor build input")


@contextmanager
def _provision_guard_scope(env: Mapping[str, str]) -> Iterator[None]:
    """One existing repo observer for this complete bootstrap transaction.

    This scope suppresses duplicate automatic repo observers only. Every child
    still enters CommandExecutor and memory_guard.run_guarded with its own birth,
    resource, descendant, temporary-output and terminal custody.
    """
    from tools import harness_memory_guard
    from tools.proof_queue_pkg import command_admission as admission

    context = harness_memory_guard.HarnessExecutionContext.from_env(
        admission._COMMANDS.prefix, env, repo_root=admission._REPO_ROOT
    )
    sentinel = context.start_repo_sentinel(
        label="proof_supervisor_provision",
        # Direct commands retain their own cleanup authority, exactly as in
        # _auto_repo_sentinel. Never turn setup observation into a broad drain.
        drain_on_exit=False,
    )
    try:
        yield
    finally:
        if sentinel is not None:
            sentinel.__exit__(*sys.exc_info())


def provision(*, cwd: Path, env: Mapping[str, str]) -> tuple[Path, dict[str, object]]:
    target = Path(env["CARGO_TARGET_DIR"])
    if not target.is_absolute():
        raise ValueError(
            "native proof supervisor requires an explicit absolute Cargo target"
        )
    target = file_publication.resolve_owned_path(target)
    cargo_output_layout.CargoOutputLayout.admit_cargo_path(target)
    store = target.parent
    cas_root = store / "custody-cas"
    name = (
        "molt-proof-supervisor.exe"
        if sys.platform == "win32"
        else "molt-proof-supervisor"
    )
    custody_cas.admit_executable_path(cas_root, name)
    custody_cas._canonical_root(store, create=True)
    lock_path = store / "provision.lock"
    started = time.perf_counter()
    lock = file_locks._acquire_file_lock(
        lock_path,
        timeout_s=600.0,
        timeout_message="native proof supervisor provisioning lock timed out",
    )
    try:
        with _provision_guard_scope(env):
            # The lock covers input capture, Cargo and the immutable copy. No
            # process can overwrite the shared Cargo output during publication.
            reuse_telemetry: list[dict[str, object]] = []
            inputs, identities = _build_inputs(env, reuse_telemetry=reuse_telemetry)
            binary, completed = build_cargo(inputs=inputs, env=env)
            if completed.returncode != 0:
                raise ValueError(
                    "native proof supervisor provisioning failed: "
                    + (completed.stderr.strip() or completed.stdout.strip())
                )
            lines = [
                line.strip() for line in completed.stdout.splitlines() if line.strip()
            ]
            binary = file_publication.resolve_owned_path(binary)
            if binary != target / "release" / name or not binary.is_file():
                raise ValueError(
                    "native proof supervisor build escaped its admitted target"
                )
            with file_locks._file_lock_owned_operation(
                lock, expected_lock_path=lock_path
            ):
                _verify_inputs(inputs, identities, env)
                image_identity = stable_regular_file_identity(
                    binary, label="supervisor build output"
                )
                reference = custody_cas.put_file(
                    cas_root, binary, logical_name=name, executable=True
                ).as_dict()
                verify_stable_regular_file_identity(
                    image_identity, label="supervisor build output"
                )
                if (
                    reference["sha256"] != image_identity.sha256
                    or reference["size_bytes"] != image_identity.size
                ):
                    raise ValueError(
                        "native proof supervisor output changed during publication"
                    )
                _verify_inputs(inputs, identities, env)
                generation = {
                    "schema": custody_cas.ARTIFACT_SCHEMA,
                    "kind": GENERATION_SCHEMA,
                    "inputs": inputs,
                    "input_sha256": canonical_json_sha256(inputs),
                    "build_target_dir": str(target),
                    "binary": {
                        "sha256": reference["sha256"],
                        "size_bytes": reference["size_bytes"],
                        "name": name,
                    },
                    "freshness_authority": "cargo-build-locked",
                }
                generation_ref = custody_cas.put_json(cas_root, generation).as_dict()
                fresh = compiled = 0
                for line in lines:
                    if not line.startswith("{"):
                        continue
                    message = loads_exact(line)
                    if (
                        isinstance(message, dict)
                        and message.get("reason") == "compiler-artifact"
                    ):
                        if message.get("fresh") is True:
                            fresh += 1
                        else:
                            compiled += 1
                return Path(str(reference["path"])), {
                    "schema": PROVISION_SCHEMA,
                    "build_s": time.perf_counter() - started,
                    "build_target_dir": str(target),
                    "build_output_sha256": reference["sha256"],
                    "build_output_size_bytes": reference["size_bytes"],
                    "cargo_fresh_artifact_count": fresh,
                    "cargo_compiled_artifact_count": compiled,
                    "generation_artifact": generation_ref,
                    "tool_identity_reuse": reuse_telemetry,
                }
    finally:
        file_locks._release_file_lock(lock)


def read_generation(
    telemetry: Mapping[str, object],
) -> dict[str, object]:
    """Read the provisioner's bounded, content-addressed generation record."""
    raw = telemetry.get("generation_artifact")
    if not isinstance(raw, Mapping):
        raise ValueError("supervisor provisioning has no immutable generation")
    shared_cas = Path(str(telemetry["build_target_dir"])).parent / "custody-cas"
    return custody_cas.read_ref(raw, expected_root=shared_cas)


def publish_receipt(
    telemetry: Mapping[str, object], *, cas_root: Path
) -> dict[str, object]:
    """Copy the immutable generation record into this execution's evidence."""
    generation = read_generation(telemetry)
    return {
        **telemetry,
        "generation_artifact": custody_cas.put_json(cas_root, generation).as_dict(),
    }


def validate_receipt(
    telemetry: object,
    *,
    binary: Mapping[str, object],
    cas_root: Path,
    expected_target: Path,
) -> None:
    """Check the copied generation without depending on a retained build cache."""
    if (
        not isinstance(telemetry, Mapping)
        or telemetry.get("schema") != PROVISION_SCHEMA
    ):
        raise ValueError("supervisor provisioning has no supported generation receipt")
    reference = telemetry.get("generation_artifact")
    if not isinstance(reference, Mapping):
        raise ValueError("supervisor provisioning has no immutable generation")
    generation = custody_cas.read_ref(reference, expected_root=cas_root)
    inputs = generation.get("inputs")
    output = generation.get("binary")
    if (
        generation.get("schema") != custody_cas.ARTIFACT_SCHEMA
        or generation.get("kind") != GENERATION_SCHEMA
        or not isinstance(inputs, Mapping)
        or inputs.get("profile") != "release"
        or generation.get("input_sha256") != canonical_json_sha256(inputs)
        or generation.get("freshness_authority") != "cargo-build-locked"
        or generation.get("build_target_dir") != str(expected_target)
        or telemetry.get("build_target_dir") != str(expected_target)
        or not isinstance(output, Mapping)
        or output.get("name") != Path(str(binary.get("path"))).name
        or output.get("sha256") != binary.get("sha256")
        or output.get("size_bytes") != binary.get("size_bytes")
        or telemetry.get("build_output_sha256") != binary.get("sha256")
        or telemetry.get("build_output_size_bytes") != binary.get("size_bytes")
    ):
        raise ValueError(
            "supervisor generation differs from admitted build or executable"
        )
