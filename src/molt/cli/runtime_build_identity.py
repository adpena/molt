"""Runtime input capture and family resolution; wire validation is pure."""

from __future__ import annotations

import hashlib
import os
import re
import shlex
import stat as stat_module
import threading
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Callable, Iterator, Mapping, Sequence, TypeVar, cast

from molt.cli.runtime_build_python import BuildPythonAdmission
from molt.cli.cargo_source_closure import _cargo_crate_source_closure
from molt.cli.runtime_artifact_selection import RuntimeArtifactSelection
from molt.cli.runtime_source_closure import runtime_source_paths
from molt.cli.runtime_cargo_plan import RuntimeCargoPlan, resolve_runtime_cargo_plan
from molt.cli.runtime_identity_schema import (
    RuntimeBuildIdentity,
    RuntimeBuildMemberPlan,
    RuntimeToolchainContentManifest,
    require_native_runtime_staticlib_identity as require_native_runtime_staticlib_identity,
    runtime_build_fingerprint as runtime_build_fingerprint,
    _FAMILY_SCHEMA,
    _digest,
    _freeze_json,
    _runtime_toolchain_build_python,
)
from molt.dx import _memory_bounded_worker_count
from molt.file_hashing import content_change_time_ns
from molt.file_publication import metadata_is_link_like
from molt.python_environment_identity import (
    python_capture_authority_paths,
)
from molt.toolchain_identity import (
    StableRegularFileChangedError,
    StableRegularFileError,
    open_stable_regular_file,
    probe_executable,
    resolve_executable,
    stable_regular_file_identity,
)
from molt.wasi_sysroot import resolve_wasi_sysroot_layout

_TREE_HASH_BUFFER_BYTES = 1024 * 1024
_TREE_HASH_BYTES_PER_WORKER = 2 * 1024 * 1024
_TREE_HASH_MEMORY_HEADROOM_BYTES = 256 * 1024 * 1024
_TREE_HASH_MAX_WORKERS = 32
_TREE_HASH_BATCH_SIZE = 32
_TREE_HASH_LOCAL = threading.local()


_RUNTIME_BUILD_TOOLING_RELPATHS = (
    "src/molt/_host_capabilities_generated.py",
    "src/molt/_runtime_feature_gates.py",
    "src/molt/_wasm_abi_generated.py",
    "src/molt/_wasm_runtime_exports.py",
    "src/molt/cargo_execution_policy.py",
    "src/molt/capability_policy.py",
    "src/molt/cli/cargo_execution.py",
    "src/molt/cli/cargo_profiles.py",
    "src/molt/cli/cargo_source_closure.py",
    "src/molt/cli/artifact_state.py",
    "src/molt/cli/atomic_io.py",
    "src/molt/cli/build_locks.py",
    "src/molt/file_locks.py",
    "src/molt/cli/command_runtime.py",
    "src/molt/cli/compiler_metadata.py",
    "src/molt/cli/config_resolution.py",
    "src/molt/cli/diagnostic_text.py",
    "src/molt/cli/installed_runtime.py",
    "src/molt/cli/json_cache.py",
    "src/molt/cli/llvm_wasi_tools.py",
    "src/molt/cli/models.py",
    "src/molt/cli/native_link_custody.py",
    "src/molt/cli/native_link_manifest.py",
    "src/molt/cli/native_link_plan.py",
    "src/molt/cli/project_roots.py",
    "src/molt/cli/runtime_artifact_selection.py",
    "src/molt/cli/runtime_build_identity.py",
    "src/molt/cli/runtime_build_python.py",
    "src/molt/process_guard.py",
    "tools/command_execution.py",
    "tools/import_file.py",
    "src/molt/cli/runtime_identity_schema.py",
    "src/molt/cli/runtime_cargo_plan.py",
    "src/molt/cli/runtime_features.py",
    "src/molt/cli/runtime_fingerprints.py",
    "src/molt/cli/runtime_native_build.py",
    "src/molt/cli/runtime_native_codegen.py",
    "src/molt/cli/runtime_native_generation.py",
    "src/molt/cli/runtime_paths.py",
    "src/molt/cli/runtime_source_closure.py",
    "src/molt/cli/runtime_wasm_build.py",
    "src/molt/cli/runtime_wasm_build_policy.py",
    "src/molt/cli/runtime_wasm_build_spec.py",
    "src/molt/cli/runtime_wasm_build_support.py",
    "src/molt/cli/runtime_wasm_build_timings.py",
    "src/molt/cli/runtime_wasm_cache.py",
    "src/molt/cli/runtime_wasm_cache_diagnostics.py",
    "src/molt/cli/runtime_wasm_failure.py",
    "src/molt/cli/runtime_wasm_generation.py",
    "src/molt/cli/runtime_wasm_pair_build.py",
    "src/molt/cli/runtime_wasm_validation.py",
    "src/molt/cli/static_archive_identity.py",
    "src/molt/cli/wasm_link_args.py",
    "src/molt/cli/wasm_link_inputs.py",
    "src/molt/cli/wasm_toolchain.py",
    "src/molt/dx.py",
    "src/molt/file_hashing.py",
    "src/molt/llvm_linker_roles.py",
    "src/molt/exact_json.py",
    "src/molt/toolchain_identity.py",
    "src/molt/ustar.py",
    "src/molt/wasm_artifact.py",
    "src/molt/wasm_linking_symbols.py",
    "src/molt/wasi_sysroot.py",
)


def _runtime_tree_candidates(
    root: Path,
    *,
    logical_root: str,
) -> Iterator[tuple[str, Path]]:
    pending: list[tuple[Path, str]] = [(root, "")]
    while pending:
        directory, relative_prefix = pending.pop()
        try:
            with os.scandir(directory) as iterator:
                entries = sorted(iterator, key=lambda entry: entry.name)
        except OSError as exc:
            raise OSError(
                f"runtime input enumeration failed for {logical_root!r}: "
                f"{directory}: {exc}"
            ) from exc
        subdirectories: list[tuple[Path, str]] = []
        for entry in entries:
            candidate = Path(entry.path)
            relative = (
                f"{relative_prefix}/{entry.name}" if relative_prefix else entry.name
            )
            try:
                candidate_stat = entry.stat(follow_symlinks=False)
            except OSError as exc:
                raise OSError(
                    f"runtime input enumeration failed for {logical_root!r}: "
                    f"{candidate}: {exc}"
                ) from exc
            if metadata_is_link_like(candidate_stat):
                raise ValueError(
                    f"runtime input path alias escaped logical root "
                    f"{logical_root!r}: {candidate}"
                )
            if stat_module.S_ISDIR(candidate_stat.st_mode):
                subdirectories.append((candidate, relative))
            elif stat_module.S_ISREG(candidate_stat.st_mode):
                yield f"{logical_root}/{relative.replace(os.sep, '/')}", candidate
        pending.extend(reversed(subdirectories))


