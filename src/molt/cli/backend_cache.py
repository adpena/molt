from __future__ import annotations

import contextlib
from contextlib import contextmanager
import functools
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import time
from dataclasses import dataclass, replace
from typing import Any, Collection, Iterator, Mapping, Sequence, cast
import uuid

from molt.cli.artifact_sync import (
    _artifact_sync_state_matches,
    _artifact_sync_state_path,
    _read_artifact_sync_state,
    _write_artifact_sync_state,
)
from molt.cli.atomic_io import (
    _atomic_copy_file,
    _atomic_write_json,
    _link_failure_wants_copy,
)
from molt.file_locks import (
    _acquire_file_lock,
    _parse_lock_timeout,
    _release_file_lock,
)
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.backend_artifact_contract import (
    BackendArtifactContract,
    BackendArtifactKind,
    BackendArtifactValidationError,
)
from molt.cli.cache_keys import _cache_key, _sorted_ir_functions
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.file_hashing import _sha256_file, content_change_time_ns
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.cli.llvm_wasi_tools import llvm_tool_candidates
from molt.cli import function_references as _function_references
from molt.cli.models import (
    _ModuleGraphMetadata,
    _SharedStdlibCacheValidationToken,
)


_DEAD_FUNCTION_ELIM_REFERENCE_KINDS = _function_references.FUNCTION_REFERENCE_OP_KINDS
_emitted_name_matches_module_symbol = (
    _function_references.emitted_name_matches_module_symbol
)
_is_protected_runtime_entrypoint = _function_references.is_protected_runtime_entrypoint
_module_symbol_name = _function_references.module_symbol_name
reachable_function_names = _function_references.reachable_function_names

_NativeObjectSymbolSets = tuple[set[str], set[str]]


@dataclass(frozen=True, slots=True)
class _NativeGlobalSymbolFacts:
    defined: frozenset[str]
    undefined: frozenset[str]
    defined_functions: frozenset[str]
    # Weak undefined references are neither providers nor required link inputs.
    weak_undefined: frozenset[str] = frozenset()
    artifact_digest: str | None = None

    def symbol_sets(self) -> _NativeObjectSymbolSets:
        return set(self.defined), set(self.undefined)


class NativeSymbolInspectionError(OSError):
    """Required symbol evidence was unavailable, never an empty symbol table."""

    def __init__(self, path: Path, attempts: Sequence[str]) -> None:
        self.path = path
        self.attempts = tuple(attempts)
        super().__init__(
            f"Cannot inspect native symbols for {path}: " + "; ".join(self.attempts)
        )


def _native_symbol_artifact_identity(path: Path) -> StableRegularFileIdentity:
    """Use the shared direct-file content and mutation identity authority."""
    try:
        return stable_regular_file_identity(
            path.resolve(strict=True), label="native symbol artifact"
        )
    except (OSError, ValueError) as error:
        raise NativeSymbolInspectionError(path, [str(error)]) from error


def _require_unchanged_symbol_artifact(
    path: Path, identity: StableRegularFileIdentity
) -> None:
    try:
        if path.resolve(strict=True) != identity.path.resolve(strict=True):
            raise ValueError("artifact path no longer names its captured generation")
        verify_stable_regular_file_identity(identity, label="native symbol artifact")
    except (OSError, ValueError) as error:
        raise NativeSymbolInspectionError(
            path,
            [
                "artifact changed during symbol inspection; no facts were published",
                str(error),
            ],
        ) from error


_NativeObjectSymbolCacheKey = tuple[
    str, int, int, int, str, str, str, str, str, tuple[str, ...]
]
_NativeArchiveSymbolCacheKey = tuple[
    str, int, int, int, str, str, str, str, tuple[str, ...], str
]
_NATIVE_OBJECT_SYMBOL_SETS_CACHE: dict[
    _NativeObjectSymbolCacheKey,
    _NativeGlobalSymbolFacts,
] = {}
_NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT = 256
_NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION = 4
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT = 32
_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION = 4
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE: dict[
    _NativeArchiveSymbolCacheKey,
    _NativeGlobalSymbolFacts,
] = {}
_SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION = 2


def _record_backend_cache_stage_ms(
    stage_timings_ms: dict[str, float] | None,
    name: str,
    started_at: float,
) -> None:
    if stage_timings_ms is None:
        return
    elapsed_ms = max(0.0, (time.perf_counter() - started_at) * 1000.0)
    stage_timings_ms[name] = round(
        stage_timings_ms.get(name, 0.0) + elapsed_ms,
        6,
    )


def _validate_backend_cache_artifact(
    path: Path,
    *,
    artifact_contract: BackendArtifactContract,
    identity: StableRegularFileIdentity | None = None,
) -> StableRegularFileIdentity:
    # Shape, symbol facts and the sync receipt must attest the same generation.
    # Hash once, then retain the shared cheap mutation token through admission.
    try:
        if identity is None:
            identity = stable_regular_file_identity(
                path, label="backend cache artifact"
            )
        else:
            if identity.path != path.expanduser().absolute():
                raise ValueError("Backend validation identity belongs to another path")
            verify_stable_regular_file_identity(
                identity, label="backend cache artifact"
            )
    except (OSError, ValueError) as error:
        raise BackendArtifactValidationError(str(error)) from error
    artifact_contract.validate(path)
    if artifact_contract.is_native:
        facts = _native_object_global_symbol_facts(
            path, target_triple=artifact_contract.target_triple, identity=identity
        )
        # Empty symbol tables are valid reader results, not application hits.
        if not (facts.defined or facts.undefined):
            raise BackendArtifactValidationError(
                f"Native application cache artifact has no symbol surface: {path}"
            )
    try:
        verify_stable_regular_file_identity(identity, label="backend cache artifact")
    except (OSError, ValueError) as error:
        raise BackendArtifactValidationError(str(error)) from error
    return identity


def _is_valid_cached_backend_artifact(
    path: Path, *, artifact_contract: BackendArtifactContract
) -> bool:
    try:
        _validate_backend_cache_artifact(path, artifact_contract=artifact_contract)
    except BackendArtifactValidationError:
        return False
    return True


def _target_uses_macho_symbol_decoration(target_triple: str | None) -> bool:
    from molt.cli.native_link_plan import (
        NativeObjectFormat,
        resolve_native_target_spec,
        target_is_wasm,
    )

    if target_triple is not None and target_is_wasm(target_triple):
        return False
    return (
        resolve_native_target_spec(target_triple).object_format
        is NativeObjectFormat.MACHO
    )


def _normalize_native_symbol_name(
    name: str,
    *,
    target_triple: str | None = None,
) -> str:
    if _target_uses_macho_symbol_decoration(target_triple) and name.startswith("_"):
        return name[1:]
    return name


def _symbol_normalization_target(target_triple: str | None) -> str:
    from molt.cli.native_link_plan import resolve_native_target_spec

    target = (
        resolve_native_target_spec(None).triple
        if target_triple is None
        else target_triple.strip().lower()
    )
    return f"target:{target}"


def _native_nm_command(nm_command: Sequence[str], path: Path) -> list[str]:
    return [*nm_command, "-g", str(path)]


def _nm_line_reports_no_symbols(
    line: str, result: subprocess.CompletedProcess[str]
) -> bool:
    argv = result.args
    if isinstance(argv, str) or not argv:
        return False
    artifact = str(argv[-1])
    tool = str(argv[0])
    if line == "no symbols":
        return True
    for name in {tool, Path(tool).name}:
        if line.startswith(f"{name}: "):
            line = line[len(name) + 2 :]
            break
    if not line.endswith(": no symbols"):
        return False
    owner = line[: -len(": no symbols")]
    return any(
        owner == prefix or (owner.startswith(prefix + "(") and owner.endswith(")"))
        for prefix in {artifact, Path(artifact).name}
    )


def _nm_result_reports_no_symbols(result: subprocess.CompletedProcess[str]) -> bool:
    # rc=1 is accepted only for a wholly empty artifact; partial archive output
    # plus a failed member must never be promoted into complete evidence.
    lines = [
        line.strip()
        for line in f"{result.stdout}\n{result.stderr}".splitlines()
        if line.strip()
    ]
    return bool(lines) and all(
        _nm_line_reports_no_symbols(line, result) for line in lines
    )


def _nm_read_timeout(default: float) -> float:
    """Resolve the ``nm``/``llvm-nm`` object-symbol read timeout.

    ``llvm-nm -g <object>`` is a bounded, read-only, non-spawning leaf tool, but
    on slow volumes (network/OneDrive-backed checkouts, antivirus-scanned exFAT
    build roots) a single spawn + read can exceed a few seconds. Expose the
    ceiling via ``MOLT_NM_TIMEOUT_SEC`` so an operator on a slow host can raise
    it without patching; the tight default keeps healthy hosts fast.
    """
    raw = os.environ.get("MOLT_NM_TIMEOUT_SEC")
    if raw:
        try:
            value = float(raw)
        except ValueError:
            value = 0.0
        if value > 0:
            return value
    return default


