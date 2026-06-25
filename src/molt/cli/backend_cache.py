from __future__ import annotations

from collections import deque
import contextlib
from contextlib import contextmanager
import errno
import functools
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from typing import Any, Collection, Iterator, Mapping, Sequence, cast
import uuid

from molt.frontend import SimpleTIRGenerator
from molt.cli.artifact_state import _artifact_state_path
from molt.cli.atomic_io import _atomic_link_or_copy_file, _atomic_write_json
from molt.cli.build_locks import (
    _acquire_file_lock,
    _parse_lock_timeout,
    _release_file_lock,
)
from molt.cli.cache_fingerprints import _cache_fingerprint, _cache_tooling_fingerprint
from molt.cli.cache_keys import _cache_key, _sorted_ir_functions
from molt.cli.command_runtime import _run_completed_command
from molt.cli.default_paths import _default_molt_cache
from molt.cli.file_hashing import _sha256_file
from molt.cli.models import _ModuleGraphMetadata
from molt.cli.runtime_wasm_validation import _is_reusable_wasm_artifact


_ARTIFACT_SYNC_STATE_CACHE: dict[Path, tuple[int, int, dict[str, Any] | None]] = {}


def _is_valid_cached_backend_artifact(path: Path, *, is_wasm: bool) -> bool:
    if is_wasm:
        return _is_reusable_wasm_artifact(path)
    try:
        if path.stat().st_size <= 0:
            return False
    except OSError:
        return False
    result = _native_object_global_symbols_result(path, timeout=5)
    return result is None or bool(result.stdout.strip())


def _normalize_native_symbol_name(name: str) -> str:
    if sys.platform == "darwin" and name.startswith("_"):
        return name[1:]
    return name


def _native_nm_command(nm_bin: str, path: Path) -> list[str]:
    return [nm_bin, "-g", str(path)]