@dataclass(frozen=True)
class _TreeInputCandidate:
    label: str
    path: Path


@dataclass(frozen=True)
class _TreeInputFile(_TreeInputCandidate):
    stat_signature: tuple[int, int, int, int, int, int]

    @property
    def size(self) -> int:
        return self.stat_signature[1]


def _tree_input_change_time_ns(path: Path, value: os.stat_result) -> int:
    change_time_ns = content_change_time_ns(path, value)
    if change_time_ns is None:
        raise OSError(f"runtime input ChangeTime is unavailable: {path}")
    return change_time_ns


def _tree_input_stat_signature(
    path: Path,
    value: os.stat_result,
) -> tuple[int, int, int, int, int, int]:
    return (
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        _tree_input_change_time_ns(path, value),
        value.st_dev,
        value.st_ino,
    )


def _tree_input_handle_signature(
    value: os.stat_result,
) -> tuple[int, int, int, int, int]:
    # Windows reports creation time as path st_ctime but mirrors mtime through
    # fstat().  File identity, mode, size, and mtime are comparable on all hosts;
    # the path-only ctime remains part of the before/after mutation guard.
    return (
        value.st_mode,
        value.st_size,
        value.st_mtime_ns,
        value.st_dev,
        value.st_ino,
    )


def _tree_hash_worker_count(file_count: int) -> int:
    if file_count <= 0:
        return 1
    resource_ceiling = _memory_bounded_worker_count(
        bytes_per_worker=_TREE_HASH_BYTES_PER_WORKER,
        headroom_bytes=_TREE_HASH_MEMORY_HEADROOM_BYTES,
    )
    return max(1, min(file_count, _TREE_HASH_MAX_WORKERS, resource_ceiling))


def _tree_hash_buffer() -> bytearray:
    buffer = getattr(_TREE_HASH_LOCAL, "buffer", None)
    if buffer is None:
        buffer = bytearray(_TREE_HASH_BUFFER_BYTES)
        _TREE_HASH_LOCAL.buffer = buffer
    return buffer


def _sha256_open_file(handle: object) -> str:
    hasher = hashlib.sha256()
    buffer = _tree_hash_buffer()
    readinto = getattr(handle, "readinto")
    while count := readinto(buffer):
        hasher.update(memoryview(buffer)[:count])
    return hasher.hexdigest()


def _runtime_input_changed(file: _TreeInputFile) -> ValueError:
    return ValueError(
        f"runtime input changed while hashing {file.label!r}: {file.path}"
    )


def _snapshot_tree_input_file(candidate: _TreeInputCandidate) -> _TreeInputFile:
    try:
        current = candidate.path.lstat()
        if metadata_is_link_like(current):
            raise ValueError(
                f"runtime input path alias is forbidden for "
                f"{candidate.label!r}: {candidate.path}"
            )
        if not stat_module.S_ISREG(current.st_mode):
            raise ValueError(
                f"runtime input is no longer a regular file for "
                f"{candidate.label!r}: {candidate.path}"
            )
        return _TreeInputFile(
            label=candidate.label,
            path=candidate.path,
            stat_signature=_tree_input_stat_signature(candidate.path, current),
        )
    except (OSError, ValueError) as exc:
        if isinstance(exc, ValueError):
            raise
        raise OSError(
            f"runtime input snapshot failed for {candidate.label!r}: "
            f"{candidate.path}: {exc}"
        ) from exc


def _hash_tree_input_file(file: _TreeInputFile) -> str:
    try:
        expected_handle_signature = (
            file.stat_signature[0],
            file.stat_signature[1],
            file.stat_signature[2],
            file.stat_signature[4],
            file.stat_signature[5],
        )
        # Source, sysroot and archive captures share the same no-follow handle
        # transaction. The tree snapshot additionally fences the interval from
        # enumeration to this read; ChangeTime detects restored-mtime writes.
        with open_stable_regular_file(
            file.path, label=f"runtime input {file.label!r}"
        ) as opened:
            if (
                _tree_input_handle_signature(opened.stat) != expected_handle_signature
                or opened.content_change_time_ns != file.stat_signature[3]
            ):
                raise _runtime_input_changed(file)
            digest = _sha256_open_file(opened.stream)
        return digest
    except StableRegularFileChangedError as exc:
        raise _runtime_input_changed(file) from exc
    except StableRegularFileError as exc:
        if isinstance(exc.__cause__, OSError):
            raise OSError(
                f"runtime input hashing failed for {file.label!r}: {file.path}: {exc}"
            ) from exc
        raise
    except (OSError, ValueError) as exc:
        if isinstance(exc, ValueError):
            raise
        raise OSError(
            f"runtime input hashing failed for {file.label!r}: {file.path}: {exc}"
        ) from exc


_ParallelInput = TypeVar("_ParallelInput")
_ParallelOutput = TypeVar("_ParallelOutput")


def _bounded_parallel_map(
    inputs: Sequence[_ParallelInput],
    operation: Callable[[_ParallelInput], _ParallelOutput],
    *,
    workers: int,
) -> tuple[_ParallelOutput, ...]:
    """Keep ordered results and bounded batches without one future per file."""
    if not inputs:
        return ()
    if workers == 1:
        return tuple(operation(item) for item in inputs)

    results: list[_ParallelOutput | None] = [None] * len(inputs)
    max_pending = workers * 2
    # Keep small closures parallel and leave a second batch per worker when
    # possible, while amortizing dispatch for the large source/sysroot trees.
    batch_size = min(
        _TREE_HASH_BATCH_SIZE, (len(inputs) + max_pending - 1) // max_pending
    )
    iterator = iter(range(0, len(inputs), batch_size))

    def run_batch(start: int) -> tuple[_ParallelOutput, ...]:
        return tuple(
            operation(inputs[index])
            for index in range(start, min(start + batch_size, len(inputs)))
        )

    with ThreadPoolExecutor(
        max_workers=workers,
        thread_name_prefix="molt-runtime-identity",
    ) as executor:
        pending: dict[Future[tuple[_ParallelOutput, ...]], int] = {}

        def submit_one() -> bool:
            try:
                start = next(iterator)
            except StopIteration:
                return False
            pending[executor.submit(run_batch, start)] = start
            return True

        for _ in range(max_pending):
            if not submit_one():
                break
        while pending:
            completed, _ = wait(pending, return_when=FIRST_COMPLETED)
            for future in completed:
                start = pending.pop(future)
                try:
                    batch = future.result()
                    results[start : start + len(batch)] = batch
                except BaseException:
                    for remaining in pending:
                        remaining.cancel()
                    raise
            for _ in completed:
                submit_one()
    return cast(tuple[_ParallelOutput, ...], tuple(results))


