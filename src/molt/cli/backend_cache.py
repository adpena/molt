from __future__ import annotations

import contextlib
from contextlib import contextmanager
import functools
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any, Collection, Iterator, Mapping, Sequence, cast
import uuid

from molt.cli.artifact_sync import (
    _artifact_sync_state_matches_stat,
    _artifact_sync_state_path,
    _read_artifact_sync_state,
    _write_artifact_sync_state,
)
from molt.cli.atomic_io import (
    _atomic_link_or_copy_file,
    _atomic_write_json,
    _link_failure_wants_copy,
)
from molt.cli.build_locks import (
    _acquire_file_lock,
    _parse_lock_timeout,
    _release_file_lock,
)
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.cache_keys import _cache_key, _sorted_ir_functions
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.file_hashing import _sha256_file
from molt.cli.llvm_wasi_tools import llvm_tool_candidates
from molt.cli import function_references as _function_references
from molt.cli.models import _ModuleGraphMetadata
from molt.cli.runtime_wasm_validation import _is_reusable_wasm_artifact


_DEAD_FUNCTION_ELIM_REFERENCE_KINDS = _function_references.FUNCTION_REFERENCE_OP_KINDS
_emitted_name_matches_module_symbol = (
    _function_references.emitted_name_matches_module_symbol
)
_is_protected_runtime_entrypoint = _function_references.is_protected_runtime_entrypoint
_module_symbol_name = _function_references.module_symbol_name
reachable_function_names = _function_references.reachable_function_names

_SharedStdlibCacheValidationToken = tuple[
    str, tuple[tuple[str, int, int, int], ...]
]
_NativeObjectSymbolSets = tuple[set[str], set[str]]
_NATIVE_OBJECT_SYMBOL_SETS_CACHE: dict[
    tuple[str, int, int, int, str, str, str, str, tuple[str, ...]],
    tuple[frozenset[str], frozenset[str]] | None,
] = {}
_NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT = 256
_NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION = 1
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT = 32
_NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION = 1
_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE: dict[
    tuple[str, int, int, int, str, str, str, tuple[str, ...]],
    tuple[frozenset[str], frozenset[str]] | None,
] = {}
_SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION = 1


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


def _is_valid_cached_backend_artifact(path: Path, *, is_wasm: bool) -> bool:
    if is_wasm:
        return _is_reusable_wasm_artifact(path)
    try:
        if path.stat().st_size <= 0:
            return False
    except OSError:
        return False
    symbol_sets = _native_object_global_symbol_sets(path)
    return symbol_sets is None or bool(symbol_sets[0] or symbol_sets[1])


def _normalize_native_symbol_name(name: str) -> str:
    if sys.platform == "darwin" and name.startswith("_"):
        return name[1:]
    return name


def _native_nm_command(nm_command: Sequence[str], path: Path) -> list[str]:
    return [*nm_command, "-g", str(path)]


def _nm_result_reports_no_symbols(result: subprocess.CompletedProcess[str]) -> bool:
    text = f"{result.stdout}\n{result.stderr}".lower()
    return "no symbols" in text


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


def _native_object_global_symbols_result(
    path: Path,
    *,
    timeout: float,
    nm_command: Sequence[str] | None = None,
) -> subprocess.CompletedProcess[str] | None:
    candidates = (
        [tuple(nm_command)]
        if nm_command is not None
        else [(candidate,) for candidate in _nm_candidate_binaries()]
    )
    if not candidates:
        return None
    read_timeout = _nm_read_timeout(timeout)
    last_failure: subprocess.CompletedProcess[str] | None = None
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
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if result.returncode == 0 and result.stdout.strip():
            return result
        if _nm_result_reports_no_symbols(result):
            return subprocess.CompletedProcess(result.args, 0, "", "")
        last_failure = result
    if last_failure is not None and last_failure.returncode == 0:
        return last_failure
    return None


def _native_object_symbol_facts_sidecar_path(path: Path) -> Path:
    return path.with_suffix(".symbols.json")


