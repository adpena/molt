from __future__ import annotations

import hashlib
import json
import os
import re
import shlex
import shutil
import stat as stat_module
import threading
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path
from types import MappingProxyType
from typing import Callable, Iterator, Mapping, Sequence, TypeVar, cast

from molt import process_guard
from molt.cli.atomic_io import _atomic_write_text
from molt.cli.runtime_artifact_selection import RuntimeArtifactSelection
from molt.cli.runtime_source_closure import runtime_source_paths
from molt.dx import _memory_bounded_worker_count
from molt.file_hashing import _content_change_time_ns
from molt.llvm_linker_roles import lexical_executable_path
from molt.wasi_sysroot import resolve_wasi_sysroot_layout


_SCHEMA = "molt.runtime-build-identity.v2"
_PAIR_SCHEMA = "molt.runtime-build-pair.v2"
_TOOLCHAIN_MANIFEST_SCHEMA = "molt.runtime-toolchain-content.v2"
_TREE_HASH_BUFFER_BYTES = 1024 * 1024
_TREE_HASH_BYTES_PER_WORKER = 2 * 1024 * 1024
_TREE_HASH_MEMORY_HEADROOM_BYTES = 256 * 1024 * 1024
_TREE_HASH_MAX_WORKERS = 32
_TREE_HASH_LOCAL = threading.local()


def _freeze_json(value: object) -> object:
    if isinstance(value, Mapping):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("runtime identity JSON object keys must be strings")
        typed = cast(Mapping[str, object], value)
        return MappingProxyType(
            {key: _freeze_json(item) for key, item in typed.items()}
        )
    if isinstance(value, (list, tuple)):
        return tuple(_freeze_json(item) for item in value)
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    raise TypeError(f"runtime identity contains non-JSON value: {type(value).__name__}")


def _thaw_json(value: object) -> object:
    if isinstance(value, Mapping):
        if not all(isinstance(key, str) for key in value):
            raise TypeError("runtime identity JSON object keys must be strings")
        typed = cast(Mapping[str, object], value)
        return {key: _thaw_json(item) for key, item in typed.items()}
    if isinstance(value, (list, tuple)):
        return [_thaw_json(item) for item in value]
    return value


def _canonical_json(value: object) -> str:
    return json.dumps(
        _thaw_json(value),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
    )


def _digest(value: object) -> str:
    return hashlib.sha256(_canonical_json(value).encode("utf-8")).hexdigest()


def _json_object_mapping(value: object) -> Mapping[str, object] | None:
    """Narrow one validated JSON object without coercing or aliasing keys."""

    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        return None
    return cast(Mapping[str, object], value)


def _sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(8 * 1024 * 1024):
            hasher.update(chunk)
    return hasher.hexdigest()


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
    change_time_ns = _content_change_time_ns(path, value)
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
    whole = Path(command.strip().strip('"')).expanduser()
    if whole.is_file():
        return lexical_executable_path(whole)
    try:
        argv = shlex.split(command, posix=os.name != "nt")
    except ValueError:
        argv = [command]
    found = shutil.which(argv[0], path=env.get("PATH")) if argv else None
    return lexical_executable_path(Path(found)) if found else None


