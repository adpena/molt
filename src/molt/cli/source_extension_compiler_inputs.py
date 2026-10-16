"""Positive argv grammar for source-extension compiler custody."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import re
from typing import Literal

from molt.cli.native_link_plan import is_zig_compiler_command
from molt.cli.compiler_target import (
    compiler_target_triple,
    source_extension_compiler_driver_mode,
    validate_compiler_target,
    compiler_frontend_arguments,
    compiler_argument_spans,
    CompilerArgumentSpan,
    validate_source_extension_compiler_dialect,
    SourceExtensionCompilerDialect,
)
from molt.llvm_linker_roles import executable_entrypoint_name


_COMPILER_ROLES = frozenset({"c", "cpp"})
_TOOL_ROLES = frozenset({"c", "cpp", "ar", "nm", "ld", "ranlib", "strip"})
_IMPLICIT_COMPILER_OVERRIDES = frozenset(
    {
        "CL",
        "_CL_",
        "CCC_OVERRIDE_OPTIONS",
        "CC_LD",
        "CXX_LD",
        "CC_LD_FOR_BUILD",
        "CXX_LD_FOR_BUILD",
    }
)


def source_extension_compiler_environment(
    environment: Mapping[str, str],
) -> dict[str, str]:
    """Keep SDK/search inputs while stripping hidden command overrides.

    Selected SDK and deployment arguments live in native compiler commands,
    including captured Meson/native generator commands. Generic subprocesses
    and non-Darwin targets perform no Apple SDK discovery here.
    """
    return {
        key: value
        for key, value in environment.items()
        if key.upper() not in _IMPLICIT_COMPILER_OVERRIDES
    }


@dataclass(frozen=True)
class _DarwinCommandInputs:
    sdk_root: str | None
    deployment_target: str | None
    spans: dict[str, tuple[CompilerArgumentSpan, ...]]
    sysroots: dict[str, tuple[tuple[int, str, str], ...]]


def _darwin_command_inputs(
    commands: Mapping[str, tuple[str, ...]], *, require_pinned: bool
) -> _DarwinCommandInputs:
    from molt.platform_toolchain import normalize_darwin_version
    from molt.cli.source_extension_link_arguments import linker_argument_tokens

    spans = {
        role: compiler_argument_spans(command) for role, command in commands.items()
    }
    sysroots = {
        role: compiler_sysroot_arguments(command) for role, command in commands.items()
    }
    roots: set[str] = set()
    versions: set[str] = set()
    for role, command in commands.items():
        sdk_operands = {index for index, _prefix, _value in sysroots[role]}
        if any(
            value.startswith("@") and not (require_pinned and index in sdk_operands)
            for index, value in enumerate(command)
            if index
        ):
            raise ValueError(
                "Darwin compiler response files require owned argument expansion"
            )
        if any(
            span.context == "driver"
            and span.option.split("=", 1)[0]
            in {"--config", "--config-system-dir", "--config-user-dir"}
            for span in spans[role]
        ):
            raise ValueError(
                "Darwin compiler configuration files require owned argument expansion"
            )
        roots.update(value for _index, _prefix, value in sysroots[role])
        forwarded = tuple(
            value
            for span in spans[role]
            if (span.context == "opaque" and span.option == "-Xlinker")
            or (span.context == "driver" and span.option.startswith("-Wl,"))
            for value in span.raw
        )
        if any(
            token.split("=", 1)[0]
            in {
                "-syslibroot",
                "--sysroot",
                "-isysroot",
                "-platform_version",
                "-macosx_version_min",
                "-sdk_version",
            }
            or token.startswith("@")
            for token in linker_argument_tokens(forwarded)
        ):
            raise ValueError(
                "Darwin SDK/deployment selection must use compiler driver options, not linker overrides"
            )
        versions.update(
            normalize_darwin_version(
                span.option.removeprefix("-mmacosx-version-min="),
                label="Darwin deployment target",
            )
            for span in spans[role]
            if span.context == "driver"
            and span.option.startswith("-mmacosx-version-min=")
        )
        if require_pinned:
            role_roots = {value for _index, _prefix, value in sysroots[role]}
            driver_roots = [
                span
                for span in spans[role]
                if span.context == "driver"
                and span.index + len(span.raw) - 1 in sdk_operands
            ]
            role_versions = {
                span.option.removeprefix("-mmacosx-version-min=")
                for span in spans[role]
                if span.context == "driver"
                and span.option.startswith("-mmacosx-version-min=")
            }
            if (
                len(role_roots) != 1
                or len(driver_roots) != 1
                or len(role_versions) != 1
                or any(
                    value
                    != normalize_darwin_version(value, label="Darwin deployment target")
                    for value in role_versions
                )
            ):
                raise ValueError(
                    "Darwin compiler commands require a pinned SDK and canonical deployment target for every role"
                )
    if len(roots) > 1 or len(versions) > 1:
        raise ValueError(
            "Darwin compiler roles have conflicting SDK or deployment selections"
        )
    return _DarwinCommandInputs(
        next(iter(roots), None), next(iter(versions), None), spans, sysroots
    )


def validate_darwin_compiler_commands(commands: Mapping[str, tuple[str, ...]]) -> None:
    """Validate published command facts without probing the producer's SDK."""
    _darwin_command_inputs(commands, require_pinned=True)