def _native_object_global_symbols_result(
    path: Path,
    *,
    timeout: float,
) -> subprocess.CompletedProcess[str] | None:
    candidates = _nm_candidate_binaries()
    if not candidates:
        return None
    last_failure: subprocess.CompletedProcess[str] | None = None
    for nm_bin in candidates:
        try:
            result = _run_completed_command(
                _native_nm_command(nm_bin, path),
                capture_output=True,
                timeout=timeout,
                env=None,
                cwd=path.parent,
                memory_guard_prefix="MOLT_BUILD",
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if result.returncode == 0 and result.stdout.strip():
            return result
        last_failure = result
    if last_failure is not None and last_failure.returncode == 0:
        return last_failure
    return None


def _native_object_global_symbol_sets(path: Path) -> tuple[set[str], set[str]] | None:
    result = _native_object_global_symbols_result(path, timeout=5)
    if result is None:
        return None
    defined: set[str] = set()
    undefined: set[str] = set()
    for raw_line in result.stdout.splitlines():
        line = raw_line.strip()
        if not line:
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
    candidates: list[str] = []
    env_override = os.environ.get("MOLT_NM")
    if env_override:
        candidates.append(env_override)
    # The Rust toolchain's own llvm-nm (the `llvm-tools` component) matches the
    # bitcode producer exactly when installed.
    try:
        sysroot_result = _run_completed_command(
            ["rustc", "--print", "sysroot"],
            capture_output=True,
            timeout=10,
            env=None,
            cwd=None,
            memory_guard_prefix="MOLT_BUILD",
        )
        sysroot = sysroot_result.stdout.strip()
    except (OSError, subprocess.SubprocessError):
        sysroot = ""
    if sysroot:
        candidates.extend(
            str(p) for p in sorted(Path(sysroot).glob("lib/rustlib/*/bin/llvm-nm"))
        )
    # Homebrew LLVM kegs (Apple Silicon + Intel prefixes), newest keg first.
    for prefix in ("/opt/homebrew/opt", "/usr/local/opt"):
        candidates.extend(
            str(p) for p in sorted(Path(prefix).glob("llvm*/bin/llvm-nm"), reverse=True)
        )
    for which_name in ("llvm-nm", "nm"):
        found = shutil.which(which_name)
        if found:
            candidates.append(found)
    return list(dict.fromkeys(candidates))


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
            if exc.errno not in {
                errno.EXDEV,
                errno.EPERM,
                errno.EACCES,
                errno.ENOTSUP,
                errno.ENOENT,
            }:
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
            if exc.errno in {
                errno.EXDEV,
                errno.EPERM,
                errno.EACCES,
                errno.ENOTSUP,
            }:
                warnings.append(
                    "Immutable cache publish skipped because no-clobber link "
                    f"is unavailable for {dst}: {exc}"
                )
                return src
            raise
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
) -> tuple[bool, str | None]:
    state_path = _artifact_sync_state_path(project_root, output_artifact)
    state = _read_artifact_sync_state(state_path)
    try:
        output_stat: os.stat_result | None = output_artifact.stat()
    except OSError:
        output_stat = None
    for tier, candidate in cache_candidates:
        if stdlib_object_path is not None:
            if not _shared_stdlib_cache_matches_key_locked(
                stdlib_object_path,
                stdlib_object_cache_key,
                stdlib_object_manifest=stdlib_object_manifest,
                stdlib_module_symbols=stdlib_module_symbols,
            ):
                if stdlib_object_path.exists():
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
                continue
        if not candidate.exists():
            continue
        if not _is_valid_cached_backend_artifact(candidate, is_wasm=is_wasm):
            warnings.append(f"Ignoring invalid cache artifact: {candidate}")
            continue
        if not is_wasm and _native_object_has_unresolved_module_chunks(
            candidate,
            stdlib_object_path,
        ):
            warnings.append(
                "Ignoring native cache artifact with unresolved user module chunks: "
                f"{candidate}"
            )
            continue
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
            return True, tier
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
            _publish_immutable_backend_cache_artifact(
                staged_source,
                function_cache_path,
                is_wasm=is_wasm_output,
                warnings=warnings,
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


def _shared_stdlib_cache_matches_key(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
) -> bool:
    if (
        stdlib_object_path is None
        or stdlib_object_cache_key is None
        or stdlib_object_manifest is None
    ):
        return False
    if not stdlib_object_path.exists():
        return False
    try:
        cached_key = _stdlib_object_key_sidecar_path(stdlib_object_path).read_text(
            encoding="utf-8"
        )
    except OSError:
        return False
    if cached_key.strip() != stdlib_object_cache_key:
        return False
    try:
        cached_manifest = _stdlib_object_manifest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        return False
    if cached_manifest.strip() != stdlib_object_manifest:
        return False
    if not _stdlib_object_partition_manifest_sidecar_path(stdlib_object_path).exists():
        return False
    try:
        cached_object_digest = _stdlib_object_digest_sidecar_path(
            stdlib_object_path
        ).read_text(encoding="utf-8")
    except OSError:
        return False
    try:
        actual_object_digest = _sha256_file(stdlib_object_path)
    except OSError:
        return False
    if cached_object_digest.strip().lower() != actual_object_digest.lower():
        return False
    return (
        _shared_stdlib_native_symbol_closure_issue(
            stdlib_object_path,
            stdlib_module_symbols=stdlib_module_symbols,
        )
        is None
    )


def _shared_stdlib_cache_matches_key_locked(
    stdlib_object_path: Path | None,
    stdlib_object_cache_key: str | None,
    *,
    stdlib_object_manifest: str | None,
    stdlib_module_symbols: Collection[str] | None = None,
) -> bool:
    if stdlib_object_path is None:
        return False
    with _shared_stdlib_cache_lock(stdlib_object_path):
        return _shared_stdlib_cache_matches_key(
            stdlib_object_path,
            stdlib_object_cache_key,
            stdlib_object_manifest=stdlib_object_manifest,
            stdlib_module_symbols=stdlib_module_symbols,
        )


def _artifact_sync_state_path(project_root: Path, artifact: Path) -> Path:
    return _artifact_state_path(
        project_root,
        artifact,
        subdir="artifact_sync",
        stem_suffix="",
        extension="json",
    )


def _read_artifact_sync_state(path: Path) -> dict[str, Any] | None:
    try:
        stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    cached = _ARTIFACT_SYNC_STATE_CACHE.get(path)
    if cached is not None:
        cached_size, cached_mtime_ns, cached_payload = cached
        if cached_size == stat.st_size and cached_mtime_ns == stat.st_mtime_ns:
            return cached_payload
    try:
        text = path.read_text().strip()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
        return None
    if not text:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, None)
        return None
    payload = data if isinstance(data, dict) else None
    _ARTIFACT_SYNC_STATE_CACHE[path] = (stat.st_size, stat.st_mtime_ns, payload)
    return payload


