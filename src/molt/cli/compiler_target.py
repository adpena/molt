from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from enum import StrEnum
from typing import Literal

from molt.cli.native_link_plan import _normalize_arch
from molt.llvm_linker_roles import executable_entrypoint_name


class SourceExtensionCompilerDialect(StrEnum):
    GNU = "gnu"
    CLANG_CL = "clang-cl"

    def forward(self, argument: str) -> str:
        return f"/clang:{argument}" if self is self.CLANG_CL else argument


def source_extension_compiler_dialect(
    command: Sequence[str],
) -> SourceExtensionCompilerDialect:
    if not command:
        raise ValueError("source-extension compiler command is empty")
    name = executable_entrypoint_name(Path(command[0]))
    if name == "cl":
        raise ValueError(
            "source-extension requires clang-cl for canonical Make depfiles"
        )
    selected = (
        SourceExtensionCompilerDialect.CLANG_CL
        if name == "clang-cl"
        else SourceExtensionCompilerDialect.GNU
    )
    for span in compiler_argument_spans(command[1:]):
        argument = span.option
        if span.context == "driver" and argument.startswith("--driver-mode="):
            mode = argument.partition("=")[2]
            if mode not in {"cl", "gcc", "g++"}:
                raise ValueError(
                    f"unsupported source-extension compiler driver mode: {mode}"
                )
            selected = (
                SourceExtensionCompilerDialect.CLANG_CL
                if mode == "cl"
                else SourceExtensionCompilerDialect.GNU
            )
    return selected


def validate_source_extension_compiler_dialect(
    command: Sequence[str],
    target_triple: str,
) -> SourceExtensionCompilerDialect:
    dialect = source_extension_compiler_dialect(command)
    msvc = target_triple.lower().endswith("-windows-msvc")
    if msvc != (dialect is SourceExtensionCompilerDialect.CLANG_CL):
        raise ValueError(
            f"source-extension compiler dialect {dialect.value} is incompatible with "
            f"{target_triple}; Windows MSVC requires clang-cl and other targets require GNU"
        )
    return dialect


def compiler_frontend_arguments(command: Sequence[str]) -> tuple[str, ...]:
    """Expose clang-cl forwarding to the same target/sysroot/flag grammar."""
    return tuple(argument.removeprefix("/clang:") for argument in command)


# This is an operand-boundary grammar, not the positive admission grammar for
# preconfigured tools. Upstream compile units retain arbitrary semantic flags.
COMPILER_OUTPUT_OPTIONS = (
    "-o",
    "-MF",
    "-MT",
    "-MQ",
    "-MJ",
    "/Fo",
    "/Fd",
    "/Fa",
    "/Fe",
    "/Fi",
    "/FR",
    "/sourceDependencies",
    "/scanDependencies",
)
COMPILER_TARGET_OPTIONS = frozenset({"-target", "--target", "-triple"})
COMPILER_OWNED_OPTIONS = COMPILER_TARGET_OPTIONS | {
    "--sysroot",
    "-isysroot",
    "/winsysroot",
    "-arch",
}
_COMPILER_VALUE_OPTIONS = (
    frozenset(COMPILER_OUTPUT_OPTIONS)
    | COMPILER_OWNED_OPTIONS
    | {
        "-I",
        "-D",
        "-U",
        "-isystem",
        "-iquote",
        "-idirafter",
        "-include",
        "-imacros",
        "-x",
        "/I",
        "/D",
        "/U",
        "/FI",
        "/Tc",
        "/Tp",
        "/external:I",
        "/sourceDependencies:directives",
        "-F",
        "-iframework",
        "-iprefix",
        "-iwithprefix",
        "-iwithprefixbefore",
        "-isystem-after",
        "-iframeworkwithsysroot",
        "-resource-dir",
        "--resource-dir",
        "-B",
        "--gcc-toolchain",
        "-gcc-toolchain",
        "-working-directory",
        "-stdlib",
        "-Xlinker",
        "-Xcuda-ptxas",
        "-Xcuda-fatbinary",
        "-Xopenmp-target",
    }
)
_COMPILER_OPAQUE_OPTIONS = frozenset(
    {
        "-mllvm",
        "-Xassembler",
        "-Xpreprocessor",
        "-Xlinker",
        "-Xcuda-ptxas",
        "-Xcuda-fatbinary",
        "-Xopenmp-target",
    }
)


@dataclass(frozen=True, slots=True)
class CompilerArgumentSpan:
    """An option and its operands, retaining transport bytes and argv offsets."""

    index: int
    raw: tuple[str, ...]
    arguments: tuple[str, ...]
    context: Literal["driver", "opaque", "cc1", "positional"]

    @property
    def option(self) -> str:
        return self.arguments[0]

    @property
    def is_output(self) -> bool:
        if self.context != "driver":
            return False
        return (
            self.output_option is not None
            or self.option in {"-MD", "-MMD", "-MP", "/showIncludes", "/nologo", "/FS"}
            or re.fullmatch(r"/(?:MP[0-9]*|FA[cs]*)", self.option) is not None
        )

    @property
    def output_option(self) -> str | None:
        if self.context != "driver":
            return None
        # Clang's longest matching option wins over Joined -o. These are
        # driver option families, not output paths or backend payloads.
        if self.option.startswith(("-objc", "-object-file-name=", "-offload")):
            return None
        return next(
            (
                option
                for option in COMPILER_OUTPUT_OPTIONS
                if self.option.startswith(option)
            ),
            None,
        )


