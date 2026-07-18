#!/usr/bin/env python3
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import AbstractContextManager
from dataclasses import asdict, dataclass, replace
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import tarfile
import threading
import urllib.request
from pathlib import Path
import sys
from typing import Callable, Mapping
import uuid


ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.llvm_toolchain import (  # noqa: E402
    _llvm_attestation_custody,
    _windows_change_time_ns,
    LlvmManagedPaths,
    LlvmToolchainConfigError,
    LlvmPrefixVerification,
    LLVM_ATTESTATION_SCHEMA,
    canonical_llvm_build_type,
    default_llvm_release,
    llvm_host_architecture,
    llvm_release,
    llvm_sys_prefix_env_var_for_version,
    load_llvm_architecture_contract,
    load_llvm_releases,
    managed_llvm_paths,
    project_llvm_toolchain_environment,
    required_llvm_targets_for_host,
    required_llvm_backend_pin,
    reject_poison_toolchain_path,
    verify_llvm_toolchain_prefix,
    write_llvm_toolchain_attestation,
)
from tools.resource_pressure import plan_resource_pressure  # noqa: E402


LLVM_HOST_TARGET_FAMILIES = tuple(
    (row.aliases, row.llvm_target)
    for row in load_llvm_architecture_contract(ROOT).architectures
)
LLVM_SOURCE_MARKER = ".molt-llvm-source.json"
LLVM_BUILD_MARKER = ".molt-llvm-build.json"
LLVM_SOURCE_SCHEMA = "molt.llvm-source.v3"
LLVM_BUILD_SCHEMA = "molt.llvm-build-cache.v2"
LLVM_PUBLICATION_SCHEMA = "molt.llvm-publication.v1"

_PROCESS_LOCKS: dict[str, threading.RLock] = {}
_PROCESS_LOCKS_GUARD = threading.Lock()


@dataclass(frozen=True)
class _BuildTool:
    path: str
    version: str