def _snapshot_tree_input_files(
    candidates: Sequence[_TreeInputCandidate],
) -> tuple[_TreeInputFile, ...]:
    return _bounded_parallel_map(
        candidates,
        _snapshot_tree_input_file,
        workers=_tree_hash_worker_count(len(candidates)),
    )


def _hash_tree_input_files(files: Sequence[_TreeInputFile]) -> dict[str, str]:
    digests = _bounded_parallel_map(
        files,
        _hash_tree_input_file,
        workers=_tree_hash_worker_count(len(files)),
    )
    return {file.label: digest for file, digest in zip(files, digests, strict=True)}


@dataclass(frozen=True)
class RuntimeTreeIndex:
    """One uncached hash pass over logical roots, projecting exact tree identities.

    ``_tree_identity`` is ``capture(roots).identity(roots)``. A caller comparing
    several receipts with one source tree captures the union of their roots once
    and projects each receipt's summary without hashing shared files again.
    """

    root_paths: dict[str, Path]
    missing: frozenset[str]
    root_files: dict[str, tuple[str, ...]]
    sizes: dict[str, int]
    digests: dict[str, str]

    @classmethod
    def capture(cls, roots: Sequence[tuple[str, Path]]) -> RuntimeTreeIndex:
        root_labels: dict[str, Path] = {}
        files: dict[str, _TreeInputCandidate] = {}
        root_files: dict[str, tuple[str, ...]] = {}
        missing: set[str] = set()
        for logical_root, raw_path in roots:
            try:
                raw_stat = raw_path.lstat()
            except FileNotFoundError:
                raw_stat = None
            if raw_stat is not None and metadata_is_link_like(raw_stat):
                raise ValueError(
                    f"runtime input root alias is forbidden for {logical_root!r}: {raw_path}"
                )
            path = raw_path.resolve(strict=False)
            prior_root = root_labels.get(logical_root)
            if prior_root is not None and prior_root != path:
                raise ValueError(
                    f"runtime input root label collision {logical_root!r}: "
                    f"{prior_root} vs {path}"
                )
            root_labels[logical_root] = path
            if logical_root in root_files or logical_root in missing:
                continue
            candidates: list[tuple[str, Path]] = []
            try:
                root_stat = path.lstat()
            except FileNotFoundError:
                missing.add(logical_root)
                continue
            if stat_module.S_ISREG(root_stat.st_mode):
                candidates.append((logical_root, path))
            elif stat_module.S_ISDIR(root_stat.st_mode):
                candidates.extend(
                    _runtime_tree_candidates(path, logical_root=logical_root)
                )
            else:
                raise ValueError(
                    f"runtime input root is not a regular file or directory: "
                    f"{logical_root!r}: {path}"
                )
            for label, candidate in candidates:
                prior = files.get(label)
                if prior is not None and prior.path != candidate:
                    raise ValueError(
                        f"runtime input file label collision {label!r}: "
                        f"{prior.path} vs {candidate}"
                    )
                files[label] = _TreeInputCandidate(
                    label=label,
                    path=candidate,
                )
            root_files[logical_root] = tuple(label for label, _path in candidates)
        ordered_files = _snapshot_tree_input_files(
            tuple(files[label] for label in sorted(files))
        )
        return cls(
            root_paths=root_labels,
            missing=frozenset(missing),
            root_files=root_files,
            sizes={file.label: file.size for file in ordered_files},
            digests=_hash_tree_input_files(ordered_files),
        )

    def identity(
        self, roots: Sequence[tuple[str, Path]], *, require_all: bool
    ) -> dict[str, object]:
        """Project the exact ``_tree_identity`` summary of captured ``roots``."""
        root_labels: set[str] = set()
        labels: set[str] = set()
        missing: list[str] = []
        for logical_root, raw_path in roots:
            if self.root_paths.get(logical_root) != raw_path.resolve(strict=False):
                raise ValueError(
                    f"runtime input root {logical_root!r} is not in this tree index"
                )
            root_labels.add(logical_root)
            if logical_root in self.missing:
                missing.append(logical_root)
            else:
                labels.update(self.root_files[logical_root])
        if require_all and missing:
            raise ValueError(
                "required runtime inputs are missing: " + ", ".join(missing)
            )
        hasher = hashlib.sha256()
        total_size = 0
        for label in sorted(labels):
            size = self.sizes[label]
            total_size += size
            hasher.update(label.encode())
            hasher.update(b"\0")
            hasher.update(str(size).encode())
            hasher.update(b"\0")
            hasher.update(self.digests[label].encode())
            hasher.update(b"\0")
        for label in sorted(missing):
            hasher.update(b"missing\0")
            hasher.update(label.encode())
            hasher.update(b"\0")
        return {
            "digest": hasher.hexdigest(),
            "file_count": len(labels),
            "total_size": total_size,
            "roots": sorted(root_labels),
            "missing": sorted(missing),
        }


def _tree_identity(
    roots: Sequence[tuple[str, Path]],
    *,
    require_all: bool,
) -> dict[str, object]:
    """Hash an uncached logical-label closure with exact mutation checks."""

    return RuntimeTreeIndex.capture(roots).identity(roots, require_all=require_all)


def runtime_source_roots(
    project_root: Path, runtime_features: Sequence[str]
) -> list[tuple[str, Path]]:
    """Logical runtime source roots recorded as ``compile.sources``."""
    root = project_root.resolve(strict=False)
    source_roots: list[tuple[str, Path]] = []
    for path in runtime_source_paths(root, tuple(runtime_features)):
        resolved = path.resolve(strict=False)
        try:
            label = "source/" + resolved.relative_to(root).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"runtime source escaped project root: {resolved}"
            ) from exc
        source_roots.append((label, resolved))
    return source_roots