def compiler_argument_spans(
    arguments: Sequence[str],
) -> tuple[CompilerArgumentSpan, ...]:
    """Parse driver operands once; forwarded backend values are never options.

    cc1 is a different language, not a driver flag alias. Only the canonical
    target/sysroot selectors have admitted cc1 custody. Other cc1 commands may
    replace inputs, outputs, language or dependency production, so fail closed.
    """
    raw = tuple(arguments)
    normalized = compiler_frontend_arguments(raw)
    spans: list[CompilerArgumentSpan] = []
    index = 0
    positional = False
    while index < len(raw):
        start = index
        option = normalized[index]
        context: Literal["driver", "opaque", "cc1", "positional"] = (
            "positional" if positional else "driver"
        )
        values = (option,)
        width = 1
        if not positional and option == "--":
            positional = True
        elif not positional and option == "-Xclang":
            if index + 1 >= len(raw) or not normalized[index + 1]:
                raise ValueError("source-extension -Xclang has no frontend operand")
            cc1 = normalized[index + 1]
            context = "cc1"
            width = 2
            values = (cc1,)
            if cc1 in COMPILER_OWNED_OPTIONS:
                if (
                    index + 3 >= len(raw)
                    or normalized[index + 2] != "-Xclang"
                    or not normalized[index + 3]
                    or normalized[index + 3].startswith("-")
                ):
                    raise ValueError(
                        f"source-extension cc1 {cc1} requires a forwarded operand"
                    )
                width = 4
                values = (cc1, normalized[index + 3])
            else:
                prefix = next(
                    (
                        option + "="
                        for option in COMPILER_OWNED_OPTIONS
                        if cc1.startswith(option + "=")
                    ),
                    None,
                )
                if prefix is None:
                    raise ValueError(
                        f"source-extension -Xclang {cc1!r} requires unsupported frontend input/output/language custody"
                    )
                if not cc1[len(prefix) :]:
                    raise ValueError(f"source-extension cc1 {cc1} has no operand")
        elif not positional and option in (
            _COMPILER_VALUE_OPTIONS | _COMPILER_OPAQUE_OPTIONS
        ):
            if index + 1 >= len(raw) or not normalized[index + 1]:
                label = "language" if option == "-x" else "operand"
                raise ValueError(f"source-extension compiler {option} has no {label}")
            width = 2
            values = normalized[index : index + width]
            if option in _COMPILER_OPAQUE_OPTIONS:
                context = "opaque"
        spans.append(
            CompilerArgumentSpan(start, raw[start : start + width], values, context)
        )
        index += width
    return tuple(spans)


def _zig_target_query(target_triple: str) -> str:
    triple = target_triple.strip()
    if not triple:
        return target_triple
    parts = [part for part in triple.split("-") if part]
    if len(parts) < 2:
        return target_triple

    arch_aliases = {
        "amd64": "x86_64",
        "x64": "x86_64",
        "arm64": "aarch64",
        "armv7l": "armv7",
        "i386": "x86",
        "i486": "x86",
        "i586": "x86",
        "i686": "x86",
    }
    os_aliases = {
        "darwin": "macos",
        "macosx": "macos",
        "win32": "windows",
        "mingw32": "windows",
        "mingw64": "windows",
        "cygwin": "windows",
        # Zig names the WASI OS "wasi" regardless of the preview revision
        # encoded in LLVM triples (wasip1/wasip2).
        "wasip1": "wasi",
        "wasip2": "wasi",
    }
    abi_aliases = {
        "sim": "simulator",
        "androideabi": "android",
    }
    abi_tokens = {
        "gnu",
        "gnueabi",
        "gnueabihf",
        "gnuabi64",
        "gnux32",
        "musl",
        "musleabi",
        "musleabihf",
        "msvc",
        "eabi",
        "eabihf",
        "android",
        "simulator",
        "sim",
        "ilp32",
        "uclibc",
        "ohos",
        "macabi",
        "androideabi",
    }
    os_tokens = {
        "linux",
        "windows",
        "darwin",
        "macos",
        "macosx",
        "ios",
        "tvos",
        "watchos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "solaris",
        "haiku",
        "hurd",
        "android",
        "wasi",
        "emscripten",
        "fuchsia",
        "uefi",
        "mingw32",
        "mingw64",
        "cygwin",
        "illumos",
        "aix",
    }

    def is_os_token(token: str) -> bool:
        lowered = token.lower()
        return lowered in os_tokens or lowered in os_aliases

    arch = arch_aliases.get(parts[0].lower(), parts[0].lower())
    remainder = [part.lower() for part in parts[1:]]
    abi = None
    if remainder:
        last = remainder[-1]
        if len(remainder) >= 2 and last in abi_tokens and is_os_token(remainder[-2]):
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
        elif last in abi_tokens and last not in os_tokens:
            abi = abi_aliases.get(last, last)
            remainder = remainder[:-1]
    os_part = remainder[-1] if remainder else None
    vendor_parts = remainder[:-1] if len(remainder) > 1 else []
    if os_part is None:
        return f"{arch}-{abi}" if abi else arch
    os_token = os_part.lower()
    match = re.match(r"^(darwin|macosx|macos|ios|tvos|watchos)([0-9].*)$", os_token)
    if match:
        os_token = match.group(1)
    os_name = os_aliases.get(os_token, os_token)
    if os_name in {"unknown", "none"}:
        os_name = "freestanding"
    if os_name == "windows" and abi is None:
        if any(token in {"w64", "mingw32", "mingw64"} for token in vendor_parts):
            abi = "gnu"
    if os_name in {"mingw32", "mingw64"}:
        os_name = "windows"
        if abi is None:
            abi = "gnu"
    if os_name in {"macos", "ios", "tvos", "watchos"}:
        if abi == "sim":
            abi = "simulator"
        elif os_name == "macos":
            abi = None
        elif abi in {
            "gnu",
            "gnueabi",
            "gnueabihf",
            "gnuabi64",
            "gnux32",
            "musl",
            "musleabi",
            "musleabihf",
            "msvc",
            "android",
            "eabi",
            "eabihf",
            "uclibc",
        }:
            abi = None

    if abi:
        return f"{arch}-{os_name}-{abi}"
    return f"{arch}-{os_name}"


