from __future__ import annotations

import re
from collections.abc import Sequence
from pathlib import Path

from molt.cli.native_link_plan import _normalize_arch
from molt.llvm_linker_roles import executable_entrypoint_name


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
    index = 0
    while index < len(command):
        argument = command[index]
        if argument in {"-target", "--target"}:
            if index + 1 >= len(command) or command[index + 1].startswith("-"):
                raise ValueError(
                    f"compiler command has {argument} without a target value"
                )
            targets.append(command[index + 1])
            index += 2
            continue
        for prefix in ("-target=", "--target="):
            if argument.startswith(prefix):
                value = argument.removeprefix(prefix)
                if not value:
                    raise ValueError(
                        f"compiler command has {prefix} without a target value"
                    )
                targets.append(value)
                break
        index += 1
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
    index = 0
    while index < len(command):
        argument = command[index]
        if argument == "-arch" or argument.startswith("-arch="):
            if argument == "-arch":
                index += 1
                if index >= len(command) or command[index].startswith("-"):
                    raise ValueError(
                        "compiler command has -arch without an architecture"
                    )
                architecture = command[index]
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
        index += 1
    return bool(configured_targets)