def runtime_identity_source_facts(
    identity: RuntimeBuildIdentity,
) -> tuple[tuple[str, ...], Mapping[str, object]]:
    """The feature set and recorded source tree of one runtime build identity."""
    family = cast(Mapping[str, object], identity.payload["family"])
    compilation = cast(Mapping[str, object], family["compile"])
    configuration = cast(Mapping[str, object], compilation["common_config"])
    return (
        tuple(cast(Sequence[str], configuration["runtime_features"])),
        cast(Mapping[str, object], compilation["sources"]),
    )


def verify_runtime_source_trees(
    project_root: Path,
    facts: Sequence[tuple[Sequence[str], Mapping[str, object]]],
) -> None:
    """Require recorded runtime source trees to be ``project_root``'s.

    The union of every receipt's source roots is hashed once; each receipt's
    summary is then projected exactly as ``_tree_identity`` computes it.
    """
    requests = [
        (tuple(features), runtime_source_roots(project_root, features), recorded)
        for features, recorded in facts
    ]
    index = RuntimeTreeIndex.capture(
        list(dict.fromkeys(root for _features, roots, _r in requests for root in roots))
    )
    for features, roots, recorded in requests:
        if _digest(index.identity(roots, require_all=False)) != _digest(recorded):
            raise ValueError(
                "runtime build identity sources differ from the release source "
                f"(features: {', '.join(features) or 'none'})"
            )


def verify_runtime_source_identities(
    project_root: Path, identities: Sequence[RuntimeBuildIdentity]
) -> None:
    verify_runtime_source_trees(
        project_root, [runtime_identity_source_facts(item) for item in identities]
    )


def _command_path(command: str, env: Mapping[str, str]) -> Path | None:
    try:
        return resolve_executable(command, environment=env, label="runtime tool")
    except ValueError:
        return None


def _executable_identity(
    logical_name: str,
    command: str,
    *,
    env: Mapping[str, str],
) -> dict[str, object]:
    path = _command_path(command, env)
    if path is None:
        raise ValueError(f"runtime tool {logical_name} is unresolved")
    identity = probe_executable(
        path,
        version_arguments=(("--version",),),
        environment=env,
        label=f"runtime tool {logical_name}",
        version_pattern=re.compile(r"\b\d+(?:\.\d+)+(?:[-+][A-Za-z0-9._-]+)?\b"),
    )
    return {
        "logical_name": logical_name,
        **identity.as_record(),
    }


def _python_identity(
    env: Mapping[str, str], *, admission: BuildPythonAdmission | None = None
) -> dict[str, object]:
    if admission is not None:
        return admission.capture(env)
    with BuildPythonAdmission() as owned:
        return owned.capture(env)


def _archive_identity(logical_name: str, path: Path | None) -> dict[str, object]:
    if path is None:
        raise ValueError(f"required runtime archive {logical_name} is unresolved")
    identity = stable_regular_file_identity(
        path, label=f"runtime archive {logical_name}"
    )
    return {
        "logical_name": logical_name,
        "sha256": identity.sha256,
        "size": identity.size,
    }


def _build_script_path(
    raw: str,
    *,
    build_script_root: Path,
) -> Path:
    path = Path(raw)
    if not path.is_absolute():
        path = build_script_root / path
    return path


def _build_script_file_environment_identity(
    name: str,
    env: Mapping[str, str],
    *,
    build_script_root: Path,
) -> dict[str, object]:
    raw = env.get(name)
    if raw is None:
        return {"state": "unset"}
    value = raw.strip()
    if not value:
        return {"state": "empty"}
    path = _build_script_path(value, build_script_root=build_script_root)
    if not path.is_file():
        # The Rust resolver treats an unresolved explicit path exactly like an
        # absent path and continues to its content-attested sysroot fallback.
        return {"state": "fallback"}
    return {
        "state": "resolved",
        "content": _archive_identity(name.lower(), path),
    }


_C_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def _build_script_symbol_environment_value(
    name: str,
    env: Mapping[str, str],
) -> tuple[str, ...]:
    raw = env.get(name, "")
    symbols = tuple(
        sorted({symbol for symbol in re.split(r"[,;\s]+", raw.strip()) if symbol})
    )
    invalid = [symbol for symbol in symbols if _C_IDENTIFIER.fullmatch(symbol) is None]
    if invalid:
        raise ValueError(
            f"runtime build-script environment {name} contains invalid C symbols: "
            + ", ".join(invalid)
        )
    return symbols


def _build_python_script_environment_identity(
    env: Mapping[str, str],
    *,
    build_python_identity: Mapping[str, object],
) -> dict[str, object]:
    build_python_values = {
        name: env.get(name, "").strip() for name in ("MOLT_BUILD_PYTHON", "PYTHON")
    }
    selected_python = next(
        (name for name, value in build_python_values.items() if value),
        "platform-default",
    )
    python_selectors = {
        name: (
            "selected" if name == selected_python else "shadowed" if value else "unset"
        )
        for name, value in build_python_values.items()
    }
    return {
        "build_python": {
            "selected_by": selected_python,
            "selectors": python_selectors,
            "content_digest": _digest(build_python_identity),
        },
        # Every Rust table generator uses build_support/build_python.rs.
        # Its import isolation matches the attested -B -I -S interpreter probe;
        # ambient project trees are not runtime build inputs.
        "python_import_policy": "isolated-no-site-v1",
    }


def _runtime_build_script_environment_identity(
    project_root: Path,
    env: Mapping[str, str],
    *,
    target_triple: str,
    build_python_identity: Mapping[str, object],
) -> dict[str, object]:
    """Return every output-bearing environment input read by molt-runtime/build.rs."""

    function_exports = _build_script_symbol_environment_value(
        "MOLT_WASM_CPYTHON_ABI_EXPORTS", env
    )
    data_exports = _build_script_symbol_environment_value(
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS", env
    )
    if not set(data_exports) <= set(function_exports):
        raise ValueError(
            "runtime build-script CPython ABI data exports are not a subset of "
            "the requested export set"
        )
    build_script_root = project_root / "runtime" / "molt-runtime"
    wasm_target = target_triple.startswith("wasm32-")
    return {
        "schema": "molt.runtime-build-script-environment.v2",
        **_build_python_script_environment_identity(
            env,
            build_python_identity=build_python_identity,
        ),
        "MOLT_WASM_CPYTHON_ABI_EXPORTS": (
            list(function_exports) if wasm_target else "ignored-for-target"
        ),
        "MOLT_WASM_CPYTHON_ABI_DATA_EXPORTS": (
            list(data_exports) if wasm_target else "ignored-for-target"
        ),
        "MOLT_WASM_LONGDOUBLE_ARCHIVE": (
            _build_script_file_environment_identity(
                "MOLT_WASM_LONGDOUBLE_ARCHIVE",
                env,
                build_script_root=build_script_root,
            )
            if wasm_target
            else {"state": "ignored-for-target"}
        ),
        "MOLT_WASM_BUILTINS_ARCHIVE": (
            _build_script_file_environment_identity(
                "MOLT_WASM_BUILTINS_ARCHIVE",
                env,
                build_script_root=build_script_root,
            )
            if wasm_target
            else {"state": "ignored-for-target"}
        ),
    }