def is_zig_compiler_command(command: Sequence[str]) -> bool:
    return (
        len(command) >= 2
        and executable_entrypoint_name(Path(command[0])) == "zig"
        and command[1] in {"cc", "c++"}
    )


def compiler_target_triple(command: Sequence[str], canonical_target: str) -> str:
    """Project target spelling from the actual driver, not its selection source."""
    return (
        _zig_target_query(canonical_target)
        if is_zig_compiler_command(command)
        else canonical_target
    )


def _compiler_target_values(command: Sequence[str]) -> tuple[str, ...]:
    targets: list[str] = []
    for span in compiler_argument_spans(command):
        if span.context not in {"driver", "cc1"}:
            continue
        argument = span.option
        if argument in COMPILER_TARGET_OPTIONS:
            value = span.arguments[1]
            if value.startswith("-"):
                raise ValueError(
                    f"compiler command has {argument} without a target value"
                )
            targets.append(value)
        else:
            for prefix in ("-target=", "--target=", "-triple="):
                if argument.startswith(prefix):
                    value = argument.removeprefix(prefix)
                    if not value:
                        raise ValueError(
                            f"compiler command has {prefix} without a target value"
                        )
                    targets.append(value)
                    break
    return tuple(targets)


def validate_compiler_target(command: Sequence[str], target_triple: str) -> bool:
    """Reject target/architecture overrides; report explicit triple selectors.

    The expected target uses the selected driver's spelling of the canonical
    target plan (including Zig target-query conversion). Native requests also
    have an exact effective target, even without an appended target selector.
    """
    expected = target_triple.lower()
    configured_targets = _compiler_target_values(command)
    mismatched = sorted(
        {target for target in configured_targets if target.strip().lower() != expected}
    )
    if mismatched:
        raise ValueError(
            "compiler command target conflicts with requested target "
            f"{target_triple}: {', '.join(mismatched)}"
        )
    try:
        expected_arch = _normalize_arch(expected.split("-", 1)[0])
    except RuntimeError as exc:
        raise ValueError(str(exc)) from exc
    for span in compiler_argument_spans(command):
        if span.context not in {"driver", "cc1"}:
            continue
        argument = span.option
        if argument == "-arch" or argument.startswith("-arch="):
            if argument == "-arch":
                if span.arguments[1].startswith("-"):
                    raise ValueError(
                        "compiler command has -arch without an architecture"
                    )
                architecture = span.arguments[1]
            else:
                architecture = argument.removeprefix("-arch=")
            try:
                arch = _normalize_arch(architecture)
            except RuntimeError as exc:
                raise ValueError(str(exc)) from exc
            if arch != expected_arch:
                raise ValueError(
                    f"compiler command -arch {architecture!r} conflicts with "
                    f"requested target {target_triple}"
                )
        elif argument in {"-m32", "-m64", "-mx32"}:
            x32_abi = expected.endswith(("-gnux32", "-muslx32"))
            matches = (
                argument == "-m32"
                and expected_arch in {"x86", "i386", "i486", "i586", "i686"}
                or argument == "-m64"
                and expected_arch == "x86_64"
                and not x32_abi
                or argument == "-mx32"
                and expected_arch == "x86_64"
                and x32_abi
            )
            if not matches:
                raise ValueError(
                    f"compiler command {argument} conflicts with or has no "
                    f"verified width policy for requested target {target_triple}"
                )
    return bool(configured_targets)
