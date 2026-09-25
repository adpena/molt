"""Positive argv grammar for source-extension compiler custody."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import os
from pathlib import Path
import re
from typing import Literal

from molt.cli.compiler_target import (
    compiler_target_triple,
    is_zig_compiler_command,
    validate_compiler_target,
)
from molt.llvm_linker_roles import executable_entrypoint_name


_COMPILER_ROLES = frozenset({"c", "cpp"})
_TOOL_ROLES = frozenset({"c", "cpp", "ar", "nm", "ld", "ranlib", "strip"})
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

    argv = tuple(command)
    values: list[tuple[int, str, str]] = []
    index = 0
    while index < len(argv):
        option = argv[index]
        if option in {"--sysroot", "-isysroot"}:
            if (
                index + 1 >= len(argv)
                or not argv[index + 1]
                or argv[index + 1].startswith("-")
            ):
                raise ValueError(f"compiler command has {option} missing value")
            values.append((index + 1, "", argv[index + 1]))
            index += 2
            continue
        for prefix in ("--sysroot=", "-isysroot="):
            if option.startswith(prefix):
                value = option.removeprefix(prefix)
                if not value or value.startswith("-"):
                    raise ValueError(f"compiler command has {prefix} missing value")
                values.append((index, prefix, value))
                break
        index += 1
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

    index = start
    while index < len(argv):
        option = argv[index]
        if option in {"-target", "--target", "-arch", "--sysroot", "-isysroot"}:
            _value, index = _option_value(argv, index, option, role=role)
            continue
        if option.startswith(
            ("-target=", "--target=", "-arch=", "--sysroot=", "-isysroot=")
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
        _reject_external_selector(option, role=role)
        raise ValueError(
            f"source-extension {role} compiler option {option!r} is outside the "
            "custody-safe positive grammar"
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