def _read_native_global_symbol_facts(
    path: Path,
    *,
    timeout: float,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
) -> _NativeGlobalSymbolFacts:
    candidates = (
        [tuple(nm_command)]
        if nm_command is not None
        else [(candidate,) for candidate in _nm_candidate_binaries()]
    )
    if not candidates:
        raise NativeSymbolInspectionError(
            path, ["no nm/llvm-nm candidate is available"]
        )
    read_timeout = _nm_read_timeout(timeout)
    failures: list[str] = []
    primary: BaseException | None = None
    for candidate in candidates:
        try:
            # Reading a static object's global symbol table is a leaf,
            # non-spawning, read-only operation: it can neither orphan a process
            # tree nor run away on memory, so it does NOT go through the
            # process-tree memory guard. Guarding it here regressed on slow
            # hosts, where the guard's per-call repo-scoped orphan cleanup blew
            # past the read timeout and killed a healthy `llvm-nm` mid-output
            # (rc=124), stalling every source-recompiled extension seal at the
            # object-fact step. A plain subprocess timeout is the correct bound.
            result = _run_completed_command(
                _native_nm_command(candidate, path),
                capture_output=True,
                timeout=read_timeout,
                env=None,
                cwd=path.parent,
                memory_guard_prefix=None,
                errors="strict",
            )
        except (OSError, subprocess.SubprocessError, UnicodeError) as error:
            if primary is None:
                primary = error
            failures.append(f"{tuple(candidate)!r}: {type(error).__name__}: {error}")
            continue
        if result.returncode in {0, 1} and _nm_result_reports_no_symbols(result):
            return _NativeGlobalSymbolFacts(frozenset(), frozenset(), frozenset())
        stderr_lines = [
            line.strip() for line in result.stderr.splitlines() if line.strip()
        ]
        if result.returncode == 0 and all(
            _nm_line_reports_no_symbols(line, result) for line in stderr_lines
        ):
            try:
                facts = _parse_native_nm_global_symbol_facts(
                    "\n".join(
                        line
                        for line in result.stdout.splitlines()
                        if not _nm_line_reports_no_symbols(line.strip(), result)
                    ),
                    target_triple=target_triple,
                )
            except ValueError as error:
                if primary is None:
                    primary = error
                failures.append(f"{tuple(candidate)!r}: {error}")
                continue
            return facts
        failures.append(
            f"{tuple(candidate)!r}: exit {result.returncode}; "
            f"stdout={result.stdout[:2048]!r}; stderr={result.stderr[:2048]!r}"
        )
    raise NativeSymbolInspectionError(path, failures) from primary


def _native_object_symbol_facts_sidecar_path(path: Path) -> Path:
    return path.with_suffix(".symbols.json")


def _native_object_symbol_cache_key(
    path: Path,
    object_digest: str,
    *,
    nm_command: Sequence[str] | None,
    target_triple: str | None,
) -> _NativeObjectSymbolCacheKey | None:
    try:
        resolved = path.resolve()
        stat = path.stat()
    except OSError:
        return None
    return (
        os.fspath(resolved),
        int(stat.st_size),
        int(stat.st_mtime_ns),
        int(getattr(stat, "st_ctime_ns", 0)),
        object_digest,
        os.environ.get("MOLT_TARGET_ROOT", ""),
        os.environ.get("PATH", ""),
        os.environ.get("MOLT_NM_TIMEOUT_SEC", ""),
        _symbol_normalization_target(target_triple),
        tuple(nm_command or ()),
    )


def _native_object_symbol_facts_payload(
    *,
    object_digest: str,
    facts: _NativeGlobalSymbolFacts,
    target_triple: str | None,
) -> dict[str, object]:
    return {
        "schema": _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION,
        "platform": sys.platform,
        "symbol_target": _symbol_normalization_target(target_triple),
        "object_digest": object_digest,
        "defined": sorted(facts.defined),
        "undefined": sorted(facts.undefined),
        "defined_functions": sorted(facts.defined_functions),
        "weak_undefined": sorted(facts.weak_undefined),
    }


def _read_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
    target_triple: str | None,
) -> _NativeGlobalSymbolFacts | None:
    try:
        payload = json.loads(
            _native_object_symbol_facts_sidecar_path(path).read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION:
        return None
    if payload.get("platform") != sys.platform:
        return None
    if payload.get("symbol_target") != _symbol_normalization_target(target_triple):
        return None
    if payload.get("object_digest") != object_digest:
        return None
    defined = payload.get("defined")
    undefined = payload.get("undefined")
    defined_functions = payload.get("defined_functions")
    weak_undefined = payload.get("weak_undefined")
    if not (
        isinstance(defined, list)
        and isinstance(undefined, list)
        and isinstance(defined_functions, list)
        and isinstance(weak_undefined, list)
    ):
        return None
    if not all(
        isinstance(symbol, str)
        for symbol in (*defined, *undefined, *defined_functions, *weak_undefined)
    ):
        return None
    facts = _NativeGlobalSymbolFacts(
        defined=frozenset(cast(list[str], defined)),
        undefined=frozenset(cast(list[str], undefined)),
        defined_functions=frozenset(cast(list[str], defined_functions)),
        weak_undefined=frozenset(cast(list[str], weak_undefined)),
        artifact_digest=object_digest,
    )
    if not facts.defined_functions <= facts.defined:
        return None
    return facts


def _write_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
    facts: _NativeGlobalSymbolFacts,
    target_triple: str | None,
) -> None:
    payload = _native_object_symbol_facts_payload(
        object_digest=object_digest,
        facts=facts,
        target_triple=target_triple,
    )
    _atomic_write_json(
        _native_object_symbol_facts_sidecar_path(path),
        payload,
        indent=2,
    )


def _native_object_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeGlobalSymbolFacts:
    if identity is None:
        identity = _native_symbol_artifact_identity(path)
    else:
        _require_unchanged_symbol_artifact(path, identity)
    object_digest = identity.sha256
    cache_key = _native_object_symbol_cache_key(
        path,
        object_digest,
        nm_command=nm_command,
        target_triple=target_triple,
    )
    if cache_key is not None:
        cached = _NATIVE_OBJECT_SYMBOL_SETS_CACHE.get(cache_key)
        if cached is not None:
            _require_unchanged_symbol_artifact(path, identity)
            return cached
    if object_digest:
        symbol_facts = _read_native_object_symbol_facts(
            path,
            object_digest=object_digest,
            target_triple=target_triple,
        )
        if symbol_facts is not None:
            _require_unchanged_symbol_artifact(path, identity)
            if cache_key is not None:
                _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = symbol_facts
            return symbol_facts
    facts = _read_native_global_symbol_facts(
        path,
        timeout=5,
        nm_command=nm_command,
        target_triple=target_triple,
    )
    _require_unchanged_symbol_artifact(path, identity)
    facts = replace(facts, artifact_digest=object_digest)
    if cache_key is not None:
        if (
            len(_NATIVE_OBJECT_SYMBOL_SETS_CACHE)
            >= _NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT
        ):
            _NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
        _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = facts
    if object_digest:
        with contextlib.suppress(OSError):
            _write_native_object_symbol_facts(
                path,
                object_digest=object_digest,
                facts=facts,
                target_triple=target_triple,
            )
    _require_unchanged_symbol_artifact(path, identity)
    return facts


def _native_object_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeObjectSymbolSets:
    facts = _native_object_global_symbol_facts(
        path,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
    )
    return facts.symbol_sets()


def _parse_native_nm_global_symbol_facts(
    output: str,
    *,
    target_triple: str | None = None,
) -> _NativeGlobalSymbolFacts:
    """Parse global ``nm`` facts for one object or static archive.

    LLVM ``nm`` emits archive-member header lines between ordinary symbol rows.
    Keeping the parser shared makes archive-backed linker custody use the same
    symbol semantics as source-extension object closure without creating symbol
    sidecars inside managed Rust/WASI toolchains.
    """

    defined: set[str] = set()
    undefined: set[str] = set()
    defined_functions: set[str] = set()
    weak_undefined: set[str] = set()
    macho_decoration = _target_uses_macho_symbol_decoration(target_triple)
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.endswith(":"):
            # nm's archive member/architecture label, not a symbol row.
            if line.lower() in {"error:", "warning:", "fatal error:"}:
                raise ValueError(f"nm diagnostic is not a symbol row: {line!r}")
            continue
        indirect_target: str | None = None
        indirect = re.fullmatch(r"(.*) \(indirect for ([^\s]+)\)", line)
        if indirect:
            line, indirect_target = indirect.groups()
        parts = line.split()
        if len(parts) == 2:
            kind, name = parts
        elif len(parts) == 3 and re.fullmatch(r"[0-9a-fA-F]+", parts[0]):
            _, kind, name = parts
        else:
            raise ValueError(f"unrecognized nm symbol row: {line[:512]!r}")
        if len(kind) != 1 or kind not in "AaBbCcDdGgIiRrSsTtUuVvWw":
            raise ValueError(f"unsupported nm symbol type in row: {line[:512]!r}")
        symbol = name[1:] if macho_decoration and name.startswith("_") else name
        if indirect_target is not None:
            if kind != "I":
                raise ValueError(
                    f"indirect target on a non-indirect nm row: {raw_line[:512]!r}"
                )
            undefined.add(
                indirect_target[1:]
                if macho_decoration and indirect_target.startswith("_")
                else indirect_target
            )
        if kind == "U":
            undefined.add(symbol)
        elif kind in {"w", "v"}:
            weak_undefined.add(symbol)
        else:
            defined.add(symbol)
            if kind in {"T", "t", "W", "i"}:
                defined_functions.add(symbol)
    return _NativeGlobalSymbolFacts(
        defined=frozenset(defined),
        undefined=frozenset(undefined),
        defined_functions=frozenset(defined_functions),
        weak_undefined=frozenset(weak_undefined),
    )