def _native_object_symbol_cache_key(
    path: Path,
    object_digest: str,
    *,
    nm_command: Sequence[str] | None,
) -> tuple[str, int, int, int, str, str, str, str, tuple[str, ...]] | None:
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
        tuple(nm_command or ()),
    )


def _native_object_symbol_facts_payload(
    *,
    object_digest: str,
    defined: Collection[str],
    undefined: Collection[str],
) -> dict[str, object]:
    return {
        "schema": _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION,
        "platform": sys.platform,
        "object_digest": object_digest,
        "defined": sorted(set(defined)),
        "undefined": sorted(set(undefined)),
    }


def _read_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
) -> _NativeObjectSymbolSets | None:
    try:
        payload = json.loads(
            _native_object_symbol_facts_sidecar_path(path).read_text(
                encoding="utf-8"
            )
        )
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict):
        return None
    if payload.get("schema") != _NATIVE_OBJECT_SYMBOL_FACTS_SCHEMA_VERSION:
        return None
    if payload.get("platform") != sys.platform:
        return None
    if payload.get("object_digest") != object_digest:
        return None
    defined = payload.get("defined")
    undefined = payload.get("undefined")
    if not isinstance(defined, list) or not isinstance(undefined, list):
        return None
    if not all(isinstance(symbol, str) for symbol in defined):
        return None
    if not all(isinstance(symbol, str) for symbol in undefined):
        return None
    return set(cast(list[str], defined)), set(cast(list[str], undefined))


def _write_native_object_symbol_facts(
    path: Path,
    *,
    object_digest: str,
    defined: Collection[str],
    undefined: Collection[str],
) -> None:
    payload = _native_object_symbol_facts_payload(
        object_digest=object_digest,
        defined=defined,
        undefined=undefined,
    )
    _atomic_write_json(
        _native_object_symbol_facts_sidecar_path(path),
        payload,
        indent=2,
    )


def _ensure_native_object_symbol_facts(path: Path, *, is_wasm: bool) -> None:
    if is_wasm:
        return
    # Best-effort cache warming: callers still validate fail-closed by reading
    # facts back through `_native_object_global_symbol_sets`.
    with contextlib.suppress(OSError):
        _native_object_global_symbol_sets(path)


def _native_object_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
) -> _NativeObjectSymbolSets | None:
    try:
        object_digest = _sha256_file(path)
    except OSError:
        object_digest = ""
    cache_key = _native_object_symbol_cache_key(
        path,
        object_digest,
        nm_command=nm_command,
    )
    if cache_key is not None:
        cached = _NATIVE_OBJECT_SYMBOL_SETS_CACHE.get(cache_key)
        if cached is not None:
            defined_cached, undefined_cached = cached
            return set(defined_cached), set(undefined_cached)
        if cache_key in _NATIVE_OBJECT_SYMBOL_SETS_CACHE:
            return None
    if object_digest:
        symbol_facts = _read_native_object_symbol_facts(
            path,
            object_digest=object_digest,
        )
        if symbol_facts is not None:
            if cache_key is not None:
                defined, undefined = symbol_facts
                _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = (
                    frozenset(defined),
                    frozenset(undefined),
                )
            return symbol_facts
    result = _native_object_global_symbols_result(
        path,
        timeout=5,
        nm_command=nm_command,
    )
    if result is None:
        if cache_key is not None:
            _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = None
        return None
    defined, undefined = _parse_native_nm_global_symbol_sets(result.stdout)
    if cache_key is not None:
        if (
            len(_NATIVE_OBJECT_SYMBOL_SETS_CACHE)
            >= _NATIVE_OBJECT_SYMBOL_SETS_CACHE_LIMIT
        ):
            _NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
        _NATIVE_OBJECT_SYMBOL_SETS_CACHE[cache_key] = (
            frozenset(defined),
            frozenset(undefined),
        )
    if object_digest:
        with contextlib.suppress(OSError):
            _write_native_object_symbol_facts(
                path,
                object_digest=object_digest,
                defined=defined,
                undefined=undefined,
            )
    return defined, undefined


