"""Runtime input capture and family resolution; wire validation is pure."""

from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import stat as stat_module
import threading
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Callable, Iterator, Mapping, Sequence, TypeVar, cast

from molt import process_guard
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
    _BUILD_PYTHON_SCHEMA,
    _FAMILY_SCHEMA,
    _digest,
    _freeze_json,
    _thaw_json,
    _json_object_mapping as _json_object_mapping,
    _runtime_toolchain_build_python,
)
from molt.dx import _memory_bounded_worker_count
from molt.file_hashing import content_change_time_ns
from molt.exact_json import ExactJsonError, loads_exact
from molt.python_environment_identity import (
    python_capture_authority_paths,
    python_identity_probe_arguments,
)
from molt.toolchain_identity import (
    probe_executable,
    resolve_executable,
    stable_executable_probe,
    stable_regular_file_identity,
)
from molt.wasi_sysroot import resolve_wasi_sysroot_layout

_TREE_HASH_BUFFER_BYTES = 1024 * 1024
_TREE_HASH_BYTES_PER_WORKER = 2 * 1024 * 1024
_TREE_HASH_MEMORY_HEADROOM_BYTES = 256 * 1024 * 1024
_TREE_HASH_MAX_WORKERS = 32
_TREE_HASH_LOCAL = threading.local()
_RUNTIME_BUILD_PYTHON_HASH_WORKERS = 4


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
    "src/molt/cli/json_cache.py",
    "src/molt/cli/llvm_wasi_tools.py",
    "src/molt/cli/models.py",
    "src/molt/cli/native_link_custody.py",
    "src/molt/cli/native_link_manifest.py",
    "src/molt/cli/native_link_plan.py",
    "src/molt/cli/project_roots.py",
    "src/molt/cli/runtime_artifact_selection.py",
    "src/molt/cli/runtime_build_identity.py",
    "src/molt/cli/runtime_identity_schema.py",
    "src/molt/cli/runtime_cargo_plan.py",
    "src/molt/cli/runtime_features.py",
    "src/molt/cli/runtime_fingerprints.py",
    "src/molt/cli/runtime_native_build.py",
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


def _is_path_alias(path: Path) -> bool:
    if path.is_symlink():
        return True
    is_junction = getattr(path, "is_junction", None)
    return bool(is_junction is not None and is_junction())


