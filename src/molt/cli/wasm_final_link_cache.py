"""Content-addressed reuse of complete final WASM link results.

A final link (``tools/wasm_link.py``: wasm-ld, post-link transforms, wasm-opt,
split-runtime processing) is a pure function of its input bytes, its options,
the published linked module name, the link tool's source closure, and the
optimizer identity. Its outputs carry no output-directory path, so one result
serves every build of the same program, wherever it publishes. The per-output
link receipt still skips a relink of an unchanged deployment in place; this
cache makes a fresh output directory (a new CI row, a temp dir, a second
checkout) cost a copy instead of a link.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
import hashlib
import os
from pathlib import Path
import sys

from molt.cli.wasm_link_cache import (
    WasmLinkCacheEntry,
    _invalidate_wasm_link_cache_bundle,
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_bundle,
    _read_wasm_link_cache_bundle,
    _wasm_link_cache_entry,
)
from molt.exact_json import canonical_json_bytes
from molt.toolchain_identity import stable_regular_file_identity

FINAL_LINK_CACHE_FAMILY = "final_link"
# Bump when the link tool's output contract changes in a way its source
# closure digest cannot see (for example a new environment input).
FINAL_LINK_CACHE_SCHEMA = "final-link-v1"
# Link-tool environment inputs that change output bytes.
_LINK_ENVIRONMENT_INPUTS = ("MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS",)
# The linked module's file name is embedded by wasm-ld; keep it, drop its dir.
_NAMED_OUTPUT_OPTIONS = frozenset({"--output"})
# Private locations whose names carry no output content: split roles have fixed
# names inside the directory, and phase timings are diagnostics only.
_LOCATION_ONLY_OPTIONS = frozenset({"--split-output-dir"})
_DIAGNOSTIC_OPTIONS = frozenset({"--phase-timings-file", "--failure-evidence-dir"})


def final_link_cache_key(
    link_cmd: Sequence[str],
    *,
    cwd: Path,
    tool_facts: Sequence[Mapping[str, object]],
) -> str:
    """Key one final link by content: files by digest, outputs by name only.

    ``link_cmd`` is the complete tool invocation (interpreter, tool, options).
    Every argument that names an existing regular file contributes its SHA-256,
    so an input added to the command later is keyed without code changes.
    The linked output contributes only its file name, which wasm-ld embeds as
    the module name; private directories and diagnostics contribute nothing.
    """
    if len(link_cmd) < 2:
        raise ValueError("final link command must name an interpreter and a tool")
    arguments: list[str] = []
    pending: str | None = None
    for argument in link_cmd[2:]:
        if pending is not None:
            if pending in _NAMED_OUTPUT_OPTIONS:
                arguments.append(f"output:{Path(argument).name}")
            pending = None
            continue
        if argument in _DIAGNOSTIC_OPTIONS:
            pending = argument
            continue
        if argument in _NAMED_OUTPUT_OPTIONS or argument in _LOCATION_ONLY_OPTIONS:
            arguments.append(argument)
            pending = argument
            continue
        # The tool resolves relative paths against its working directory.
        path = Path(argument) if Path(argument).is_absolute() else cwd / argument
        if not argument.startswith("-") and path.is_file():
            identity = stable_regular_file_identity(path, label="final link input")
            arguments.append(f"sha256:{identity.sha256}")
        else:
            arguments.append(argument)
    payload = {
        "schema": FINAL_LINK_CACHE_SCHEMA,
        "python": list(sys.version_info[:3]),
        "arguments": arguments,
        "tool_facts": [dict(fact) for fact in tool_facts],
        "environment": {
            name: os.environ.get(name, "") for name in _LINK_ENVIRONMENT_INPUTS
        },
    }
    return hashlib.sha256(canonical_json_bytes(payload)).hexdigest()


def final_link_cache_entry(key: str) -> WasmLinkCacheEntry:
    return _wasm_link_cache_entry(FINAL_LINK_CACHE_FAMILY, FINAL_LINK_CACHE_SCHEMA, key)


def restore_final_link_result(
    entry: WasmLinkCacheEntry, outputs: Mapping[str, Path]
) -> bool:
    """Materialize a cached result into ``outputs``; False on a miss.

    A present but invalid entry is removed so the next link republishes it.
    """
    with _locked_wasm_link_cache_entry(entry):
        cached = _read_wasm_link_cache_bundle(entry, outputs.keys())
        if cached.status == "corrupt":
            _invalidate_wasm_link_cache_bundle(entry)
    if cached.files is None:
        return False
    for role, path in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(cached.files[role])
    return True


def publish_final_link_result(
    entry: WasmLinkCacheEntry, outputs: Mapping[str, Path]
) -> None:
    """Record the complete private output family of one successful link."""
    files = {role: path.read_bytes() for role, path in outputs.items()}
    with _locked_wasm_link_cache_entry(entry):
        _publish_wasm_link_cache_bundle(entry, files)