def _parse_native_nm_global_symbol_sets(
    output: str,
) -> _NativeObjectSymbolSets:
    """Parse global ``nm`` facts for one object or static archive.

    LLVM ``nm`` emits archive-member header lines between ordinary symbol rows.
    Keeping the parser shared makes archive-backed linker custody use the same
    symbol semantics as source-extension object closure without creating symbol
    sidecars inside managed Rust/WASI toolchains.
    """

    defined: set[str] = set()
    undefined: set[str] = set()
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.lower().endswith(": no symbols") or line.lower() == "no symbols":
            continue
        parts = line.split()
        if len(parts) >= 2:
            if len(parts) == 2:
                kind, name = parts
            else:
                _, kind, name = parts[0], parts[1], parts[2]
            symbol = _normalize_native_symbol_name(name)
            if kind.upper() == "U":
                undefined.add(symbol)
            else:
                defined.add(symbol)
    return defined, undefined


def _native_archive_global_symbol_sets(
    path: Path,
    *,
    nm_command: Sequence[str] | None = None,
) -> _NativeObjectSymbolSets | None:
    """Read one provider archive's globals without mutating the toolchain.

    Provider archives are immutable installation inputs, not build outputs.
    Their symbol facts therefore use bounded process caching plus one central,
    stat-keyed cache under Molt's cache root; unlike object facts, this function
    never writes a ``*.symbols.json`` sibling into Rust or WASI SDK directories.
    """

    try:
        resolved = path.resolve(strict=True)
        stat = resolved.stat()
    except OSError:
        return None
    cache_key = (
        os.fspath(resolved),
        int(stat.st_size),
        int(stat.st_mtime_ns),
        int(getattr(stat, "st_ctime_ns", 0)),
        os.environ.get("MOLT_TARGET_ROOT", ""),
        os.environ.get("PATH", ""),
        os.environ.get("MOLT_NM_TIMEOUT_SEC", ""),
        tuple(nm_command or ()),
    )
    cached = _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.get(cache_key)
    if cached is not None:
        defined_cached, undefined_cached = cached
        return set(defined_cached), set(undefined_cached)
    if cache_key in _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE:
        return None
    persistent_cache_path = _native_archive_symbol_cache_path(cache_key)
    persistent_facts = _read_native_archive_symbol_cache(
        persistent_cache_path,
        cache_key=cache_key,
    )
    if persistent_facts is not None:
        defined, undefined = persistent_facts
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = (
            frozenset(defined),
            frozenset(undefined),
        )
        return defined, undefined
    result = _native_object_global_symbols_result(
        resolved,
        timeout=120,
        nm_command=nm_command,
    )
    if result is None:
        facts = None
    else:
        defined, undefined = _parse_native_nm_global_symbol_sets(result.stdout)
        facts = (frozenset(defined), frozenset(undefined))
    if len(_NATIVE_ARCHIVE_SYMBOL_SETS_CACHE) >= _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE_LIMIT:
        _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()
    _NATIVE_ARCHIVE_SYMBOL_SETS_CACHE[cache_key] = facts
    if facts is None:
        return None
    with contextlib.suppress(OSError):
        _write_native_archive_symbol_cache(
            persistent_cache_path,
            cache_key=cache_key,
            defined=facts[0],
            undefined=facts[1],
        )
    return set(facts[0]), set(facts[1])


def _native_archive_symbol_cache_identity(
    cache_key: tuple[str, int, int, int, str, str, str, tuple[str, ...]],
) -> dict[str, object]:
    return {
        "path": cache_key[0],
        "size": cache_key[1],
        "mtime_ns": cache_key[2],
        "ctime_ns": cache_key[3],
        "target_root": cache_key[4],
        "path_env": cache_key[5],
        "timeout_env": cache_key[6],
        "nm_command": list(cache_key[7]),
    }


def _native_archive_symbol_cache_path(
    cache_key: tuple[str, int, int, int, str, str, str, tuple[str, ...]],
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
    cache_key: tuple[str, int, int, int, str, str, str, tuple[str, ...]],
) -> _NativeObjectSymbolSets | None:
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
    if not isinstance(defined, list) or not isinstance(undefined, list):
        return None
    if not all(isinstance(symbol, str) for symbol in (*defined, *undefined)):
        return None
    return (
        {symbol for symbol in defined if isinstance(symbol, str)},
        {symbol for symbol in undefined if isinstance(symbol, str)},
    )