def _executable_identity(
    logical_name: str,
    command: str,
    *,
    env: Mapping[str, str],
) -> dict[str, object]:
    path = _command_path(command, env)
    if path is None:
        raise ValueError(f"runtime tool {logical_name} is unresolved")
    completed = process_guard.run_completed_command(
        [os.fspath(path), "--version"],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=dict(env),
        timeout=30,
        memory_guard_prefix=None,
    )
    version_output = (completed.stdout + "\n" + completed.stderr).strip()
    if completed.returncode != 0:
        raise ValueError(
            f"runtime tool {logical_name} identity failed: {version_output}"
        )
    version_match = re.search(
        r"\b\d+(?:\.\d+)+(?:[-+][A-Za-z0-9._-]+)?\b", version_output
    )
    if version_match is None:
        raise ValueError(f"runtime tool {logical_name} emitted no semantic version")
    return {
        "logical_name": logical_name,
        "sha256": _sha256_file(path),
        # Raw --version banners frequently contain InstalledDir/CWD paths.
        # Executable bytes carry exact identity; this semantic field is stable
        # evidence only and therefore never serializes the host banner.
        "version": version_match.group(0),
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
    script = (
        "import hashlib,json,os,sys,sysconfig,unicodedata;"
        "p=getattr(unicodedata,'__file__',None);"
        "names=(sysconfig.get_config_var('INSTSONAME'),"
        "sysconfig.get_config_var('LDLIBRARY'));"
        "libdir=sysconfig.get_config_var('LIBDIR');"
        "owners=([p] if p else [])+"
        "([os.path.join(libdir,n) for n in names if libdir and n]);"
        "owner=next((v for v in owners if v and os.path.isfile(v)),sys.executable);"
        "print(json.dumps({'version':sys.version,'version_info':list(sys.version_info[:5]),"
        "'unicodedata_version':unicodedata.unidata_version,"
        "'unicodedata_storage':'module-file' if p else 'python-runtime',"
        "'unicodedata_sha256':hashlib.sha256(open(owner,'rb').read()).hexdigest()},"
        "sort_keys=True))"
    )
    completed = process_guard.run_completed_command(
        [os.fspath(path), "-c", script],
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
        runtime = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise ValueError("runtime build Python identity probe emitted invalid JSON") from exc
    return {
        "logical_name": "build-python",
        "sha256": _sha256_file(path),
        "runtime": runtime,
    }


def _archive_identity(logical_name: str, path: Path | None) -> dict[str, object]:
    if path is None or not path.is_file():
        raise ValueError(f"required runtime archive {logical_name} is unresolved")
    return {
        "logical_name": logical_name,
        "sha256": _sha256_file(path),
        "size": path.stat().st_size,
    }


def _is_host_absolute(value: str) -> bool:
    return bool(re.match(r"(?i)^[a-z]:[\\/]", value)) or value.startswith("/")


def _canonical_path_operand(
    raw: str,
    *,
    logical_paths: Sequence[tuple[str, Path]],
) -> str:
    value = raw.strip('"')
    if not _is_host_absolute(value):
        return value
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
) -> str:
    # Response files are generated configuration inputs, not stable locations.
    # Commit their bytes at the point where the path appears in the flag plan.
    if "@" in token:
        prefix, raw_response = token.rsplit("@", 1)
        response = Path(raw_response.strip('"'))
        if response.is_file():
            token = (
                f"{prefix}@response:sha256={_sha256_file(response)}:"
                f"size={response.stat().st_size}"
            )
            return token
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
                    operand, logical_paths=logical_paths
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
    if re.search(r"(?i)[a-z]:[\\/]", token) or re.search(r"(?:^|[=@])/[A-Za-z]", token):
        raise ValueError(
            f"embedded absolute host path in canonical runtime flags: {token!r}"
        )
    return token


def _canonical_flag_text(
    value: str,
    *,
    logical_paths: Sequence[tuple[str, Path]],
) -> list[str]:
    if os.name != "nt" and re.search(r"(?i)[a-z]:[\\/]", value):
        raise ValueError(f"unknown absolute host path in canonical runtime flags: {value!r}")
    try:
        tokens = shlex.split(value, posix=os.name != "nt")
    except ValueError as exc:
        raise ValueError(f"invalid runtime flag plan: {value!r}") from exc
    return [
        _canonical_flag_token(token.strip('"'), logical_paths=logical_paths)
        for token in tokens
    ]


def _ambient_c_build_environment(
    env: Mapping[str, str],
    *,
    target_triple: str,
    logical_paths: Sequence[tuple[str, Path]],
) -> dict[str, list[str]]:
    """Attest ambient C/C++ flags inherited by Cargo build scripts."""

    target_forms = (
        target_triple,
        target_triple.replace("-", "_"),
        target_triple.upper().replace("-", "_"),
    )
    names = {
        "CFLAGS",
        "CXXFLAGS",
        "CPPFLAGS",
        "LDFLAGS",
        "ARFLAGS",
        "HOST_CFLAGS",
        "HOST_CXXFLAGS",
        "TARGET_CFLAGS",
        "TARGET_CXXFLAGS",
        "CRATE_CC_NO_DEFAULTS",
        "CC_ENABLE_DEBUG_OUTPUT",
        "SOURCE_DATE_EPOCH",
    }
    for prefix in ("CFLAGS", "CXXFLAGS", "CPPFLAGS", "LDFLAGS", "ARFLAGS"):
        names.update(f"{prefix}_{target}" for target in target_forms)
    return {
        name: _canonical_flag_text(env[name], logical_paths=logical_paths)
        for name in sorted(names)
        if env.get(name, "").strip()
    }


def _normalized_link_args(
    args: Sequence[str],
    *,
    logical_paths: Sequence[tuple[str, Path]],
) -> list[str]:
    return [_canonical_flag_token(arg, logical_paths=logical_paths) for arg in args]


@dataclass(frozen=True)
class RuntimeToolchainContentManifest(Mapping[str, object]):
    digest: str
    payload: Mapping[str, object]

    def __post_init__(self) -> None:
        frozen = _freeze_json(self.payload)
        if not isinstance(frozen, Mapping) or self.digest != _digest(frozen):
            raise ValueError("runtime toolchain manifest digest is invalid")
        object.__setattr__(self, "payload", frozen)

    def __iter__(self) -> Iterator[str]:
        return iter(("schema", "digest", "payload"))

    def __len__(self) -> int:
        return 3

    def __getitem__(self, key: str) -> object:
        return self.to_dict()[key]

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _TOOLCHAIN_MANIFEST_SCHEMA,
            "digest": self.digest,
            "payload": _thaw_json(self.payload),
        }

    @classmethod
    def from_dict(cls, value: object) -> RuntimeToolchainContentManifest:
        outer = _json_object_mapping(value)
        if outer is None or outer.get("schema") != _TOOLCHAIN_MANIFEST_SCHEMA:
            raise ValueError("runtime toolchain manifest schema is invalid")
        digest = outer.get("digest")
        payload = _json_object_mapping(outer.get("payload"))
        if not isinstance(digest, str) or payload is None or digest != _digest(payload):
            raise ValueError("runtime toolchain manifest digest is invalid")
        return cls(digest=digest, payload=payload)

    @classmethod
    def read(cls, path: Path) -> RuntimeToolchainContentManifest:
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise ValueError(
                f"runtime toolchain manifest is unreadable: {path.name}"
            ) from exc
        return cls.from_dict(value)

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        _atomic_write_text(path, _canonical_json(self.to_dict()) + "\n")