def _parse_native_nm_global_symbol_sets(
    output: str,
    *,
    target_triple: str | None = None,
) -> _NativeObjectSymbolSets:
    return _parse_native_nm_global_symbol_facts(
        output,
        target_triple=target_triple,
    ).symbol_sets()


def _native_archive_global_symbol_facts(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeGlobalSymbolFacts:
    """Read one provider archive's globals without mutating the toolchain.

    Provider archives are immutable installation inputs, not build outputs.
    Their symbol facts therefore use bounded process caching plus one central,
    content-keyed cache under Molt's cache root; unlike object facts, this function
    never writes a ``*.symbols.json`` sibling into Rust or WASI SDK directories.
    """

    if identity is None:
        identity = _native_symbol_artifact_identity(path)
    else:
        _require_unchanged_symbol_artifact(path, identity)
    resolved = identity.path
    stat = resolved.stat()
    changed = content_change_time_ns(resolved, stat)
    if changed is None:
        raise NativeSymbolInspectionError(
            path, ["content-change identity is unavailable"]
        )
    symbol_target = _symbol_normalization_target(target_triple)
    cache_key: _NativeArchiveSymbolCacheKey = (
        os.fspath(resolved),
        identity.size,
        stat.st_mtime_ns,
        changed,
        os.environ.get("MOLT_TARGET_ROOT", ""),
        os.environ.get("PATH", ""),
        os.environ.get("MOLT_NM_TIMEOUT_SEC", ""),
        symbol_target,
        tuple(nm_command or ()),
        identity.sha256,
    )
    cached = _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.get(cache_key)
    if cached is not None:
        _require_unchanged_symbol_artifact(path, identity)
        return cached
    persistent_cache_path = _native_archive_symbol_cache_path(cache_key)
    persistent_facts = _read_native_archive_symbol_cache(
        persistent_cache_path,
        cache_key=cache_key,
    )
    if persistent_facts is not None:
        _require_unchanged_symbol_artifact(path, identity)
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = persistent_facts
        return persistent_facts
    facts = _read_native_global_symbol_facts(
        resolved,
        timeout=120,
        nm_command=nm_command,
        target_triple=target_triple,
    )
    _require_unchanged_symbol_artifact(path, identity)
    facts = replace(facts, artifact_digest=identity.sha256)
    if (
        len(_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE)
        >= _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT
    ):
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = facts
    with contextlib.suppress(OSError):
        _write_native_archive_symbol_cache(
            persistent_cache_path,
            cache_key=cache_key,
            facts=facts,
        )
    _require_unchanged_symbol_artifact(path, identity)
    return facts


def _native_archive_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> _NativeObjectSymbolSets:
    facts = _native_archive_global_symbol_facts(
        path,
        nm_command=nm_command,
        target_triple=target_triple,
        identity=identity,
    )
    return facts.symbol_sets()


def _native_archive_symbol_cache_identity(
    cache_key: _NativeArchiveSymbolCacheKey,
) -> dict[str, object]:
    return {
        "path": cache_key[0],
        "size": cache_key[1],
        "mtime_ns": cache_key[2],
        "ctime_ns": cache_key[3],
        "target_root": cache_key[4],
        "path_env": cache_key[5],
        "timeout_env": cache_key[6],
        "symbol_target": cache_key[7],
        "nm_command": list(cache_key[8]),
        "artifact_digest": cache_key[9],
    }


def _native_archive_symbol_cache_path(
    cache_key: _NativeArchiveSymbolCacheKey,
) -> Path:
    identity = _native_archive_symbol_cache_identity(cache_key)
    digest = hashlib.sha256(
        json.dumps(identity, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return (
        _default_molt_cache()
        / "toolchain_symbol_facts"
        / f"v{_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION}"
        / f"{digest}.json"
    )


def _read_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: _NativeArchiveSymbolCacheKey,
) -> _NativeGlobalSymbolFacts | None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION:
        return None
    if payload.get("identity") != _native_archive_symbol_cache_identity(cache_key):
        return None
    defined = payload.get("defined")
    undefined = payload.get("undefined")
    defined_functions = payload.get("defined_functions")
    weak_undefined = payload.get("weak_undefined")
    if not (
        isinstance(defined, list)
        and isinstance(undefined, list)
        and isinstance(defined_functions, list)
        and isinstance(weak_undefined, list)
    ):
        return None
    if not all(
        isinstance(symbol, str)
        for symbol in (*defined, *undefined, *defined_functions, *weak_undefined)
    ):
        return None
    facts = _NativeGlobalSymbolFacts(
        defined=frozenset(symbol for symbol in defined if isinstance(symbol, str)),
        undefined=frozenset(symbol for symbol in undefined if isinstance(symbol, str)),
        defined_functions=frozenset(
            symbol for symbol in defined_functions if isinstance(symbol, str)
        ),
        weak_undefined=frozenset(cast(list[str], weak_undefined)),
        artifact_digest=cache_key[9],
    )
    if not facts.defined_functions <= facts.defined:
        return None
    return facts


def _write_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: _NativeArchiveSymbolCacheKey,
    facts: _NativeGlobalSymbolFacts,
) -> None:
    _atomic_write_json(
        path,
        {
            "schema": _NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION,
            "identity": _native_archive_symbol_cache_identity(cache_key),
            "defined": sorted(facts.defined),
            "undefined": sorted(facts.undefined),
            "defined_functions": sorted(facts.defined_functions),
            "weak_undefined": sorted(facts.weak_undefined),
        },
        indent=None,
        sort_keys=True,
    )


def _native_object_has_unresolved_module_chunks(
    candidate: Path,
    stdlib_object_path: Path | None,
    *,
    target_triple: str | None = None,
    identity: StableRegularFileIdentity | None = None,
) -> bool:
    candidate_symbols = _native_object_global_symbol_sets(
        candidate, target_triple=target_triple, identity=identity
    )
    defined, undefined = candidate_symbols
    # Compiler archives whole-load all members. A reference and its provider
    # can appear in different member symbol tables without being unresolved.
    unresolved_chunks = {
        symbol for symbol in undefined - defined if "__molt_module_chunk_" in symbol
    }
    if not unresolved_chunks:
        return False
    stdlib_defined: set[str] = set()
    if stdlib_object_path is not None:
        stdlib_symbols = _native_object_global_symbol_sets(
            stdlib_object_path, target_triple=target_triple
        )
        stdlib_defined, _ = stdlib_symbols
    return any(symbol not in stdlib_defined for symbol in unresolved_chunks)


def _read_shared_stdlib_partition_functions(
    stdlib_object_path: Path,
) -> frozenset[str] | None:
    try:
        raw = _stdlib_object_partition_manifest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
        payload = json.loads(raw)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _SHARED_STDLIB_PARTITION_SCHEMA_VERSION:
        return None
    raw_functions = payload.get("functions")
    if not isinstance(raw_functions, list) or not all(
        isinstance(name, str) and name for name in raw_functions
    ):
        return None
    functions = cast(list[str], raw_functions)
    function_count = payload.get("function_count")
    if isinstance(function_count, int) and function_count != len(functions):
        return None
    return frozenset(functions)


def _unresolved_stdlib_module_symbols(
    undefined_symbols: Collection[str],
    stdlib_module_symbols: Collection[str],
) -> tuple[str, ...]:
    module_symbols = tuple(sorted(set(stdlib_module_symbols)))
    if not module_symbols:
        return ()
    unresolved: list[str] = []
    for symbol in sorted(set(undefined_symbols)):
        if symbol.startswith("molt_"):
            continue
        if any(
            _emitted_name_matches_module_symbol(symbol, module_symbol)
            for module_symbol in module_symbols
        ):
            unresolved.append(symbol)
    return tuple(unresolved)


def _shared_stdlib_native_symbol_closure_issue(
    stdlib_object_path: Path,
    *,
    stdlib_module_symbols: Collection[str] | None,
    target_triple: str | None = None,
) -> str | None:
    symbol_sets = _native_object_global_symbol_sets(
        stdlib_object_path, target_triple=target_triple
    )
    defined, undefined = symbol_sets
    # All members of a compiler archive are included by the final link plan.
    # References between those members are resolved within this artifact, even
    # though nm reports both their provider and consumer rows independently.
    undefined = undefined - defined
    issues: list[str] = []

    partition_functions = _read_shared_stdlib_partition_functions(stdlib_object_path)
    if partition_functions is None:
        issues.append("missing or malformed partition manifest")
    else:
        missing_definitions = sorted(partition_functions - defined)
        if missing_definitions:
            preview = ", ".join(missing_definitions[:8])
            suffix = "" if len(missing_definitions) <= 8 else ", ..."
            issues.append(f"missing partition definitions: {preview}{suffix}")
        unresolved_declared = sorted(partition_functions & undefined)
        if unresolved_declared:
            preview = ", ".join(unresolved_declared[:8])
            suffix = "" if len(unresolved_declared) <= 8 else ", ..."
            issues.append(f"unresolved partition references: {preview}{suffix}")

    if stdlib_module_symbols is not None:
        unresolved_stdlib = _unresolved_stdlib_module_symbols(
            undefined, stdlib_module_symbols
        )
        if unresolved_stdlib:
            preview = ", ".join(unresolved_stdlib[:8])
            suffix = "" if len(unresolved_stdlib) <= 8 else ", ..."
            issues.append(f"unresolved stdlib module references: {preview}{suffix}")

    return "; ".join(issues) if issues else None


def _nm_candidate_binaries() -> list[str]:
    """Ordered candidate `nm` binaries for reading the runtime staticlib.

    The staticlib's members are LLVM *bitcode* when the runtime profile builds
    with LTO, and bitcode is only readable by an ``llvm-nm`` whose LLVM is at
    least as new as the producing rustc's. Apple's Xcode ``nm`` (an older LLVM
    reader) rejects newer Rust bitcode with ``Unknown attribute kind`` — the
    failure that silently broke symbol extraction when the toolchain moved to
    Rust 1.96/LLVM 22 while ``shutil.which("nm")`` kept resolving to Xcode's.
    Order newest/most-capable readers first; the extraction loop validates each
    candidate (clean exit AND a non-empty ``molt_*`` set) before trusting it.
    """
    return [
        str(path) for path in llvm_tool_candidates("nm", include_rust_toolchain=True)
    ]


@functools.lru_cache(maxsize=64)
def _shared_cache_lock_dir_cached(cache_root_str: str) -> Path:
    return Path(cache_root_str) / "locks"


@contextmanager
def _shared_cache_lock(name: str, *, cache_root: Path | None = None):
    if cache_root is None:
        cache_root = _default_molt_cache()
    lock_dir = _shared_cache_lock_dir_cached(os.fspath(cache_root))
    lock_path = lock_dir / f"{name}.lock"
    timeout_raw = (
        os.environ.get("MOLT_CACHE_LOCK_TIMEOUT", "").strip()
        or os.environ.get("MOLT_BUILD_LOCK_TIMEOUT", "").strip()
    )
    lock_timeout = _parse_lock_timeout(timeout_raw, default_s=300.0)
    timeout_label = "unbounded" if lock_timeout is None else f"{lock_timeout:.1f}s"
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=lock_timeout,
        timeout_message=(
            "Timed out waiting for shared cache lock "
            f"{lock_path} after {timeout_label}. "
            "Check for stale molt build/backend helper processes."
        ),
    )
    try:
        yield
    finally:
        _release_file_lock(handle)


