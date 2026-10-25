"""Deterministic execution-environment and source-custody authority."""

from __future__ import annotations

import hashlib
import hmac
from functools import partial
import json
import os
from pathlib import Path
import re
import stat
import sys
import time
import tomllib
from typing import Callable, Mapping, Sequence, cast

from molt.exact_json import canonical_json_sha256
from molt.cargo_execution_policy import (
    cargo_native_dependency_environment_name,
    validate_cargo_native_dependency_environment,
)
from molt import file_publication
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from molt.toolchain_identity import resolve_explicit_tool_command
from molt.rust_toolchain import cargo_config_arguments
from molt.python_environment_identity import (
    PythonEnvironmentIdentityError,
    python_identity_verifier_prefix,
    validate_python_environment_location,
)
from molt.python_runtime_identity import python_runtime_selection_locations
from tools import proof_plan
from tools.git_identity import repository_source_paths
from tools.proof_queue_pkg import command_admission as admission
from tools.proof_queue_pkg import cargo_output_environment
from tools.proof_queue_pkg import command_identity
from tools.proof_queue_pkg import custody_cas
from tools.proof_queue_pkg import process_image_capture
from tools.proof_queue_pkg import supervisor_custody
from tools.proof_queue_pkg import toolchain_capture


def command_secret_policy_error(command: Sequence[str]) -> str | None:
    for index, value in enumerate(command):
        if re.search(r"://[^/@\s]+@", value):
            return f"command argument {index} embeds URL credentials"
        if command_identity._SECRET_ARGUMENT_FLAG.match(value):
            return (
                f"secret-bearing command option {value.split('=', 1)[0]!r} is forbidden"
            )
    return None


def _environment_name_class(name: str, *, rule: NameRule) -> str | None:
    key = rule.key(name)
    if key in command_identity._QUEUE_CUSTODY_ENV_NAMES - {"PYTHONPATH"}:
        return "queue-owned-custody"
    if key in command_identity._NONDETERMINISTIC_ENV_NAMES:
        return "denied-nondeterministic"
    if key in command_identity._ENVIRONMENT_EXACT_NAMES:
        return "host-runtime"
    if (
        key in command_identity._ENVIRONMENT_BUILD_NAMES
        or command_identity.llvm_toolchain_environment_name(key)
        or command_identity._runtime_c_environment_name(key)
        or cargo_native_dependency_environment_name(key)
    ):
        return "build-toolchain"
    if any(key.startswith(prefix) for prefix in command_identity._ENVIRONMENT_PREFIXES):
        return "semantic-prefix"
    return None


def environment_override_policy_error(env_overrides: Mapping[str, str]) -> str | None:
    # Overrides are proof-plan declarations that run on every host, so the
    # judgment cannot depend on the host: `Foo` and `FOO` are one variable on
    # Windows and are refused everywhere, and a name is classified only in
    # its exact canonical spelling (POSIX tools read no other).
    portable = NameRule.WINDOWS
    seen: dict[str, str] = {}
    for name, value in sorted(env_overrides.items()):
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name) is None:
            return f"non-canonical environment override name {name!r}"
        folded = portable.key(name)
        if folded in seen:
            return (
                "case-ambiguous environment overrides are forbidden: "
                f"{seen[folded]!r}, {name!r}"
            )
        seen[folded] = name
        if (
            folded in command_identity.CANONICAL_EXECUTION_ENV
            or folded == "NODE_OPTIONS"
            or folded in command_identity._QUEUE_CUSTODY_ENV_NAMES
        ):
            return f"queue-owned canonical environment override {name!r} is forbidden"
        classification = _environment_name_class(name, rule=NameRule.EXACT)
        if classification is None:
            return f"unclassified environment override {name!r}"
        if classification == "denied-nondeterministic":
            return f"nondeterministic environment override {name!r} is forbidden"
        if command_identity._SECRET_ENV_NAME.search(name):
            return f"secret-bearing environment override {name!r} is forbidden"
        if "\x00" in value or "\n" in value or "\r" in value:
            return f"environment override {name!r} has non-canonical control characters"
        if re.search(r"://[^/@\s]+@", value):
            return f"environment override {name!r} embeds URL credentials"
    return None


def _wrapper_execution_environment(
    envelope: Mapping[str, object], inherited: Mapping[str, str]
) -> ProcessEnvironment:
    """Bind the modeled wrapper before interpreter capture or custody starts."""
    inherited = ProcessEnvironment(inherited)
    wrapper = envelope.get("wrapper")
    if wrapper is None:
        return inherited
    if not isinstance(wrapper, Mapping):
        raise ValueError("proof wrapper descriptor is malformed")
    from tools import venv_exec, uv_project_env

    if wrapper.get("kind") == "venv":
        venv = venv_exec.resolve_venv(env=inherited, explicit=wrapper.get("venv"))
        return venv_exec.venv_env(venv=venv, env=inherited)
    if wrapper.get("kind") == "uv-project":
        return uv_project_env.uv_project_env(
            python=str(wrapper["python"]),
            purpose=str(wrapper["purpose"]),
            explicit=wrapper.get("venv"),
            env=inherited,
        )
    raise ValueError("proof wrapper descriptor has an unknown kind")


