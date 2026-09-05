from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path

from molt.cli.native_link_plan import _normalize_arch
from molt.cli.native_toolchain import _zig_target_query
from molt.llvm_linker_roles import executable_entrypoint_name


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