def _immutable_publish_lock_name(dst: Path) -> str:
    """Stable per-destination lock name for the no-hard-link copy-publish path."""
    digest = hashlib.sha256(str(dst.resolve()).encode("utf-8")).hexdigest()[:16]
    return f"immutable-publish-{digest}"


def _copy_verified_backend_artifact(
    src: Path, dst: Path, *, identity: StableRegularFileIdentity
) -> StableRegularFileIdentity:
    """Publish attested bytes without aliasing mutable output to immutable cache."""
    try:
        if identity.path != src.expanduser().absolute():
            raise ValueError("Backend copy identity belongs to another source path")
        verify_stable_regular_file_identity(identity, label="backend copy source")
        _atomic_copy_file(src, dst, expected_sha256=identity.sha256)
        copied = stable_regular_file_identity(dst, label="backend copied artifact")
        if copied.sha256 != identity.sha256 or copied.size != identity.size:
            raise ValueError("Backend destination changed after verified publication")
        return copied
    except (OSError, ValueError) as error:
        raise BackendArtifactValidationError(
            f"Cannot publish attested backend artifact {src} to {dst}: {error}"
        ) from error


def _require_matching_backend_content(
    expected: StableRegularFileIdentity, actual: StableRegularFileIdentity
) -> None:
    if actual.sha256 != expected.sha256 or actual.size != expected.size:
        raise BackendArtifactValidationError(
            f"Backend cache key has conflicting content: {actual.path}; "
            f"expected {expected.sha256}, found {actual.sha256}"
        )


def _publish_immutable_backend_cache_artifact(
    src: Path,
    dst: Path,
    *,
    artifact_contract: BackendArtifactContract,
    warnings: list[str],
    identity: StableRegularFileIdentity | None = None,
) -> StableRegularFileIdentity:
    """Publish one admitted generation; a key cannot silently name other bytes."""
    identity = _validate_backend_cache_artifact(
        src, artifact_contract=artifact_contract, identity=identity
    )
    dst.parent.mkdir(parents=True, exist_ok=True)

    def existing_generation() -> StableRegularFileIdentity:
        try:
            existing = _validate_backend_cache_artifact(
                dst, artifact_contract=artifact_contract
            )
        except BackendArtifactValidationError:
            warnings.append(
                f"Ignoring invalid immutable cache artifact; cleanup owns removal: {dst}"
            )
            return identity
        _require_matching_backend_content(identity, existing)
        return existing

    if dst.exists():
        return existing_generation()
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        # Copy through the existing verified publication primitive. Linking the
        # caller's writable source into a cache transfers neither ownership nor
        # immutability; a later in-place write would poison every linked tier.
        _copy_verified_backend_artifact(src, tmp_path, identity=identity)
        try:
            os.link(tmp_path, dst)
        except FileExistsError:
            return existing_generation()
        except OSError as exc:
            if not _link_failure_wants_copy(exc):
                raise
            with _shared_cache_lock(
                _immutable_publish_lock_name(dst), cache_root=dst.parent
            ):
                if dst.exists():
                    return existing_generation()
                os.replace(tmp_path, dst)
    finally:
        try:
            tmp_path.unlink(missing_ok=True)
        except OSError as error:
            warnings.append(
                f"Backend cache publication temporary cleanup failed: {error}"
            )
    # Removing the last private hard link changes the published inode's change
    # time. Capture the returned generation only after that owned mutation, not
    # before finally runs; peers and consumers must see the final identity.
    return existing_generation()


def _materialize_cached_backend_artifact(
    project_root: Path,
    candidate: Path,
    output_artifact: Path,
    *,
    tier: str,
    source_key: str,
    cache_path: Path | None,
    module_cache_key: str | None = None,
    warnings: list[str],
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
    candidate_identity: StableRegularFileIdentity | None = None,
) -> bool:
    try:
        candidate_identity = _validate_backend_cache_artifact(
            candidate, artifact_contract=artifact_contract, identity=candidate_identity
        )
    except OSError as exc:
        warnings.append(f"Cache candidate admission failed: {exc}")
        return False
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        with contextlib.suppress(OSError):
            output_stat = output_artifact.stat()
    if output_stat is not None:
        synced_identity = _backend_artifact_sync_identity(
            state,
            source_key=source_key,
            tier=tier,
            artifact=output_artifact,
            artifact_contract=artifact_contract,
        )
        if synced_identity is not None:
            try:
                _require_matching_backend_content(candidate_identity, synced_identity)
            except BackendArtifactValidationError as exc:
                warnings.append(str(exc))
                return False
            return True
    sync_tier = tier
    sync_source_key = source_key
    try:
        output_identity = _copy_verified_backend_artifact(
            candidate, output_artifact, identity=candidate_identity
        )
        if tier == "function" and cache_path is not None and candidate != cache_path:
            try:
                published_module_cache = _publish_immutable_backend_cache_artifact(
                    candidate,
                    cache_path,
                    artifact_contract=artifact_contract,
                    warnings=warnings,
                    identity=candidate_identity,
                )
                if (
                    module_cache_key
                    and published_module_cache.path
                    == cache_path.expanduser().absolute()
                ):
                    # Once the canonical module cache path is valid, future
                    # daemon sync checks should treat output.o as module-synced
                    # rather than function-only.
                    sync_tier = "module"
                    sync_source_key = module_cache_key
            except OSError as exc:
                warnings.append(f"Module cache promotion failed: {exc}")
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=sync_source_key,
                tier=sync_tier,
                artifact=output_artifact,
                identity=output_identity,
            )
        except OSError as exc:
            warnings.append(f"Backend cache sync receipt write failed: {exc}")
        return True
    except OSError as exc:
        warnings.append(f"Cache copy failed: {exc}")
        return False