def _host_absolute_style(value: str) -> str | None:
    """Classify absolute paths lexically, independent of the current host OS."""

    if not value:
        return None
    windows = PureWindowsPath(value)
    if windows.is_absolute() or value.startswith("\\"):
        return "windows"
    if PurePosixPath(value).is_absolute():
        return "posix"
    return None


def _is_host_absolute(value: str) -> bool:
    return _host_absolute_style(value) is not None


@dataclass(frozen=True)
class _RuntimeFlagProjection:
    """Ordered logical paths captured once for one runtime identity operation.

    Resolved roots and their text forms are local to this capture, never a
    process cache. Absolute operands still resolve when inspected; response
    files retain the Cargo plan's separate content and custody checks.
    """

    roots: tuple[tuple[str, Path, str], ...]
    cargo_plan: RuntimeCargoPlan | None = None

    @classmethod
    def capture(
        cls,
        logical_paths: Sequence[tuple[str, Path]],
        *,
        cargo_plan: RuntimeCargoPlan | None = None,
    ) -> _RuntimeFlagProjection:
        return cls(
            roots=tuple(
                (label, resolved, os.fspath(resolved))
                for label, root in logical_paths
                for resolved in (root.resolve(strict=False),)
            ),
            cargo_plan=cargo_plan,
        )

    def prepend_root(self, label: str, root: Path) -> _RuntimeFlagProjection:
        prefix = self.capture(((label, root),), cargo_plan=self.cargo_plan)
        return type(self)(
            roots=prefix.roots + self.roots,
            cargo_plan=self.cargo_plan,
        )

    def path_operand(self, raw: str) -> str:
        value = raw.strip('"')
        style = _host_absolute_style(value)
        if style is None:
            return value
        native_style = "windows" if os.name == "nt" else "posix"
        if style != native_style:
            raise ValueError(
                f"unknown absolute host path in canonical runtime flags: {raw!r}"
            )
        candidate = Path(value).resolve(strict=False)
        for label, resolved_root, _text in self.roots:
            try:
                relative = candidate.relative_to(resolved_root)
            except ValueError:
                continue
            suffix = "" if not relative.parts else "/" + relative.as_posix()
            return f"${{{label}}}{suffix}"
        raise ValueError(
            f"unknown absolute host path in canonical runtime flags: {raw!r}"
        )

    def token(self, token: str) -> str:
        if "@" in token:
            if self.cargo_plan is None:
                raise ValueError(
                    "runtime response argument requires captured Cargo plan custody"
                )
            return self.cargo_plan.project_link_arguments((token,))[0]
        for prefix in (
            "-Clink-arg=",
            "-Clinker=",
            "--sysroot=",
            "-Lnative=",
            "-Ldependency=",
            "/LIBPATH:",
            "-I",
            "-L",
        ):
            if token.startswith(prefix) and len(token) > len(prefix):
                operand = token[len(prefix) :]
                if prefix == "-Clink-arg=":
                    return prefix + self.token(operand)
                return prefix + self.path_operand(operand)
        if _is_host_absolute(token):
            return self.path_operand(token)
        if "=" in token:
            prefix, operand = token.split("=", 1)
            if _is_host_absolute(operand):
                return prefix + "=" + self.path_operand(operand)
        for _label, _root, root_text in self.roots:
            if token.find(root_text) > 0:
                raise ValueError(
                    f"embedded absolute host path in canonical runtime flags: {token!r}"
                )
        if (
            re.search(r"(?i)[a-z]:[\\/]", token)
            or "\\\\" in token
            or re.search(r"(?:^|[=@])[/\\][A-Za-z?]", token)
        ):
            raise ValueError(
                f"embedded absolute host path in canonical runtime flags: {token!r}"
            )
        return token

    def rustflags(self, value: str | Sequence[str]) -> list[str]:
        if isinstance(value, str):
            if os.name != "nt" and re.search(r"(?i)[a-z]:[\\/]", value):
                raise ValueError(
                    f"unknown absolute host path in canonical runtime flags: {value!r}"
                )
            try:
                tokens = shlex.split(value, posix=os.name != "nt")
            except ValueError as exc:
                raise ValueError(f"invalid runtime flag plan: {value!r}") from exc
        else:
            tokens = value
        return [self.token(token.strip('"')) for token in tokens]

    def link_args(self, args: Sequence[str]) -> list[str]:
        return [self.token(arg) for arg in args]


def _ambient_c_build_environment(cargo_plan: RuntimeCargoPlan) -> dict[str, list[str]]:
    """Project the exact cc-rs token/search authority captured for execution."""
    return {name: list(tokens) for name, tokens in cargo_plan.c_environment.items()}


def _target_environment_forms(target_triple: str) -> tuple[str, ...]:
    return (
        target_triple,
        target_triple.replace("-", "_"),
        target_triple.upper().replace("-", "_"),
    )


def _runtime_build_environment_identity(
    cargo_plan: RuntimeCargoPlan,
    *,
    cargo_profile: str,
) -> dict[str, str]:
    env = cargo_plan.environment
    exact_names = {
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_DEP_INFO_BASEDIR",
        "CARGO_INCREMENTAL",
        "RUSTC_BOOTSTRAP",
        "RUSTUP_TOOLCHAIN",
        "SOURCE_DATE_EPOCH",
        "MACOSX_DEPLOYMENT_TARGET",
        "CRATE_CC_NO_DEFAULTS",
    }
    exact_names.update(cargo_plan.profile_environment(cargo_profile))
    return {
        name: hashlib.sha256(env[name].encode("utf-8")).hexdigest()
        for name in sorted(exact_names)
        if env.get(name, "")
    }


def runtime_build_tooling_paths(project_root: Path) -> tuple[Path, ...]:
    """Project the canonical implementation closure into a source checkout."""
    root = project_root.resolve(strict=False)
    paths = {root / relative for relative in _RUNTIME_BUILD_TOOLING_RELPATHS}
    paths.update(python_capture_authority_paths(source_root=root))
    return tuple(sorted(paths))


