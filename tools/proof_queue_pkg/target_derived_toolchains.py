"""Target-resolved compiler-family identity, without a second tool resolver."""

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path
import re

from molt.cli.compiler_target import is_zig_compiler_command
from molt.cli.source_extension_compiler_inputs import (
    validate_source_extension_compiler_command,
)
from molt.cli.source_extension_language import SourceExtensionLanguage
from molt.cli.source_extension_set_validation_target import (
    _source_extension_tool_role_contract,
)
from molt.cli.source_extension_target import (
    SourceExtensionTargetPlan,
    resolve_source_extension_target_plan,
    source_extension_recorded_target_plan,
)
from molt.cli.source_extension_toolchain import _resolve_source_extension_toolchain
from molt.dx import _reject_onedrive
from molt.exact_json import canonical_json_sha256
from molt.source_extension_link_inputs import validate_source_extension_link_inputs
from molt.llvm_linker_roles import (
    executable_entrypoint_name,
    executable_selects_linker_role,
    lexical_executable_path,
)
from molt.toolchain_identity import (
    executable_content_path,
    native_executable_content_identity,
)
from tools import proof_plan
from tools.proof_queue_pkg import process_image_capture


SOURCE_EXTENSION_PROVIDER = "source-extension"
SOURCE_EXTENSION_SCHEMA = "molt.proof-source-extension-toolchain.v2"
SOURCE_EXTENSION_VERSION = "molt-source-extension-toolchain-v2"
_COMPILER_ROLES = frozenset(
    language.compiler_role for language in SourceExtensionLanguage
)
_UNINVENTORIED_LAUNCHERS = frozenset({"ccache", "distcc", "sccache"})
_SHA256 = re.compile(r"[0-9a-f]{64}")


def _command(value: object, *, role: str) -> tuple[str, ...]:
    if (
        not isinstance(value, list)
        or not value
        or not all(
            isinstance(token, str) and token and "\0" not in token for token in value
        )
    ):
        raise ValueError(f"source-extension {role} requires a non-empty exact argv")
    return tuple(value)


def _entrypoint(value: object, *, role: str) -> Path:
    if not isinstance(value, str) or not Path(value).is_absolute():
        raise ValueError(f"source-extension {role} requires an absolute entrypoint")
    path = lexical_executable_path(Path(value))
    if str(path) != value:
        raise ValueError(f"source-extension {role} entrypoint is not canonical")
    _reject_onedrive(path, f"source-extension {role} entrypoint")
    content = executable_content_path(path, label=f"source-extension {role}")
    _reject_onedrive(content, f"source-extension {role} content")
    return path


def _tool_image_specs(
    tools: Mapping[str, object], *, target: SourceExtensionTargetPlan
) -> list[tuple[str, Path, str, bool]]:
    roles, required = _source_extension_tool_role_contract(target.target_triple)
    if set(tools) != set(roles.values()):
        raise ValueError("source-extension tool-role identity is incomplete")
    if any(tools[roles[role]] is None for role in required):
        raise ValueError("source-extension required tool identity is missing")
    specs: list[tuple[str, Path, str, bool]] = []
    for role, raw in sorted(tools.items()):
        if raw is None:
            continue
        if not isinstance(raw, Mapping) or set(raw) != {
            "command",
            "path",
            "sha256",
            "version",
        }:
            raise ValueError(f"source-extension {role} tool identity is malformed")
        command = _command(raw["command"], role=role)
        path = _entrypoint(raw["path"], role=role)
        if role == roles["ld"] and not executable_selects_linker_role(path, "wasm-ld"):
            raise ValueError(
                "source-extension linker entrypoint does not select wasm-ld"
            )
        if command[0] != str(path):
            raise ValueError(
                f"source-extension {role} command selects another entrypoint"
            )
        digest = raw["sha256"]
        if not isinstance(digest, str) or _SHA256.fullmatch(digest) is None:
            raise ValueError(f"source-extension {role} has no SHA-256 identity")
        if raw["version"] is not None and not isinstance(raw["version"], str):
            raise ValueError(f"source-extension {role} version is malformed")
        # The lexical name selects LLVM's driver role (wasm-ld versus lld).
        # Capture its resolved bytes as well without replacing argv[0].
        specs.append((f"source-extension:{role}", path, digest, True))
        content = executable_content_path(path, label=f"source-extension {role}")
        if os.path.normcase(str(content)) != os.path.normcase(str(path)):
            specs.append((f"source-extension:{role}:content", content, digest, False))
    if not specs:
        raise ValueError("source-extension compiler family has no process images")
    return specs