def _backend_artifact_sync_identity(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
    artifact_contract: BackendArtifactContract,
) -> StableRegularFileIdentity | None:
    # Reject nonmatching source/tier before any byte inspection. A receipt owns
    # exactly one tier, so checking module and function reuse never hashes twice.
    if (
        not source_key
        or state is None
        or state.get("source_key") != source_key
        or state.get("tier") != tier
    ):
        return None
    try:
        identity = _validate_backend_cache_artifact(
            artifact, artifact_contract=artifact_contract
        )
    except BackendArtifactValidationError:
        return None
    if not _artifact_sync_state_matches(
        state,
        source_key=source_key,
        tier=tier,
        artifact=artifact,
        identity=identity,
    ):
        return None
    return identity


@dataclass(frozen=True, slots=True)
class _SyncedBackendOutput:
    tier: str
    identity: StableRegularFileIdentity


def _synced_backend_output_cache_hit(
    state: dict[str, Any] | None,
    output_artifact: Path,
    output_stat: os.stat_result | None,
    *,
    artifact_contract: BackendArtifactContract,
    cache_key: str | None,
    function_cache_key: str | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_path: Path | None = None,
) -> _SyncedBackendOutput | None:
    if output_stat is None:
        return None
    for tier, key in (("module", cache_key), ("function", function_cache_key)):
        identity = _backend_artifact_sync_identity(
            state,
            source_key=_backend_artifact_source_key(
                key,
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            tier=tier,
            artifact=output_artifact,
            artifact_contract=artifact_contract,
        )
        if identity is None:
            continue
        if artifact_contract.is_native and _native_object_has_unresolved_module_chunks(
            output_artifact,
            stdlib_object_path,
            target_triple=artifact_contract.target_triple,
            identity=identity,
        ):
            return None
        return _SyncedBackendOutput(tier, identity)
    return None


def _validated_stdlib_contract_token_for_backend_cache_hit(
    *,
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None,
    stage_timings_ms: dict[str, float] | None,
    target_triple: str | None = None,
) -> tuple[bool, _SharedStdlibCacheValidationToken | None]:
    if stdlib_object_path is None:
        return True, None
    stage_start = time.perf_counter()
    active_stdlib_contract_token = _shared_stdlib_cache_validation_token(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=target_triple,
        previous_token=stdlib_contract_validation_token,
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_contract_validate",
        stage_start,
    )
    return active_stdlib_contract_token is not None, active_stdlib_contract_token


def _backend_artifact_source_key(
    base_key: str | None,
    *,
    stdlib_object_cache_key: str | None,
    artifact_contract: BackendArtifactContract,
) -> str:
    artifact_contract.validate_shared_stdlib(enabled=bool(stdlib_object_cache_key))
    if not base_key:
        # Cache disabled (e.g. --rebuild): return empty key so the daemon
        # does not match against a shared sentinel that is identical for
        # every file.  Previously `base_key or ""` produced the same
        # "|stdlib:<hash>" key for every --rebuild invocation, causing the
        # daemon in-memory cache to return the first file's compiled output
        # for all subsequent files in the same daemon session.
        return ""
    key = f"{base_key}|artifact:{artifact_contract.cache_identity}"
    if not stdlib_object_cache_key:
        return key
    return f"{key}|stdlib:{stdlib_object_cache_key}"


def _backend_cache_artifact_path(
    cache_root: Path,
    base_key: str | None,
    *,
    stdlib_object_cache_key: str | None,
    artifact_contract: BackendArtifactContract,
) -> Path | None:
    source_key = _backend_artifact_source_key(
        base_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        artifact_contract=artifact_contract,
    )
    if not source_key:
        return None
    filename_key = source_key.replace("|artifact:", ".artifact-").replace(
        "|stdlib:", ".stdlib-"
    )
    return cache_root / f"{filename_key}{artifact_contract.suffix}"


def _try_cached_backend_candidates(
    *,
    project_root: Path,
    cache_candidates: Sequence[tuple[str, Path]],
    output_artifact: Path,
    artifact_contract: BackendArtifactContract,
    cache_key: str | None,
    function_cache_key: str | None,
    cache_path: Path | None,
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    warnings: list[str],
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> tuple[bool, str | None]:
    artifact_contract.validate_shared_stdlib(
        enabled=stdlib_object_path is not None or bool(stdlib_object_cache_key)
    )
    stage_start = time.perf_counter()
    state_path = _artifact_sync_state_path(project_root, output_artifact)
    state = _read_artifact_sync_state(state_path)
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_sync_state",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        output_stat: os.stat_result | None = output_artifact.stat()
    except OSError:
        output_stat = None
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_output_stat",
        stage_start,
    )
    stdlib_contract_valid, _active_stdlib_contract_token = (
        _validated_stdlib_contract_token_for_backend_cache_hit(
            stdlib_object_path=stdlib_object_path,
            stdlib_object_cache_key=stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            stdlib_contract_validation_token=stdlib_contract_validation_token,
            stage_timings_ms=stage_timings_ms,
            target_triple=artifact_contract.target_triple,
        )
    )
    del _active_stdlib_contract_token
    if not stdlib_contract_valid:
        if stdlib_object_path is not None and stdlib_object_path.exists():
            warnings.append(
                "Ignoring shared stdlib cache with mismatched contract: "
                + _shared_stdlib_cache_mismatch_detail(
                    stdlib_object_path,
                    stdlib_object_cache_key,
                    stdlib_object_manifest=stdlib_object_manifest,
                    stdlib_module_symbols=stdlib_module_symbols,
                    target_triple=artifact_contract.target_triple,
                )
            )
        # Native application archive cache hits are invalid without the matching
        # stdlib_shared object they were compiled against.
        return False, None

    stage_start = time.perf_counter()
    synced = _synced_backend_output_cache_hit(
        state,
        output_artifact,
        output_stat,
        artifact_contract=artifact_contract,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_path=stdlib_object_path,
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_synced_output",
        stage_start,
    )
    if synced is not None:
        return True, synced.tier

    for tier, candidate in cache_candidates:
        stage_start = time.perf_counter()
        if not candidate.exists():
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_candidate_exists",
                stage_start,
            )
            continue
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_candidate_exists",
            stage_start,
        )
        stage_start = time.perf_counter()
        try:
            candidate_identity = _validate_backend_cache_artifact(
                candidate, artifact_contract=artifact_contract
            )
        except BackendArtifactValidationError:
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_artifact_valid",
                stage_start,
            )
            warnings.append(f"Ignoring invalid cache artifact: {candidate}")
            continue
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_artifact_valid",
            stage_start,
        )
        stage_start = time.perf_counter()
        if artifact_contract.is_native and _native_object_has_unresolved_module_chunks(
            candidate,
            stdlib_object_path,
            target_triple=artifact_contract.target_triple,
            identity=candidate_identity,
        ):
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_unresolved_chunks",
                stage_start,
            )
            warnings.append(
                "Ignoring native cache artifact with unresolved user module chunks: "
                f"{candidate}"
            )
            continue
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_unresolved_chunks",
            stage_start,
        )
        stage_start = time.perf_counter()
        if _materialize_cached_backend_artifact(
            project_root,
            candidate,
            output_artifact,
            tier=tier,
            source_key=_backend_artifact_source_key(
                cache_key
                if tier == "module"
                else (function_cache_key or cache_key or ""),
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            cache_path=cache_path,
            module_cache_key=_backend_artifact_source_key(
                cache_key,
                stdlib_object_cache_key=stdlib_object_cache_key,
                artifact_contract=artifact_contract,
            ),
            warnings=warnings,
            state_path=state_path,
            state=state,
            output_stat=output_stat,
            artifact_contract=artifact_contract,
            candidate_identity=candidate_identity,
        ):
            _record_backend_cache_stage_ms(
                stage_timings_ms,
                "backend_cache_try_materialize",
                stage_start,
            )
            return True, tier
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_try_materialize",
            stage_start,
        )
    return False, None


def _backend_daemon_skip_output_sync_flags(
    project_root: Path,
    output_artifact: Path,
    *,
    cache_key: str | None,
    function_cache_key: str | None,
    stdlib_object_path: Path | None = None,
    stdlib_object_cache_key: str | None = None,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
) -> tuple[bool, bool]:
    artifact_contract.validate_shared_stdlib(
        enabled=stdlib_object_path is not None or bool(stdlib_object_cache_key)
    )
    if output_stat is None:
        try:
            output_stat = output_artifact.stat()
        except OSError:
            return False, False
    if stdlib_object_path is not None and not _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=artifact_contract.target_triple,
    ):
        return False, False
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    synced = _synced_backend_output_cache_hit(
        state,
        output_artifact,
        output_stat,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_path=stdlib_object_path,
        artifact_contract=artifact_contract,
    )
    if synced is None:
        return False, False
    return synced.tier == "module", synced.tier == "function"