def _deterministic_execution_environment(
    inherited: Mapping[str, str],
    *,
    override_names: Sequence[str],
    required_environment: Mapping[str, str] | None = None,
) -> tuple[ProcessEnvironment, dict[str, object]]:
    rule = ProcessEnvironment(inherited).rule
    required = ProcessEnvironment(required_environment or {}, rule=rule)
    # Declared override names are judged under the Windows rule on every host,
    # as in environment_override_policy_error; the child keeps the host rule.
    portable = NameRule.WINDOWS
    override_keys = [portable.key(name) for name in override_names]
    if len(override_keys) != len(set(override_keys)):
        raise ValueError("environment overrides contain case-ambiguous names")
    overrides = {rule.key(name) for name in override_names}
    portable_required = ProcessEnvironment(required, rule=portable)
    if any(name in portable_required for name in override_names):
        raise ValueError("native supervisor launch environment cannot be overridden")
    inherited = supervisor_custody.bind_required_environment(inherited, required)
    selected = ProcessEnvironmentBuilder({}, rule=rule)
    omitted: list[str] = []
    for name, value in inherited.items():
        folded = rule.key(name)
        if folded in {
            "MOLT_PROOF_SOURCE_ROOT",
            command_identity.SOURCE_EXTENSION_LINK_INPUTS_ENV,
        }:
            # Only captured source/toolchain authority may publish these values.
            omitted.append(name)
            continue
        classification = (
            "supervisor-owned-launch"
            if name in required
            else _environment_name_class(name, rule=rule)
        )
        if classification in {
            None,
            "denied-nondeterministic",
        } or (
            classification != "queue-owned-custody"
            and command_identity._SECRET_ENV_NAME.search(name)
        ):
            omitted.append(name)
            continue
        selected[name] = str(value)
    missing = sorted(name for name in override_names if name not in selected)
    if missing:
        raise ValueError(
            "classified environment overrides disappeared: " + ", ".join(missing)
        )
    contract: dict[str, object] = {
        "schema": "molt.proof-execution-environment.v1",
        "passed_names": sorted(selected, key=str.casefold),
        "override_names": sorted(
            (name for name in selected if rule.key(name) in overrides),
            key=str.casefold,
        ),
        "omitted_names": sorted(omitted, key=str.casefold),
    }
    if required:
        contract["supervisor_owned_names"] = sorted(required, key=str.casefold)
    return ProcessEnvironment(selected), contract


def _execution_environment_authority(
    env: Mapping[str, str],
    *,
    applied_cargo_policies: Sequence[str],
    fingerprint_key: bytes,
    contract: Mapping[str, object],
) -> dict[str, object]:
    env = ProcessEnvironment(env)
    names = sorted(env, key=str.casefold)
    supervisor_owned_names = contract.get("supervisor_owned_names", [])
    if not isinstance(supervisor_owned_names, list) or not all(
        isinstance(name, str) for name in supervisor_owned_names
    ):
        raise ValueError("supervisor-owned environment names must be strings")
    values: dict[str, object] = {}
    for name in names:
        normalized = str(env[name]).replace("\\", "/")
        values[name] = {
            "class": (
                "supervisor-owned-launch"
                if name in supervisor_owned_names
                else "queue-owned-custody"
                if env.rule.key(name) == "PYTHONPATH"
                else _environment_name_class(name, rule=env.rule)
            ),
            "fingerprint": hmac.new(
                fingerprint_key,
                f"{env.rule.key(name)}\0{normalized}".encode(),
                hashlib.sha256,
            ).hexdigest(),
            "redacted": True,
        }
    payload: dict[str, object] = {
        **dict(contract),
        "variables": values,
        "cargo_policies": list(applied_cargo_policies),
        "fingerprint_key_id": hashlib.sha256(fingerprint_key).hexdigest(),
        "canonical_values_sha256": _canonical_environment_sha256(env),
    }
    payload["identity_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True).encode()
    ).hexdigest()
    return payload


def _bind_cargo_build_tool_environment(
    envelope: Mapping[str, object],
    env: Mapping[str, str],
    contract: Mapping[str, object],
    *,
    cwd: Path,
) -> tuple[ProcessEnvironment, dict[str, object]]:
    """Publish Cargo build-tool selection before any executable/input capture."""
    selected = ProcessEnvironmentBuilder(env)
    bound_contract = dict(contract)
    closure = envelope.get("process_closure")
    if "cargo" not in envelope.get("toolchains", []) or (
        isinstance(closure, Mapping) and closure.get("descendants") == "forbidden"
    ):
        return ProcessEnvironment(selected), bound_contract
    outer_command = [str(value) for value in envelope["argv"]]
    command = admission._nested_command(outer_command) or outer_command
    invocation = admission.cargo_invocation_for_envelope(envelope)
    if invocation is not None:
        # Cargo forwards CARGO to build scripts and external subcommands. An
        # explicit typed payload therefore owns this selector too, even when
        # the inherited environment points to another installation.
        # Resolve in the outer Python/uv context; Cargo configuration below
        # consumes only the nested payload.
        cargo = command_identity._cargo_executable_path(
            envelope, outer_command, cwd=cwd, env=selected, token=command[0]
        )
        selected.pop("CARGO", None)
        selected["CARGO"] = str(cargo)
    if invocation is not None and invocation.toolchain_selector is not None:
        selected.pop("RUSTUP_TOOLCHAIN", None)
        # Rustup's leading +selector overrides inherited RUSTUP_TOOLCHAIN.
        # Publish that context before every Rust/formatter/toolchain capture,
        # not merely to the build script's formatter selection.
        selected["RUSTUP_TOOLCHAIN"] = invocation.toolchain_selector
    outputs = cargo_output_environment.CargoOutputEnvironment.for_envelope(envelope)
    configured = _require_cargo_build_tool_environment_context(
        envelope,
        outputs=outputs,
        cwd=cwd,
        env=selected,
        native_required=bool(envelope.get("cargo_native_units")),
    )
    # Publish effective dependency inputs before identity/reuse capture. This
    # is the same Cargo [env] resolver the staged runtime build uses; binding
    # a default only after capture would attest a different compilation input.
    for name in list(selected):
        if cargo_native_dependency_environment_name(selected.rule.key(name)):
            del selected[name]
    selected.update(
        (name, value)
        for name, value in configured.items()
        if cargo_native_dependency_environment_name(selected.rule.key(name))
    )
    updates, selection = toolchain_capture.select_cargo_build_tool_environment(
        cwd=cwd,
        env=selected,
        rustdoc_required=invocation is None or invocation.requires_documenter,
    )
    for name in updates:
        selected.pop(name, None)
    selected.update(updates)
    bound_contract["passed_names"] = sorted(selected, key=str.casefold)
    overrides = contract.get("override_names", [])
    if not isinstance(overrides, list):
        raise ValueError("environment override-name contract is malformed")
    override_keys = {selected.rule.key(str(name)) for name in overrides}
    bound_contract["override_names"] = sorted(
        (name for name in selected if selected.rule.key(name) in override_keys),
        key=str.casefold,
    )
    bound_contract["build_tool_selection"] = selection
    return ProcessEnvironment(selected), bound_contract