class _ExclusiveFileLock(AbstractContextManager["_ExclusiveFileLock"]):
    """Cross-process advisory lock plus an in-process thread lock."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._handle = None
        with _PROCESS_LOCKS_GUARD:
            self._thread_lock = _PROCESS_LOCKS.setdefault(
                str(path.resolve()), threading.RLock()
            )

    def __enter__(self) -> "_ExclusiveFileLock":
        self._thread_lock.acquire()
        try:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self._handle = self.path.open("a+b")
            self._handle.seek(0, os.SEEK_END)
            if self._handle.tell() == 0:
                self._handle.write(b"\0")
                self._handle.flush()
                os.fsync(self._handle.fileno())
            self._handle.seek(0)
            if os.name == "nt":
                import msvcrt

                msvcrt.locking(self._handle.fileno(), msvcrt.LK_LOCK, 1)
            else:
                import fcntl

                fcntl.flock(self._handle.fileno(), fcntl.LOCK_EX)
            return self
        except BaseException:
            if self._handle is not None:
                self._handle.close()
                self._handle = None
            self._thread_lock.release()
            raise

    def __exit__(self, *_exc: object) -> None:
        try:
            if self._handle is not None:
                if os.name == "nt":
                    import msvcrt

                    self._handle.seek(0)
                    msvcrt.locking(self._handle.fileno(), msvcrt.LK_UNLCK, 1)
                else:
                    import fcntl

                    fcntl.flock(self._handle.fileno(), fcntl.LOCK_UN)
                self._handle.close()
                self._handle = None
        finally:
            self._thread_lock.release()


def _fsync_directory(path: Path) -> None:
    if os.name == "nt":
        return
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _atomic_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    try:
        with temporary.open("x", encoding="utf-8") as handle:
            json.dump(payload, handle, sort_keys=True, indent=2)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        _fsync_directory(path.parent)
    finally:
        if temporary.exists():
            temporary.unlink()


def _release_source_sha256(version: str) -> str | None:
    release = llvm_release(version, ROOT)
    return None if release is None else release.source_sha256


def _source_sha256(version: str, development_sha256: str | None) -> str:
    pinned_sha256 = _release_source_sha256(version)
    if pinned_sha256 is None and development_sha256 is None:
        raise SystemExit(
            f"LLVM {version} has no canonical source checksum; add it to "
            "config/llvm_toolchain_releases.toml or pass "
            "--development-source-sha256 for a development-only build"
        )
    if pinned_sha256 is not None and development_sha256 is not None:
        raise SystemExit(
            "--development-source-sha256 cannot override a canonical release checksum"
        )
    digest = pinned_sha256 or str(development_sha256).lower()
    if len(digest) != 64 or any(ch not in "0123456789abcdef" for ch in digest):
        raise SystemExit("LLVM source SHA-256 must be exactly 64 hex digits")
    return digest


def _sha256(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def _required_llvm_major(root: Path) -> int:
    pin = required_llvm_backend_pin(root)
    if pin is None:
        raise SystemExit(f"Unable to find LLVM backend feature pin under {root}")
    return pin.major


def _default_release_for_major(major: int) -> str:
    return default_llvm_release(major)


def _default_llvm_targets(machine: str | None = None) -> str:
    """Build the host backend plus WebAssembly without unrelated target libraries."""
    try:
        return ";".join(required_llvm_targets_for_host(ROOT, machine))
    except LlvmToolchainConfigError as exc:
        raise SystemExit(str(exc)) from exc


def _default_llvm_jobs() -> int:
    plan = plan_resource_pressure(
        prefix="MOLT_LLVM",
        max_compile_slots=os.cpu_count() or 1,
        compile_gb_per_slot=2.0,
    )
    return plan.compile_max_slots


def _development_llvm_paths(prefix: Path, version: str) -> LlvmManagedPaths:
    custody = prefix.with_name(f".{prefix.name}.development-custody")
    return LlvmManagedPaths(
        root=custody,
        prefix=prefix,
        archive=custody / "downloads" / f"llvm-project-{version}.tar.xz",
        source_root=custody / "sources" / f"llvm-project-{version}",
        build_dir=custody / "build" / f"llvm-{version}",
    )


def _preflight_resources(
    path: Path,
    *,
    required_free_gb: float,
    required_memory_gb: float,
) -> None:
    existing = path
    while not existing.exists() and existing.parent != existing:
        existing = existing.parent
    free_gb = shutil.disk_usage(existing).free / (1024**3)
    if free_gb < required_free_gb:
        raise SystemExit(
            f"LLVM bootstrap requires at least {required_free_gb:.1f} GiB free under "
            f"{path}; only {free_gb:.1f} GiB is available. Free canonical custody "
            "space before downloading or configuring LLVM."
        )
    plan = plan_resource_pressure(prefix="MOLT_LLVM")
    memory_gb = plan.available_gb if plan.available_gb is not None else plan.physical_gb
    if memory_gb is not None and memory_gb < required_memory_gb:
        raise SystemExit(
            f"LLVM bootstrap requires at least {required_memory_gb:.1f} GiB of "
            f"available/physical memory; resource authority reports {memory_gb:.1f} GiB"
        )


def _llvm_sys_prefix_env_var(version: str) -> str:
    return llvm_sys_prefix_env_var_for_version(version)


def _run(cmd: list[str], *, cwd: Path | None, env: dict[str, str]) -> None:
    printable = " ".join(_quote(part) for part in cmd)
    print(f"[bootstrap-llvm] {printable}", flush=True)
    proc = subprocess.run(cmd, cwd=cwd, env=env, check=False)
    if proc.returncode != 0:
        location = f" in {cwd}" if cwd is not None else ""
        raise SystemExit(
            f"Command failed with exit code {proc.returncode}{location}: {printable}"
        )


def _quote(value: str) -> str:
    if not value or any(ch.isspace() for ch in value):
        return '"' + value.replace('"', '\\"') + '"'
    return value


def _which_required(name: str) -> str:
    resolved = shutil.which(name)
    if resolved is None:
        raise SystemExit(f"Required executable not found on PATH: {name}")
    return resolved


def _executable_candidates(name: str, *, path: str | None = None) -> tuple[str, ...]:
    """Return every PATH candidate in deterministic search order."""

    search_path = os.environ.get("PATH", "") if path is None else path
    candidates: list[str] = []
    seen: set[str] = set()
    for directory in search_path.split(os.pathsep):
        directory = directory.strip().strip('"')
        if not directory:
            continue
        candidate = shutil.which(name, path=directory)
        if candidate is None:
            continue
        resolved = str(Path(candidate).resolve())
        key = os.path.normcase(resolved)
        if key not in seen:
            seen.add(key)
            candidates.append(resolved)
    return tuple(candidates)


def _tool_version(path: str, *, role: str) -> str:
    try:
        proc = subprocess.run(
            [path, "--version"],
            capture_output=True,
            text=True,
            check=False,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise SystemExit(f"Could not query {role} version at {path}: {exc}") from exc
    output = "\n".join((proc.stdout, proc.stderr)).strip()
    match = re.search(r"(?<!\d)(\d+\.\d+(?:\.\d+)?)(?!\d)", output)
    if proc.returncode != 0 or match is None:
        raise SystemExit(
            f"Could not parse {role} version at {path} (exit {proc.returncode}): {output}"
        )
    parts = match.group(1).split(".")
    return ".".join((*parts, *("0" for _ in range(3 - len(parts)))))


def _version_key(version: str) -> tuple[int, int, int]:
    match = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)", version)
    if match is None:
        raise SystemExit(f"Invalid semantic tool version: {version!r}")
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def _compatible_cmake(minimum: str) -> _BuildTool:
    required = _version_key(minimum)
    observed: list[_BuildTool] = []
    for path in _executable_candidates("cmake"):
        try:
            version = _tool_version(path, role="CMake")
        except SystemExit:
            continue
        observed.append(_BuildTool(path=path, version=version))
    compatible = [tool for tool in observed if _version_key(tool.version) >= required]
    if not compatible:
        found = (
            ", ".join(f"{tool.version} at {tool.path}" for tool in observed) or "none"
        )
        raise SystemExit(
            f"LLVM requires CMake >= {minimum}; compatible executable not found on PATH "
            f"(observed: {found}). Install a current Kitware CMake or place it on PATH."
        )
    selected = max(compatible, key=lambda tool: (_version_key(tool.version), tool.path))
    first = observed[0] if observed else None
    if first is not None and first != selected:
        print(
            "[bootstrap-llvm] bypassing incompatible/older PATH CMake "
            f"{first.version} at {first.path}; selected {selected.version} at {selected.path}",
            flush=True,
        )
    return selected


def _required_build_tool(name: str) -> _BuildTool:
    path = _which_required(name)
    return _BuildTool(
        path=str(Path(path).resolve()), version=_tool_version(path, role=name)
    )


def _vswhere_path() -> Path | None:
    candidates = [
        Path(os.environ.get("ProgramFiles(x86)", ""))
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe",
        Path(os.environ.get("ProgramFiles", ""))
        / "Microsoft Visual Studio"
        / "Installer"
        / "vswhere.exe",
    ]
    return next((path for path in candidates if path.exists()), None)


def _visual_studio_installation(component: str) -> Path | None:
    vswhere = _vswhere_path()
    if vswhere is None:
        return None
    proc = subprocess.run(
        [
            str(vswhere),
            "-latest",
            "-products",
            "*",
            "-requires",
            component,
            "-property",
            "installationPath",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        return None
    path = proc.stdout.strip().splitlines()
    if not path:
        return None
    install = Path(path[0])
    return install if install.exists() else None


def _require_windows_atl(env: Mapping[str, str], install: Path | None) -> None:
    include_dirs = tuple(
        Path(part).expanduser()
        for part in env.get("INCLUDE", "").split(os.pathsep)
        if part.strip()
    )
    if any((directory / "atlbase.h").is_file() for directory in include_dirs):
        return
    install_text = str(install) if install is not None else "<BuildTools-install-path>"
    raise SystemExit(
        "Visual Studio C++ ATL is required for LLVM PDB support, but atlbase.h "
        "is absent from the activated INCLUDE path. Install component "
        "Microsoft.VisualStudio.Component.VC.ATL from an elevated terminal:\n"
        '  "C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\setup.exe" '
        f'modify --installPath "{install_text}" --add '
        "Microsoft.VisualStudio.Component.VC.ATL --quiet --norestart"
    )


def _windows_msvc_env(
    base: dict[str, str],
    *,
    machine: str | None = None,
) -> dict[str, str]:
    if platform.system() != "Windows":
        return base
    raw_machine = machine or platform.machine()
    host = llvm_host_architecture(ROOT, raw_machine)
    if (
        host is None
        or host.windows_component is None
        or host.windows_target_arch is None
        or host.windows_host_arch is None
    ):
        raise SystemExit(
            f"LLVM source bootstrap is not configured for Windows host {raw_machine!r}; "
            "add its Visual Studio component and host/target architecture to "
            "config/llvm_toolchain_arches.toml"
        )
    active_target = base.get("VSCMD_ARG_TGT_ARCH", "").lower()
    active_host = base.get("VSCMD_ARG_HOST_ARCH", "").lower()
    if (
        shutil.which("cl", path=base.get("PATH"))
        and active_target == host.windows_target_arch.lower()
        and active_host == host.windows_host_arch.lower()
    ):
        _require_windows_atl(base, _visual_studio_installation(host.windows_component))
        return base
    install = _visual_studio_installation(host.windows_component)
    if install is None:
        raise SystemExit(
            "MSVC Build Tools were not found. Install Visual Studio Build Tools "
            f"with component {host.windows_component} before building LLVM for "
            f"Windows {host.id}."
        )
    vsdevcmd = install / "Common7" / "Tools" / "VsDevCmd.bat"
    if not vsdevcmd.exists():
        raise SystemExit(f"Visual Studio developer command file not found: {vsdevcmd}")
    activation_var = "MOLT_LLVM_VSDEVCMD_CALL"
    activation_env = base.copy()
    # Python's Windows argv quoting escapes embedded quotes using MSVCRT rules,
    # but cmd.exe does not interpret those backslashes.  Expand a trusted,
    # pre-quoted environment value inside cmd instead, keeping the /c argument
    # itself quote-free even when the Visual Studio path contains spaces.
    activation_env[activation_var] = f'"{vsdevcmd}"'
    command = (
        f"call %{activation_var}% -arch={host.windows_target_arch} "
        f"-host_arch={host.windows_host_arch} >nul && set"
    )
    proc = subprocess.run(
        ["cmd.exe", "/d", "/s", "/c", command],
        check=False,
        capture_output=True,
        text=True,
        env=activation_env,
    )
    if proc.returncode != 0:
        raise SystemExit(proc.stderr.strip() or "Failed to activate VsDevCmd.bat")
    env = base.copy()
    for line in proc.stdout.splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key.casefold() == activation_var.casefold():
            continue
        env[key] = value
    if shutil.which("cl", path=env.get("PATH")) is None:
        raise SystemExit("VsDevCmd.bat completed, but cl.exe is still not on PATH")
    _require_windows_atl(env, install)
    return env


def _download(
    url: str,
    archive: Path,
    *,
    expected_sha256: str,
    expected_size: int | None = None,
) -> None:
    reject_poison_toolchain_path(archive, authority="LLVM archive cache")
    archive.parent.mkdir(parents=True, exist_ok=True)
    lock = archive.with_name(f".{archive.name}.lock")
    with _ExclusiveFileLock(lock):
        if archive.exists():
            size_matches = (
                expected_size is None or archive.stat().st_size == expected_size
            )
            if size_matches and _sha256(archive) == expected_sha256:
                print(f"[bootstrap-llvm] using cached archive {archive}", flush=True)
                return
            print(
                f"[bootstrap-llvm] replacing corrupt cached archive {archive}",
                flush=True,
            )
            archive.unlink()
        tmp = archive.with_name(f".{archive.name}.{uuid.uuid4().hex}.partial")
        print(f"[bootstrap-llvm] downloading {url}", flush=True)
        try:
            with urllib.request.urlopen(url) as response, tmp.open("xb") as fh:
                shutil.copyfileobj(response, fh)
                fh.flush()
                os.fsync(fh.fileno())
            actual_size = tmp.stat().st_size
            if expected_size is not None and actual_size != expected_size:
                raise SystemExit(
                    "LLVM source archive size mismatch: "
                    f"expected {expected_size}, found {actual_size}: {tmp}"
                )
            actual = _sha256(tmp)
            if actual != expected_sha256:
                raise SystemExit(
                    f"LLVM source archive checksum mismatch: expected {expected_sha256}, "
                    f"found {actual}: {tmp}"
                )
            os.replace(tmp, archive)
            _fsync_directory(archive.parent)
        finally:
            if tmp.exists():
                tmp.unlink()


def _stable_file_sha256(path: Path) -> tuple[int, str]:
    before = path.stat()
    before_change_ns = _windows_change_time_ns(path) if os.name == "nt" else None
    digest = _sha256(path)
    after = path.stat()
    if (
        before.st_size != after.st_size
        or before.st_mtime_ns != after.st_mtime_ns
        or (os.name != "nt" and before.st_ctime_ns != after.st_ctime_ns)
        or (
            os.name == "nt"
            and before_change_ns is not None
            and _windows_change_time_ns(path) != before_change_ns
        )
    ):
        raise SystemExit(f"LLVM source changed while hashing: {path}")
    if os.name == "nt" and before_change_ns is None:
        # NTFS creation time (Python's st_ctime) is not a content-change
        # authority.  If the real ChangeTime query is unavailable, a second
        # complete hash is the only fail-closed stability proof.
        repeated = _sha256(path)
        repeated_stat = path.stat()
        if (
            repeated != digest
            or repeated_stat.st_size != after.st_size
            or repeated_stat.st_mtime_ns != after.st_mtime_ns
        ):
            raise SystemExit(f"LLVM source changed while hashing: {path}")
    return before.st_size, digest


def _source_tree_identity(destination: Path) -> dict[str, object]:
    entries = tuple(
        sorted(
            (
                path
                for path in destination.rglob("*")
                if path.name != LLVM_SOURCE_MARKER
                and (path.is_file() or path.is_symlink())
            ),
            key=lambda path: path.relative_to(destination).as_posix(),
        )
    )
    regular = tuple(path for path in entries if not path.is_symlink())
    workers = min(16, max(1, len(regular)))
    with ThreadPoolExecutor(max_workers=workers) as executor:
        hashed = dict(
            zip(regular, executor.map(_stable_file_sha256, regular), strict=True)
        )
    aggregate = hashlib.sha256()
    total_bytes = 0
    for path in entries:
        relative = path.relative_to(destination).as_posix()
        if path.is_symlink():
            target = os.readlink(path)
            size = len(target.encode("utf-8", errors="surrogateescape"))
            digest = hashlib.sha256(
                target.encode("utf-8", errors="surrogateescape")
            ).hexdigest()
            kind = "symlink"
        else:
            size, digest = hashed[path]
            kind = "file"
        total_bytes += size
        aggregate.update(kind.encode("ascii"))
        aggregate.update(b"\0")
        aggregate.update(relative.encode("utf-8", errors="surrogateescape"))
        aggregate.update(b"\0")
        aggregate.update(str(size).encode("ascii"))
        aggregate.update(b"\0")
        aggregate.update(digest.encode("ascii"))
        aggregate.update(b"\n")
    return {
        "digest": aggregate.hexdigest(),
        "file_count": len(entries),
        "total_bytes": total_bytes,
    }


def _source_tree_projection(
    destination: Path,
) -> tuple[dict[str, object], bool]:
    aggregate = hashlib.sha256()
    file_count = 0
    total_bytes = 0
    change_time_complete = True
    for path in sorted(
        (
            path
            for path in destination.rglob("*")
            if path.name != LLVM_SOURCE_MARKER and (path.is_file() or path.is_symlink())
        ),
        key=lambda path: path.relative_to(destination).as_posix(),
    ):
        relative = path.relative_to(destination).as_posix()
        if path.is_symlink():
            target = os.readlink(path)
            row: tuple[object, ...] = ("symlink", relative, target)
            total_bytes += len(target.encode("utf-8", errors="surrogateescape"))
            file_count += 1
            aggregate.update(
                json.dumps(row, ensure_ascii=False, separators=(",", ":")).encode(
                    "utf-8", errors="surrogateescape"
                )
            )
            aggregate.update(b"\n")
            continue
        stat = path.stat()
        change_ns = (
            _windows_change_time_ns(path) if os.name == "nt" else stat.st_ctime_ns
        )
        if change_ns is None:
            change_time_complete = False
        row = ("file", relative, stat.st_size, stat.st_mtime_ns, change_ns)
        total_bytes += stat.st_size
        file_count += 1
        aggregate.update(
            json.dumps(row, ensure_ascii=False, separators=(",", ":")).encode(
                "utf-8", errors="surrogateescape"
            )
        )
        aggregate.update(b"\n")
    return (
        {
            "digest": aggregate.hexdigest(),
            "file_count": file_count,
            "total_bytes": total_bytes,
        },
        os.name != "nt" or change_time_complete,
    )


def _source_marker_payload(
    *,
    archive_sha256: str,
    source_contract: dict[str, object],
    source_tree: dict[str, object],
    source_projection: dict[str, object],
) -> dict[str, object]:
    payload: dict[str, object] = {
        "schema": LLVM_SOURCE_SCHEMA,
        "archive_sha256": archive_sha256,
        "source_contract": source_contract,
        "source_tree": source_tree,
        "source_projection": source_projection,
    }
    payload["record_sha256"] = hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return payload


def _source_marker_record_is_valid(payload: dict[str, object]) -> bool:
    record_sha256 = payload.get("record_sha256")
    if not isinstance(record_sha256, str):
        return False
    record = {key: value for key, value in payload.items() if key != "record_sha256"}
    expected = hashlib.sha256(
        json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return record_sha256 == expected


def _safe_extract_tar_xz(
    archive: Path,
    destination: Path,
    *,
    archive_sha256: str,
    source_contract: dict[str, object] | None = None,
    simulate_publication_crash_after: str | None = None,
) -> dict[str, object]:
    reject_poison_toolchain_path(destination, authority="LLVM source custody")
    marker = destination / LLVM_SOURCE_MARKER
    lock = destination.with_name(f".{destination.name}.extract.lock")
    with _ExclusiveFileLock(lock):
        _recover_publication(destination)
        actual_archive_sha256 = _sha256(archive)
        if actual_archive_sha256 != archive_sha256:
            raise SystemExit(
                "LLVM source archive changed before extraction: "
                f"expected {archive_sha256}, found {actual_archive_sha256}: {archive}"
            )
        marker_payload: dict[str, object] = {}
        if destination.is_dir() and marker.is_file():
            try:
                decoded = json.loads(marker.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                decoded = None
            if isinstance(decoded, dict):
                marker_payload = decoded
        marker_has_attestation_shape = (
            marker_payload.get("schema") == LLVM_SOURCE_SCHEMA
            and isinstance(marker_payload.get("archive_sha256"), str)
            and isinstance(marker_payload.get("source_contract"), dict)
            and isinstance(marker_payload.get("source_tree"), dict)
            and _source_marker_record_is_valid(marker_payload)
        )
        live_tree_matches_marker = False
        if marker_has_attestation_shape:
            recorded_projection = marker_payload.get("source_projection")
            live_projection: dict[str, object] | None = None
            live_projection_is_trusted = False
            projection_matches = False
            if isinstance(recorded_projection, dict):
                live_projection, live_projection_is_trusted = _source_tree_projection(
                    destination
                )
                projection_matches = (
                    live_projection_is_trusted
                    and recorded_projection.get("trusted") is True
                    and {
                        key: value
                        for key, value in recorded_projection.items()
                        if key != "trusted"
                    }
                    == live_projection
                )
                live_tree_matches_marker = projection_matches
            if not live_tree_matches_marker:
                live_identity = _source_tree_identity(destination)
                live_tree_matches_marker = (
                    marker_payload.get("source_tree") == live_identity
                )
            if (
                live_tree_matches_marker
                and marker_payload.get("archive_sha256") == archive_sha256
                and marker_payload.get("source_contract") == (source_contract or {})
            ):
                if not projection_matches:
                    if live_projection is None:
                        live_projection, live_projection_is_trusted = (
                            _source_tree_projection(destination)
                        )
                    upgraded = _source_marker_payload(
                        archive_sha256=archive_sha256,
                        source_contract=source_contract or {},
                        source_tree=live_identity,
                        source_projection={
                            **live_projection,
                            "trusted": live_projection_is_trusted,
                        },
                    )
                    _atomic_json(marker, upgraded)
                    marker_payload = upgraded
                print(
                    f"[bootstrap-llvm] using verified source tree {destination}",
                    flush=True,
                )
                return marker_payload
        if destination.exists() and not marker_has_attestation_shape:
            raise SystemExit(
                "refusing to replace an unattested LLVM source directory: "
                f"{destination}"
            )
        staging = _publication_staging(destination, uuid.uuid4().hex)
        staging.mkdir(parents=True)
        try:
            with tarfile.open(archive, "r:xz") as tf:
                tf.extractall(staging, filter="data")
            source_tree = _source_tree_identity(staging)
            source_projection, source_projection_is_trusted = _source_tree_projection(
                staging
            )
            payload = _source_marker_payload(
                archive_sha256=archive_sha256,
                source_contract=source_contract or {},
                source_tree=source_tree,
                source_projection={
                    **source_projection,
                    "trusted": source_projection_is_trusted,
                },
            )
            _atomic_json(staging / LLVM_SOURCE_MARKER, payload)

            def validate(published: Path) -> None:
                published_marker = published / LLVM_SOURCE_MARKER
                try:
                    observed = json.loads(published_marker.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError) as exc:
                    raise SystemExit(
                        f"published LLVM source marker is unreadable: {published_marker}: {exc}"
                    ) from exc
                published_projection, published_projection_is_trusted = (
                    _source_tree_projection(published)
                )
                projection_matches = (
                    source_projection_is_trusted
                    and published_projection_is_trusted
                    and published_projection == source_projection
                )
                if observed != payload or (
                    not projection_matches
                    and _source_tree_identity(published) != payload["source_tree"]
                ):
                    raise SystemExit(
                        f"published LLVM source tree failed integrity projection: {published}"
                    )

            _publish_staged_prefix(
                staging,
                destination,
                validate=validate,
                simulate_crash_after=simulate_publication_crash_after,
            )
            return payload
        except (OSError, tarfile.TarError) as exc:
            raise SystemExit(
                f"Could not safely extract LLVM source archive: {exc}"
            ) from exc
        finally:
            if staging.exists():
                shutil.rmtree(staging)


def _publication_journal(destination: Path) -> Path:
    return destination.with_name(f".{destination.name}.publish.json")


def _publication_lock(destination: Path) -> Path:
    return destination.with_name(f".{destination.name}.publish.lock")


def _publication_staging(destination: Path, transaction: str) -> Path:
    return destination.with_name(f".{destination.name}.{transaction}.staging")


def _publication_backup(destination: Path, transaction: str) -> Path:
    return destination.with_name(f".{destination.name}.{transaction}.rollback")


def _publication_transaction(staging: Path, destination: Path) -> str:
    pattern = re.compile(
        rf"^\.{re.escape(destination.name)}\.(?P<transaction>[0-9a-f]{{32}})\.staging$"
    )
    match = pattern.fullmatch(staging.name)
    if match is None:
        raise SystemExit(
            "canonical LLVM staging must use a unique transaction name: "
            f"expected .{destination.name}.<32 lowercase hex>.staging, found {staging}"
        )
    return match.group("transaction")


def _remove_tree(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    elif path.exists() or path.is_symlink():
        path.unlink()


def _recover_publication_locked(destination: Path) -> None:
    journal = _publication_journal(destination)
    if not journal.is_file():
        return
    try:
        payload = json.loads(journal.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise SystemExit(f"invalid LLVM publication journal {journal}: {exc}") from exc
    if payload.get("schema") != LLVM_PUBLICATION_SCHEMA:
        raise SystemExit(f"unsupported LLVM publication journal: {journal}")
    transaction = payload.get("transaction")
    if (
        not isinstance(transaction, str)
        or re.fullmatch(r"[0-9a-f]{32}", transaction) is None
    ):
        raise SystemExit(f"invalid LLVM publication transaction in {journal}")
    parent = destination.parent.resolve()

    def transaction_path(name: str) -> Path:
        raw = payload.get(name)
        if not isinstance(raw, str):
            raise SystemExit(f"LLVM publication journal omits {name}: {journal}")
        path = Path(raw).resolve()
        if path.parent != parent:
            raise SystemExit(
                f"LLVM publication journal {name} escapes custody parent: {path}"
            )
        return path

    recorded_destination = transaction_path("destination")
    if recorded_destination != destination.resolve():
        raise SystemExit(
            f"LLVM publication journal targets {recorded_destination}, not {destination}"
        )
    staging = transaction_path("staging")
    backup = transaction_path("backup")
    expected_staging = _publication_staging(destination, transaction).resolve()
    expected_backup = _publication_backup(destination, transaction).resolve()
    if staging != expected_staging or backup != expected_backup:
        raise SystemExit(
            "LLVM publication journal paths do not match its transaction: "
            f"staging={staging} expected={expected_staging}; "
            f"backup={backup} expected={expected_backup}"
        )
    phase = payload.get("phase")
    if phase == "validated":
        if not destination.exists():
            raise SystemExit(
                f"validated LLVM publication lost its destination: {destination}"
            )
        _remove_tree(backup)
        _remove_tree(staging)
    elif phase in {"prepared", "old-moved", "new-moved"}:
        # Any transaction not durably marked validated rolls back.  File-system
        # topology handles crashes between rename and phase-journal writes.
        if backup.exists():
            if destination.exists():
                _remove_tree(destination)
            backup.rename(destination)
        elif phase in {"old-moved", "new-moved"} and destination.exists():
            # Fresh installation had no prior prefix; the unvalidated new tree
            # cannot become canonical authority.
            _remove_tree(destination)
        _remove_tree(staging)
        _remove_tree(backup)
    else:
        raise SystemExit(f"invalid LLVM publication phase {phase!r}: {journal}")
    journal.unlink()
    _fsync_directory(parent)


def _recover_publication(destination: Path) -> None:
    with _ExclusiveFileLock(_publication_lock(destination)):
        _recover_publication_locked(destination)


class _SimulatedPublicationCrash(BaseException):
    """Test-only process-death injection after a durable publication phase."""


def _publish_staged_prefix(
    staging: Path,
    destination: Path,
    *,
    validate: Callable[[Path], None],
    simulate_crash_after: str | None = None,
) -> None:
    reject_poison_toolchain_path(destination, authority="canonical LLVM publication")
    if staging.parent.resolve() != destination.parent.resolve():
        raise SystemExit(
            "LLVM staging and publication prefixes must share one parent for "
            "same-volume rename publication"
        )
    if not staging.exists():
        raise SystemExit(f"LLVM publication staging prefix does not exist: {staging}")
    if staging.is_symlink() or not staging.is_dir():
        raise SystemExit(
            f"LLVM publication staging prefix must be a real directory: {staging}"
        )
    if destination.is_symlink():
        raise SystemExit(
            f"canonical LLVM publication destination cannot be a symlink: {destination}"
        )
    transaction = _publication_transaction(staging, destination)
    with _ExclusiveFileLock(_publication_lock(destination)):
        _recover_publication_locked(destination)
        backup = _publication_backup(destination, transaction)
        journal = _publication_journal(destination)
        payload: dict[str, object] = {
            "schema": LLVM_PUBLICATION_SCHEMA,
            "transaction": transaction,
            "destination": str(destination.resolve()),
            "staging": str(staging.resolve()),
            "backup": str(backup.resolve()),
            "phase": "prepared",
        }

        def record(phase: str) -> None:
            payload["phase"] = phase
            _atomic_json(journal, payload)
            if simulate_crash_after == phase:
                raise _SimulatedPublicationCrash(phase)

        def mutate(phase: str) -> None:
            if simulate_crash_after == phase:
                raise _SimulatedPublicationCrash(phase)

        record("prepared")
        try:
            if destination.exists():
                destination.rename(backup)
            mutate("old-renamed")
            record("old-moved")
            staging.rename(destination)
            _fsync_directory(destination.parent)
            mutate("new-renamed")
            record("new-moved")
            validate(destination)
            record("validated")
        except _SimulatedPublicationCrash:
            raise
        except BaseException:
            _recover_publication_locked(destination)
            raise
        else:
            _recover_publication_locked(destination)


def _llvm_source_root(extract_root: Path, version: str) -> Path:
    direct = extract_root / f"llvm-project-llvmorg-{version}" / "llvm"
    if direct.exists():
        return direct
    matches = sorted(extract_root.glob("llvm-project-*/llvm"))
    if matches:
        return matches[0]
    raise SystemExit(f"Unable to find extracted LLVM source under {extract_root}")


def _build_cache_identity(
    *,
    release_identity: dict[str, object],
    source_identity: dict[str, object],
    architecture_contract_sha256: str,
    targets: str,
    projects: str,
    build_type: str,
    cmake: _BuildTool,
    ninja: _BuildTool,
) -> dict[str, object]:
    config: dict[str, object] = {
        "architecture_contract_sha256": architecture_contract_sha256,
        "targets": sorted(item for item in targets.split(";") if item),
        "projects": sorted(item for item in projects.split(";") if item),
        "build_type": build_type,
        "generator": "Ninja",
        "build_tools": {
            "cmake": asdict(cmake),
            "ninja": asdict(ninja),
        },
        "cmake_contract": {
            "assertions": True,
            "benchmarks": False,
            "docs": False,
            "examples": False,
            "tests": False,
            "install_utils": True,
        },
    }
    config_digest = hashlib.sha256(
        json.dumps(config, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    inputs: dict[str, object] = {
        "release": release_identity,
        "source": source_identity,
        "config_digest": config_digest,
        "config": config,
    }
    digest = hashlib.sha256(
        json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return {"schema": LLVM_BUILD_SCHEMA, "digest": digest, "inputs": inputs}


def _validate_bootstrap_path_topology(
    *,
    prefix: Path,
    archive: Path,
    source_root: Path,
    build_dir: Path,
) -> None:
    directories = {
        "prefix": prefix.resolve(),
        "source root": source_root.resolve(),
        "build directory": build_dir.resolve(),
    }
    rows = tuple(directories.items())
    for index, (left_name, left) in enumerate(rows):
        for right_name, right in rows[index + 1 :]:
            if (
                left == right
                or left.is_relative_to(right)
                or right.is_relative_to(left)
            ):
                raise SystemExit(
                    "LLVM bootstrap authorities must be disjoint; "
                    f"{left_name}={left} conflicts with {right_name}={right}"
                )
    resolved_archive = archive.resolve()
    for name, directory in rows:
        if resolved_archive == directory or resolved_archive.is_relative_to(directory):
            raise SystemExit(
                "LLVM archive custody must be outside mutable install/source/build "
                f"trees; archive={resolved_archive} conflicts with {name}={directory}"
            )


def _prepare_build_cache(
    build_dir: Path,
    identity: dict[str, object],
) -> None:
    reject_poison_toolchain_path(build_dir, authority="LLVM build cache")
    marker = build_dir / LLVM_BUILD_MARKER
    observed: object = None
    if marker.is_file():
        try:
            observed = json.loads(marker.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            observed = None
    observed_is_attested = False
    if isinstance(observed, dict) and observed.get("schema") == LLVM_BUILD_SCHEMA:
        inputs = observed.get("inputs")
        digest = observed.get("digest")
        if isinstance(inputs, dict) and isinstance(digest, str):
            expected_digest = hashlib.sha256(
                json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode(
                    "utf-8"
                )
            ).hexdigest()
            observed_is_attested = digest == expected_digest
    if build_dir.exists() and not observed_is_attested and any(build_dir.iterdir()):
        raise SystemExit(
            f"refusing to delete an unattested LLVM build directory: {build_dir}"
        )
    if build_dir.exists() and observed != identity:
        print(
            f"[bootstrap-llvm] invalidating build cache with changed inputs {build_dir}",
            flush=True,
        )
        shutil.rmtree(build_dir)
    build_dir.mkdir(parents=True, exist_ok=True)
    _atomic_json(marker, identity)


def _verify_llvm_config(prefix: Path, version: str) -> Path:
    """Compatibility projection of the complete shared prefix verifier."""

    try:
        return verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            version=version,
            expected_targets=required_llvm_targets_for_host(ROOT),
        ).llvm_config
    except LlvmToolchainConfigError as exc:
        raise SystemExit(str(exc)) from exc


def _validate_projected_publication(
    verification: LlvmPrefixVerification,
    destination: Path,
) -> LlvmPrefixVerification:
    """Project a fully verified staging result across a same-volume rename."""

    attestation = destination / ".molt-llvm-toolchain.json"
    try:
        payload = json.loads(attestation.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise LlvmToolchainConfigError(
            f"published LLVM attestation is unreadable: {attestation}: {exc}"
        ) from exc
    expected_release = (
        asdict(verification.release) if verification.release is not None else None
    )
    expected = {
        "schema": LLVM_ATTESTATION_SCHEMA,
        "prefix": str(destination.resolve()),
        "version": verification.version,
        "release": expected_release,
        "custody": _llvm_attestation_custody(ROOT, destination, verification.release),
        "release_manifest_sha256": load_llvm_releases(ROOT).digest,
        "build_config": {
            "projects": sorted(load_llvm_architecture_contract(ROOT).required_projects),
            "targets": list(verification.targets),
            "build_type": canonical_llvm_build_type(ROOT),
        },
        "content_digest": verification.content_digest,
    }
    mismatches = {
        key: (payload.get(key), value)
        for key, value in expected.items()
        if payload.get(key) != value
    }
    if mismatches:
        raise LlvmToolchainConfigError(
            f"published LLVM attestation projection drift: {mismatches}"
        )
    for fact in verification.content_facts:
        path = destination / fact.path
        try:
            stat = path.stat()
        except OSError as exc:
            raise LlvmToolchainConfigError(
                f"published LLVM content disappeared after rename: {path}"
            ) from exc
        if stat.st_size != fact.size or stat.st_mtime_ns != fact.mtime_ns:
            raise LlvmToolchainConfigError(
                f"published LLVM content metadata changed after rename: {path}"
            )
    relative_config = verification.llvm_config.relative_to(verification.prefix)
    return replace(
        verification,
        prefix=destination.resolve(),
        llvm_config=destination.resolve() / relative_config,
    )


def _build_and_publish(
    args: argparse.Namespace,
    *,
    prefix: Path,
    build_dir: Path,
    llvm_source: Path,
    targets: str,
    required_targets: set[str],
    project_set: set[str],
    env: dict[str, str],
    is_canonical: bool,
    build_identity: dict[str, object],
    cmake: _BuildTool,
    ninja: _BuildTool,
) -> tuple[LlvmPrefixVerification, Path] | None:
    lock_path = build_dir.with_name(f".{build_dir.name}.build.lock")
    with _ExclusiveFileLock(lock_path):
        if is_canonical:
            _recover_publication(prefix)
            try:
                current = verify_llvm_toolchain_prefix(
                    ROOT,
                    prefix,
                    version=args.version,
                    expected_targets=tuple(sorted(required_targets)),
                    require_attestation=True,
                    content_policy="cached",
                )
            except LlvmToolchainConfigError:
                pass
            else:
                print(
                    f"[bootstrap-llvm] canonical prefix already verified {prefix}",
                    flush=True,
                )
                return current, prefix / ".molt-llvm-toolchain.json"

        _prepare_build_cache(
            build_dir,
            build_identity,
        )
        prefix.parent.mkdir(parents=True, exist_ok=True)
        install_prefix = prefix
        if is_canonical:
            install_prefix = _publication_staging(prefix, uuid.uuid4().hex)
            install_prefix.mkdir()

        cmake_configure = [
            cmake.path,
            "-S",
            str(llvm_source),
            "-B",
            str(build_dir),
            "-G",
            "Ninja",
            f"-DCMAKE_MAKE_PROGRAM={ninja.path}",
            f"-DCMAKE_BUILD_TYPE={args.build_type}",
            f"-DCMAKE_INSTALL_PREFIX={install_prefix}",
            f"-DLLVM_TARGETS_TO_BUILD={targets}",
            f"-DLLVM_ENABLE_PROJECTS={args.projects}",
            "-DLLVM_ENABLE_ASSERTIONS=ON",
            "-DLLVM_INCLUDE_BENCHMARKS=OFF",
            "-DLLVM_INCLUDE_DOCS=OFF",
            "-DLLVM_INCLUDE_EXAMPLES=OFF",
            "-DLLVM_INCLUDE_TESTS=OFF",
            "-DLLVM_INSTALL_UTILS=ON",
        ]
        try:
            _run(cmake_configure, cwd=ROOT, env=env)
            if args.configure_only:
                print(f"[bootstrap-llvm] configured {build_dir}")
                return None
            _run(
                [
                    cmake.path,
                    "--build",
                    str(build_dir),
                    "--target",
                    "install",
                    "--",
                    "-j",
                    str(args.jobs),
                ],
                cwd=ROOT,
                env=env,
            )
            verification = verify_llvm_toolchain_prefix(
                ROOT,
                install_prefix,
                version=args.version,
                expected_targets=tuple(sorted(required_targets)),
                content_policy="full",
            )
            attestation = write_llvm_toolchain_attestation(
                ROOT,
                verification,
                projects=tuple(sorted(project_set)),
                build_type=args.build_type,
                published_prefix=prefix if is_canonical else None,
            )
            if not is_canonical:
                return verification, attestation
            projected: LlvmPrefixVerification | None = None

            def validate(_published: Path) -> None:
                nonlocal projected
                projected = _validate_projected_publication(verification, prefix)

            _publish_staged_prefix(install_prefix, prefix, validate=validate)
            if projected is None:
                raise SystemExit(
                    "canonical LLVM publication validation did not complete"
                )
            return projected, prefix / ".molt-llvm-toolchain.json"
        finally:
            if is_canonical and install_prefix.exists():
                shutil.rmtree(install_prefix)


def main(argv: list[str] | None = None) -> int:
    try:
        pin = required_llvm_backend_pin(ROOT)
        if pin is None:
            raise SystemExit(f"Unable to find LLVM backend feature pin under {ROOT}")
        major = pin.major
    except LlvmToolchainConfigError as exc:
        raise SystemExit(str(exc)) from exc
    default_version = _default_release_for_major(major)
    contract = load_llvm_architecture_contract(ROOT)
    parser = argparse.ArgumentParser(
        description="Build and install a complete LLVM dev prefix for Molt."
    )
    parser.add_argument("--version", default=default_version)
    parser.add_argument(
        "--prefix",
        type=Path,
        default=None,
        help="Install prefix. Defaults to the canonical Molt toolchain custody root.",
    )
    parser.add_argument(
        "--archive",
        type=Path,
        default=None,
        help="Cached llvm-project tar.xz path.",
    )
    parser.add_argument(
        "--development-source-sha256",
        default=None,
        help=(
            "Explicit SHA-256 for an unpinned development release. Cannot override "
            "a canonical release checksum."
        ),
    )
    parser.add_argument(
        "--development-source-url",
        default=None,
        help=(
            "Explicit source URL for an unpinned development build. Development "
            "builds also require --prefix and --development-source-sha256."
        ),
    )
    parser.add_argument(
        "--development-minimum-cmake",
        default=None,
        help=(
            "Required CMake semantic version for an unpinned development build. "
            "Canonical releases derive this from the release manifest."
        ),
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        default=None,
        help="Extraction root containing llvm-project-llvmorg-<version>/llvm.",
    )
    parser.add_argument(
        "--build-dir",
        type=Path,
        default=None,
        help="CMake build directory.",
    )
    parser.add_argument(
        "--targets",
        default=None,
        help="Semicolon-separated LLVM targets; defaults to host + required targets.",
    )
    parser.add_argument(
        "--projects",
        default=";".join(contract.required_projects),
        help="LLVM subprojects required by Molt's native, linker, and MLIR backends.",
    )
    parser.add_argument("--build-type", default=canonical_llvm_build_type(ROOT))
    parser.add_argument("--jobs", type=int, default=_default_llvm_jobs())
    parser.add_argument("--required-free-gb", type=float, default=40.0)
    parser.add_argument("--required-memory-gb", type=float, default=8.0)
    parser.add_argument("--configure-only", action="store_true")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Only verify an existing prefix and print the required env var.",
    )
    args = parser.parse_args(argv)

    for name in (
        "MOLT_TARGET_ROOT",
        "MOLT_LLVM_PREFIX",
        pin.env_var,
        f"MLIR_SYS_{major * 10}_PREFIX",
        f"TABLEGEN_{major * 10}_PREFIX",
        "LLVM_CONFIG_PATH",
    ):
        if value := os.environ.get(name, "").strip():
            reject_poison_toolchain_path(value, authority=name)
    managed = managed_llvm_paths(ROOT, pin, version=args.version)
    raw_prefix = args.prefix or managed.prefix
    reject_poison_toolchain_path(raw_prefix, authority="LLVM bootstrap prefix")
    prefix = raw_prefix.resolve()
    targets = args.targets or _default_llvm_targets()
    target_set = {item for item in targets.split(";") if item}
    project_set = {item for item in args.projects.split(";") if item}
    canonical_prefix = managed.prefix.resolve()
    is_canonical = prefix == canonical_prefix
    release = llvm_release(args.version, ROOT)
    if release is None:
        if args.prefix is None:
            raise SystemExit(
                "development LLVM builds require an explicit noncanonical --prefix"
            )
        if not args.check and not args.development_source_url:
            raise SystemExit("development LLVM builds require --development-source-url")
        if (
            not args.check
            and re.fullmatch(r"\d+\.\d+\.\d+", args.development_minimum_cmake or "")
            is None
        ):
            raise SystemExit(
                "development LLVM builds require --development-minimum-cmake X.Y.Z"
            )
    elif (
        args.development_source_url is not None
        or args.development_minimum_cmake is not None
    ):
        raise SystemExit(
            "development source/tool requirements cannot override a canonical release manifest"
        )
    if is_canonical and release is None:
        raise SystemExit(
            "canonical LLVM custody accepts only exact patch releases from "
            "config/llvm_toolchain_releases.toml"
        )
    canonical_custody_root = managed.root.resolve()
    if not is_canonical:
        if (
            prefix == canonical_custody_root
            or prefix.is_relative_to(canonical_custody_root)
            or canonical_custody_root.is_relative_to(prefix)
        ):
            raise SystemExit(
                "noncanonical LLVM custody must be disjoint from canonical managed "
                f"custody: prefix={prefix} canonical={canonical_custody_root}"
            )
    host = llvm_host_architecture(ROOT, platform.machine())
    required_targets = set(contract.required_targets)
    if host is not None:
        required_targets.add(host.llvm_target)
    elif is_canonical:
        raise SystemExit(
            f"canonical managed LLVM is unsupported on host {platform.machine()!r}; "
            "add its architecture and Cargo mapping to config/llvm_toolchain_arches.toml"
        )
    if is_canonical:
        expected_projects = set(contract.required_projects)
        expected_build_type = canonical_llvm_build_type(ROOT)
        if (
            project_set != expected_projects
            or target_set != required_targets
            or args.build_type != expected_build_type
        ):
            raise SystemExit(
                "the canonical managed prefix requires the complete toolchain contract; "
                f"projects expected={sorted(expected_projects)} found={sorted(project_set)}; "
                f"targets expected={sorted(required_targets)} found={sorted(target_set)}; "
                f"build type expected={expected_build_type} found={args.build_type}"
            )
    env_var = _llvm_sys_prefix_env_var(args.version)
    if args.check:
        verification = verify_llvm_toolchain_prefix(
            ROOT,
            prefix,
            version=args.version,
            expected_targets=tuple(sorted(required_targets)),
            require_attestation=is_canonical,
            content_policy="full",
        )
        projected = project_llvm_toolchain_environment(
            ROOT,
            verification,
            environ=os.environ,
        )
        for name in (
            "MOLT_LLVM_PREFIX",
            env_var,
            f"MLIR_SYS_{major * 10}_PREFIX",
            f"TABLEGEN_{major * 10}_PREFIX",
            "LLVM_CONFIG_PATH",
        ):
            print(f"{name}={projected[name]}")
        print(f"llvm-config={verification.llvm_config}")
        return 0

    for label, raw in (
        ("archive", args.archive),
        ("source root", args.source_root),
        ("build directory", args.build_dir),
    ):
        if raw is not None:
            reject_poison_toolchain_path(raw, authority=f"LLVM bootstrap {label}")

    custody_paths = (
        managed
        if release is not None
        else _development_llvm_paths(prefix, args.version)
    )
    archive = (
        args.archive.resolve() if args.archive is not None else custody_paths.archive
    )
    source_root = (
        args.source_root.resolve()
        if args.source_root is not None
        else custody_paths.source_root
    )
    build_dir = (
        args.build_dir.resolve()
        if args.build_dir is not None
        else custody_paths.build_dir
    )
    for label, path in (
        ("archive", archive),
        ("source root", source_root),
        ("build directory", build_dir),
    ):
        reject_poison_toolchain_path(path, authority=f"LLVM bootstrap {label}")
    minimum_cmake = (
        release.minimum_cmake
        if release is not None
        else str(args.development_minimum_cmake)
    )
    cmake = _compatible_cmake(minimum_cmake)
    ninja = _required_build_tool("ninja")
    env = _windows_msvc_env(os.environ.copy())
    if is_canonical:
        for label, path, expected in (
            ("archive", archive, managed.archive),
            ("source root", source_root, managed.source_root),
            ("build directory", build_dir, managed.build_dir),
        ):
            if path.resolve() != expected.resolve():
                raise SystemExit(
                    f"canonical managed {label} is fixed by custody authority: "
                    f"expected {expected}, found {path}"
                )
    _validate_bootstrap_path_topology(
        prefix=prefix,
        archive=archive,
        source_root=source_root,
        build_dir=build_dir,
    )
    _preflight_resources(
        managed.root if is_canonical else prefix.parent,
        required_free_gb=max(0.0, args.required_free_gb),
        required_memory_gb=max(0.0, args.required_memory_gb),
    )
    url = release.url if release is not None else str(args.development_source_url)
    source_sha256 = _source_sha256(args.version, args.development_source_sha256)
    expected_size = release.size if release is not None else None
    release_identity: dict[str, object] = (
        asdict(release)
        if release is not None
        else {
            "version": args.version,
            "url": url,
            "size": expected_size,
            "source_sha256": source_sha256,
            "custody": "development-noncanonical",
        }
    )
    _download(
        url,
        archive,
        expected_sha256=source_sha256,
        expected_size=expected_size,
    )
    source_identity = _safe_extract_tar_xz(
        archive,
        source_root,
        archive_sha256=source_sha256,
        source_contract=release_identity,
    )
    llvm_source = _llvm_source_root(source_root, args.version)
    build_identity = _build_cache_identity(
        release_identity=release_identity,
        source_identity=source_identity,
        architecture_contract_sha256=contract.digest,
        targets=targets,
        projects=args.projects,
        build_type=args.build_type,
        cmake=cmake,
        ninja=ninja,
    )
    result = _build_and_publish(
        args,
        prefix=prefix,
        build_dir=build_dir,
        llvm_source=llvm_source,
        targets=targets,
        required_targets=required_targets,
        project_set=project_set,
        env=env,
        is_canonical=is_canonical,
        build_identity=build_identity,
        cmake=cmake,
        ninja=ninja,
    )
    if result is None:
        return 0
    verification, attestation = result
    print(f"[bootstrap-llvm] installed {verification.llvm_config}")
    print(f"[bootstrap-llvm] attested {attestation}")
    projected = project_llvm_toolchain_environment(
        ROOT,
        verification,
        environ=env,
    )
    for name in (
        "MOLT_LLVM_PREFIX",
        env_var,
        f"MLIR_SYS_{major * 10}_PREFIX",
        f"TABLEGEN_{major * 10}_PREFIX",
        "LLVM_CONFIG_PATH",
    ):
        print(f"{name}={projected[name]}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except LlvmToolchainConfigError as exc:
        raise SystemExit(str(exc)) from exc