def _capture_runtime_build_trees(
    project_root: Path, source_roots: Sequence[tuple[str, Path]]
) -> tuple[dict[str, object], dict[str, object]]:
    """Capture source and publication trees together for one live resolution.

    Their logical labels and receipt roles stay separate. Each resolver call
    owns a fresh index, including post-build and final-link recaptures.
    """

    root = project_root.resolve(strict=False)
    tooling_roots = tuple(
        ("runtime-tooling/" + path.relative_to(root).as_posix(), path)
        for path in runtime_build_tooling_paths(root)
    )
    index = RuntimeTreeIndex.capture((*source_roots, *tooling_roots))
    publication_authority = {
        "schema": "molt.runtime-build-tooling-authority.v2",
        **index.identity(tooling_roots, require_all=True),
    }
    return index.identity(source_roots, require_all=False), publication_authority


def _capture_plan_toolchain(
    cargo_plan: RuntimeCargoPlan,
    *,
    build_python_admission: BuildPythonAdmission | None = None,
) -> dict[str, object]:
    # Runtime generators execute Python; Cargo-only consumers do not. Capture
    # that extra first, then close shared Cargo custody after its admission.
    build_python = _python_identity(
        cargo_plan.environment, admission=build_python_admission
    )
    result = cargo_plan.toolchain_identity()
    cast(dict[str, object], result["tools"])["build_python"] = build_python
    return result


def _verify_plan_toolchain_content(
    plan: RuntimeCargoPlan, content: Mapping[str, object]
) -> None:
    """Reconcile newly captured tools/resources with their live execution plan."""
    tools = cast(Mapping[str, object], content["tools"])
    wrappers = cast(Mapping[str, object], content["wrappers"])
    if set(tools) - {"build_python", "wasm_linker"} != set(plan.tools) or set(
        wrappers
    ) != set(plan.wrappers):
        raise ValueError(
            "runtime toolchain manifest selected tools differ from Cargo plan"
        )
    for item in plan.executable_custody:
        group, role = item.label.split("/", 1)
        actual = tools[role] if group == "tool" else wrappers[role]
        expected = {
            "logical_name": role if group == "tool" else role.casefold(),
            **item.content_record(),
        }
        if actual != expected:
            raise ValueError(
                f"runtime toolchain manifest {role} differs from captured Cargo plan"
            )
    if (
        content["effective_target"] != plan.target
        or _freeze_json(content["cargo_configuration"])
        != _freeze_json(plan.configuration_identity())
        or _freeze_json(content["rust_resources"])
        != _freeze_json(plan.rust_resource_identity())
    ):
        raise ValueError(
            "runtime toolchain manifest configuration/resources differ from Cargo plan"
        )


def _verify_plan_toolchain_manifest(
    plan: RuntimeCargoPlan, manifest: RuntimeToolchainContentManifest
) -> None:
    _verify_plan_toolchain_content(
        plan, cast(Mapping[str, object], manifest.payload["toolchain"])
    )


def _plan_for_capture(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_command: Sequence[str],
    target_triple: str | None,
    cargo_plan: RuntimeCargoPlan | None,
) -> RuntimeCargoPlan:
    if cargo_plan is None:
        return resolve_runtime_cargo_plan(
            project_root,
            env=env,
            cargo_command=cargo_command,
            requested_target=target_triple,
        )
    if cargo_plan.project_root != project_root.resolve(strict=False):
        raise ValueError("runtime Cargo plan belongs to another source root")
    if cargo_plan.target != (target_triple or cargo_plan.host_target):
        raise ValueError("runtime Cargo plan target differs from identity target")
    if tuple(cargo_command[1:]) != cargo_plan.command[1:]:
        raise ValueError(
            "runtime Cargo command differs from the resolved execution plan"
        )
    if dict(env) != dict(cargo_plan.environment):
        raise ValueError(
            "runtime Cargo environment differs from the resolved execution plan"
        )
    return cargo_plan