def _cargo_output_environment_contract(
    env: Mapping[str, str],
    contract: Mapping[str, object],
    *,
    outputs: cargo_output_environment.CargoOutputEnvironment,
    target: Path,
) -> tuple[ProcessEnvironment, dict[str, object]]:
    """Project the lease's already-bound output environment into final custody."""
    selected, bound_contract = ProcessEnvironment(env), dict(contract)
    names = set(outputs.names)
    supervisor_names = contract.get("supervisor_owned_names", [])
    if not isinstance(supervisor_names, list) or not all(
        isinstance(name, str) for name in supervisor_names
    ):
        raise ValueError("supervisor-owned environment names must be strings")
    if any(outputs.owns(name) for name in supervisor_names):
        raise ValueError("Cargo output conflicts with supervisor-owned environment")
    outputs.validate(selected, target=target)
    bound_contract["passed_names"] = sorted(selected, key=str.casefold)
    bound_contract["override_names"] = sorted(
        {
            str(name)
            for name in contract.get("override_names", [])
            if not outputs.owns(str(name))
        }
        | names,
        key=str.casefold,
    )
    bound_contract["cargo_output_environment"] = outputs.identity()
    return selected, bound_contract


def _require_cargo_build_tool_environment_context(
    envelope: Mapping[str, object],
    *,
    outputs: cargo_output_environment.CargoOutputEnvironment,
    cwd: Path,
    env: Mapping[str, str],
    native_required: bool = False,
) -> Mapping[str, str]:
    """Capture effective Cargo inputs and reject unowned tool/output selection.

    Config custody already owns the file set. This check only detects a
    supported driver hook whose effective Cargo value we cannot obtain yet.
    The runtime resolver owns configuration precedence; its result also feeds
    the caller's native-dependency binding without a second configuration read.
    """
    env = ProcessEnvironment(env)
    if outputs.external_placement and "CARGO_BUILD_TARGET_DIR" in env:
        raise ValueError(
            "Cargo secondary build-dir/target-dir environment bypasses declared output placement"
        )
    invocation = admission.cargo_invocation_for_envelope(envelope)
    delegated = envelope.get("delegated")
    owner = delegated if isinstance(delegated, Mapping) else envelope
    # Python drivers declare Cargo children, but their own argv is not Cargo
    # configuration or target syntax. Preserve the admitted role before binding.
    command = (
        [str(value) for value in owner["argv"]] if invocation is not None else ["cargo"]
    )
    from molt.cli.runtime_cargo_plan import capture_cargo_configuration_inputs

    captured = capture_cargo_configuration_inputs(cwd, env=env, cargo_command=command)
    outputs.validate_command_configuration(captured[7])
    arguments = cargo_config_arguments(command, cwd=cwd)[1::2]
    definitions: list[tuple[str, object]] = []
    for row in command_identity._tool_configuration_identities(
        "cargo", cwd=cwd, env=env, command_argv=command
    ):
        path = Path(str(row["path"]))
        definitions.append((str(path), tomllib.loads(path.read_text(encoding="utf-8"))))
    for index, value in enumerate(arguments, start=1):
        if "=" in value:
            definitions.append((f"Cargo --config entry {index}", tomllib.loads(value)))
    inherited_keys = {env.rule.key(name) for name in env}
    protected_environment = {
        "CARGO",
        "RUSTC",
        "CARGO_BUILD_RUSTC",
        "CLANG_PATH",
        "LLVM_CONFIG_PATH",
        "RUSTFMT",
        "RUSTDOC",
        "CARGO_BUILD_RUSTDOC",
        "CARGO",
        "RUSTC",
        "CARGO_BUILD_RUSTC",
        "PATH",
        *cargo_output_environment.TEMPORARY_VARIABLE_NAMES,
        *outputs.names,
        *(
            {"CARGO_BUILD_BUILD_DIR", "CARGO_BUILD_TARGET_DIR"}
            if outputs.external_placement
            else set()
        ),
    }
    if invocation is not None and invocation.subcommand == "clippy":
        # cargo-clippy supplies these two values to nested Cargo. Its [env]
        # defaults cannot replace them, but force=true reaches compiler children.
        # SYSROOT and CLIPPY_CONF_DIR have no admitted external resource owner.
        protected_environment.update(
            {"SYSROOT", "CLIPPY_CONF_DIR", "CLIPPY_ARGS", "RUSTC_WORKSPACE_WRAPPER"}
        )
        inherited_keys.update({"CLIPPY_ARGS", "RUSTC_WORKSPACE_WRAPPER"})
    for origin, payload in definitions:
        build = payload.get("build") if isinstance(payload, Mapping) else None
        if (
            outputs.external_placement
            and isinstance(build, Mapping)
            and "artifact-dir" in build
        ):
            raise ValueError(
                f"Cargo {origin} redirects output outside declared placement"
            )
        for role in ("rustc", "rustdoc"):
            if (
                isinstance(build, Mapping)
                and role in build
                and not env.get(role.upper())
            ):
                raise ValueError(
                    f"unsupported Cargo build-tool environment context: {origin} defines build.{role}; "
                    f"select the tool through explicit {role.upper()} before capture"
                )
        if (
            native_required
            and isinstance(build, Mapping)
            and "target" in build
            and command_identity._rust_target(envelope, env, cwd=cwd) is None
        ):
            raise ValueError(
                "native build selection requires explicit Cargo target before capture; unresolved build.target"
            )
        if isinstance(build, Mapping) and any(
            key in build for key in ("rustc-wrapper", "rustc-workspace-wrapper")
        ):
            for key, variable in (
                ("rustc-wrapper", "RUSTC_WRAPPER"),
                ("rustc-workspace-wrapper", "RUSTC_WORKSPACE_WRAPPER"),
            ):
                if (
                    key in build
                    and variable not in env
                    and not (variable == "RUSTC" and "CARGO_BUILD_RUSTC" in env)
                ):
                    raise ValueError(
                        f"unsupported Cargo build-tool environment context: {origin} defines build.{key}; select {variable} before capture"
                    )
        environment = payload.get("env") if isinstance(payload, Mapping) else None
        if not isinstance(environment, Mapping):
            continue
        for name, value in environment.items():
            key = env.rule.key(str(name))
            if (
                key not in protected_environment
                and not command_identity.llvm_toolchain_environment_name(key)
                and not (
                    native_required
                    and command_identity._runtime_c_environment_name(key)
                )
            ):
                continue
            if key in inherited_keys and not (
                isinstance(value, Mapping) and value.get("force") is True
            ):
                continue
            raise ValueError(
                f"unsupported Cargo build-tool environment context: {origin} defines env.{name}; "
                "the effective driver requires Cargo-owned build-script environment selection "
                "before capture, and must not be replaced by an inherited toolchain choice"
            )
    return captured[1]