@dataclass(frozen=True)
class RuntimeBuildIdentity(Mapping[str, object]):
    digest: str
    pair_digest: str
    payload: Mapping[str, object]

    def __post_init__(self) -> None:
        frozen = _freeze_json(self.payload)
        pair = frozen.get("pair") if isinstance(frozen, Mapping) else None
        if (
            not isinstance(frozen, Mapping)
            or self.digest != _digest(frozen)
            or not isinstance(pair, Mapping)
            or pair.get("schema") != _PAIR_SCHEMA
            or self.pair_digest != _digest(pair)
        ):
            raise ValueError("runtime build identity digest is invalid")
        object.__setattr__(self, "payload", frozen)

    def __iter__(self) -> Iterator[str]:
        return iter(("schema", "digest", "pair_digest", "payload"))

    def __len__(self) -> int:
        return 4

    def __getitem__(self, key: str) -> object:
        return self.to_dict()[key]

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": _SCHEMA,
            "digest": self.digest,
            "pair_digest": self.pair_digest,
            "payload": _thaw_json(self.payload),
        }

    @classmethod
    def from_dict(cls, value: object) -> RuntimeBuildIdentity:
        outer = _json_object_mapping(value)
        if outer is None or outer.get("schema") != _SCHEMA:
            raise ValueError("runtime build identity schema is invalid")
        payload = _json_object_mapping(outer.get("payload"))
        digest = outer.get("digest")
        pair_digest = outer.get("pair_digest")
        if (
            payload is None
            or not isinstance(digest, str)
            or not isinstance(pair_digest, str)
        ):
            raise ValueError("runtime build identity is incomplete")
        pair = _json_object_mapping(payload.get("pair"))
        if (
            digest != _digest(payload)
            or pair is None
            or pair.get("schema") != _PAIR_SCHEMA
            or pair_digest != _digest(pair)
        ):
            raise ValueError("runtime build identity digest is invalid")
        return cls(digest=digest, pair_digest=pair_digest, payload=payload)