def darwin_compiler_commands(
    commands: Mapping[str, tuple[str, ...]],
    *,
    environment: Mapping[str, str],
    arch: str,
) -> dict[str, tuple[str, ...]]:
    """Pin one SDK and deployment policy into every actual compiler command."""
    from molt.platform_toolchain import (
        darwin_deployment_target,
        select_darwin_sdk,
        normalize_darwin_version,
    )

    inputs = _darwin_command_inputs(commands, require_pinned=False)
    selected = dict(environment)
    if inputs.sdk_root is not None:
        selected["SDKROOT"] = inputs.sdk_root
    sdk = select_darwin_sdk(selected)
    if inputs.deployment_target is not None:
        explicit = selected.get("MOLT_MACOSX_DEPLOYMENT_TARGET") or selected.get(
            "MACOSX_DEPLOYMENT_TARGET"
        )
        version = inputs.deployment_target
        if explicit and version != normalize_darwin_version(
            explicit, label="Darwin deployment target"
        ):
            raise ValueError(
                "Darwin compiler deployment option conflicts with environment"
            )
        selected["MACOSX_DEPLOYMENT_TARGET"] = version
    deployment = darwin_deployment_target(
        selected, sdk_version=sdk.sdk_version, arch=arch
    )
    projected: dict[str, tuple[str, ...]] = {}
    for role, command in commands.items():
        sdk_operands = {index for index, _prefix, _raw in inputs.sysroots[role]}
        replaced_indices = {
            index
            for span in inputs.spans[role]
            if span.index + len(span.raw) - 1 in sdk_operands
            or (
                span.context == "driver"
                and span.option.startswith("-mmacosx-version-min=")
            )
            for index in range(span.index, span.index + len(span.raw))
        }
        materialized = [
            value
            for index, value in enumerate(command)
            if index not in replaced_indices
        ]
        # cc1-only roots govern headers, while the driver selects the linker's
        # SDK. Collapse every admitted transport to the driver-owned root.
        materialized.extend(
            ("-isysroot", str(sdk.sdk_root), f"-mmacosx-version-min={deployment}")
        )
        projected[role] = tuple(materialized)
    return projected


_SYSROOT_POLICY = Literal["forbidden", "optional", "required"]
_CODEGEN_FLAGS = frozenset(
    {
        "-fPIC",
        "-fno-PIC",
        "-fPIE",
        "-fno-PIE",
        "-ffunction-sections",
        "-fdata-sections",
        "-fexceptions",
        "-fno-exceptions",
        "-frtti",
        "-fno-rtti",
        "-g",
        "-g0",
        "-gline-tables-only",
        "-pedantic",
        "-pthread",
        "-Qunused-arguments",
        "--driver-mode=gcc",
        "--driver-mode=g++",
        "--driver-mode=cl",
        "-w",
    }
)
_DANGEROUS_PREFIXES = (
    "@",
    "/clang:@",
    "-B",
    "-fuse-ld",
    "--gcc-toolchain",
    "--resource-dir",
    "-resource-dir",
    "-Xclang",
    "-Xassembler",
    "-Xlinker",
    "-Wl,",
    "-fplugin",
    "-ivfsoverlay",
    "-include",
    "-imacros",
    "-I",
    "-isystem",
    "-iquote",
    "-idirafter",
    "-F",
    "-iframework",
)


@dataclass(frozen=True, slots=True)
class SourceExtensionCompilerCommand:
    """One exact compiler argv after positive-grammar admission."""

    argv: tuple[str, ...]
    target_is_explicit: bool
    sysroot: str | None


def _exact_argv(command: Sequence[str], *, role: str) -> tuple[str, ...]:
    if role not in _TOOL_ROLES:
        raise ValueError(f"source-extension has unknown tool role {role!r}")
    argv = tuple(command)
    if not argv or any(
        type(token) is not str or not token or "\0" in token for token in argv
    ):
        raise ValueError(
            f"source-extension {role} command must be exact non-empty argv"
        )
    return argv