def _write_native_archive_symbol_cache(
    path: Path,
    *,
    cache_key: tuple[str, int, int, int, str, str, str, tuple[str, ...]],
    defined: Collection[str],
    undefined: Collection[str],
) -> None:
    _atomic_write_json(
        path,
        {
            "schema": _NATIVE_ARCHIVE_SYMBOL_CACHE_SCHEMA_VERSION,
            "identity": _native_archive_symbol_cache_identity(cache_key),
            "defined": sorted(set(defined)),
            "undefined": sorted(set(undefined)),
        },
        indent=None,
        sort_keys=True,
    )


def _native_object_has_unresolved_module_chunks(
    candidate: Path,
    stdlib_object_path: Path | None,
) -> bool:
    candidate_symbols = _native_object_global_symbol_sets(candidate)
    if candidate_symbols is None:
        return False
    _, undefined = candidate_symbols
    unresolved_chunks = {
        symbol for symbol in undefined if "__molt_module_chunk_" in symbol
    }
    if not unresolved_chunks:
        return False
    stdlib_defined: set[str] = set()
    if stdlib_object_path is not None:
        stdlib_symbols = _native_object_global_symbol_sets(stdlib_object_path)
        if stdlib_symbols is not None:
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
) -> str | None:
    symbol_sets = _native_object_global_symbol_sets(stdlib_object_path)
    if symbol_sets is None:
        return None
    defined, undefined = symbol_sets
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