@contextmanager
def _temporary_backend_output_path(
    artifacts_root: Path,
    *,
    artifact_contract: BackendArtifactContract,
) -> Iterator[Path]:
    suffix = artifact_contract.suffix
    artifacts_root.mkdir(parents=True, exist_ok=True)
    path = artifacts_root / f"backend_{os.getpid()}_{uuid.uuid4().hex}{suffix}"
    try:
        yield path
    finally:
        with contextlib.suppress(OSError):
            path.unlink()


def _stage_backend_output_and_caches(
    project_root: Path,
    backend_output: Path,
    output_artifact: Path,
    *,
    cache_path: Path | None,
    cache_key: str | None,
    stdlib_object_cache_key: str | None,
    function_cache_path: Path | None,
    warnings: list[str],
    output_already_synced: bool | None = None,
    state_path: Path | None = None,
    state: dict[str, Any] | None = None,
    output_stat: os.stat_result | None = None,
    artifact_contract: BackendArtifactContract,
) -> str | None:
    artifact_contract.validate_shared_stdlib(enabled=bool(stdlib_object_cache_key))
    try:
        staged_identity = _validate_backend_cache_artifact(
            backend_output, artifact_contract=artifact_contract
        )
        if output_artifact.parent != Path("."):
            output_artifact.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return f"Failed to move backend output: {exc}"

    staged_source = backend_output
    if cache_path is not None:
        if backend_output != cache_path:
            try:
                staged_identity = _publish_immutable_backend_cache_artifact(
                    backend_output,
                    cache_path,
                    artifact_contract=artifact_contract,
                    warnings=warnings,
                    identity=staged_identity,
                )
                staged_source = staged_identity.path
            except OSError as exc:
                return f"Failed to publish backend cache output: {exc}"
        else:
            staged_source = cache_path

    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
    if output_already_synced is not False:
        if state is None:
            state = _read_artifact_sync_state(state_path)
        if output_stat is None:
            try:
                output_stat = output_artifact.stat()
            except OSError:
                output_stat = None
        synced_identity = (
            _backend_artifact_sync_identity(
                state,
                source_key=_backend_artifact_source_key(
                    cache_key,
                    stdlib_object_cache_key=stdlib_object_cache_key,
                    artifact_contract=artifact_contract,
                ),
                tier="module",
                artifact=output_artifact,
                artifact_contract=artifact_contract,
            )
            if cache_key and output_stat is not None
            else None
        )
        output_already_synced = synced_identity is not None
        if synced_identity is not None:
            try:
                _require_matching_backend_content(staged_identity, synced_identity)
            except BackendArtifactValidationError as exc:
                return str(exc)
    try:
        if output_already_synced:
            pass
        elif staged_source == output_artifact:
            verify_stable_regular_file_identity(
                staged_identity, label="backend staged output"
            )
            output_identity = staged_identity
        else:
            output_identity = _copy_verified_backend_artifact(
                staged_source, output_artifact, identity=staged_identity
            )
    except (OSError, ValueError) as exc:
        return f"Failed to move backend output: {exc}"

    if cache_path is not None:
        if function_cache_path is not None and function_cache_path != cache_path:
            try:
                _publish_immutable_backend_cache_artifact(
                    staged_source,
                    function_cache_path,
                    artifact_contract=artifact_contract,
                    warnings=warnings,
                    identity=staged_identity,
                )
            except OSError as exc:
                warnings.append(f"Function cache write failed: {exc}")
        if cache_key and not output_already_synced:
            try:
                state_path.parent.mkdir(parents=True, exist_ok=True)
                _write_artifact_sync_state(
                    state_path,
                    source_key=_backend_artifact_source_key(
                        cache_key,
                        stdlib_object_cache_key=stdlib_object_cache_key,
                        artifact_contract=artifact_contract,
                    ),
                    tier="module",
                    artifact=output_artifact,
                    identity=output_identity,
                )
            except OSError as exc:
                warnings.append(f"Backend cache sync receipt write failed: {exc}")

    if backend_output not in {output_artifact, cache_path, function_cache_path}:
        try:
            backend_output.unlink(missing_ok=True)
        except OSError as exc:
            warnings.append(f"Backend private output cleanup failed: {exc}")
    return None


def _stdlib_object_count_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".count")


def _stdlib_object_key_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".key")


def _stdlib_object_manifest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".manifest.json")


def _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".partition.json")


def _stdlib_object_digest_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".sha256")


def _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_suffix(".symbol-contract.json")


def _shared_stdlib_publish_lock_path(stdlib_object_path: Path) -> Path:
    return stdlib_object_path.with_name(f"{stdlib_object_path.name}.publish.lock")


def _shared_stdlib_manifest(
    *,
    cache_key: str | None,
    cache_variant: str,
    target_triple: str | None,
    compiler_fingerprint: str | None = None,
) -> str | None:
    if not cache_key:
        return None
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint()
    payload = {
        "schema": _SHARED_STDLIB_MANIFEST_SCHEMA_VERSION,
        "artifact_kind": "archive",
        "cache_key": cache_key,
        "cache_variant": cache_variant,
        "compiler_fingerprint": compiler_fingerprint,
        "target_triple": target_triple,
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":"))


@contextmanager
def _shared_stdlib_cache_lock(stdlib_object_path: Path) -> Iterator[None]:
    lock_path = _shared_stdlib_publish_lock_path(stdlib_object_path)
    handle = _acquire_file_lock(
        lock_path,
        timeout_s=None,
        timeout_message=f"Timed out waiting for shared stdlib cache lock {lock_path}.",
    )
    try:
        yield
    finally:
        _release_file_lock(handle)


def _stage_shared_stdlib_object_for_link(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    artifacts_root: Path,
    target_triple: str | None = None,
) -> Path:
    # A stable build-owned destination keeps the linker off the mutable cache
    # generation, including when the source itself lives under artifacts_root.
    staged_stdlib_obj = artifacts_root / "shared-stdlib-link" / stdlib_object_path.name
    if staged_stdlib_obj.resolve() == stdlib_object_path.resolve():
        raise OSError(
            f"Shared stdlib link snapshot aliases its source: {stdlib_object_path}"
        )
    staged_key_path = _stdlib_object_key_sidecar_path(staged_stdlib_obj)
    staged_count_path = _stdlib_object_count_sidecar_path(staged_stdlib_obj)
    staged_manifest_path = _stdlib_object_manifest_sidecar_path(staged_stdlib_obj)
    staged_partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        staged_stdlib_obj
    )
    staged_digest_path = _stdlib_object_digest_sidecar_path(staged_stdlib_obj)
    source_key_path = _stdlib_object_key_sidecar_path(stdlib_object_path)
    source_count_path = _stdlib_object_count_sidecar_path(stdlib_object_path)
    source_manifest_path = _stdlib_object_manifest_sidecar_path(stdlib_object_path)
    source_partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        stdlib_object_path
    )
    source_digest_path = _stdlib_object_digest_sidecar_path(stdlib_object_path)
    try:
        with _shared_stdlib_cache_lock(stdlib_object_path):
            generation = _shared_stdlib_cache_generation_token(
                stdlib_object_path,
                stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                target_triple=target_triple,
            )
            if generation is None:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"generation unavailable for {stdlib_object_path}"
                )
            identities = {entry.path: entry for entry in generation[1]}
            if not _shared_stdlib_cache_matches_key(
                stdlib_object_path,
                stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                target_triple=target_triple,
            ):
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    + _shared_stdlib_cache_mismatch_detail(
                        stdlib_object_path,
                        stdlib_object_cache_key,
                        stdlib_object_manifest=stdlib_object_manifest,
                        stdlib_module_symbols=stdlib_module_symbols,
                        target_triple=target_triple,
                    )
                )

            def copy_snapshot(source: Path, destination: Path) -> None:
                identity = identities.get(source.expanduser().absolute())
                if identity is None:
                    # Optional count metadata is not part of semantic admission.
                    identity = _native_symbol_artifact_identity(source)
                _copy_verified_backend_artifact(source, destination, identity=identity)

            copy_snapshot(stdlib_object_path, staged_stdlib_obj)
            if source_key_path.exists():
                copy_snapshot(source_key_path, staged_key_path)
            elif stdlib_object_cache_key:
                raise OSError(
                    "Shared stdlib cache key mismatch during staging: "
                    f"missing key sidecar for {stdlib_object_path}"
                )
            elif staged_key_path.exists():
                staged_key_path.unlink()
            if source_count_path.exists():
                copy_snapshot(source_count_path, staged_count_path)
            elif staged_count_path.exists():
                staged_count_path.unlink()
            if source_manifest_path.exists():
                copy_snapshot(source_manifest_path, staged_manifest_path)
            elif stdlib_object_manifest:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing manifest sidecar for {stdlib_object_path}"
                )
            elif staged_manifest_path.exists():
                staged_manifest_path.unlink()
            if source_partition_manifest_path.exists():
                copy_snapshot(
                    source_partition_manifest_path, staged_partition_manifest_path
                )
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing partition manifest sidecar for {stdlib_object_path}"
                )
            if source_digest_path.exists():
                copy_snapshot(source_digest_path, staged_digest_path)
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing object digest sidecar for {stdlib_object_path}"
                )
    except OSError as exc:
        try:
            _remove_shared_stdlib_cache_artifacts(staged_stdlib_obj)
        except OSError as cleanup_error:
            exc.add_note(f"Shared stdlib staging cleanup also failed: {cleanup_error}")
        raise
    return staged_stdlib_obj