def _write_artifact_sync_state(
    path: Path,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> None:
    stat = artifact.stat()
    payload = {
        "version": 1,
        "source_key": source_key,
        "tier": tier,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
    }
    _atomic_write_json(path, payload, indent=2)
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _write_artifact_sync_payload(
    path: Path,
    payload: dict[str, Any],
    *,
    default: Any | None = None,
) -> None:
    _atomic_write_json(path, payload, indent=2, default=default)
    try:
        written_stat = path.stat()
    except OSError:
        _ARTIFACT_SYNC_STATE_CACHE.pop(path, None)
    else:
        _ARTIFACT_SYNC_STATE_CACHE[path] = (
            written_stat.st_size,
            written_stat.st_mtime_ns,
            dict(payload),
        )


def _artifact_sync_state_matches(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    artifact: Path,
) -> bool:
    try:
        stat = artifact.stat()
    except OSError:
        return False
    return _artifact_sync_state_matches_stat(
        state,
        source_key=source_key,
        tier=tier,
        stat=stat,
    )


def _artifact_sync_state_matches_stat(
    state: dict[str, Any] | None,
    *,
    source_key: str,
    tier: str,
    stat: os.stat_result,
) -> bool:
    if state is None:
        return False
    if state.get("source_key") != source_key or state.get("tier") != tier:
        return False
    return (
        state.get("size") == stat.st_size and state.get("mtime_ns") == stat.st_mtime_ns
    )


def _native_stdlib_object_split_enabled(*, target: str, emit_mode: str) -> bool:
    return target == "native"


def _module_symbol_name(module_name: str) -> str:
    init_symbol = SimpleTIRGenerator.module_init_symbol(module_name)
    assert init_symbol.startswith("molt_init_")
    return init_symbol[len("molt_init_") :]


def _emitted_name_matches_module_symbol(name: str, module_symbol: str) -> bool:
    if name.startswith("molt_init_"):
        return name[len("molt_init_") :] == module_symbol
    return name.startswith(f"{module_symbol}__")


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


_DEAD_FUNCTION_ELIM_REFERENCE_KINDS = frozenset(
    {
        "call",
        "call_internal",
        "func_new",
        "func_new_closure",
        "func_new_builtin",
        "code_new",
        "call_guarded",
        "call_indirect",
        "alloc_task",
        "generator_create",
        "coro_create",
        "fn_ptr_code_set",
        "asyncgen_locals_register",
        "gen_locals_register",
        "task_new",
        "generator_send",
        "spawn",
        "call_func",
        "call_method",
        "import_from",
        "import_name",
        "class_def",
        "decorator",
        "super_call",
        "yield_from",
        "await",
    }
)


def _is_protected_runtime_entrypoint(name: str) -> bool:
    return name in {"molt_main", "molt_host_init", "_start"} or name.startswith(
        "molt_isolate_"
    )


def _reachable_function_names_for_stdlib_cache(
    ir: Mapping[str, Any],
    *,
    stdlib_module_symbols: Collection[str],
) -> set[str]:
    functions = ir.get("functions")
    if not isinstance(functions, list) or not functions:
        return set()

    defined: set[str] = set()
    references: dict[str, set[str]] = {}

    for func in functions:
        if not isinstance(func, Mapping):
            continue
        name = func.get("name")
        if isinstance(name, str) and name:
            defined.add(name)

    for func in functions:
        if not isinstance(func, Mapping):
            continue
        name = func.get("name")
        if not isinstance(name, str) or not name:
            continue
        refs: set[str] = set()
        ops = func.get("ops")
        if isinstance(ops, list):
            for op in ops:
                if not isinstance(op, Mapping):
                    continue
                kind = op.get("kind")
                if not isinstance(kind, str):
                    continue
                target = op.get("s_value")
                if kind in _DEAD_FUNCTION_ELIM_REFERENCE_KINDS and isinstance(
                    target, str
                ):
                    if target in defined:
                        refs.add(target)
                    if kind in {
                        "generator_create",
                        "coro_create",
                    } and not target.endswith("_poll"):
                        poll_name = f"{target}_poll"
                        if poll_name in defined:
                            refs.add(poll_name)
        references[name] = refs

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

    reachable: set[str] = set()
    queue: deque[str] = deque()
    for root in roots:
        if root not in reachable:
            reachable.add(root)
            queue.append(root)

    while queue:
        current = queue.popleft()
        for target in references.get(current, ()):
            if target not in reachable:
                reachable.add(target)
                queue.append(target)

    return reachable


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
) -> str:
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
    )


def _shared_stdlib_compiler_fingerprint() -> str:
    payload = {
        "runtime_backend": _cache_fingerprint(),
        "tooling": _cache_tooling_fingerprint(),
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
) -> None:
    """Validate a shared stdlib entry and evict corrupt exact-key artifacts."""
    del project_root, target_triple
    if not stdlib_object_path.exists():
        return
    if _shared_stdlib_cache_matches_key_locked(
        stdlib_object_path,
        expected_key,
        stdlib_object_manifest=expected_manifest,
        stdlib_module_symbols=stdlib_module_symbols,
    ):
        return
    actual_key = _read_stdlib_cache_key(stdlib_object_path)
    if expected_key and actual_key == expected_key:
        _remove_shared_stdlib_cache_artifacts(stdlib_object_path)


_SHARED_STDLIB_CACHE_SCHEMA_VERSION = "stdlib-v3"


_SHARED_STDLIB_MANIFEST_SCHEMA_VERSION = "stdlib-manifest-v1"


_SHARED_STDLIB_PARTITION_SCHEMA_VERSION = "stdlib-partition-v1"
