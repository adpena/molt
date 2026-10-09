from __future__ import annotations

import hashlib
import re
import shlex
from collections.abc import Sequence
from pathlib import Path

from molt.rust_toolchain import rust_flag_spans
from molt.cli import atomic_io
from molt.cli.runtime_paths import _build_state_root
from molt.wasi_sdk_identity import WasiCAbiProjection


def wasm_link_output_arguments(
    published: Path, *, staged_output: Path
) -> tuple[str, str, str]:
    """Name the module by its publication coordinate, not its transaction file.

    wasm-ld otherwise embeds the output basename in the name section. Atomic
    publishers use unique staging names, which must not change artifact bytes
    or discard the function/local names needed for debugging.
    """
    if not published.name or "\0" in published.name:
        raise ValueError("WASM publication requires a nonempty module filename")
    return (f"--soname={published.name}", "-o", str(staged_output))


_RUNTIME_LINK_SWITCHES = frozenset(
    {
        "--relocatable",
        "--import-memory",
        "--import-table",
        "--growable-table",
        "--export-table",
        "--no-entry",
        "--allow-undefined",
        "--shared-memory",
        "--shared",
        "--stack-first",
        "--gc-sections",
        "--no-gc-sections",
        "--export-dynamic",
    }
)


def validate_runtime_link_arguments(arguments: Sequence[str]) -> tuple[str, ...]:
    """Admit the generated runtime's non-resource linker argument language."""
    for index, argument in enumerate(arguments):
        if (
            not argument
            or "\0" in argument
            or any(character.isspace() for character in argument)
        ):
            raise ValueError(
                f"runtime linker response argument {index} is empty or contains unsafe whitespace/NUL"
            )
        if "@" in argument:
            raise ValueError(
                f"runtime linker response argument {index} nests an unsupported response resource"
            )
        if argument in _RUNTIME_LINK_SWITCHES:
            continue
        if re.fullmatch(
            r"--export(?:-if-defined)?=[A-Za-z_$][A-Za-z0-9_.$]*", argument
        ):
            continue
        if (
            argument.startswith("--table-base=")
            and argument.removeprefix("--table-base=").isascii()
            and argument.removeprefix("--table-base=").isdigit()
        ):
            if int(argument.removeprefix("--table-base=")) <= 0xFFFFFFFF:
                continue
        raise ValueError(
            f"runtime linker response argument {index} is unsupported or requires explicit resource custody: {argument!r}"
        )
    return tuple(arguments)


def runtime_link_response_arguments(payload: bytes) -> tuple[str, ...]:
    """Decode the generated raw wasm-ld response grammar exactly."""
    try:
        text = payload.decode("utf-8", errors="strict")
    except UnicodeError as exc:
        raise ValueError("runtime linker response is not UTF-8") from exc
    return validate_runtime_link_arguments(text.splitlines())


def wasi_external_libc_rustflags(
    flags: Sequence[str],
    *,
    plan: WasiCAbiProjection,
    include_search: bool = True,
) -> tuple[str, ...]:
    """Admit raw WASM arguments and the selected external-libc Rust mode."""
    result = plan.rustflags(flags, include_search=include_search)
    for span in rust_flag_spans(result):
        option = span.codegen
        if option is None:
            continue
        if option.startswith("link-arg="):
            operand = option.removeprefix("link-arg=")
            if not operand.startswith("@"):
                validate_runtime_link_arguments((operand,))
        elif option.startswith("link-args="):
            raise ValueError(
                "WASI linking requires individually admitted link-arg operands"
            )
    return result


def wasm_link_args_from_rustflags(flags: str) -> list[str]:
    """Extract ordered linker arguments from a Rust flags string."""
    try:
        tokens = shlex.split(flags, posix=True)
    except ValueError as exc:
        raise ValueError(f"invalid Rust flags: {exc}") from exc
    return [
        span.codegen.removeprefix("link-arg=")
        for span in rust_flag_spans(tokens)
        if span.codegen is not None and span.codegen.startswith("link-arg=")
    ]


def write_wasm_link_args_response_file(
    response_root: Path,
    *,
    label: str,
    link_args: Sequence[str],
) -> Path:
    """Publish one content-addressed, byte-stable linker response file."""
    link_args = validate_runtime_link_arguments(link_args)
    payload = ("\n".join(link_args) + "\n").encode("utf-8")
    digest = hashlib.sha256(payload).hexdigest()
    safe_label = re.sub(r"[^A-Za-z0-9_.-]+", "_", label).strip("._-") or "runtime"
    response_path = response_root / f"{safe_label}.{digest}.rsp"
    try:
        current = response_path.read_bytes()
    except OSError:
        current = None
    if current != payload:
        atomic_io._atomic_write_bytes(response_path, payload)
    return response_path.resolve(strict=False)


def wasm_link_args_response_file(
    project_root: Path,
    *,
    label: str,
    link_flags: str,
) -> Path | None:
    """Materialize ordered link flags under the canonical build-state root."""
    link_args = wasm_link_args_from_rustflags(link_flags)
    if not link_args:
        return None
    return write_wasm_link_args_response_file(
        _build_state_root(project_root) / "wasm_link_args",
        label=label,
        link_args=link_args,
    )
