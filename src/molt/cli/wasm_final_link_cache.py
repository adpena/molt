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

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import hashlib
from pathlib import Path
import sys
from typing import assert_never

from molt.cli.wasm_link_cache import (
    WasmLinkCacheBundleHit,
    WasmLinkCacheBundleMiss,
    WasmLinkCacheBundleMissReason,
    WasmLinkCacheEntry,
    _invalidate_wasm_link_cache_bundle,
    _locked_wasm_link_cache_entry,
    _publish_wasm_link_cache_bundle,
    _read_wasm_link_cache_bundle,
    _transfer_file,
    _wasm_link_cache_entry,
)
from molt.exact_json import canonical_json_bytes
from molt.toolchain_identity import StableRegularFileObservation

FINAL_LINK_CACHE_FAMILY = "final_link"
# Bump when the link tool's output contract changes in a way its source
# closure digest cannot see (for example a new environment input).
FINAL_LINK_CACHE_SCHEMA = "final-link-v1"
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
    observed_inputs: Mapping[str, StableRegularFileObservation] | None = None,
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
    files: dict[str, StableRegularFileObservation] = {}
    reused: dict[str, StableRegularFileObservation] = {}
    label = "final link input"
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
        path = (cwd / argument).absolute()
        name = str(path)
        if argument.startswith("-"):
            arguments.append(argument)
            continue
        observation = files.get(name)
        if observation is None and observed_inputs is not None:
            observation = observed_inputs.get(name)
        if observation is not None:
            if str(observation.identity.path) != name:
                raise ValueError(f"{label} changes the selected path: {path}")
            reused[name] = observation
        elif path.is_file():
            observation = StableRegularFileObservation.capture(path, label=label)
        if observation is None:
            arguments.append(argument)
        else:
            files[name] = observation
            arguments.append(f"sha256:{observation.identity.sha256}")
    for observation in reused.values():
        observation.verify(label=label)
    payload = {
        "schema": FINAL_LINK_CACHE_SCHEMA,
        "python": list(sys.version_info[:3]),
        "arguments": arguments,
        "tool_facts": [dict(fact) for fact in tool_facts],
    }
    return hashlib.sha256(canonical_json_bytes(payload)).hexdigest()


def final_link_cache_entry(key: str) -> WasmLinkCacheEntry:
    return _wasm_link_cache_entry(FINAL_LINK_CACHE_FAMILY, FINAL_LINK_CACHE_SCHEMA, key)


@dataclass(frozen=True)
class FinalLinkResult:
    reused: bool
    publication_error: OSError | ValueError | None = None


def ensure_final_link_result(
    entry: WasmLinkCacheEntry,
    outputs: Mapping[str, Path],
    produce: Callable[[], None],
) -> FinalLinkResult:
    """Restore or produce one admitted output family under its existing key lock.

    The nonrecursive producer must return only after all input, selection and
    optimizer admission succeeds. Its exceptions publish nothing and release
    the lock so a waiter can retry. Publication errors leave the admitted private
    outputs available and are returned for the caller's existing warning policy.
    """
    with _locked_wasm_link_cache_entry(entry):
        cached = _read_wasm_link_cache_bundle(entry, outputs.keys())
        match cached:
            case WasmLinkCacheBundleMiss(reason=reason):
                if reason is WasmLinkCacheBundleMissReason.CORRUPT:
                    _invalidate_wasm_link_cache_bundle(entry)
            case WasmLinkCacheBundleHit(files=files):
                for role, path in outputs.items():
                    _transfer_file(files[role], path)
                return FinalLinkResult(reused=True)
            case _:
                assert_never(cached)
        produce()
        try:
            _publish_wasm_link_cache_bundle(entry, outputs)
        except (OSError, ValueError) as exc:
            return FinalLinkResult(reused=False, publication_error=exc)
        return FinalLinkResult(reused=False)