def _stat_is_path_alias(value: os.stat_result) -> bool:
    reparse_point = getattr(stat_module, "FILE_ATTRIBUTE_REPARSE_POINT", 0)
    return stat_module.S_ISLNK(value.st_mode) or bool(
        reparse_point and getattr(value, "st_file_attributes", 0) & reparse_point
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
            if _stat_is_path_alias(candidate_stat):
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
        if _is_path_alias(candidate.path):
            raise ValueError(
                f"runtime input path alias is forbidden for "
                f"{candidate.label!r}: {candidate.path}"
            )
        current = candidate.path.stat()
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
        if _is_path_alias(file.path):
            raise ValueError(
                f"runtime input path alias is forbidden for {file.label!r}: {file.path}"
            )
        expected_handle_signature = (
            file.stat_signature[0],
            file.stat_signature[1],
            file.stat_signature[2],
            file.stat_signature[4],
            file.stat_signature[5],
        )
        with file.path.open("rb", buffering=0) as handle:
            if (
                _tree_input_handle_signature(os.fstat(handle.fileno()))
                != expected_handle_signature
            ):
                raise _runtime_input_changed(file)
            digest = _sha256_open_file(handle)
            if (
                _tree_input_handle_signature(os.fstat(handle.fileno()))
                != expected_handle_signature
            ):
                raise _runtime_input_changed(file)
        if _is_path_alias(file.path):
            raise ValueError(
                f"runtime input path alias is forbidden for {file.label!r}: {file.path}"
            )
        if (
            _tree_input_stat_signature(file.path, file.path.stat())
            != file.stat_signature
        ):
            raise _runtime_input_changed(file)
        return digest
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
    if not inputs:
        return ()
    if workers == 1:
        return tuple(operation(item) for item in inputs)

    results: list[_ParallelOutput | None] = [None] * len(inputs)
    iterator = iter(enumerate(inputs))
    max_pending = workers * 2
    with ThreadPoolExecutor(
        max_workers=workers,
        thread_name_prefix="molt-runtime-identity",
    ) as executor:
        pending: dict[Future[_ParallelOutput], int] = {}

        def submit_one() -> bool:
            try:
                index, item = next(iterator)
            except StopIteration:
                return False
            pending[executor.submit(operation, item)] = index
            return True

        for _ in range(max_pending):
            if not submit_one():
                break
        while pending:
            completed, _ = wait(pending, return_when=FIRST_COMPLETED)
            for future in completed:
                index = pending.pop(future)
                try:
                    results[index] = future.result()
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


def _tree_identity(
    roots: Sequence[tuple[str, Path]],
    *,
    require_all: bool,
) -> dict[str, object]:
    """Hash an uncached logical-label closure with exact mutation checks."""

    root_labels: dict[str, Path] = {}
    files: dict[str, _TreeInputCandidate] = {}
    missing: list[str] = []
    for logical_root, raw_path in roots:
        if _is_path_alias(raw_path):
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
        candidates: list[tuple[str, Path]] = []
        try:
            root_stat = path.lstat()
        except FileNotFoundError:
            missing.append(logical_root)
            continue
        if stat_module.S_ISREG(root_stat.st_mode):
            candidates.append((logical_root, path))
        elif stat_module.S_ISDIR(root_stat.st_mode):
            candidates.extend(_runtime_tree_candidates(path, logical_root=logical_root))
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
    if require_all and missing:
        raise ValueError("required runtime inputs are missing: " + ", ".join(missing))
    ordered_files = _snapshot_tree_input_files(
        tuple(files[label] for label in sorted(files))
    )
    digests = _hash_tree_input_files(ordered_files)
    hasher = hashlib.sha256()
    total_size = 0
    for file in ordered_files:
        total_size += file.size
        hasher.update(file.label.encode())
        hasher.update(b"\0")
        hasher.update(str(file.size).encode())
        hasher.update(b"\0")
        hasher.update(digests[file.label].encode())
        hasher.update(b"\0")
    for label in sorted(missing):
        hasher.update(b"missing\0")
        hasher.update(label.encode())
        hasher.update(b"\0")
    return {
        "digest": hasher.hexdigest(),
        "file_count": len(files),
        "total_size": total_size,
        "roots": sorted(root_labels),
        "missing": sorted(missing),
    }


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


def _python_identity(env: Mapping[str, str]) -> dict[str, object]:
    command = (
        env.get("MOLT_BUILD_PYTHON", "").strip()
        or env.get("PYTHON", "").strip()
        or ("python" if os.name == "nt" else "python3")
    )
    path = _command_path(command, env)
    if path is None:
        raise ValueError("runtime build Python is unresolved")
    # Build and environment provisioning consume the same isolated capture.
    with stable_executable_probe(path, label="runtime build Python") as (
        entrypoint,
        executable,
    ):
        completed = process_guard.run_completed_command(
            [
                os.fspath(entrypoint),
                *python_identity_probe_arguments(
                    (
                        "--capture-runtime",
                        "--hash-workers",
                        str(_RUNTIME_BUILD_PYTHON_HASH_WORKERS),
                    ),
                    no_site=True,
                ),
            ],
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=dict(env),
            timeout=30,
            memory_guard_prefix=None,
        )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout or "").strip()
        raise ValueError(
            "runtime build Python identity probe failed"
            + (f": {detail}" if detail else "")
        )
    try:
        from molt.python_runtime_identity import validate_python_runtime_identity

        runtime = validate_python_runtime_identity(loads_exact(completed.stdout))
    except (json.JSONDecodeError, ExactJsonError) as exc:
        raise ValueError(
            "runtime build Python identity probe emitted invalid JSON"
        ) from exc
    material = {
        "schema": _BUILD_PYTHON_SCHEMA,
        "logical_name": "build_python",
        "selected_executable": {
            "entrypoint": entrypoint.name.casefold()
            if os.name == "nt"
            else entrypoint.name,
            "content_filename": executable.path.name.casefold()
            if os.name == "nt"
            else executable.path.name,
            "size": executable.size,
            "sha256": executable.sha256,
        },
        "runtime": runtime,
    }
    return {**material, "identity_sha256": _digest(material)}


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