def _remove_shared_stdlib_cache_artifacts(stdlib_object_path: Path) -> None:
    """Attempt every owned path; a partial invalidation is never a cache miss."""
    paths = (
        stdlib_object_path,
        _stdlib_object_count_sidecar_path(stdlib_object_path),
        _stdlib_object_key_sidecar_path(stdlib_object_path),
        _stdlib_object_manifest_sidecar_path(stdlib_object_path),
        _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path),
        _stdlib_object_digest_sidecar_path(stdlib_object_path),
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path),
        _native_object_symbol_facts_sidecar_path(stdlib_object_path),
    )
    failures: list[tuple[Path, OSError]] = []
    for path in paths:
        try:
            path.unlink()
        except FileNotFoundError:
            continue
        except OSError as exc:
            failures.append((path, exc))
    if failures:
        detail = "; ".join(f"{path}: {exc}" for path, exc in failures)
        raise OSError(
            f"Failed to remove shared stdlib cache artifacts: {detail}"
        ) from ExceptionGroup(
            "Shared stdlib artifact deletion failures",
            [exc for _path, exc in failures],
        )


def _shared_stdlib_artifact_validation_error(
    path: Path, *, target_triple: str | None
) -> BackendArtifactValidationError | None:
    contract = BackendArtifactContract(
        BackendArtifactKind.NATIVE_ARCHIVE, target_triple
    )
    try:
        contract.validate(path)
    except BackendArtifactValidationError as error:
        return error
    return None


def _shared_stdlib_cache_matches_key(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    target_triple: str | None = None,
) -> bool:
    if (
        stdlib_object_path is None
        or stdlib_object_cache_key is None
        or stdlib_object_manifest is None
    ):
        return False
    stage_start = time.perf_counter()
    if not stdlib_object_path.exists():
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_exists",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_exists",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        cached_key = _stdlib_object_key_sidecar_path(stdlib_object_path).read_text(
            encoding="utf-8"
        )
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    if cached_key.strip() != stdlib_object_cache_key:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    try:
        cached_manifest = _stdlib_object_manifest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    if cached_manifest.strip() != stdlib_object_manifest:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    partition_manifest_path = _stdlib_object_partition_manifest_sidecar_path(
        stdlib_object_path
    )
    if not partition_manifest_path.exists():
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    try:
        cached_object_digest = _stdlib_object_digest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_sidecars",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_sidecars",
        stage_start,
    )
    stage_start = time.perf_counter()
    try:
        actual_object_digest = _sha256_file(stdlib_object_path)
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_digest",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_digest",
        stage_start,
    )
    if cached_object_digest.strip().lower() != actual_object_digest.lower():
        return False
    stage_start = time.perf_counter()
    artifact_error = _shared_stdlib_artifact_validation_error(
        stdlib_object_path, target_triple=target_triple
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_artifact",
        stage_start,
    )
    if artifact_error is not None:
        return False
    stage_start = time.perf_counter()
    try:
        partition_manifest_digest = _sha256_file(partition_manifest_path)
    except OSError:
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_partition",
            stage_start,
        )
        return False
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_partition",
        stage_start,
    )
    stage_start = time.perf_counter()
    if _shared_stdlib_symbol_contract_matches(
        stdlib_object_path,
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=actual_object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    ):
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_symbol_token",
            stage_start,
        )
        return True
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_symbol_token",
        stage_start,
    )
    stage_start = time.perf_counter()
    symbol_closure_ok = (
        _shared_stdlib_native_symbol_closure_issue(
            stdlib_object_path,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        is None
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_stdlib_contract_symbols",
        stage_start,
    )
    if symbol_closure_ok:
        with contextlib.suppress(OSError):
            _write_shared_stdlib_symbol_contract(
                stdlib_object_path,
                stdlib_object_cache_key=stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
                object_digest=actual_object_digest,
                partition_manifest_digest=partition_manifest_digest,
                target_triple=target_triple,
            )
    return symbol_closure_ok


def _shared_stdlib_cache_matches_key_locked(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
    evict_corrupt: bool = False,
    target_triple: str | None = None,
) -> bool:
    return (
        _shared_stdlib_cache_validation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            evict_corrupt=evict_corrupt,
            stage_timings_ms=stage_timings_ms,
        )
        is not None
    )