def _canonical_environment_sha256(env: Mapping[str, str]) -> str:
    """Bind every admitted environment value without publishing those values."""
    canonical = {name: str(env[name]) for name in sorted(env, key=str.casefold)}
    return hashlib.sha256(
        json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def _execution_environment_executable_identities(
    env: Mapping[str, str], *, cwd: Path
) -> dict[str, object]:
    env = ProcessEnvironment(env)
    identities: dict[str, object] = {}
    for name, value in sorted(env.items()):
        key = env.rule.key(name)
        if key not in command_identity._EXECUTABLE_ENV_NAMES and not any(
            pattern.fullmatch(key)
            for pattern in command_identity._EXECUTABLE_ENV_PATTERNS
        ):
            continue
        if not value:
            continue
        parts = resolve_explicit_tool_command(
            value, label=f"executable environment {name}", environment=env, cwd=cwd
        )
        path = Path(parts[0])
        identity = command_identity._executable_identity(path)
        if not command_identity._content_identity_available(identity):
            raise ValueError(f"executable environment {name} has no content identity")
        images = [
            process_image_capture.capture_image(f"env:{name}", path, preserve_path=True)
        ]
        resolved = Path(str(identity["resolved_path"]))
        if resolved != path:
            images.append(process_image_capture.capture_image(f"env:{name}", resolved))
        rust_role = key.removeprefix("CARGO_BUILD_").lower()
        if rust_role in {"cargo", "rustc", "rustdoc", "rustfmt"}:
            from molt.rust_toolchain import resolve_rustup_proxy

            content = resolve_rustup_proxy(
                path, role=rust_role, root=cwd, env=env
            ).resolve(strict=True)
            if content != resolved:
                images.append(
                    process_image_capture.capture_image(f"env:{name}", content)
                )
        identities[name] = {
            "executable": identity,
            "argument_count": len(parts) - 1,
            "process_images": process_image_capture.canonical_images(images),
        }
    return identities


def _capture_toolchains(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    source_root: Path,
    hash_workers: int,
    located_toolchains: Mapping[str, object],
) -> tuple[dict[str, object] | None, dict[str, object]]:
    env = ProcessEnvironment(env)
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if (
        not isinstance(requested_raw, list)
        or not requested_raw
        or not all(isinstance(name, str) and name for name in requested_raw)
    ):
        raise ValueError("proof command envelope has no non-empty toolchain authority")
    requested = [str(name) for name in requested_raw]
    if len(requested) != len(set(requested)):
        raise ValueError("proof command envelope has duplicate toolchain authorities")
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    known = set(policies)
    unknown = sorted(set(requested) - known)
    if unknown:
        raise ValueError(f"proof command envelope has unknown toolchains: {unknown!r}")
    located_python = located_toolchains.get("python")
    if "python" in requested and not isinstance(located_python, Mapping):
        raise ValueError("located Python selection identity is unavailable")
    proof_python = command_identity._python_identity(
        envelope,
        cwd=cwd,
        env=env,
        source_root=source_root,
        selection=cast(Mapping[str, object], located_python),
        hash_workers=hash_workers,
    )
    if proof_python is None and "python" in requested:
        synthetic_envelope = admission.envelope_for_command(
            [sys.executable, "-c", "raise SystemExit('identity-only')"]
        )
        proof_python = command_identity._python_identity(
            synthetic_envelope,
            cwd=cwd,
            env=env,
            source_root=source_root,
            selection=cast(Mapping[str, object], located_python),
            hash_workers=hash_workers,
        )
    toolchains: dict[str, object] = {}
    if proof_python is not None:
        toolchains["python"] = proof_python
    sdk_resources: dict[str, dict[str, object]] = {}
    sdk_verified_files = {}
    for name in requested:
        if policies[name].python_identity_role == "environment":
            continue
        located = located_toolchains.get(name)
        if not isinstance(located, Mapping):
            raise ValueError(f"located {name} toolchain identity is unavailable")
        located = dict(located)
        selected_runtime = located.get("python_runtime")
        if policies[name].python_identity_role == "runtime":
            if not isinstance(selected_runtime, Mapping):
                raise ValueError(
                    f"{name}: selected Python runtime capture is unavailable"
                )
            proof_plan.validate_selected_python_runtime(
                policies[name], selected_runtime
            )
            with python_identity_verifier_prefix() as verifier_prefix:
                fresh = command_identity._parse_json_output(
                    command_identity._run_captured(
                        [
                            selected_runtime["launch_argv"][0],
                            *command_identity.python_identity_probe_arguments(
                                ("--runtime-selection",),
                                no_site=True,
                                verifier_bytecode_prefix=verifier_prefix,
                            ),
                        ],
                        cwd=cwd,
                        env=env,
                    ),
                    purpose=f"{name} fresh isolated startup selection",
                )
            from molt.python_runtime_identity import startup_selection_difference

            if startup_selection_difference(
                selected_runtime["startup_selection"], fresh, admit_new_images=False
            ):
                raise ValueError(
                    f"{name}: selected Python startup changed while live custody armed"
                )
        verified_sdk_files = frozenset()
        verified_resource_files = frozenset()
        if toolchain_capture.sdk_required(policies[name].data, located):
            closure = toolchain_capture.validate_wasi_sdk_closure(
                located, full_capture=False
            )
            key = str(closure["sdk"])
            if key not in sdk_resources:
                sdk_verified_files[key] = (
                    toolchain_capture.revalidate_wasi_sdk_selection(located)
                )
                sdk_resources[key] = toolchain_capture.capture_wasi_sdk_resources(
                    closure
                )
            elif {
                k: v for k, v in sdk_resources[key].items() if k != "resources"
            } != dict(closure):
                raise ValueError("one proof selected conflicting SDK generations")
            verified_sdk_files = sdk_verified_files[key]
            located["wasi_sdk"] = sdk_resources[key]
            if located.get("identity_kind") == "target-derived":
                # Additional family images (e.g. nm) retain actual image custody.
                # Content aliases retain lexical membership, but one boundary
                # reads each already admitted image content only once.
                verified_contents = {
                    Path(item.path).resolve(strict=True): (item.sha256, item.size)
                    for item in verified_sdk_files
                }
                extra = []
                images = process_image_capture.toolchain_images(name, located)
                for image in images:
                    content = Path(str(image["path"])).resolve(strict=True)
                    expected = (image["sha256"], image["size_bytes"])
                    if content in verified_contents:
                        if verified_contents[content] != expected:
                            raise ValueError(
                                "SDK family image differs from verified content"
                            )
                    else:
                        extra.append(image)
                        verified_contents[content] = expected
                process_image_capture.revalidate_images(extra)
                verified_sdk_files |= frozenset(toolchain_capture.frozen_files(images))
                sdk_verified_files[key] = verified_sdk_files
            verified_sdk_files |= frozenset(
                toolchain_capture.frozen_files(sdk_resources[key])
            )
            material = dict(located)
            material.pop("identity_sha256", None)
            located["identity_sha256"] = canonical_json_sha256(material)
        verified_images = []
        if name == "rustc":
            verified_images, selection = (
                toolchain_capture.revalidate_rust_link_process_images(
                    located,
                    target=command_identity._rust_target(envelope, env, cwd=cwd),
                    command_argv=admission._nested_command(exact) or exact,
                    required_native_units=envelope.get("cargo_native_units", {}),
                )
            )
            located["link_selection"] = selection
            verified_resource_files = frozenset(
                toolchain_capture.frozen_files(
                    [
                        row["resources"]
                        for row in located["link_selection"]["native_build"]
                    ]
                )
            )
        if not toolchain_capture.sdk_required(policies[name].data, located):
            images = process_image_capture.toolchain_images(name, located)
            verified_paths = {row["path"] for row in verified_images}
            process_image_capture.revalidate_images(
                [row for row in images if row["path"] not in verified_paths]
            )
            verified_resource_files |= frozenset(toolchain_capture.frozen_files(images))
        package = command_identity._validate_node_package_identity(
            policies[name], located
        )
        if package is not None:
            if set(package) == {"root"}:
                package = command_identity._directory_manifest_identity(
                    Path(package["root"]), label=f"{name} node package"
                )
                located["node_package"] = {
                    **located["node_package"],
                    "package": package,
                }
                command_identity._validate_node_package_identity(
                    policies[name], located, full_capture=True
                )
            else:
                command_identity._revalidate_directory_manifest_identity(
                    package,
                    selected_root=Path(package["root"]),
                    label=f"{name} node package",
                )
            verified_resource_files |= frozenset(
                toolchain_capture.frozen_files(package)
            )
        for frozen in toolchain_capture.frozen_files({name: located}):
            if frozen in verified_sdk_files or frozen in verified_resource_files:
                continue
            path = Path(frozen.path)
            if command_identity._hash_file(path) != frozen.sha256 or (
                frozen.size is not None and path.stat().st_size != frozen.size
            ):
                raise ValueError(
                    f"{name} toolchain file changed while live custody armed: {path}"
                )
        material = dict(located)
        material.pop("identity_sha256", None)
        if policies[name].python_identity_role == "runtime":
            material["python_runtime"] = proof_plan.selected_python_runtime_material(
                selected_runtime
            )
        located["identity_sha256"] = canonical_json_sha256(material)
        if policies[name].python_identity_role == "runtime":
            command_identity._validate_toolchain_identity(
                plan, name, located, full_capture=True, envelope=envelope
            )
        toolchains[name] = dict(located)
    if set(toolchains) != set(requested):
        raise ValueError(
            "proof command toolchain capture is incomplete: "
            f"requested={sorted(requested)!r} captured={sorted(toolchains)!r}"
        )
    return proof_python, toolchains


def validate_typed_source_root(
    envelope: Mapping[str, object], source_snapshot: Mapping[str, object]
) -> None:
    """Reject a foreign checkout before capturing or launching typed producers."""
    if envelope.get("typed_command") is None:
        return
    root = source_snapshot.get("root")
    if not isinstance(root, str) or not Path(root).is_absolute():
        raise ValueError("typed producer has no canonical Git source root")
    source_root = Path(root).resolve(strict=True)
    if (
        source_root != admission._REPO_ROOT.resolve(strict=True)
        or Path(root) != source_root
    ):
        raise ValueError(
            "typed producer Git source root differs from its bootstrap checkout"
        )


def _locate_toolchain_watch_roots(
    envelope: Mapping[str, object],
    exact: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    supervisor_binary: Path,
    reuse_root: Path | None = None,
) -> tuple[list[Path], dict[str, object], dict[str, object], dict[str, str]]:
    """Locate broad roots and child executables without a full inventory."""
    started = time.perf_counter()
    reuse_telemetry: list[dict[str, object]] = []
    plan = proof_plan.ProofPlan.load()
    requested_raw = envelope.get("toolchains")
    if not isinstance(requested_raw, list) or not all(
        isinstance(name, str) and name for name in requested_raw
    ):
        raise ValueError("proof command envelope has no toolchain authority")
    requested = [str(name) for name in requested_raw]
    # Rust metadata selects the host once. Its captured physical tools and C
    # selections become the environment used by later policies and payload.
    requested.sort(
        key=lambda name: 0 if name == "cargo" else 1 if name == "rustc" else 2
    )
    env = ProcessEnvironment(env)
    selected_updates: dict[str, str] = {}
    policies = {policy.name: policy for policy in plan.toolchain_policies}
    roots: list[Path] = []
    policy_identities: dict[str, object] = {}
    typed = envelope.get("typed_command")
    if (
        isinstance(typed, Mapping)
        and typed.get("family") == "source-extension-producer"
    ):
        source = Path(str(typed["source"])).resolve(strict=True)
        if not source.is_dir():
            raise ValueError("source-extension source watch root is not a directory")
        roots.append(source)
    if "python" in requested:
        python_envelope = envelope
        python_exact = list(exact)
        if envelope.get("python") is None:
            python_envelope = admission.envelope_for_command(
                [sys.executable, "-c", "raise SystemExit('location-only')"]
            )
            python_exact = [sys.executable, "-c", "raise SystemExit('location-only')"]
        with python_identity_verifier_prefix() as verifier_prefix:
            command = command_identity._python_auxiliary_command(
                python_envelope,
                python_exact,
                arguments=("--locate-active-environment",),
                no_site=True,
                verifier_bytecode_prefix=verifier_prefix,
            )
            if command is None:
                raise ValueError("proof Python locator has no selected interpreter")
            located = command_identity._parse_json_output(
                command_identity._run_captured(command, cwd=cwd, env=env),
                purpose="proof Python toolchain locator",
            )
        try:
            located = validate_python_environment_location(located)
        except PythonEnvironmentIdentityError as exc:
            raise ValueError(
                f"proof Python toolchain locator is invalid: {exc}"
            ) from exc
        policy_identities["python"] = {
            **command_identity.python_selection(located),
            "external_roots": [],
            "location": located,
        }
        for field in ("roots", "external_roots"):
            values = located.get(field)
            if not isinstance(values, list) or not all(
                isinstance(value, str) for value in values
            ):
                raise ValueError(f"proof Python locator has malformed {field}")
            for value in cast(list[str], values):
                path = Path(value)
                if path.is_dir():
                    resolved = path.resolve(strict=True)
                    roots.append(resolved)
                    if field == "external_roots":
                        external = policy_identities["python"]["external_roots"]
                        assert isinstance(external, list)
                        external.append(str(resolved))
    for name in requested:
        if policies[name].python_identity_role == "environment":
            continue
        identity = command_identity._tool_identity(
            plan,
            name,
            envelope,
            exact,
            cwd=cwd,
            env=env,
            reuse_root=reuse_root,
            reuse_telemetry=reuse_telemetry,
        )
        if name in {"rustc", "cargo"}:
            # Tool roles name canonical selectors; this is not an environment lookup.
            updates = {name.upper(): str(identity["content_path"])}
            if name == "rustc":
                selection = cast(Mapping[str, object], identity["link_selection"])
                selected_target = selection["target"] or selection["compiler_host"]
                assert isinstance(selected_target, str)
                compiler_host = selection["compiler_host"]
                assert isinstance(compiler_host, str)
                native_environment = toolchain_capture.native_build_environment(
                    selection
                )
                validate_cargo_native_dependency_environment(
                    ProcessEnvironment(env).updated(native_environment),
                    target=selected_target,
                    compiler_host=compiler_host,
                    native_bound=True,
                )
                updates.update(native_environment)
                from molt.rust_toolchain import rust_toolchain_library_environment

                updates.update(
                    rust_toolchain_library_environment(
                        Path(str(identity["content_path"])), env
                    )
                )
            selected_updates.update(updates)
            env = env.updated(updates)
        probes = policies[name].data.get("process_image_probes", [])
        assert isinstance(probes, list)
        if probes:
            process_images = list(identity["process_images"])
            inventories: list[dict[str, object]] = []
            for probe in probes:
                assert isinstance(probe, list)
                observed, telemetry = (
                    supervisor_custody.capture_process_image_inventory(
                        binary=supervisor_binary,
                        role=name,
                        executable=Path(str(identity["path"])),
                        probe_args=[str(value) for value in probe],
                        cwd=cwd,
                        env=env,
                    )
                )
                process_images.extend(observed)
                inventories.append(telemetry)
            identity["process_images"] = process_image_capture.canonical_images(
                process_images
            )
            identity["process_image_inventories"] = inventories
            identity_without_digest = dict(identity)
            identity_without_digest.pop("identity_sha256", None)
            identity["identity_sha256"] = canonical_json_sha256(identity_without_digest)
        command_identity._validate_toolchain_identity(
            plan, name, identity, envelope=envelope
        )
        policy_identities[name] = identity
        roots.extend(_broad_toolchain_roots({name: identity}))
    roots = list(dict.fromkeys(roots))
    telemetry = {
        "schema": "molt.proof-toolchain-location-telemetry.v1",
        "non_python_selection_capture_count": sum(
            name != "python" for name in requested
        ),
        "root_count": len(roots),
        "locate_s": time.perf_counter() - started,
        "tool_identity_reuse": reuse_telemetry,
    }
    return roots, policy_identities, telemetry, selected_updates


def _python_editable_ineligible_reasons(
    identity: Mapping[str, object] | None,
    *,
    source_snapshot: Mapping[str, object],
) -> list[str]:
    if identity is None:
        return []
    reasons: list[str] = []
    environment = identity.get("environment")
    if not isinstance(environment, Mapping):
        return ["python-environment-closure-malformed"]
    external_roots = environment.get("external_roots")
    if not isinstance(external_roots, list):
        return ["python-external-root-closure-malformed"]
    roots = {
        str(row.get("id")): str(row.get("path"))
        for row in external_roots
        if isinstance(row, Mapping)
        and isinstance(row.get("id"), str)
        and isinstance(row.get("path"), str)
    }
    if len(roots) != len(external_roots):
        reasons.append("python-external-root-closure-malformed")
    distributions = environment.get("distributions")
    if not isinstance(distributions, list):
        return ["python-distribution-inventory-malformed"]
    source_root = source_snapshot.get("root")
    for distribution in distributions:
        if not isinstance(distribution, Mapping):
            reasons.append("python-distribution-inventory-malformed")
            continue
        external = distribution.get("external_source")
        if external is None:
            continue
        name = str(distribution.get("name") or "unknown")
        if not isinstance(external, Mapping):
            reasons.append(f"python-editable-source-malformed:{name}")
            continue
        root = roots.get(str(external.get("root")))
        inside_source = False
        if root is not None and isinstance(source_root, str):
            root_path = Path(root)
            source_path = Path(source_root)
            inside_source = os.path.normcase(str(root_path)) == os.path.normcase(
                str(source_path)
            ) or root_path.is_relative_to(source_path)
        if not inside_source:
            reasons.append(f"python-editable-source-outside:{name}")
        if source_snapshot.get("available") is not True:
            reasons.append(f"python-editable-source-git-unavailable:{name}")
        elif source_snapshot.get("clean") is not True:
            reasons.append(f"python-editable-source-dirty:{name}")
    return reasons


def _git_snapshot(cwd: Path, env: Mapping[str, str]) -> dict[str, object]:
    env = ProcessEnvironment(env)
    head = command_identity._run_captured(
        ("git", "rev-parse", "HEAD"), cwd=cwd, env=env
    )
    if head.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", head.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": None,
            "status_sha256": None,
        }
    root = command_identity._run_captured(
        ("git", "rev-parse", "--show-toplevel"), cwd=cwd, env=env
    )
    if root.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    source_root = Path(root.stdout.strip()).resolve(strict=True)
    tree = command_identity._run_captured(
        ("git", "rev-parse", "HEAD^{tree}"), cwd=cwd, env=env
    )
    if tree.returncode != 0 or not re.fullmatch(
        r"[0-9a-fA-F]{40}|[0-9a-fA-F]{64}", tree.stdout.strip()
    ):
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "tree": None,
            "status_sha256": None,
        }
    status = command_identity._run_captured(
        (
            "git",
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ),
        cwd=cwd,
        env=env,
        text=False,
    )
    if status.returncode != 0:
        return {
            "available": False,
            "clean": False,
            "commit": head.stdout.strip().lower(),
            "status_sha256": None,
        }
    return {
        "available": True,
        "root": str(source_root),
        "clean": not status.stdout,
        "commit": head.stdout.strip().lower(),
        "tree": tree.stdout.strip().lower(),
        "status_sha256": hashlib.sha256(status.stdout).hexdigest(),
    }


