"""Guarded external-command and linker symbol-probe authority."""

from __future__ import annotations

from tools.wasm_link_fact_provider import WasmLinkFacts

from collections.abc import Iterable, Mapping, Sequence
import contextlib
import os
from pathlib import Path
import subprocess
import tempfile
import time

from tools.command_execution import CommandExecutor
from molt.wasm_linking_symbols import (
    FLAG_BINDING_GLOBAL,
    SYMBOL_BINDING_MASK,
)
from tools.wasm_link_format import (
    FLAG_UNDEFINED,
    _is_wasm_binary,
    canonical_extern_type,
    generated_function_type,
    is_call_indirect_import_name,
    is_obsolete_call_indirect_import_name,
)


_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_WASM_LINK")


def _run_external_tool(
    cmd: Sequence[str],
    *,
    capture_output: bool = True,
    text: bool = True,
    timeout: float | None = None,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run typed argv under repository process custody.

    Windows imposes a short command-line limit even when the linker itself can
    consume an arbitrary number of inputs. Materialize only oversized
    ``wasm-ld`` invocations as response files; every other command preserves its
    exact argv for the shared executor.
    """

    guarded_cmd = list(cmd)
    response_path: Path | None = None
    if (
        os.name == "nt"
        and Path(guarded_cmd[0]).stem.casefold() == "wasm-ld"
        and len(subprocess.list2cmdline(guarded_cmd)) >= 30_000
    ):
        handle = tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            suffix=".rsp",
            prefix="molt-wasm-ld-",
            delete=False,
        )
        try:
            response_path = Path(handle.name)
            handle.write(
                "\n".join(
                    subprocess.list2cmdline([argument]) for argument in guarded_cmd[1:]
                )
            )
            handle.write("\n")
        finally:
            handle.close()
        guarded_cmd = [guarded_cmd[0], f"@{response_path}"]
    try:
        return _COMMANDS.run(
            guarded_cmd,
            cwd=cwd,
            env=env,
            capture_output=capture_output,
            text=text,
            timeout=timeout,
        )
    finally:
        if response_path is not None:
            with contextlib.suppress(OSError):
                response_path.unlink()


def _wasm_ld_signature_mismatch_warning(stderr: str | None) -> str | None:
    if not stderr or "function signature mismatch:" not in stderr:
        return None
    return stderr.strip()


def _read_wasm_bytes_with_retry(
    path: Path,
    *,
    attempts: int = 8,
    delay_sec: float = 0.05,
) -> bytes:
    data = b""
    for _ in range(max(1, attempts)):
        try:
            data = path.read_bytes()
        except OSError:
            data = b""
        if _is_wasm_binary(data):
            return data
        time.sleep(delay_sec)
    return data


def _deduplicated_export_flags(*groups: Iterable[str]) -> list[str]:
    flags: list[str] = []
    seen: set[str] = set()
    for group in groups:
        for flag in group:
            if flag not in seen:
                seen.add(flag)
                flags.append(flag)
    return flags


def _preflight_relocatable_runtime(
    wasm_ld: str,
    runtime: Path,
) -> str | None:
    # The relink is the check; nothing reads its output. lld buffers a
    # non-regular output in memory, so the null device takes it without the
    # 80 MB scratch write (8 s against 0.3 s on a USB volume).
    result = _run_external_tool(
        [wasm_ld, "-r", "-o", os.devnull, str(runtime)],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        return None
    detail = (result.stderr or result.stdout or "").strip()
    crash = (
        "PLEASE submit a bug report" in detail
        or result.returncode < 0
        or result.returncode > 255
    )
    if crash:
        return (
            "relocatable runtime linker metadata is inconsistent: wasm-ld crashed "
            f"while relinking {runtime}. This commonly means publication stripping "
            "removed sections without rewriting the linking/reloc custom-section "
            "indices; rebuild and republish the reloc runtime from one build identity. "
            f"linker={wasm_ld} returncode={result.returncode}"
        )
    return f"relocatable runtime preflight failed for {runtime}: {detail}"


def _validate_call_indirect_bindings(
    output_facts: WasmLinkFacts,
    runtime_facts: WasmLinkFacts,
) -> None:
    """Require the generated physical ABI at both producer linker boundaries."""
    for item in runtime_facts.imports:
        if item.module == "env" and is_obsolete_call_indirect_import_name(item.name):
            raise ValueError(
                f"runtime call_indirect import uses an obsolete execution ABI identity: {item.name}"
            )
    abi_imports = tuple(
        item
        for item in runtime_facts.imports
        if item.module == "env" and is_call_indirect_import_name(item.name)
    )
    import_count = int(output_facts["function_import_count"])
    function_count = import_count + int(output_facts["defined_function_count"])
    for item in abi_imports:
        if item.kind != 0:
            raise ValueError(
                f"runtime call_indirect import is not a function: {item.name}"
            )
        generated = generated_function_type(item.name)
        if generated is None or canonical_extern_type(
            item.extern_type
        ) != canonical_extern_type(generated):
            raise ValueError(f"runtime call_indirect ABI type mismatch: {item.name}")
        declarations = [
            symbol
            for symbol in runtime_facts.linking_symbols.function_symbols
            if symbol.index == item.index
        ]
        if (
            len(declarations) != 1
            or declarations[0].name != item.name
            or not declarations[0].flags & FLAG_UNDEFINED
            or declarations[0].flags & SYMBOL_BINDING_MASK != FLAG_BINDING_GLOBAL
        ):
            raise ValueError(
                f"runtime call_indirect import lacks its exact physical linker binding: {item.name}"
            )
        exported = output_facts.exports.get(item.name)
        if (
            exported is None
            or exported.kind != 0
            or not import_count <= exported.index < function_count
        ):
            raise ValueError(
                f"app call_indirect export is not a defined function: {item.name}"
            )
        if canonical_extern_type(exported.extern_type) != canonical_extern_type(
            item.extern_type
        ):
            raise ValueError(f"app/runtime call_indirect type mismatch: {item.name}")
        definitions = [
            symbol
            for symbol in output_facts.linking_symbols.symbols
            if symbol.name == item.name
        ]
        if (
            len(definitions) != 1
            or definitions[0].kind != "function"
            or definitions[0].index != exported.index
            or not definitions[0].is_externally_linkable
        ):
            raise ValueError(
                f"app call_indirect export has no unambiguous linkable definition: {item.name}"
            )
        if definitions[0].flags & SYMBOL_BINDING_MASK != FLAG_BINDING_GLOBAL:
            raise ValueError(
                f"app call_indirect definition must have global binding: {item.name}"
            )