def _compiler_argument_start(argv: tuple[str, ...], *, role: str) -> int:
    if not is_zig_compiler_command(argv):
        return 1
    expected = "cc" if role == "c" else "c++"
    if argv[1] != expected:
        raise ValueError(
            f"source-extension {role} compiler must invoke zig {expected}, not {argv[1]!r}"
        )
    return 2


def _option_value(
    argv: tuple[str, ...], index: int, option: str, *, role: str
) -> tuple[str, int]:
    if index + 1 >= len(argv) or not argv[index + 1] or argv[index + 1].startswith("-"):
        raise ValueError(
            f"source-extension {role} compiler has {option} without a value"
        )
    return argv[index + 1], index + 2


def compiler_sysroot_arguments(
    command: Sequence[str],
) -> tuple[tuple[int, str, str], ...]:
    """Return every sysroot operand as ``(index, prefix, value)``.

    ``prefix`` is empty for a separate operand, allowing the resolver to
    materialize selected-home paths without re-parsing the command.
    """

    values: list[tuple[int, str, str]] = []
    for span in compiler_argument_spans(command):
        if span.context not in {"driver", "cc1"}:
            continue
        option = span.option
        index = span.index + len(span.raw) - 1
        if option in {"--sysroot", "-isysroot"}:
            value = span.arguments[1]
            if not value or value.startswith("-"):
                raise ValueError(f"compiler command has {option} missing value")
            prefix = "/clang:" if command[index].startswith("/clang:") else ""
            if option == "-isysroot" and command[index].removeprefix(
                "/clang:"
            ).startswith("-isysroot"):
                prefix += "-isysroot"
            values.append((index, prefix, value))
            continue
        for prefix in ("--sysroot=",):
            if option.startswith(prefix):
                value = option.removeprefix(prefix)
                if not value or value.startswith("-"):
                    raise ValueError(f"compiler command has {prefix} missing value")
                forwarding = "/clang:" if command[index].startswith("/clang:") else ""
                values.append((index, forwarding + prefix, value))
                break
    return tuple(values)


def compiler_sysroot_arg_value(command: Sequence[str]) -> str | None:
    """Return the one admitted sysroot value, rejecting conflicting selectors."""

    values = tuple(
        value for _index, _prefix, value in compiler_sysroot_arguments(command)
    )
    if len(set(values)) > 1:
        raise ValueError("compiler command has conflicting sysroot selectors")
    return values[0] if values else None


def _safe_codegen_flag(option: str) -> bool:
    if option in _CODEGEN_FLAGS:
        return True
    if option in {"-O0", "-O1", "-O2", "-O3", "-Os", "-Oz", "-Og"}:
        return True
    if option.startswith("-W") and re.fullmatch(r"-W(?:no-)?[A-Za-z0-9_+.-]+", option):
        return True
    return any(
        re.fullmatch(pattern, option) is not None
        for pattern in (
            r"-D[A-Za-z_][A-Za-z0-9_]*(?:=[A-Za-z0-9_+.,()'\" -]*)?",
            r"-U[A-Za-z_][A-Za-z0-9_]*",
            r"-fvisibility=(?:default|hidden|internal|protected)",
            r"-fdiagnostics-color=(?:always|auto|never)",
            r"-fdiagnostics-format=(?:clang|msvc|vi)",
            r"-ferror-limit=[0-9]+",
            r"-std=(?:c|gnu|iso)(?:89|90|99|11|17|18|23|2x|\+\+98|\+\+03|\+\+11|\+\+14|\+\+17|\+\+20|\+\+23|\+\+2[bc])",
            r"-m(?:abi|arch|cpu|tune)=[A-Za-z0-9_.+-]+",
            r"-mmacosx-version-min=[0-9]+(?:\.[0-9]+){0,2}",
        )
    )


def _reject_external_selector(option: str, *, role: str) -> None:
    if any(option.startswith(prefix) for prefix in _DANGEROUS_PREFIXES):
        raise ValueError(
            f"source-extension {role} compiler option {option!r} requires unowned "
            "external input or helper custody"
        )