def _git_tracked_paths(cwd: Path, env: ProcessEnvironment) -> list[Path]:
    return [
        path
        for path in repository_source_paths(
            cwd, run_git=partial(_read_source_git, env=env)
        )
        if path.is_file()
    ]


def _read_source_git(
    cwd: Path, arguments: list[str], *, env: ProcessEnvironment
) -> bytes:
    completed = command_identity._run_captured(
        ("git", *arguments),
        cwd=cwd,
        env=env,
        text=False,
    )
    if completed.returncode != 0:
        operation = (
            "resolve its Git root"
            if arguments[0] == "rev-parse"
            else "enumerate tracked inputs"
        )
        raise ValueError(f"proof source custody cannot {operation}")
    return completed.stdout


SOURCE_CONTENT_KIND = "molt.proof-source-content.v1"


def capture_source_content(
    *,
    source_root: Path,
    env: Mapping[str, str],
    overlays: Sequence[Path],
    cas_root: Path,
    hash_workers: int,
    expected_files: Mapping[str, str] | None = None,
) -> tuple[dict[str, object], dict[str, object], Callable[[], None]]:
    """Capture Git-admitted source and explicit overlays, never local caches."""
    env = ProcessEnvironment(env)
    from molt.python_file_node_custody import PythonFileCaptureContext

    started = time.perf_counter()
    capture = PythonFileCaptureContext(hash_workers=hash_workers)
    root = file_publication.resolve_owned_path(source_root)
    read_git = partial(_read_source_git, env=env)
    selected = repository_source_paths(root, run_git=read_git, include_untracked=True)
    paths = sorted(set(selected) | set(overlays), key=lambda path: path.as_posix())
    regular: list[tuple[Path, os.stat_result]] = []
    missing: list[Path] = []
    hardlinks: dict[tuple[int, int], tuple[int, int]] = {}
    for path in paths:
        if not path.is_relative_to(root):
            raise ValueError("source content overlay escaped admitted Git root")
        file_publication.resolve_owned_path(path)
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            missing.append(path)
            continue
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(
                f"source content input is not a direct regular file: {path}"
            )
        regular.append((path, metadata))
        key = metadata.st_dev, metadata.st_ino
        count, links = hardlinks.get(key, (0, metadata.st_nlink))
        hardlinks[key] = count + 1, links
    if any(count != links for count, links in hardlinks.values()):
        raise ValueError("source input has aliases outside admitted source content")
    files: list[dict[str, object]] = []
    remaining_expected = dict(expected_files or {})
    identities = capture.bind_many(regular, label="proof source input")
    for (path, metadata), identity in zip(regular, identities, strict=True):
        relative = path.relative_to(root).as_posix()
        expected = remaining_expected.pop(relative, None)
        if expected is not None and identity.sha256 != expected:
            raise ValueError(
                f"selected source changed since command admission: {relative}"
            )
        files.append(
            {
                "path": path.relative_to(root).as_posix(),
                "sha256": identity.sha256,
                "size": identity.size,
                "mode": stat.S_IMODE(metadata.st_mode),
            }
        )

    if remaining_expected:
        raise ValueError("selected source is missing from the captured file inventory")

    def verify_selection() -> None:
        if selected != repository_source_paths(
            root, run_git=read_git, include_untracked=True
        ):
            raise ValueError("source content admission set changed during capture")
        if any(path.exists() for path in missing):
            raise ValueError("missing source input appeared during capture")

    capture.register_verification_fence(verify_selection)
    capture.verify()
    payload = {
        "schema": custody_cas.ARTIFACT_SCHEMA,
        "kind": SOURCE_CONTENT_KIND,
        "root": str(root),
        "selection": "git-tracked-plus-nonignored-untracked-and-overlays",
        "files": files,
        "missing": [path.relative_to(root).as_posix() for path in missing],
    }
    reference = custody_cas.put_json(cas_root, payload).as_dict()
    capture.verify()
    return (
        reference,
        {
            "file_count": len(files),
            "bytes_hashed": sum(int(row["size"]) for row in files),
            "capture_s": time.perf_counter() - started,
            "inventory_profile": capture.inventory_profile(),
        },
        capture.verify,
    )