@dataclass(frozen=True)
class RuntimePairMemberPlan:
    kind: str
    resolved_rustflags: str
    link_args: tuple[str, ...]
    publication_transform: str
    preserve_debug: bool


def _runtime_toolchain_content(
    *,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
) -> dict[str, object]:
    layout = resolve_wasi_sysroot_layout(wasi_sysroot)
    if layout is None:
        raise ValueError(f"runtime WASI sysroot layout is unresolved: {wasi_sysroot}")
    sysroot = _tree_identity(
        tuple((f"wasi/{label}", path) for label, path in layout.content_roots()),
        require_all=False,
    )
    cc = (
        env.get(f"CC_{target_triple}")
        or env.get(f"CC_{target_triple.replace('-', '_')}")
        or env.get("CC")
        or "clang"
    )
    ar = (
        env.get(f"AR_{target_triple}")
        or env.get(f"AR_{target_triple.replace('-', '_')}")
        or env.get("AR")
        or "llvm-ar"
    )
    cxx = (
        env.get(f"CXX_{target_triple}")
        or env.get(f"CXX_{target_triple.replace('-', '_')}")
        or env.get("CXX")
        or "clang++"
    )
    return {
        "rustc": _executable_identity("rustc", env.get("RUSTC", "rustc"), env=env),
        "cargo": _executable_identity("cargo", env.get("CARGO", "cargo"), env=env),
        "build_python": _python_identity(env),
        "cc": _executable_identity("cc", cc, env=env),
        "cxx": _executable_identity("cxx", cxx, env=env),
        "ar": _executable_identity("ar", ar, env=env),
        "wasm_linker": _executable_identity("wasm-ld", os.fspath(wasm_linker), env=env),
        "wasi_sysroot": sysroot,
        "archives": [
            _archive_identity("wasi-libc", wasi_libc_archive),
            _archive_identity("rust-compiler-builtins", rust_builtins_archive),
            _archive_identity("wasi-long-double", long_double_archive),
            _archive_identity("clang-rt-builtins", builtins_archive),
        ],
    }


def provision_runtime_toolchain_content_manifest(
    *,
    env: Mapping[str, str],
    target_triple: str,
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
) -> RuntimeToolchainContentManifest:
    """Produce the immutable content manifest consumed by normal identity reads."""

    payload = {
        "target_triple": target_triple,
        "toolchain": _runtime_toolchain_content(
            env=env,
            target_triple=target_triple,
            wasi_sysroot=wasi_sysroot,
            wasm_linker=wasm_linker,
            long_double_archive=long_double_archive,
            builtins_archive=builtins_archive,
            wasi_libc_archive=wasi_libc_archive,
            rust_builtins_archive=rust_builtins_archive,
        ),
    }
    return RuntimeToolchainContentManifest(digest=_digest(payload), payload=payload)