def validate_source_extension_compiler_command(
    command: Sequence[str],
    *,
    role: str,
    target_triple: str,
    require_explicit_target: bool = False,
    sysroot_policy: _SYSROOT_POLICY = "forbidden",
    expected_sysroot: str | None = None,
) -> SourceExtensionCompilerCommand:
    """Admit the only preconfigured compiler argv safe for proof custody.

    The grammar permits an optional matching target selector, one exact sysroot
    selector under the caller's policy, and code-generation-only flags. Every
    path, include, plugin, response, linker-forwarding, and unknown operand is
    rejected before a compiler probe can execute it.
    """

    if role not in _COMPILER_ROLES:
        raise ValueError(f"source-extension {role!r} is not a compiler role")
    if sysroot_policy not in {"forbidden", "optional", "required"}:
        raise ValueError("source-extension compiler sysroot policy is invalid")
    if expected_sysroot is not None and sysroot_policy == "forbidden":
        raise ValueError("source-extension compiler has an impossible sysroot policy")
    argv = _exact_argv(command, role=role)
    dialect = validate_source_extension_compiler_dialect(argv, target_triple)
    start = _compiler_argument_start(argv, role=role)
    target = compiler_target_triple(argv, target_triple)
    explicit_target = validate_compiler_target(argv, target)
    if require_explicit_target and not explicit_target:
        raise ValueError(
            f"source-extension {role} compiler has no explicit target selector"
        )

    sysroot = compiler_sysroot_arg_value(argv[start:])
    if sysroot is not None and not (os.path.isabs(sysroot) or sysroot.startswith("~")):
        raise ValueError(
            f"source-extension {role} compiler sysroot must be absolute or selected-home relative"
        )
    if sysroot_policy == "forbidden" and sysroot is not None:
        raise ValueError(f"source-extension {role} compiler has an unowned sysroot")
    if sysroot_policy == "required" and sysroot is None:
        raise ValueError(
            f"source-extension {role} compiler requires an explicit sysroot"
        )
    if expected_sysroot is not None and sysroot != expected_sysroot:
        raise ValueError(
            f"source-extension {role} compiler sysroot differs from captured root"
        )

    frontend_argv = compiler_frontend_arguments(argv)
    index = start
    while index < len(argv):
        option = frontend_argv[index]
        if option.startswith("-std=") and ("++" in option) != (role == "cpp"):
            raise ValueError(
                f"source-extension {role} compiler has a standard for a different language"
            )
        if option in {"-target", "--target", "-arch", "--sysroot", "-isysroot"}:
            _value, index = _option_value(frontend_argv, index, option, role=role)
            continue
        if option.startswith(
            ("-target=", "--target=", "-arch=", "--sysroot=", "-isysroot")
        ):
            index += 1
            continue
        if option in {
            "-m32",
            "-m64",
            "-mx32",
            "--no-default-config",
        } or _safe_codegen_flag(option):
            index += 1
            continue
        if dialect is SourceExtensionCompilerDialect.CLANG_CL and re.fullmatch(
            r"/(?:O[012dsxtb]+|W[0-4]|WX-?|MDd?|MTd?|EH[a-z-]+|Z[7iI]|Brepro|nologo|D[A-Za-z_][A-Za-z0-9_]*(?:=[A-Za-z0-9_+.,()'\" -]*)?|U[A-Za-z_][A-Za-z0-9_]*)",
            option,
        ):
            index += 1
            continue
        _reject_external_selector(option, role=role)
        raise ValueError(
            f"source-extension {role} compiler option {option!r} is outside the "
            "custody-safe positive grammar"
        )

    if dialect is SourceExtensionCompilerDialect.GNU and not is_zig_compiler_command(
        argv
    ):
        expected_mode = "gcc" if role == "c" else "g++"
        if source_extension_compiler_driver_mode(argv) != expected_mode:
            raise ValueError(
                f"source-extension {role} compiler requires {expected_mode} driver mode"
            )
    return SourceExtensionCompilerCommand(argv, explicit_target, sysroot)


def validate_source_extension_tool_command(
    command: Sequence[str],
    *,
    role: str,
    target_triple: str | None = None,
    require_explicit_target: bool = False,
    sysroot_policy: _SYSROOT_POLICY = "forbidden",
    expected_sysroot: str | None = None,
) -> tuple[str, ...]:
    """Validate either a compiler command or an argument-free auxiliary tool."""

    argv = _exact_argv(command, role=role)
    if role in _COMPILER_ROLES:
        if target_triple is None:
            raise ValueError(
                f"source-extension {role} compiler has no target authority"
            )
        return validate_source_extension_compiler_command(
            argv,
            role=role,
            target_triple=target_triple,
            require_explicit_target=require_explicit_target,
            sysroot_policy=sysroot_policy,
            expected_sysroot=expected_sysroot,
        ).argv
    if len(argv) != 1:
        zig_subcommands = {
            "ar": "ar",
            "ranlib": "ranlib",
            "strip": "strip",
        }
        if (
            len(argv) == 2
            and executable_entrypoint_name(Path(argv[0])) == "zig"
            and zig_subcommands.get(role) == argv[1]
        ):
            return argv
        raise ValueError(
            f"source-extension {role} tool must be an argument-free exact executable"
        )
    return argv