def _broad_toolchain_roots(toolchains: Mapping[str, object]) -> list[Path]:
    roots: list[Path] = []
    for identity in toolchains.values():
        if not isinstance(identity, Mapping):
            continue
        selected_runtime = identity.get("python_runtime")
        if isinstance(selected_runtime, Mapping):
            startup = cast(Mapping[str, object], selected_runtime["startup_selection"])
            for location, directory_role in python_runtime_selection_locations(startup):
                if directory_role:
                    for spelling in ("lexical", "resolved"):
                        path = Path(str(location[spelling]))
                        if path.is_dir():
                            roots.append(path)
        sdk = identity.get("wasi_sdk")
        if isinstance(sdk, Mapping):
            from molt.wasi_sdk_identity import SDK_RESOURCE_ROOTS

            selected = toolchain_capture.validate_wasi_sdk_closure(
                identity, full_capture=False
            )
            roots.extend(
                Path(str(selected["sdk"])) / relative for relative in SDK_RESOURCE_ROOTS
            )
        node_package = identity.get("node_package")
        package = (
            node_package.get("package") if isinstance(node_package, Mapping) else None
        )
        raw_root = package.get("root") if isinstance(package, Mapping) else None
        if isinstance(raw_root, str) and Path(raw_root).is_dir():
            roots.append(Path(raw_root).resolve(strict=True))
        selection = identity.get("link_selection")
        if isinstance(selection, Mapping):
            for unit in selection.get("native_build", []):
                for selected_root in unit["selection"]["resource_roots"]:
                    lexical = Path(selected_root)
                    if lexical.is_dir():
                        roots.append(lexical.resolve(strict=True))
    return list(dict.fromkeys(roots))