def _tool_images(
    tools: Mapping[str, object], *, target: SourceExtensionTargetPlan
) -> list[dict[str, object]]:
    images: list[dict[str, object]] = []
    for role, path, digest, selection in _tool_image_specs(tools, target=target):
        fact = native_executable_content_identity(path, label=role)
        if fact["sha256"] != digest:
            raise ValueError(f"{role} executable content changed")
        image = process_image_capture.capture_image(role, path, preserve_path=selection)
        if image["sha256"] != digest or image["size_bytes"] != fact["size"]:
            raise ValueError(f"{role} changed during image capture")
        images.append(image)
    return process_image_capture.canonical_images(images)


def family_process_images(identity: Mapping[str, object]) -> list[dict[str, object]]:
    """Project the recorded family, with no fabricated root executable or probes.

    Cold validation rehashes these images through ``validate_identity``; this
    projection only checks the exact family membership and selected entrypoints.
    """
    if (
        identity.get("schema") != SOURCE_EXTENSION_SCHEMA
        or identity.get("identity_kind") != "target-derived"
    ):
        raise ValueError("source-extension process image provider is invalid")
    requested, triple = identity.get("target"), identity.get("target_triple")
    if not isinstance(requested, str) or not isinstance(triple, str):
        raise ValueError("source-extension recorded target is malformed")
    target = source_extension_recorded_target_plan(requested, target_triple=triple)
    tools, raw_images = identity.get("tools"), identity.get("process_images")
    if not isinstance(tools, Mapping) or not isinstance(raw_images, list):
        raise ValueError("source-extension process image family is malformed")
    expected = {
        (role, str(path), digest, selection)
        for role, path, digest, selection in _tool_image_specs(tools, target=target)
    }
    images = process_image_capture.canonical_images(raw_images)
    observed = {
        (row["role"], row["path"], row["sha256"], row.get("path_kind") == "selection")
        for row in images
    }
    if observed != expected or any("root_exit_disposition" in row for row in images):
        raise ValueError("source-extension process images differ from compiler family")
    return images


def _validate_commands(
    target: SourceExtensionTargetPlan,
    tools: Mapping[str, object],
    commands: Mapping[str, object],
    sysroot: object,
) -> None:
    roles, required = _source_extension_tool_role_contract(target.target_triple)
    if not required.issubset(commands) or not set(commands).issubset(roles):
        raise ValueError("source-extension command family differs from target contract")
    compiler_sysroots: list[str | None] = []
    for role, raw in commands.items():
        command = _command(raw, role=role)
        tool = tools.get(roles[role])
        if not isinstance(tool, Mapping):
            raise ValueError(f"source-extension {role} command has no tool identity")
        base = _command(tool.get("command"), role=roles[role])
        if command[: len(base)] != base:
            raise ValueError(
                f"source-extension {role} command differs from tool identity"
            )
        if role not in _COMPILER_ROLES:
            if command != base or len(command) != 1:
                raise ValueError(f"source-extension {role} has undeclared arguments")
            continue
        if executable_entrypoint_name(
            Path(command[0])
        ) in _UNINVENTORIED_LAUNCHERS or is_zig_compiler_command(command):
            raise ValueError(
                "source-extension compiler launcher requires helper-process custody"
            )
        admitted = validate_source_extension_compiler_command(
            command,
            role=role,
            target_triple=target.target_triple,
            require_explicit_target=target.compiler_target_triple is not None,
            sysroot_policy="required"
            if target.target_triple == "wasm32-wasip1"
            else "forbidden",
            expected_sysroot=sysroot if isinstance(sysroot, str) else None,
        )
        compiler_sysroots.append(admitted.sysroot)

    if target.target_triple != "wasm32-wasip1":
        if sysroot is not None or any(compiler_sysroots):
            raise ValueError("source-extension non-WASI target has an unowned sysroot")
        return
    if not isinstance(sysroot, str) or not Path(sysroot).is_absolute():
        raise ValueError("source-extension WASI target requires an explicit sysroot")
    root = Path(sysroot)
    _reject_onedrive(root, "source-extension WASI sysroot")
    if not root.is_dir() or str(root.resolve(strict=True)) != sysroot:
        raise ValueError("source-extension WASI sysroot is not a canonical directory")
    if any(root != sysroot for root in compiler_sysroots):
        raise ValueError("source-extension compiler sysroot differs from captured root")


def _sysroot_custody(sysroot: object) -> dict[str, object] | None:
    if sysroot is None:
        return None
    if not isinstance(sysroot, str):
        raise ValueError("source-extension sysroot is malformed")
    # This existing authority feeds frozen_files and queue live-directory custody.
    from tools.proof_queue_pkg.command_identity import _directory_manifest_identity

    return _directory_manifest_identity(
        Path(sysroot), label="source-extension WASI sysroot", strict_owned=True
    )