def resolve_runtime_build_pair_identities(
    project_root: Path,
    *,
    env: Mapping[str, str],
    cargo_profile: str,
    target_triple: str,
    runtime_features: tuple[str, ...],
    base_rustflags: str,
    producer_artifact_selection: RuntimeArtifactSelection,
    shared: RuntimePairMemberPlan,
    reloc: RuntimePairMemberPlan,
    wasi_sysroot: Path,
    wasm_linker: Path,
    long_double_archive: Path,
    builtins_archive: Path,
    wasi_libc_archive: Path,
    rust_builtins_archive: Path,
    toolchain_manifest: RuntimeToolchainContentManifest | None = None,
) -> tuple[RuntimeBuildIdentity, RuntimeBuildIdentity]:
    root = project_root.resolve(strict=False)
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
    if toolchain_manifest is None:
        toolchain_manifest = provision_runtime_toolchain_content_manifest(
            env=env,
            target_triple=target_triple,
            wasi_sysroot=wasi_sysroot,
            wasm_linker=wasm_linker,
            long_double_archive=long_double_archive,
            builtins_archive=builtins_archive,
            wasi_libc_archive=wasi_libc_archive,
            rust_builtins_archive=rust_builtins_archive,
        )
    else:
        # Revalidate and detach the caller-provided object before consuming its
        # nested payload. Frozen dataclasses do not freeze nested mappings.
        toolchain_manifest = RuntimeToolchainContentManifest.from_dict(
            toolchain_manifest.to_dict()
        )
    if toolchain_manifest.payload.get("target_triple") != target_triple:
        raise ValueError("runtime toolchain manifest target is invalid")
    toolchain = toolchain_manifest.payload.get("toolchain")
    if not isinstance(toolchain, Mapping):
        raise ValueError("runtime toolchain manifest content is invalid")
    logical_paths = (
        ("wasi-sysroot", wasi_sysroot),
        ("tool/wasm-ld", wasm_linker),
        ("archive/wasi-long-double", long_double_archive),
        ("archive/clang-rt-builtins", builtins_archive),
        ("archive/wasi-libc", wasi_libc_archive),
        ("archive/rust-compiler-builtins", rust_builtins_archive),
    )

    def member(plan: RuntimePairMemberPlan) -> dict[str, object]:
        return {
            "kind": plan.kind,
            "resolved_rustflags": _canonical_flag_text(
                plan.resolved_rustflags, logical_paths=logical_paths
            ),
            "link_args": _normalized_link_args(
                plan.link_args, logical_paths=logical_paths
            ),
            "publication_transform": plan.publication_transform,
            "preserve_debug": plan.preserve_debug,
        }

    members = {"shared": member(shared), "reloc": member(reloc)}
    if shared.kind != "shared" or reloc.kind != "reloc":
        raise ValueError("runtime pair member kinds are invalid")
    pair = {
        "schema": _PAIR_SCHEMA,
        "sources": sources,
        "toolchain": toolchain,
        "common_config": {
            "cargo_profile": cargo_profile,
            "target_triple": target_triple,
            "runtime_features": sorted(set(runtime_features)),
            "producer_artifact_selection": (
                producer_artifact_selection.source_identity
            ),
            "base_rustflags": _canonical_flag_text(
                base_rustflags, logical_paths=logical_paths
            ),
            "ambient_c_build_environment": _ambient_c_build_environment(
                env,
                target_triple=target_triple,
                logical_paths=logical_paths,
            ),
        },
        "members": members,
    }
    pair_digest = _digest(pair)

    def identity(kind: str) -> RuntimeBuildIdentity:
        payload = {"pair": pair, "member": members[kind]}
        return RuntimeBuildIdentity(_digest(payload), pair_digest, payload)

    return identity("shared"), identity("reloc")