def _wasm_compile_toolchain_content(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    include_cxx: bool = True,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> dict[str, object]:
    plan = cargo_plan or resolve_runtime_cargo_plan(
        project_root,
        env=env,
        cargo_command=(env.get("CARGO", "cargo"), "rustc", "--target", target_triple),
        requested_target=target_triple,
    )
    if plan.target != target_triple:
        raise ValueError("runtime WASM toolchain plan target is invalid")
    layout = resolve_wasi_sysroot_layout(wasi_sysroot)
    if layout is None:
        raise ValueError(f"runtime WASI sysroot layout is unresolved: {wasi_sysroot}")
    content = _capture_plan_toolchain(
        plan, build_python_admission=build_python_admission
    )
    tools = cast(Mapping[str, object], content["tools"])
    required = {"cc", "ar", "ranlib"} | ({"cxx"} if include_cxx else set())
    if not required.issubset(tools):
        raise ValueError("runtime WASM C/C++ archive toolchain is incomplete")
    content["sysroots"] = {
        "wasi": _tree_identity(
            tuple((f"wasi/{label}", path) for label, path in layout.content_roots()),
            require_all=False,
        )
    }
    return content


def _wasm_runtime_toolchain_content(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> dict[str, object]:
    content = _wasm_compile_toolchain_content(
        build_python_admission=build_python_admission,
        project_root=project_root,
        env=env,
        target_triple=target_triple,
        wasi_sysroot=wasi_sysroot,
        cargo_plan=cargo_plan,
    )
    tools = cast(dict[str, object], content["tools"])
    tools["wasm_linker"] = _executable_identity(
        "wasm_linker", os.fspath(wasm_linker), env=env
    )
    content["archives"] = [
        _archive_identity("wasi-libc", wasi_libc_archive),
        _archive_identity("rust-compiler-builtins", rust_builtins_archive),
        _archive_identity("wasi-long-double", long_double_archive),
        _archive_identity("clang-rt-builtins", builtins_archive),
    ]
    return content


def provision_wasm_runtime_toolchain_content_manifest(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeToolchainContentManifest:
    """Produce the immutable content manifest consumed by normal identity reads."""

    payload = {
        "target_triple": target_triple,
        "toolchain": _wasm_runtime_toolchain_content(
            build_python_admission=build_python_admission,
            project_root=project_root,
            env=env,
            target_triple=target_triple,
            wasi_sysroot=wasi_sysroot,
            cargo_plan=cargo_plan,
            wasm_linker=wasm_linker,
            long_double_archive=long_double_archive,
            builtins_archive=builtins_archive,
            wasi_libc_archive=wasi_libc_archive,
            rust_builtins_archive=rust_builtins_archive,
        ),
    }
    return RuntimeToolchainContentManifest.from_payload(payload)


def provision_wasm_cpython_abi_toolchain_content_manifest(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeToolchainContentManifest:
    """Capture the complete compiler inputs for the standalone ABI staticlib."""

    payload = {
        "target_triple": target_triple,
        "toolchain": _wasm_compile_toolchain_content(
            build_python_admission=build_python_admission,
            project_root=project_root,
            env=env,
            target_triple=target_triple,
            wasi_sysroot=wasi_sysroot,
            cargo_plan=cargo_plan,
            include_cxx=False,
        ),
    }
    return RuntimeToolchainContentManifest.from_payload(payload)


def provision_native_runtime_toolchain_content_manifest(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str | None,
    cargo_command: Sequence[str],
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeToolchainContentManifest:
    plan = _plan_for_capture(
        project_root,
        env=env,
        cargo_command=cargo_command,
        target_triple=target_triple,
        cargo_plan=cargo_plan,
    )
    payload = {
        "target_triple": target_triple or "native",
        "toolchain": _capture_plan_toolchain(
            plan, build_python_admission=build_python_admission
        ),
    }
    return RuntimeToolchainContentManifest.from_payload(payload)


def resolve_native_runtime_build_identity(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str | None,
    runtime_features: tuple[str, ...],
    cargo_command: Sequence[str],
    artifact_selection: RuntimeArtifactSelection,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeBuildIdentity:
    """Resolve one exact native staticlib identity through the shared family model."""

    root = project_root.resolve(strict=False)
    plan = _plan_for_capture(
        root,
        env=env,
        cargo_command=cargo_command,
        target_triple=target_triple,
        cargo_plan=cargo_plan,
    )
    env = plan.environment
    cargo_command = plan.command
    logical_target = target_triple or "native"
    flag_projection = _RuntimeFlagProjection.capture(
        (*plan.logical_paths, ("source", root)), cargo_plan=plan
    )
    source_roots = runtime_source_roots(root, runtime_features)
    sources, publication_authority = _capture_runtime_build_trees(root, source_roots)
    toolchain_manifest = provision_native_runtime_toolchain_content_manifest(
        build_python_admission=build_python_admission,
        project_root=root,
        env=env,
        cargo_plan=plan,
        target_triple=target_triple,
        cargo_command=cargo_command,
    )
    if not cargo_command:
        raise ValueError("native runtime Cargo command is empty")
    build_python_identity = _runtime_toolchain_build_python(toolchain_manifest)
    compile_command, final_link_arguments = plan.partition_command()
    command = flag_projection.rustflags(compile_command)
    rustflags = plan.rustflags
    member = RuntimeBuildMemberPlan(
        kind="staticlib",
        resolved_rustflags=tuple(flag_projection.rustflags(rustflags)),
        link_args=(
            "--print",
            "native-static-libs",
            *flag_projection.link_args(final_link_arguments),
        ),
        publication_transform="native-staticlib-and-link-manifest-v1",
        preserve_debug=plan.preserve_debug_for_profile(cargo_profile),
    )
    identities = _resolve_runtime_build_family_identities(
        sources=sources,
        toolchain_manifest=toolchain_manifest,
        target_triple=logical_target,
        common_config={
            "cargo_profile": cargo_profile,
            "target_triple": logical_target,
            "runtime_features": sorted(set(runtime_features)),
            "producer_artifact_selection": artifact_selection.source_identity,
            "cargo_command": command,
            "base_rustflags": flag_projection.rustflags(plan.rustflags),
            "ambient_c_build_environment": _ambient_c_build_environment(plan),
            "environment": _runtime_build_environment_identity(
                plan,
                cargo_profile=cargo_profile,
            ),
            "build_script_environment": _runtime_build_script_environment_identity(
                root,
                env,
                target_triple=logical_target,
                build_python_identity=build_python_identity,
            ),
        },
        publication_authority=publication_authority,
        members=(member,),
    )
    return identities[0]


def _resolve_runtime_build_family_identities(
    *,
    sources: Mapping[str, object],
    toolchain_manifest: RuntimeToolchainContentManifest,
    target_triple: str,
    common_config: Mapping[str, object],
    publication_authority: Mapping[str, object],
    members: Sequence[RuntimeBuildMemberPlan],
) -> tuple[RuntimeBuildIdentity, ...]:
    manifest = RuntimeToolchainContentManifest.from_dict(toolchain_manifest)
    if manifest.payload.get("target_triple") != target_triple:
        raise ValueError("runtime toolchain manifest target is invalid")
    toolchain = manifest.payload.get("toolchain")
    if not isinstance(toolchain, Mapping):
        raise ValueError("runtime toolchain manifest content is invalid")
    if not members:
        raise ValueError("runtime build family has no members")
    member_payloads: dict[str, dict[str, object]] = {}
    for plan in members:
        if not plan.kind or plan.kind in member_payloads:
            raise ValueError("runtime build family member kinds are invalid")
        if isinstance(plan.resolved_rustflags, str):
            raise ValueError("runtime build family member flags are not canonical")
        member_payloads[plan.kind] = {
            "kind": plan.kind,
            "resolved_rustflags": list(plan.resolved_rustflags),
            "link_args": list(plan.link_args),
            "publication_transform": plan.publication_transform,
            "preserve_debug": plan.preserve_debug,
        }
    publication_payload = _freeze_json(publication_authority)
    if not isinstance(publication_payload, Mapping):
        raise ValueError("runtime publication authority is invalid")
    compile_payload = _freeze_json(
        {
            "sources": sources,
            "toolchain": toolchain,
            "common_config": common_config,
        }
    )
    compile_digest = _digest(compile_payload)
    family = _freeze_json(
        {
            "schema": _FAMILY_SCHEMA,
            "compile_digest": compile_digest,
            "compile": compile_payload,
            "publication_authority": publication_payload,
            "members": member_payloads,
        }
    )
    family_digest = _digest(family)

    def identity(kind: str) -> RuntimeBuildIdentity:
        payload = _freeze_json({"family": family, "member_kind": kind})
        return RuntimeBuildIdentity(
            _digest(payload),
            compile_digest,
            family_digest,
            payload,
        )

    return tuple(identity(plan.kind) for plan in members)


def resolve_wasm_cpython_abi_build_identity(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str,
    rustflags: str,
    cargo_command: Sequence[str],
    artifact_selection: RuntimeArtifactSelection,
    wasi_sysroot: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> RuntimeBuildIdentity:
    """Resolve the exact standalone CPython-ABI WASM staticlib identity."""

    root = project_root.resolve(strict=False)
    plan = _plan_for_capture(
        root,
        env=env,
        cargo_command=cargo_command,
        target_triple=target_triple,
        cargo_plan=cargo_plan,
    )
    env = plan.environment
    cargo_command = plan.command
    crate_root = root / "runtime" / "molt-cpython-abi"
    source_paths = _cargo_crate_source_closure(
        project_root=root,
        crate_root=crate_root,
        crate_features=(),
        extra_source_paths=(
            root / "Cargo.toml",
            root / "Cargo.lock",
            root / "runtime" / "build_support",
            root / "include" / "molt" / "shared",
        ),
    )
    source_roots: list[tuple[str, Path]] = []
    for path in source_paths:
        resolved = path.resolve(strict=False)
        try:
            label = "source/" + resolved.relative_to(root).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"CPython ABI source escaped project root: {resolved}"
            ) from exc
        source_roots.append((label, resolved))
    sources, publication_authority = _capture_runtime_build_trees(root, source_roots)
    toolchain_manifest = provision_wasm_cpython_abi_toolchain_content_manifest(
        build_python_admission=build_python_admission,
        project_root=root,
        env=env,
        cargo_plan=plan,
        target_triple=target_triple,
        wasi_sysroot=wasi_sysroot,
    )
    _verify_plan_toolchain_manifest(plan, toolchain_manifest)
    build_python_identity = _runtime_toolchain_build_python(toolchain_manifest)
    if not cargo_command:
        raise ValueError("CPython ABI Cargo command is empty")
    flag_projection = _RuntimeFlagProjection.capture(
        (
            *plan.logical_paths,
            ("source", root),
            ("wasi-sysroot", wasi_sysroot),
        ),
        cargo_plan=plan,
    )
    member = RuntimeBuildMemberPlan(
        kind="staticlib",
        resolved_rustflags=tuple(flag_projection.rustflags(rustflags)),
        link_args=(),
        publication_transform="wasm-cpython-abi-staticlib-v1",
        preserve_debug=plan.preserve_debug_for_profile(cargo_profile),
    )
    identity = _resolve_runtime_build_family_identities(
        sources=sources,
        toolchain_manifest=toolchain_manifest,
        target_triple=target_triple,
        common_config={
            "cargo_profile": cargo_profile,
            "target_triple": target_triple,
            "runtime_features": ["molt-cpython-abi-static-link"],
            "producer_artifact_selection": artifact_selection.source_identity,
            "cargo_command": flag_projection.rustflags(plan.partition_command()[0]),
            "ambient_c_build_environment": _ambient_c_build_environment(plan),
            "environment": _runtime_build_environment_identity(
                plan,
                cargo_profile=cargo_profile,
            ),
            "build_script_environment": {
                "schema": "molt.cpython-abi-build-script-environment.v2",
                **_build_python_script_environment_identity(
                    env,
                    build_python_identity=build_python_identity,
                ),
            },
        },
        publication_authority=publication_authority,
        members=(member,),
    )
    return identity[0]


def resolve_wasm_runtime_build_family_identities(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str,
    runtime_features: tuple[str, ...],
    base_rustflags: str,
    cargo_command: Sequence[str],
    producer_artifact_selection: RuntimeArtifactSelection,
    members: Sequence[RuntimeBuildMemberPlan],
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
    build_python_admission: BuildPythonAdmission | None = None,
) -> tuple[RuntimeBuildIdentity, ...]:
    root = project_root.resolve(strict=False)
    plan = _plan_for_capture(
        root,
        env=env,
        cargo_command=cargo_command,
        target_triple=target_triple,
        cargo_plan=cargo_plan,
    )
    env = plan.environment
    cargo_command = plan.command
    source_roots = runtime_source_roots(root, runtime_features)
    sources, publication_authority = _capture_runtime_build_trees(root, source_roots)
    toolchain_manifest = provision_wasm_runtime_toolchain_content_manifest(
        build_python_admission=build_python_admission,
        project_root=root,
        env=env,
        cargo_plan=plan,
        target_triple=target_triple,
        wasi_sysroot=wasi_sysroot,
        wasm_linker=wasm_linker,
        long_double_archive=long_double_archive,
        builtins_archive=builtins_archive,
        wasi_libc_archive=wasi_libc_archive,
        rust_builtins_archive=rust_builtins_archive,
    )
    _verify_plan_toolchain_manifest(plan, toolchain_manifest)
    build_python_identity = _runtime_toolchain_build_python(toolchain_manifest)
    flag_projection = _RuntimeFlagProjection.capture(
        (
            *plan.logical_paths,
            ("wasi-sysroot", wasi_sysroot),
            ("tool/wasm-ld", wasm_linker),
            ("archive/wasi-long-double", long_double_archive),
            ("archive/clang-rt-builtins", builtins_archive),
            ("archive/wasi-libc", wasi_libc_archive),
            ("archive/rust-compiler-builtins", rust_builtins_archive),
        ),
        cargo_plan=plan,
    )
    compile_command, cargo_link_args = plan.partition_command()
    canonical_members = tuple(
        RuntimeBuildMemberPlan(
            kind=member.kind,
            resolved_rustflags=tuple(
                flag_projection.rustflags(member.resolved_rustflags)
            ),
            link_args=tuple(
                flag_projection.link_args(
                    (
                        *member.link_args,
                        *(cargo_link_args if member.kind == "shared" else ()),
                    )
                )
            ),
            publication_transform=member.publication_transform,
            preserve_debug=member.preserve_debug,
        )
        for member in members
    )
    return _resolve_runtime_build_family_identities(
        sources=sources,
        toolchain_manifest=toolchain_manifest,
        target_triple=target_triple,
        common_config={
            "cargo_profile": cargo_profile,
            "target_triple": target_triple,
            "runtime_features": sorted(set(runtime_features)),
            "producer_artifact_selection": (
                producer_artifact_selection.source_identity
            ),
            "cargo_command": flag_projection.prepend_root("source", root).rustflags(
                compile_command
            ),
            "base_rustflags": flag_projection.rustflags(plan.rustflags),
            "ambient_c_build_environment": _ambient_c_build_environment(plan),
            "environment": _runtime_build_environment_identity(
                plan,
                cargo_profile=cargo_profile,
            ),
            "build_script_environment": _runtime_build_script_environment_identity(
                root,
                env,
                target_triple=target_triple,
                build_python_identity=build_python_identity,
            ),
        },
        publication_authority=publication_authority,
        members=canonical_members,
    )