def _publish_immutable_backend_cache_artifact(
    src: Path,
    dst: Path,
    *,
    is_wasm: bool,
    warnings: list[str],
) -> Path:
    """Publish a key-addressed backend cache artifact without clobbering peers."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.exists():
        if _is_valid_cached_backend_artifact(dst, is_wasm=is_wasm):
            return dst
        warnings.append(
            "Ignoring invalid existing immutable cache artifact; "
            f"cleanup owns removal: {dst}"
        )
        return src

    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        try:
            os.link(src, tmp_path)
        except OSError as exc:
            if not _link_failure_wants_copy(exc):
                raise
            shutil.copyfile(src, tmp_path)
            with contextlib.suppress(OSError):
                shutil.copymode(src, tmp_path)
        try:
            os.link(tmp_path, dst)
        except FileExistsError:
            if _is_valid_cached_backend_artifact(dst, is_wasm=is_wasm):
                return dst
            warnings.append(
                "Ignoring concurrently published invalid immutable cache artifact; "
                f"cleanup owns removal: {dst}"
            )
            return src
        except OSError as exc:
            if not _link_failure_wants_copy(exc):
                raise
            # No hard links here (exFAT/FAT on the artifact SSD): publish via a
            # lock-guarded atomic rename so the backend cache still POPULATES
            # rather than silently disabling itself on the primary build volume.
            # The immutable cache is key-addressed, so the lock + existence
            # re-check preserves the no-clobber contract (a peer that already
            # published wins) and os.replace of a fully-staged tmp is atomic for
            # readers.
            with _shared_cache_lock(
                _immutable_publish_lock_name(dst), cache_root=dst.parent
            ):
                if dst.exists():
                    if _is_valid_cached_backend_artifact(dst, is_wasm=is_wasm):
                        return dst
                    warnings.append(
                        "Ignoring invalid existing immutable cache artifact; "
                        f"cleanup owns removal: {dst}"
                    )
                    return src
                os.replace(tmp_path, dst)
        return dst
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


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
) -> bool:
    is_wasm_output = output_artifact.suffix == ".wasm"
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        with contextlib.suppress(OSError):
            output_stat = output_artifact.stat()
    if output_stat is not None:
        synced = _artifact_sync_state_matches_stat(
            state,
            source_key=source_key,
            tier=tier,
            stat=output_stat,
        )
        if synced and (
            not is_wasm_output or _is_reusable_wasm_artifact(output_artifact)
        ):
            return True
    sync_tier = tier
    sync_source_key = source_key
    try:
        _atomic_link_or_copy_file(candidate, output_artifact)
        if tier == "function" and cache_path is not None and candidate != cache_path:
            with contextlib.suppress(OSError):
                published_module_cache = _publish_immutable_backend_cache_artifact(
                    candidate,
                    cache_path,
                    is_wasm=is_wasm_output,
                    warnings=warnings,
                )
                if module_cache_key and published_module_cache == cache_path:
                    # Once the canonical module cache path is valid, future
                    # daemon sync checks should treat output.o as module-synced
                    # rather than function-only.
                    sync_tier = "module"
                    sync_source_key = module_cache_key
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=sync_source_key,
                tier=sync_tier,
                artifact=output_artifact,
            )
        except OSError:
            pass
        return True
    except OSError as exc:
        warnings.append(f"Cache copy failed: {exc}")
        return False


def _synced_backend_output_cache_hit_tier(
    state: dict[str, Any] | None,
    output_artifact: Path,
    output_stat: os.stat_result | None,
    *,
    is_wasm: bool,
    cache_key: str | None,
    function_cache_key: str | None,
    stdlib_object_cache_key: str | None,
) -> str | None:
    if output_stat is None:
        return None
    module_source_key = _native_artifact_source_key(
        cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    if module_source_key and _artifact_sync_state_matches_stat(
        state,
        source_key=module_source_key,
        tier="module",
        stat=output_stat,
    ):
        if not is_wasm or _is_reusable_wasm_artifact(output_artifact):
            return "module"
    function_source_key = _native_artifact_source_key(
        function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    if function_source_key and _artifact_sync_state_matches_stat(
        state,
        source_key=function_source_key,
        tier="function",
        stat=output_stat,
    ):
        if not is_wasm or _is_reusable_wasm_artifact(output_artifact):
            return "function"
    return None


def _validated_stdlib_contract_token_for_backend_cache_hit(
    *,
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None,
    stdlib_contract_validation_token: _SharedStdlibCacheValidationToken | None,
    stage_timings_ms: dict[str, float] | None,
) -> tuple[bool, _SharedStdlibCacheValidationToken | None]:
    if stdlib_object_path is None:
        return True, None
    stage_start = time.perf_counter()
    active_stdlib_contract_token = stdlib_contract_validation_token
    if (
        active_stdlib_contract_token is not None
        and not _shared_stdlib_cache_validation_token_matches(
            stdlib_object_path,
            stdlib_object_cache_key,
            active_stdlib_contract_token,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        )
    ):
        active_stdlib_contract_token = None
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_contract_token",
        stage_start,
    )
    if active_stdlib_contract_token is not None:
        return True, active_stdlib_contract_token

    stage_start = time.perf_counter()
    stdlib_contract_valid = _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        stage_timings_ms=stage_timings_ms,
    )
    if stdlib_contract_valid:
        active_stdlib_contract_token = _shared_stdlib_cache_validation_token(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_contract_validate",
        stage_start,
    )
    return stdlib_contract_valid, active_stdlib_contract_token


def _native_artifact_source_key(
    base_key: str | None,
    *,
    stdlib_object_cache_key: str | None,
    is_wasm: bool,
) -> str:
    if base_key is None:
        # Cache disabled (e.g. --rebuild): return empty key so the daemon
        # does not match against a shared sentinel that is identical for
        # every file.  Previously `base_key or ""` produced the same
        # "|stdlib:<hash>" key for every --rebuild invocation, causing the
        # daemon in-memory cache to return the first file's compiled output
        # for all subsequent files in the same daemon session.
        return ""
    key = base_key or ""
    if is_wasm or not stdlib_object_cache_key:
        return key
    return f"{key}|stdlib:{stdlib_object_cache_key}"


def _backend_cache_artifact_path(
    cache_root: Path,
    base_key: str | None,
    *,
    ext: str,
    stdlib_object_cache_key: str | None,
    is_wasm: bool,
) -> Path | None:
    source_key = _native_artifact_source_key(
        base_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
        is_wasm=is_wasm,
    )
    if not source_key:
        return None
    filename_key = source_key.replace("|stdlib:", ".stdlib-")
    return cache_root / f"{filename_key}.{ext}"


def _try_cached_backend_candidates(
    *,
    project_root: Path,
    cache_candidates: Sequence[tuple[str, Path]],
    output_artifact: Path,
    is_wasm: bool,
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
                )
            )
        # Native output.o cache hits are invalid without the matching
        # stdlib_shared object they were compiled against.
        return False, None

    stage_start = time.perf_counter()
    synced_tier = _synced_backend_output_cache_hit_tier(
        state,
        output_artifact,
        output_stat,
        is_wasm=is_wasm,
        cache_key=cache_key,
        function_cache_key=function_cache_key,
        stdlib_object_cache_key=stdlib_object_cache_key,
    )
    _record_backend_cache_stage_ms(
        stage_timings_ms,
        "backend_cache_try_synced_output",
        stage_start,
    )
    if synced_tier is not None:
        return True, synced_tier

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
        if not _is_valid_cached_backend_artifact(candidate, is_wasm=is_wasm):
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
        if not is_wasm and _native_object_has_unresolved_module_chunks(
            candidate,
            stdlib_object_path,
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
            source_key=_native_artifact_source_key(
                cache_key
                if tier == "module"
                else (function_cache_key or cache_key or ""),
                stdlib_object_cache_key=stdlib_object_cache_key,
                is_wasm=is_wasm,
            ),
            cache_path=cache_path,
            module_cache_key=_native_artifact_source_key(
                cache_key,
                stdlib_object_cache_key=stdlib_object_cache_key,
                is_wasm=is_wasm,
            ),
            warnings=warnings,
            state_path=state_path,
            state=state,
            output_stat=output_stat,
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
) -> tuple[bool, bool]:
    is_wasm_output = output_artifact.suffix == ".wasm"
    if not is_wasm_output and _native_object_has_unresolved_module_chunks(
        output_artifact,
        stdlib_object_path,
    ):
        return False, False
    if stdlib_object_path is not None and not _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
    ):
        return False, False
    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
        state = _read_artifact_sync_state(state_path)
    if output_stat is None:
        try:
            output_stat = output_artifact.stat()
        except OSError:
            return False, False
    skip_module_output = bool(cache_key) and _artifact_sync_state_matches_stat(
        state,
        source_key=_native_artifact_source_key(
            cache_key,
            stdlib_object_cache_key=stdlib_object_cache_key,
            is_wasm=is_wasm_output,
        ),
        tier="module",
        stat=output_stat,
    )
    skip_function_output = bool(
        function_cache_key
    ) and _artifact_sync_state_matches_stat(
        state,
        source_key=_native_artifact_source_key(
            function_cache_key,
            stdlib_object_cache_key=stdlib_object_cache_key,
            is_wasm=is_wasm_output,
        ),
        tier="function",
        stat=output_stat,
    )
    if is_wasm_output and not _is_reusable_wasm_artifact(output_artifact):
        return False, False
    return skip_module_output, skip_function_output


@contextmanager
def _temporary_backend_output_path(
    artifacts_root: Path,
    *,
    is_wasm: bool,
) -> Iterator[Path]:
    suffix = ".wasm" if is_wasm else ".o"
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
) -> str | None:
    is_wasm_output = output_artifact.suffix == ".wasm"
    try:
        if output_artifact.parent != Path("."):
            output_artifact.parent.mkdir(parents=True, exist_ok=True)
    except OSError as exc:
        return f"Failed to move backend output: {exc}"

    staged_source = backend_output
    if cache_path is not None:
        if backend_output != cache_path:
            try:
                staged_source = _publish_immutable_backend_cache_artifact(
                    backend_output,
                    cache_path,
                    is_wasm=is_wasm_output,
                    warnings=warnings,
                )
                if staged_source == cache_path:
                    _ensure_native_object_symbol_facts(
                        staged_source,
                        is_wasm=is_wasm_output,
                    )
                if staged_source == cache_path:
                    with contextlib.suppress(OSError):
                        backend_output.unlink()
            except OSError as exc:
                return f"Failed to publish backend cache output: {exc}"
        else:
            staged_source = cache_path

    if state_path is None:
        state_path = _artifact_sync_state_path(project_root, output_artifact)
    if output_already_synced is None:
        state = _read_artifact_sync_state(state_path)
        if output_stat is None:
            try:
                output_stat = output_artifact.stat()
            except OSError:
                output_stat = None
        output_already_synced = (
            bool(cache_key)
            and output_stat is not None
            and (
                _artifact_sync_state_matches_stat(
                    state,
                    source_key=_native_artifact_source_key(
                        cache_key,
                        stdlib_object_cache_key=stdlib_object_cache_key,
                        is_wasm=is_wasm_output,
                    ),
                    tier="module",
                    stat=output_stat,
                )
            )
        )
        if output_already_synced and is_wasm_output:
            output_already_synced = _is_reusable_wasm_artifact(output_artifact)

    try:
        if output_already_synced and not output_artifact.exists():
            output_already_synced = False
        if output_already_synced:
            pass
        elif staged_source == backend_output and cache_path is None:
            backend_output.replace(output_artifact)
        else:
            _atomic_link_or_copy_file(staged_source, output_artifact)
    except OSError as exc:
        return f"Failed to move backend output: {exc}"

    if cache_path is None:
        return None

    if function_cache_path is not None and function_cache_path != cache_path:
        try:
            published_function_cache = _publish_immutable_backend_cache_artifact(
                staged_source,
                function_cache_path,
                is_wasm=is_wasm_output,
                warnings=warnings,
            )
            if published_function_cache == function_cache_path:
                _ensure_native_object_symbol_facts(
                    published_function_cache,
                    is_wasm=is_wasm_output,
                )
        except OSError as exc:
            warnings.append(f"Function cache write failed: {exc}")
    if cache_key and not output_already_synced:
        try:
            state_path.parent.mkdir(parents=True, exist_ok=True)
            _write_artifact_sync_state(
                state_path,
                source_key=_native_artifact_source_key(
                    cache_key,
                    stdlib_object_cache_key=stdlib_object_cache_key,
                    is_wasm=is_wasm_output,
                ),
                tier="module",
                artifact=output_artifact,
            )
        except OSError:
            pass
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
) -> Path:
    staged_stdlib_obj = artifacts_root / stdlib_object_path.name
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
            if not _shared_stdlib_cache_matches_key(
                stdlib_object_path,
                stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
            ):
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    + _shared_stdlib_cache_mismatch_detail(
                        stdlib_object_path,
                        stdlib_object_cache_key,
                        stdlib_object_manifest=stdlib_object_manifest,
                        stdlib_module_symbols=stdlib_module_symbols,
                    )
                )
            _atomic_link_or_copy_file(stdlib_object_path, staged_stdlib_obj)
            if source_key_path.exists():
                _atomic_link_or_copy_file(source_key_path, staged_key_path)
            elif stdlib_object_cache_key:
                raise OSError(
                    "Shared stdlib cache key mismatch during staging: "
                    f"missing key sidecar for {stdlib_object_path}"
                )
            elif staged_key_path.exists():
                staged_key_path.unlink()
            if source_count_path.exists():
                _atomic_link_or_copy_file(source_count_path, staged_count_path)
            elif staged_count_path.exists():
                staged_count_path.unlink()
            if source_manifest_path.exists():
                _atomic_link_or_copy_file(source_manifest_path, staged_manifest_path)
            elif stdlib_object_manifest:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing manifest sidecar for {stdlib_object_path}"
                )
            elif staged_manifest_path.exists():
                staged_manifest_path.unlink()
            if source_partition_manifest_path.exists():
                _atomic_link_or_copy_file(
                    source_partition_manifest_path, staged_partition_manifest_path
                )
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing partition manifest sidecar for {stdlib_object_path}"
                )
            if source_digest_path.exists():
                _atomic_link_or_copy_file(source_digest_path, staged_digest_path)
            else:
                raise OSError(
                    "Shared stdlib cache contract mismatch during staging: "
                    f"missing object digest sidecar for {stdlib_object_path}"
                )
    except OSError:
        _remove_shared_stdlib_cache_artifacts(staged_stdlib_obj)
        raise
    return staged_stdlib_obj


def _remove_shared_stdlib_cache_artifacts(stdlib_object_path: Path) -> None:
    with contextlib.suppress(OSError):
        stdlib_object_path.unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_count_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_key_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_manifest_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_digest_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path).unlink()
    with contextlib.suppress(OSError):
        _native_object_symbol_facts_sidecar_path(stdlib_object_path).unlink()


def _shared_stdlib_cache_matches_key(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
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
            )
    return symbol_closure_ok


def _shared_stdlib_cache_matches_key_locked(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
    stage_timings_ms: dict[str, float] | None = None,
) -> bool:
    if stdlib_object_path is None:
        return False
    stage_start = time.perf_counter()
    with _shared_stdlib_cache_lock(stdlib_object_path):
        _record_backend_cache_stage_ms(
            stage_timings_ms,
            "backend_cache_stdlib_contract_lock",
            stage_start,
        )
        return _shared_stdlib_cache_matches_key(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
            stage_timings_ms=stage_timings_ms,
        )


def _shared_stdlib_contract_identity(
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
) -> str:
    payload = {
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
) -> dict[str, object]:
    return {
        "schema": _SHARED_STDLIB_SYMBOL_CONTRACT_SCHEMA_VERSION,
        "contract_identity": _shared_stdlib_contract_identity(
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
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
) -> None:
    payload = _shared_stdlib_symbol_contract_payload(
        stdlib_object_cache_key=stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        object_digest=object_digest,
        partition_manifest_digest=partition_manifest_digest,
    )
    _atomic_write_json(
        _stdlib_object_symbol_contract_sidecar_path(stdlib_object_path),
        payload,
        indent=2,
    )


def _shared_stdlib_cache_validation_file_token(
    path: Path,
) -> tuple[str, int, int, int] | None:
    try:
        stat_result = path.stat()
    except OSError:
        return None
    return (
        os.fspath(path),
        int(stat_result.st_size),
        int(stat_result.st_mtime_ns),
        int(getattr(stat_result, "st_ctime_ns", 0)),
    )


def _shared_stdlib_cache_validation_token(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
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
    entries: list[tuple[str, int, int, int]] = []
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
        ),
        tuple(entries),
    )


def _shared_stdlib_cache_validation_token_matches(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    token: _SharedStdlibCacheValidationToken,
    *,
    stdlib_object_manifest: str | None = None,
    stdlib_module_symbols: Collection[str] | None = None,
) -> bool:
    return token == _shared_stdlib_cache_validation_token(
        stdlib_object_path,
        stdlib_object_cache_key,
        stdlib_object_manifest=stdlib_object_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
    )


def _native_stdlib_object_split_enabled(*, target: str, emit_mode: str) -> bool:
    return target == "native"


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
        issue = _shared_stdlib_native_symbol_closure_issue(
            stdlib_path,
            stdlib_module_symbols=stdlib_module_symbols,
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
    return cache_root / f"stdlib_shared_{stdlib_cache_key}.o"


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
    del project_root, target_triple
    if not stdlib_object_path.exists():
        return False
    if _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        expected_key,
        stdlib_object_manifest=expected_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
        stage_timings_ms=stage_timings_ms,
    ):
        return True
    actual_key = _read_stdlib_cache_key(stdlib_object_path)
    if expected_key and actual_key == expected_key:
        _remove_shared_stdlib_cache_artifacts(stdlib_object_path)
    return False


_SHARED_STDLIB_CACHE_SCHEMA_VERSION = "stdlib-v3"


_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION = "stdlib-manifest-v1"


_SHARED_STDLIB_PARTITION_SCHEMA_VERSION = "stdlib-partition-v2-exact-linkage-abi"
