"""Guarded external-command and linker symbol-probe authority."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider

from collections.abc import Iterable, Mapping, Sequence
import contextlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from command_execution import CommandExecutor
from molt.cli import wasm_toolchain
from wasm_link_edit import _add_symtab_alias
from wasm_link_format import (
    CALL_INDIRECT_MANGLED_RE,
    CALL_INDIRECT_RE,
    FLAG_UNDEFINED,
    _is_wasm_binary,
    call_indirect_import_name_for_arity,
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


def _find_wasm_ld() -> str | None:
    """Return the attested ``wasm-ld`` selected by toolchain authority."""

    try:
        identity = wasm_toolchain.resolve_wasm_linker()
    except wasm_toolchain.WasmLinkerContractError as exc:
        print(f"Wasm linker contract failed: {exc}", file=sys.stderr)
        return None
    if identity is None:
        return None
    print(f"Wasm linker identity: {identity.diagnostic}", file=sys.stderr)
    return str(identity.path)


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


def _dump_symbols(
    path: Path,
    *,
    facts_provider: WasmFactsProvider,
) -> list[tuple[int, int, str, str]]:
    try:
        data = path.read_bytes()
    except OSError as exc:
        print(f"Failed to read wasm symbols from {path}: {exc}", file=sys.stderr)
        return []
    try:
        parsed = [
            (symbol.flags, symbol.index, symbol.name, "")
            for symbol in facts_provider(data).linking_symbols.function_symbols
            if symbol.index is not None
        ]
    except ValueError as exc:
        print(
            f"Failed to parse linking symbol table from {path}: {exc}",
            file=sys.stderr,
        )
        return []
    return parsed


def _find_call_indirect_mangled(
    runtime: Path, *, facts_provider: WasmFactsProvider
) -> dict[str, str]:
    names: dict[str, str] = {}
    for flags, _index, name, _flags_text in _dump_symbols(
        runtime, facts_provider=facts_provider
    ):
        if not (flags & FLAG_UNDEFINED):
            continue
        if match := CALL_INDIRECT_RE.fullmatch(name):
            if import_name := call_indirect_import_name_for_arity(match.group(1)):
                names[import_name] = name
            continue
        if match := CALL_INDIRECT_MANGLED_RE.search(name):
            if import_name := call_indirect_import_name_for_arity(match.group(1)):
                names[import_name] = name
    if not names:
        print("Unable to locate runtime call_indirect symbol names.", file=sys.stderr)
    return names


def _find_output_call_indirect_symbol(
    output: Path, *, facts_provider: WasmFactsProvider
) -> dict[str, tuple[int, int]]:
    symbols = {
        name: (index, flags)
        for flags, index, name, _flags_text in _dump_symbols(
            output, facts_provider=facts_provider
        )
        if is_call_indirect_import_name(name)
    }
    if not symbols:
        print("Unable to locate output call_indirect symbols.", file=sys.stderr)
    return symbols


def _inject_call_indirect_alias(
    output: Path,
    runtime: Path,
    temp_dir: tempfile.TemporaryDirectory[str],
    *,
    facts_provider: WasmFactsProvider,
) -> Path:
    mangled = _find_call_indirect_mangled(runtime, facts_provider=facts_provider)
    symbol_info = _find_output_call_indirect_symbol(
        output, facts_provider=facts_provider
    )
    if not mangled or not symbol_info:
        return output
    updated = output.read_bytes()
    modified = False
    for name, mangled_name in mangled.items():
        alias = symbol_info.get(name)
        if alias is None:
            print(f"Unable to locate output {name} symbol.", file=sys.stderr)
            continue
        alias_index, alias_flags = alias
        if next_data := _add_symtab_alias(
            updated,
            mangled_name,
            alias_index,
            alias_flags,
            facts_provider=facts_provider,
        ):
            updated = next_data
            modified = True
    if not modified:
        return output
    alias_path = Path(temp_dir.name) / "output_alias.wasm"
    alias_path.write_bytes(updated)
    return alias_path
