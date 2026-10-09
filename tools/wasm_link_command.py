"""Guarded external-command and linker symbol-probe authority."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider, WasmLinkFacts

from collections.abc import Iterable, Mapping, Sequence
import contextlib
import os
from pathlib import Path
import subprocess
import tempfile
from molt.temporary_artifacts import OwnedTemporaryDirectory
import time

from command_execution import CommandExecutor
from molt.wasm_linking_symbols import (
    FLAG_BINDING_GLOBAL,
    FLAG_BINDING_WEAK,
    FLAG_EXPLICIT_NAME,
    FLAG_EXPORTED,
    SYMBOL_BINDING_MASK,
    WasmLinkingSymbol,
)
from wasm_link_format import (
    FLAG_UNDEFINED,
    _append_linking_function_symbols,
    _is_wasm_binary,
    canonical_extern_type,
    generated_function_type,
    is_call_indirect_import_name,
)


_COMMANDS = CommandExecutor.for_file(__file__)


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
    scratch_root: Path,
) -> str | None:
    output = scratch_root / "runtime_reloc_preflight.wasm"
    result = _run_external_tool(
        [wasm_ld, "-r", "-o", str(output), str(runtime)],
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


def _call_indirect_alias_entries(
    output_facts: WasmLinkFacts,
    runtime_facts: WasmLinkFacts,
) -> list[tuple[str, int, int]]:
    """Join the generated ABI to opaque linker identities through function indices."""
    abi_imports = tuple(
        item
        for item in runtime_facts.imports
        if item.module == "env" and is_call_indirect_import_name(item.name)
    )
    if not abi_imports:
        return []
    for item in abi_imports:
        if item.kind != 0:
            raise ValueError(
                f"runtime call_indirect import is not a function: {item.name}"
            )
        generated = generated_function_type(item.name)
        if generated is None or canonical_extern_type(item.extern_type) != (
            canonical_extern_type(generated)
        ):
            raise ValueError(f"runtime call_indirect ABI type mismatch: {item.name}")

    imports = {item.index: item for item in abi_imports}
    runtime_aliases: dict[int, set[str]] = {index: set() for index in imports}
    for symbol in runtime_facts.linking_symbols.function_symbols:
        if symbol.index not in imports:
            continue
        if (
            not symbol.flags & FLAG_UNDEFINED
            or symbol.flags & SYMBOL_BINDING_MASK
            not in {FLAG_BINDING_GLOBAL, FLAG_BINDING_WEAK}
            or not symbol.name
        ):
            raise ValueError(
                "runtime call_indirect import has an invalid linker symbol: "
                f"{imports[symbol.index].name} -> {symbol.name!r}"
            )
        runtime_aliases[symbol.index].add(symbol.name)

    relevant_names = {item.name for item in abi_imports}.union(
        *(names for names in runtime_aliases.values())
    )
    output_symbols: dict[str, list[WasmLinkingSymbol]] = {}
    for symbol in output_facts.linking_symbols.symbols:
        if symbol.name in relevant_names:
            output_symbols.setdefault(symbol.name, []).append(symbol)
    import_count = int(output_facts["function_import_count"])
    function_count = import_count + int(output_facts["defined_function_count"])
    pending: dict[str, tuple[int, int]] = {}
    for index, item in imports.items():
        aliases = runtime_aliases[index]
        if not aliases:
            raise ValueError(
                f"runtime call_indirect import has no undefined linker symbol: {item.name}"
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
        definitions = output_symbols.get(item.name, [])
        if not definitions or any(
            symbol.kind != "function"
            or symbol.index != exported.index
            or not symbol.is_externally_linkable
            for symbol in definitions
        ):
            raise ValueError(
                f"app call_indirect export has no unambiguous linkable definition: {item.name}"
            )
        definition_flags = {symbol.flags for symbol in definitions}
        if len(definition_flags) != 1:
            raise ValueError(
                f"app call_indirect definition flags disagree: {item.name}"
            )
        canonical_flags = next(iter(definition_flags))
        if canonical_flags & SYMBOL_BINDING_MASK != FLAG_BINDING_GLOBAL:
            raise ValueError(
                f"app call_indirect definition must have global binding: {item.name}"
            )
        alias_flags = (canonical_flags & ~FLAG_EXPORTED) | FLAG_EXPLICIT_NAME
        for name in sorted(aliases):
            existing = output_symbols.get(name, [])
            if existing:
                if any(
                    symbol.kind != "function"
                    or symbol.index != exported.index
                    or not symbol.is_externally_linkable
                    or (symbol.flags & SYMBOL_BINDING_MASK)
                    != (alias_flags & SYMBOL_BINDING_MASK)
                    for symbol in existing
                ):
                    raise ValueError(
                        f"call_indirect linker alias conflicts with app symbol: {name!r}"
                    )
                continue
            requested = (exported.index, alias_flags)
            if name in pending and pending[name] != requested:
                raise ValueError(
                    f"call_indirect linker alias has conflicting targets: {name!r}"
                )
            pending[name] = requested
    return sorted(
        ((name, index, flags) for name, (index, flags) in pending.items()),
        key=lambda item: (item[1], item[0]),
    )


def _inject_call_indirect_alias(
    output: Path,
    runtime: Path,
    temp_dir: OwnedTemporaryDirectory,
    *,
    facts_provider: WasmFactsProvider,
) -> Path:
    runtime_facts = facts_provider(runtime.read_bytes())
    output_data = output.read_bytes()
    entries = _call_indirect_alias_entries(facts_provider(output_data), runtime_facts)
    updated = _append_linking_function_symbols(
        output_data, entries, facts_provider=facts_provider
    )
    if updated is None:
        return output
    alias_path = Path(temp_dir.name) / "output_alias.wasm"
    alias_path.write_bytes(updated)
    return alias_path