def _require_policy(policy: proof_plan.ToolchainPolicy) -> None:
    if (
        policy.identity_kind != "target-derived"
        or policy.data.get("identity_provider") != SOURCE_EXTENSION_PROVIDER
    ):
        raise ValueError(
            f"{policy.name} has no admitted target-derived identity provider"
        )
    if policy.data.get("process_image_probes", []) != []:
        raise ValueError("source-extension provider has no declared helper inventory")
    pattern = policy.data.get("version_pattern")
    if (
        not isinstance(pattern, str)
        or re.fullmatch(pattern, SOURCE_EXTENSION_VERSION) is None
    ):
        raise ValueError("source-extension toolchain identity policy is invalid")


def capture_identity(
    policy: proof_plan.ToolchainPolicy,
    envelope: Mapping[str, object],
    *,
    environment: Mapping[str, str] | None = None,
) -> dict[str, object]:
    _require_policy(policy)
    typed = envelope.get("typed_command")
    if (
        not isinstance(typed, Mapping)
        or typed.get("family") != "source-extension-producer"
    ):
        raise ValueError("source-extension toolchain has no typed producer command")
    requested = typed.get("target")
    if not isinstance(requested, str):
        raise ValueError("source-extension producer has no target authority")
    target = resolve_source_extension_target_plan(requested)
    if requested != target.requested:
        raise ValueError("source-extension producer target is not canonical")
    # One actual resolver owns discovery and target command materialization.
    # Its compiler probes are capture-time work, never receipt validation.
    resolved = _resolve_source_extension_toolchain(target, environment=environment)
    if resolved.target_plan != target:
        raise ValueError("source-extension resolver returned another target")
    tools = resolved.tools.metadata()
    commands = {
        role: list(command) for role, command in sorted(resolved.commands.items())
    }
    sysroot = str(resolved.wasi_sysroot) if resolved.wasi_sysroot is not None else None
    images = _tool_images(tools, target=target)
    _validate_commands(target, tools, commands, sysroot)
    material: dict[str, object] = {
        "schema": SOURCE_EXTENSION_SCHEMA,
        "identity_kind": "target-derived",
        "version": SOURCE_EXTENSION_VERSION,
        "target": target.requested,
        "target_triple": target.target_triple,
        "tools": tools,
        "commands": commands,
        "wasi_sysroot": sysroot,
        "sysroot_custody": _sysroot_custody(sysroot),
        "link_inputs": resolved.link_inputs.metadata(),
        "process_images": images,
        "process_image_inventories": [],
        "tool_family_sha256": canonical_json_sha256(
            {"tools": tools, "commands": commands}
        ),
    }
    identity = {**material, "identity_sha256": canonical_json_sha256(material)}
    return identity


def validate_identity(
    policy: proof_plan.ToolchainPolicy, identity: Mapping[str, object]
) -> None:
    _require_policy(policy)
    if set(identity) != {
        "schema",
        "identity_kind",
        "identity_sha256",
        "version",
        "target",
        "target_triple",
        "tools",
        "commands",
        "wasi_sysroot",
        "sysroot_custody",
        "link_inputs",
        "process_images",
        "process_image_inventories",
        "tool_family_sha256",
    }:
        raise ValueError("source-extension toolchain identity fields are invalid")
    if (
        identity["schema"] != SOURCE_EXTENSION_SCHEMA
        or identity["identity_kind"] != "target-derived"
        or identity["version"] != SOURCE_EXTENSION_VERSION
    ):
        raise ValueError("source-extension toolchain identity policy is invalid")
    material = dict(identity)
    digest = material.pop("identity_sha256")
    if digest != canonical_json_sha256(material):
        raise ValueError("source-extension toolchain identity digest is invalid")
    requested, triple = identity["target"], identity["target_triple"]
    if not isinstance(requested, str) or not isinstance(triple, str):
        raise ValueError("source-extension recorded target is malformed")
    target = source_extension_recorded_target_plan(requested, target_triple=triple)
    validate_source_extension_link_inputs(identity["link_inputs"], target_triple=triple)
    tools, commands = identity["tools"], identity["commands"]
    if not isinstance(tools, Mapping) or not isinstance(commands, Mapping):
        raise ValueError("source-extension tools/commands are malformed")
    if identity["process_image_inventories"] != []:
        raise ValueError("source-extension helper inventory is not admitted")
    if identity["process_images"] != _tool_images(tools, target=target):
        raise ValueError("source-extension process images differ from compiler family")
    _validate_commands(target, tools, commands, identity["wasi_sysroot"])
    if identity["tool_family_sha256"] != canonical_json_sha256(
        {"tools": dict(tools), "commands": dict(commands)}
    ):
        raise ValueError("source-extension tool-family digest is invalid")
    if identity["sysroot_custody"] != _sysroot_custody(identity["wasi_sysroot"]):
        raise ValueError("source-extension WASI sysroot content changed")