def _pythonpath_content_identity(
    project_root: Path,
    env: Mapping[str, str],
) -> dict[str, object]:
    """Capture PYTHONPATH by ordered content, never by host pathname."""

    raw = env.get("PYTHONPATH")
    if raw is None:
        return {"state": "unset"}
    if not raw:
        return {"state": "empty"}
    build_script_root = project_root / "runtime" / "molt-runtime"
    roots: list[tuple[str, Path]] = []
    for index, entry in enumerate(raw.split(os.pathsep)):
        # Python interprets an empty PYTHONPATH component as the child process
        # working directory. Cargo runs this build script from the package root.
        path = (
            build_script_root
            if not entry
            else _build_script_path(entry, build_script_root=build_script_root)
        )
        roots.append((f"pythonpath/{index}", path))
    return {
        "state": "set",
        "entry_count": len(roots),
        "content": _tree_identity(roots, require_all=False),
    }


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
    project_root: Path,
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
        "PYTHONPATH": _pythonpath_content_identity(project_root, env),
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
        "schema": "molt.runtime-build-script-environment.v1",
        **_build_python_script_environment_identity(
            project_root,
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


def _canonical_path_operand(
    raw: str,
    *,
    logical_paths: Sequence[tuple[str, Path]],
) -> str:
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
    for label, root in logical_paths:
        resolved_root = root.resolve(strict=False)
        try:
            relative = candidate.relative_to(resolved_root)
        except ValueError:
            continue
        suffix = "" if not relative.parts else "/" + relative.as_posix()
        return f"${{{label}}}{suffix}"
    raise ValueError(f"unknown absolute host path in canonical runtime flags: {raw!r}")


def _canonical_flag_token(
    token: str,
    *,
    logical_paths: Sequence[tuple[str, Path]],
    cargo_plan: RuntimeCargoPlan | None = None,
) -> str:
    # Response files are generated configuration inputs, not stable locations.
    # Commit their bytes at the point where the path appears in the flag plan.
    if "@" in token:
        if cargo_plan is None:
            raise ValueError(
                "runtime response argument requires captured Cargo plan custody"
            )
        return cargo_plan.project_link_arguments((token,))[0]
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
                return prefix + _canonical_flag_token(
                    operand,
                    logical_paths=logical_paths,
                    cargo_plan=cargo_plan,
                )
            return prefix + _canonical_path_operand(
                operand, logical_paths=logical_paths
            )
    if _is_host_absolute(token):
        return _canonical_path_operand(token, logical_paths=logical_paths)
    if "=" in token:
        prefix, operand = token.split("=", 1)
        if _is_host_absolute(operand):
            return (
                prefix
                + "="
                + _canonical_path_operand(operand, logical_paths=logical_paths)
            )
    for _label, root in logical_paths:
        root_text = os.fspath(root.resolve(strict=False))
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


def _canonical_flag_text(
    value: str,
    *,
    logical_paths: Sequence[tuple[str, Path]],
    cargo_plan: RuntimeCargoPlan | None = None,
) -> list[str]:
    if os.name != "nt" and re.search(r"(?i)[a-z]:[\\/]", value):
        raise ValueError(
            f"unknown absolute host path in canonical runtime flags: {value!r}"
        )
    try:
        tokens = shlex.split(value, posix=os.name != "nt")
    except ValueError as exc:
        raise ValueError(f"invalid runtime flag plan: {value!r}") from exc
    return [
        _canonical_flag_token(
            token.strip('"'), logical_paths=logical_paths, cargo_plan=cargo_plan
        )
        for token in tokens
    ]


def _ambient_c_build_environment(cargo_plan: RuntimeCargoPlan) -> dict[str, list[str]]:
    """Project the exact cc-rs token/search authority captured for execution."""
    return {name: list(tokens) for name, tokens in cargo_plan.c_environment.items()}


def _normalized_link_args(
    args: Sequence[str],
    *,
    logical_paths: Sequence[tuple[str, Path]],
    cargo_plan: RuntimeCargoPlan,
) -> list[str]:
    return [
        _canonical_flag_token(arg, logical_paths=logical_paths, cargo_plan=cargo_plan)
        for arg in args
    ]


def _canonical_rustflags(
    value: str | Sequence[str],
    *,
    logical_paths: Sequence[tuple[str, Path]],
    cargo_plan: RuntimeCargoPlan | None = None,
) -> list[str]:
    if isinstance(value, str):
        return _canonical_flag_text(
            value, logical_paths=logical_paths, cargo_plan=cargo_plan
        )
    return [
        _canonical_flag_token(
            token.strip('"'), logical_paths=logical_paths, cargo_plan=cargo_plan
        )
        for token in value
    ]


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
    implementation_root = Path(__file__).resolve().parents[3]
    relative_paths = set(_RUNTIME_BUILD_TOOLING_RELPATHS)
    for path in python_capture_authority_paths():
        try:
            relative_paths.add(
                path.resolve().relative_to(implementation_root).as_posix()
            )
        except ValueError as exc:
            raise ValueError(
                f"Python capture authority escaped implementation root: {path}"
            ) from exc
    root = project_root.resolve(strict=False)
    return tuple(root / relative for relative in sorted(relative_paths))


def runtime_build_tooling_authority(project_root: Path) -> dict[str, object]:
    """Hash only the runtime planner, receipt, and publication authority."""

    root = project_root.resolve(strict=False)
    tree = _tree_identity(
        tuple(
            ("runtime-tooling/" + path.relative_to(root).as_posix(), path)
            for path in runtime_build_tooling_paths(root)
        ),
        require_all=True,
    )
    return {
        "schema": "molt.runtime-build-tooling-authority.v2",
        **tree,
    }


def _rust_toolchain_resources(*, cargo_plan: RuntimeCargoPlan) -> dict[str, object]:
    return {
        "host_triple": cargo_plan.host_target,
        "selected_target": cargo_plan.target,
        "content": cargo_plan.rust_resources.content_identity(),
    }


def _capture_plan_toolchain(cargo_plan: RuntimeCargoPlan) -> dict[str, object]:
    cargo_plan.verify()
    tools: dict[str, object] = {}
    wrappers: dict[str, object] = {}
    for item in cargo_plan.executable_custody:
        group, role = item.label.split("/", 1)
        destination = tools if group == "tool" else wrappers
        destination[role] = {
            "logical_name": role if group == "tool" else role.casefold(),
            **item.content_record(),
        }
    tools["build_python"] = _python_identity(cargo_plan.environment)
    result = {
        "tools": tools,
        "wrappers": wrappers,
        "cargo_configuration": cargo_plan.configuration_identity(),
        "effective_target": cargo_plan.target,
        "rust_resources": _rust_toolchain_resources(cargo_plan=cargo_plan),
        "sysroots": {},
        "archives": [],
    }
    _verify_plan_toolchain_content(cargo_plan, result)
    return result


def _verify_plan_toolchain_content(
    plan: RuntimeCargoPlan, content: Mapping[str, object]
) -> None:
    """Reconcile newly captured tools/resources with their live execution plan."""
    plan.verify()
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
        != _freeze_json(_rust_toolchain_resources(cargo_plan=plan))
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
    cargo_plan.verify()
    return cargo_plan


def _wasm_compile_toolchain_content(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    include_cxx: bool = True,
    cargo_plan: RuntimeCargoPlan | None = None,
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
    content = _capture_plan_toolchain(plan)
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
) -> dict[str, object]:
    content = _wasm_compile_toolchain_content(
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
) -> RuntimeToolchainContentManifest:
    """Produce the immutable content manifest consumed by normal identity reads."""

    payload = {
        "target_triple": target_triple,
        "toolchain": _wasm_runtime_toolchain_content(
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
    return RuntimeToolchainContentManifest(digest=_digest(payload), payload=payload)


def provision_wasm_cpython_abi_toolchain_content_manifest(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
) -> RuntimeToolchainContentManifest:
    """Capture the complete compiler inputs for the standalone ABI staticlib."""

    payload = {
        "target_triple": target_triple,
        "toolchain": _wasm_compile_toolchain_content(
            project_root=project_root,
            env=env,
            target_triple=target_triple,
            wasi_sysroot=wasi_sysroot,
            cargo_plan=cargo_plan,
            include_cxx=False,
        ),
    }
    return RuntimeToolchainContentManifest(digest=_digest(payload), payload=payload)


def provision_native_runtime_toolchain_content_manifest(
    *,
    project_root: Path,
    env: Mapping[str, str],
    target_triple: str | None,
    cargo_command: Sequence[str],
    cargo_plan: RuntimeCargoPlan | None = None,
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
        "toolchain": _capture_plan_toolchain(plan),
    }
    return RuntimeToolchainContentManifest(digest=_digest(payload), payload=payload)


def resolve_native_runtime_build_identity(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str | None,
    runtime_features: tuple[str, ...],
    cargo_command: Sequence[str],
    artifact_selection: RuntimeArtifactSelection,
    publication_authority: Mapping[str, object],
    cargo_plan: RuntimeCargoPlan | None = None,
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
    logical_paths = (*plan.logical_paths, ("source", root))
    source_roots: list[tuple[str, Path]] = []
    for path in runtime_source_paths(root, runtime_features):
        resolved = path.resolve(strict=False)
        try:
            label = "source/" + resolved.relative_to(root).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"runtime source escaped project root: {resolved}"
            ) from exc
        source_roots.append((label, resolved))
    sources = _tree_identity(source_roots, require_all=False)
    toolchain_manifest = provision_native_runtime_toolchain_content_manifest(
        project_root=root,
        env=env,
        cargo_plan=plan,
        target_triple=target_triple,
        cargo_command=cargo_command,
    )
    if not cargo_command:
        raise ValueError("native runtime Cargo command is empty")
    _verify_plan_toolchain_manifest(plan, toolchain_manifest)
    build_python_identity = _runtime_toolchain_build_python(toolchain_manifest)
    compile_command, final_link_arguments = plan.partition_command()
    command = _canonical_rustflags(
        compile_command, logical_paths=logical_paths, cargo_plan=plan
    )
    rustflags = plan.rustflags
    member = RuntimeBuildMemberPlan(
        kind="staticlib",
        resolved_rustflags=tuple(
            _canonical_rustflags(
                rustflags,
                logical_paths=logical_paths,
                cargo_plan=plan,
            )
        ),
        link_args=(
            "--print",
            "native-static-libs",
            *_normalized_link_args(
                final_link_arguments, logical_paths=logical_paths, cargo_plan=plan
            ),
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
            "base_rustflags": _canonical_rustflags(
                plan.rustflags, logical_paths=logical_paths, cargo_plan=plan
            ),
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
    manifest = RuntimeToolchainContentManifest.from_dict(toolchain_manifest.to_dict())
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
    publication_payload = _thaw_json(_freeze_json(publication_authority))
    if not isinstance(publication_payload, dict):
        raise ValueError("runtime publication authority is invalid")
    compile_payload = {
        "sources": _thaw_json(sources),
        "toolchain": _thaw_json(toolchain),
        "common_config": _thaw_json(common_config),
    }
    compile_digest = _digest(compile_payload)
    family = {
        "schema": _FAMILY_SCHEMA,
        "compile_digest": compile_digest,
        "compile": compile_payload,
        "publication_authority": publication_payload,
        "members": member_payloads,
    }
    family_digest = _digest(family)

    def identity(kind: str) -> RuntimeBuildIdentity:
        payload = {"family": family, "member_kind": kind}
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
    publication_authority: Mapping[str, object],
    wasi_sysroot: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
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
    sources = _tree_identity(source_roots, require_all=False)
    toolchain_manifest = provision_wasm_cpython_abi_toolchain_content_manifest(
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
    logical_paths = (
        *plan.logical_paths,
        ("source", root),
        ("wasi-sysroot", wasi_sysroot),
    )
    member = RuntimeBuildMemberPlan(
        kind="staticlib",
        resolved_rustflags=tuple(
            _canonical_flag_text(
                rustflags, logical_paths=logical_paths, cargo_plan=plan
            )
        ),
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
            "cargo_command": _canonical_rustflags(
                plan.partition_command()[0],
                logical_paths=logical_paths,
                cargo_plan=plan,
            ),
            "ambient_c_build_environment": _ambient_c_build_environment(plan),
            "environment": _runtime_build_environment_identity(
                plan,
                cargo_profile=cargo_profile,
            ),
            "build_script_environment": {
                "schema": "molt.cpython-abi-build-script-environment.v1",
                **_build_python_script_environment_identity(
                    root,
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
    publication_authority: Mapping[str, object],
    members: Sequence[RuntimeBuildMemberPlan],
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
    cargo_plan: RuntimeCargoPlan | None = None,
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
    source_roots: list[tuple[str, Path]] = []
    for path in runtime_source_paths(root, runtime_features):
        resolved = path.resolve(strict=False)
        try:
            label = "source/" + resolved.relative_to(root).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"runtime source escaped project root: {resolved}"
            ) from exc
        source_roots.append((label, resolved))
    sources = _tree_identity(source_roots, require_all=False)
    toolchain_manifest = provision_wasm_runtime_toolchain_content_manifest(
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
    logical_paths = (
        *plan.logical_paths,
        ("wasi-sysroot", wasi_sysroot),
        ("tool/wasm-ld", wasm_linker),
        ("archive/wasi-long-double", long_double_archive),
        ("archive/clang-rt-builtins", builtins_archive),
        ("archive/wasi-libc", wasi_libc_archive),
        ("archive/rust-compiler-builtins", rust_builtins_archive),
    )
    compile_command, cargo_link_args = plan.partition_command()
    canonical_members = tuple(
        RuntimeBuildMemberPlan(
            kind=member.kind,
            resolved_rustflags=tuple(
                _canonical_rustflags(
                    member.resolved_rustflags,
                    logical_paths=logical_paths,
                    cargo_plan=plan,
                )
            ),
            link_args=tuple(
                _normalized_link_args(
                    (
                        *member.link_args,
                        *(cargo_link_args if member.kind == "shared" else ()),
                    ),
                    logical_paths=logical_paths,
                    cargo_plan=plan,
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
            "cargo_command": _canonical_rustflags(
                compile_command,
                logical_paths=(("source", root), *logical_paths),
                cargo_plan=plan,
            ),
            "base_rustflags": _canonical_rustflags(
                plan.rustflags,
                logical_paths=logical_paths,
                cargo_plan=plan,
            ),
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