def _shared_stdlib_contract_identity(
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> str:
    payload = {
        "symbol_schema": _SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION,
        "symbol_target": _symbol_normalization_target(target_triple),
        "key": stdlib_object_cache_key,
        "manifest": stdlib_object_manifest,
        "symbols": sorted(set(stdlib_module_symbols or ())),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _shared_stdlib_symbol_contract_payload(
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> dict[str, object]:
    return {
        "schema": _SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION,
        "contract_identity": _shared_stdlib_contract_identity(
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        ),
        "object_digest": object_digest,
        "partition_manifest_digest": partition_manifest_digest,
    }


def _shared_stdlib_symbol_contract_matches(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> bool:
    path = _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path)
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    expected = _shared_stdlib_symbol_contract_payload(
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    )
    return payload == expected


def _write_shared_stdlib_symbol_contract(
    stdlib_object_path: Path,
    *,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    object_digest: str,
    partition_manifest_digest: str,
    target_triple: str | None = None,
) -> None:
    payload = _shared_stdlib_symbol_contract_payload(
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=object_digest,
        partition_manifest_digest=partition_manifest_digest,
        target_triple=target_triple,
    )
    _atomic_write_json(
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path),
        payload,
        indent=2,
    )


def _shared_stdlib_cache_validation_file_token(
    path: Path,
) -> StableRegularFileIdentity | None:
    try:
        return _native_symbol_artifact_identity(path)
    except NativeSymbolInspectionError:
        return None


def _shared_stdlib_cache_generation_token(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> _SharedStdlibCacheValidationToken | None:
    if stdlib_object_path is None:
        return None
    paths = [
        stdlib_object_path,
        _stdlib_object_key_sidecar_path(stdlib_object_path),
        _stdlib_object_digest_sidecar_path(stdlib_object_path),
        _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path),
    ]
    if stdlib_object_manifest is not None:
        paths.append(_stdlib_object_manifest_sidecar_path(stdlib_object_path))
    entries: list[StableRegularFileIdentity] = []
    for path in paths:
        token = _shared_stdlib_cache_validation_file_token(path)
        if token is None:
            return None
        entries.append(token)
    return (
        _shared_stdlib_contract_identity(
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        ),
        tuple(entries),
    )


def _shared_stdlib_cache_validation_token(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
    previous_token: _SharedStdlibCacheValidationToken | None = None,
    evict_corrupt: bool = False,
    stage_timings_ms: dict[str, float] | None = None,
) -> _SharedStdlibCacheValidationToken | None:
    """Validate and issue a token while owning the same publication generation."""
    if stdlib_object_path is None:
        return None
    stage_start = time.perf_counter()
    with _shared_stdlib_cache_lock(stdlib_object_path):
        _record_backend_cache_stage_ms(
            stage_timings_ms, "backend_cache_stdlib_contract_lock", stage_start
        )
        before = _shared_stdlib_cache_generation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        reuse_previous = (
            before is not None
            and previous_token is not None
            and before == previous_token
            and (
                _shared_stdlib_artifact_validation_error(
                    stdlib_object_path, target_triple=target_triple
                )
                is None
            )
        )
        if not reuse_previous and not _shared_stdlib_cache_matches_key(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
            stage_timings_ms=stage_timings_ms,
        ):
            if (
                evict_corrupt
                and stdlib_object_cache_key
                and _read_stdlib_cache_key(stdlib_object_path)
                == stdlib_object_cache_key
            ):
                _remove_shared_stdlib_cache_artifacts(stdlib_object_path)
            return None
        after = _shared_stdlib_cache_generation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        if before != after:
            raise NativeSymbolInspectionError(
                stdlib_object_path,
                ["shared stdlib generation changed during locked validation"],
            )
        return after


def _shared_stdlib_cache_validation_token_matches(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    token: _SharedStdlibCacheValidationToken,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> bool:
    return token == _shared_stdlib_cache_validation_token(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        target_triple=target_triple,
        previous_token=token,
    )


def _native_stdlib_object_split_enabled(*, target: str, emit_mode: str) -> bool:
    # An explicit object is one complete relocatable compilation unit, not
    # an archive disguised as an object after merging a cached stdlib.
    return target == "native" and emit_mode != "obj"


def _stdlib_module_symbols(
    module_graph_metadata: _ModuleGraphMetadata,
) -> frozenset[str]:
    stdlib_like_by_module = module_graph_metadata.stdlib_like_by_module or {}
    return frozenset(
        _module_symbol_name(module_name)
        for module_name, is_stdlib in sorted(stdlib_like_by_module.items())
        if is_stdlib
    )


def _encode_stdlib_module_symbols(stdlib_module_symbols: Collection[str]) -> str:
    return json.dumps(sorted(set(stdlib_module_symbols)), separators=(",", ":"))


def _is_user_owned_symbol(
    name: str,
    entry_module: str,
    *,
    stdlib_module_symbols: Collection[str] | None = None,
) -> bool:
    entry_init = f"molt_init_{entry_module}"
    if (
        name == "molt_main"
        or name == "molt_host_init"
        or name.startswith(f"{entry_module}__")
        or name == entry_init
        or name == "molt_init___main__"
        or name == "molt_isolate_import"
        or name == "molt_isolate_bootstrap"
    ):
        return True
    if stdlib_module_symbols is not None:
        return not any(
            _emitted_name_matches_module_symbol(name, module_symbol)
            for module_symbol in stdlib_module_symbols
        )
    return False


def _is_stdlib_owned_symbol(
    name: str,
    *,
    stdlib_module_symbols: Collection[str],
) -> bool:
    if name in {
        "molt_main",
        "molt_host_init",
        "molt_init___main__",
        "molt_isolate_import",
        "molt_isolate_bootstrap",
    }:
        return False
    return any(
        _emitted_name_matches_module_symbol(name, module_symbol)
        for module_symbol in stdlib_module_symbols
    )


def _reachable_function_names_for_stdlib_cache(
    ir: Mapping[str, Any],
    *,
    stdlib_module_symbols: Collection[str],
) -> set[str]:
    functions = ir.get("functions")
    if not isinstance(functions, list) or not functions:
        return set()

    function_maps = [func for func in functions if isinstance(func, Mapping)]
    defined: set[str] = {
        name
        for func in function_maps
        if isinstance((name := func.get("name")), str) and name
    }

    roots: list[str] = []
    first_function = functions[0]
    if isinstance(first_function, Mapping):
        first_name = first_function.get("name")
        if isinstance(first_name, str) and first_name in defined:
            roots.append(first_name)
    if "molt_main" in defined:
        roots.append("molt_main")
    roots.extend(
        sorted(name for name in defined if _is_protected_runtime_entrypoint(name))
    )
    roots.extend(
        init_name
        for init_name in (
            f"molt_init_{module_symbol}"
            for module_symbol in sorted(set(stdlib_module_symbols))
        )
        if init_name in defined
    )
    return set(reachable_function_names(function_maps, extra_roots=roots))


def _shared_stdlib_cache_payload_ir(
    ir: Mapping[str, Any],
    *,
    entry_module: str,
    stdlib_module_symbols: Collection[str],
    compiler_fingerprint: str | None = None,
) -> dict[str, Any]:
    """Build a cache payload for the stdlib shared object.

    The key is based on the sorted stdlib function subset and their
    backend-facing IR bodies, excluding user-owned symbols. This preserves
    sharing across programs that import the same stdlib surface while
    invalidating automatically when stdlib lowering changes.
    """
    functions = ir.get("functions")
    stdlib_functions: list[dict[str, Any]] = []
    reachable = _reachable_function_names_for_stdlib_cache(
        ir,
        stdlib_module_symbols=stdlib_module_symbols,
    )
    if isinstance(functions, list):
        for func in functions:
            if not isinstance(func, dict):
                continue
            name = func.get("name")
            if (
                not isinstance(name, str)
                or _is_user_owned_symbol(
                    name,
                    entry_module,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
                or not _is_stdlib_owned_symbol(
                    name,
                    stdlib_module_symbols=stdlib_module_symbols,
                )
            ):
                continue
            if reachable and name not in reachable:
                continue
            stdlib_functions.append(func)
    stdlib_functions = _sorted_ir_functions(stdlib_functions)
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint()
    return {
        "cache_schema": _SHARED_STDLIB_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": compiler_fingerprint,
        "functions": stdlib_functions,
        "profile": ir.get("profile"),
        "stdlib_module_symbols": sorted(set(stdlib_module_symbols)),
    }


def _shared_stdlib_cache_key(
    ir: Mapping[str, Any],
    *,
    entry_module: str,
    stdlib_module_symbols: Collection[str],
    target_triple: str | None,
    cache_variant: str,
    compiler_fingerprint: str | None = None,
    cache_compiler_fingerprint: str | None = None,
    cache_tooling_fingerprint: str | None = None,
) -> str:
    if compiler_fingerprint is None:
        compiler_fingerprint = _shared_stdlib_compiler_fingerprint(
            cache_compiler_fingerprint=cache_compiler_fingerprint,
            cache_tooling_fingerprint=cache_tooling_fingerprint,
        )
    payload_ir = _shared_stdlib_cache_payload_ir(
        ir,
        entry_module=entry_module,
        stdlib_module_symbols=stdlib_module_symbols,
        compiler_fingerprint=compiler_fingerprint,
    )
    return _cache_key(
        cast(dict[str, Any], ir),
        "native-stdlib",
        target_triple,
        cache_variant,
        payload_ir=payload_ir,
        compiler_fingerprint=cache_compiler_fingerprint,
        tooling_fingerprint=cache_tooling_fingerprint,
    )


def _shared_stdlib_compiler_fingerprint(
    *,
    cache_compiler_fingerprint: str | None = None,
    cache_tooling_fingerprint: str | None = None,
) -> str:
    if cache_compiler_fingerprint is None:
        cache_compiler_fingerprint = _cache_fingerprint()
    if cache_tooling_fingerprint is None:
        cache_tooling_fingerprint = _cache_tooling_fingerprint()
    payload = {
        "runtime_backend": cache_compiler_fingerprint,
        "tooling": cache_tooling_fingerprint,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _read_stdlib_cache_key(stdlib_path: Path) -> str | None:
    try:
        raw = _stdlib_object_key_sidecar_path(stdlib_path).read_text(encoding="utf-8")
    except OSError:
        return None
    key = raw.strip()
    return key or None


def _shared_stdlib_cache_mismatch_detail(
    stdlib_path: Path,
    expected_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    target_triple: str | None = None,
) -> str:
    actual_key = _read_stdlib_cache_key(stdlib_path)
    if not expected_key:
        return f"{stdlib_path} (missing expected key)"
    if actual_key is None:
        return f"{stdlib_path} (missing sidecar; expected key {expected_key})"
    if actual_key == expected_key:
        if stdlib_object_manifest is not None:
            manifest_path = _stdlib_object_manifest_sidecar_path(stdlib_path)
            try:
                actual_manifest = manifest_path.read_text(encoding="utf-8").strip()
            except OSError:
                return f"{stdlib_path} (missing manifest sidecar)"
            if actual_manifest != stdlib_object_manifest:
                return f"{stdlib_path} (manifest sidecar mismatch)"
        artifact_error = _shared_stdlib_artifact_validation_error(
            stdlib_path, target_triple=target_triple
        )
        if artifact_error is not None:
            return f"{stdlib_path} ({artifact_error})"
        issue = _shared_stdlib_native_symbol_closure_issue(
            stdlib_path,
            stdlib_module_symbols=stdlib_module_symbols,
            target_triple=target_triple,
        )
        if issue is not None:
            return f"{stdlib_path} ({issue})"
        return str(stdlib_path)
    return f"{stdlib_path} (expected {expected_key}, found {actual_key})"


def _stdlib_object_cache_path(
    cache_path: Path | None,
    stdlib_cache_key: str | None,
) -> Path | None:
    """Return a shared stdlib cache path scoped to exact stdlib IR identity."""
    if cache_path is None or stdlib_cache_key is None:
        return None
    cache_root = cache_path.parent
    cache_root.mkdir(parents=True, exist_ok=True)
    return cache_root / f"stdlib_shared_{stdlib_cache_key}.a"


def _validate_shared_stdlib_cache_contract(
    stdlib_object_path: Path,
    project_root: Path | None,
    expected_key: str | None = None,
    *,
    expected_manifest: str | None = None,
    target_triple: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    """Validate a shared stdlib entry and evict corrupt exact-key artifacts."""
    del project_root
    return _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        expected_key,
        stdlib_object_manifest=expected_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        stage_timings_ms=stage_timings_ms,
        evict_corrupt=True,
        target_triple=target_triple,
    )


_SHARED_STDLIB_CACHE_SCHEMA_VERSION = "stdlib-v4-archive"


_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION = "stdlib-manifest-v2-archive"


_SHARED_STDLIB_PARTITION_SCHEMA_VERSION = "stdlib-partition-v2-exact-linkage-abi"
