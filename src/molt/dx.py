from __future__ import annotations

from collections.abc import Collection
from dataclasses import dataclass
import hashlib
import json
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import tomllib
import uuid
from pathlib import Path
from typing import Literal, Mapping, Sequence, cast

from molt import custody_layout
from molt.environment_registry import (
    EnvironmentRegistryError,
    check_process_environment,
)
from molt.source_root import compiler_source_root
from molt.path_custody import (
    host_path_is_within,
    same_host_path,
)


TEST_PYTHONS = ["3.12", "3.13", "3.14"]
GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV = "MOLT_CI_EPHEMERAL_CUSTODY_ROOT"
# The operator's artifact root. Only this module reads it: every other
# consumer asks `configured_artifact_root` or `artifact_root`.
ARTIFACT_ROOT_ENV = "MOLT_EXT_ROOT"
# Run scratch storage: `disk` (the default, `<artifact root>/tmp`), `memory`
# (Linux `/dev/shm`), or the absolute path of a memory-backed directory the
# operator mounted (a macOS RAM disk). Molt never creates or mounts one.
SCRATCH_STORAGE_ENV = "MOLT_SCRATCH_STORAGE"
SHARED_MEMORY_ROOT = Path("/dev/shm")
CANONICAL_ROOT_ENV_KEYS = (
    ARTIFACT_ROOT_ENV,
    "CARGO_TARGET_DIR",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "MOLT_TARGET_ROOT",
    "MOLT_CACHE",
    "MOLT_DIFF_ROOT",
    "MOLT_DIFF_TMPDIR",
    "UV_CACHE_DIR",
    "UV_PROJECT_ENVIRONMENT",
    "PIP_CACHE_DIR",
    "RUFF_CACHE_DIR",
    "PYTHONPYCACHEPREFIX",
    "TMPDIR",
    "TMP",
    "TEMP",
)
# Scratch roots derive from `scratch_root`, which keeps them out of the
# checkout. Project configuration must not restate them.
SCRATCH_ENV_KEYS = (
    "MOLT_DIFF_ROOT",
    "MOLT_DIFF_TMPDIR",
    "PYTHONPYCACHEPREFIX",
    "TMPDIR",
    "TMP",
    "TEMP",
)
CANONICAL_RUN_ENV_KEYS = (
    *CANONICAL_ROOT_ENV_KEYS,
    "CARGO_INCREMENTAL",
    "MOLT_SESSION_ID",
    "MOLT_SESSION_ID_GENERATED",
)
DX_ENV_KEYS = (
    *CANONICAL_RUN_ENV_KEYS,
    GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV,
    "PYTHONPATH",
    "MOLT_BACKEND_DAEMON_SOCKET_DIR",
    "MOLT_USE_SCCACHE",
    "MOLT_DIFF_ALLOW_RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "MOLT_CACHE_MAX_GB",
    "MOLT_CACHE_MAX_AGE_DAYS",
    "UV_LINK_MODE",
)
# Toolchain root (wasi-sysroot / binaryen / zig) is DERIVED from the durable
# Molt custody root, never from a capacity-selected scratch/output volume.
DEFAULT_TARGET_ROOT_DIRNAME = "target-root"
# Provisioned toolchains (LLVM SDK, WASI sysroot, pinned tool releases) live
# under this directory of the toolchain root; every provisioner and discoverer
# derives the path from here.
TOOLCHAINS_DIRNAME = "toolchains"
# A guarded proof run receives one fresh, custody-external scratch root for
# everything it builds or emits (the proof queue creates it per run). Tools
# that produce artifacts default their output roots to it, so a proof never
# writes into the watched source checkout.
PROOF_SCRATCH_ROOT_ENV = "MOLT_PROOF_SCRATCH_ROOT"
DEFAULT_SCCACHE_CACHE_SIZE = "10G"
DEFAULT_MOLT_CACHE_MAX_GB = "30"
DEFAULT_MOLT_CACHE_MAX_AGE_DAYS = "30"
REQUIRE_EXTERNAL_ARTIFACTS_ENV = "MOLT_REQUIRE_EXTERNAL_ARTIFACTS"
PREFER_EXTERNAL_ARTIFACTS_ENV = "MOLT_PREFER_EXTERNAL_ARTIFACTS"
EXTERNAL_ARTIFACT_ROOTS_ENV = "MOLT_EXTERNAL_ARTIFACT_ROOTS"
# Either request knob asks for guarded artifact custody; REQUIRE also fails
# when no external root is healthy.
DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS = (
    REQUIRE_EXTERNAL_ARTIFACTS_ENV,
    PREFER_EXTERNAL_ARTIFACTS_ENV,
)
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}


class DxConfigError(RuntimeError):
    pass


CheckoutCustodyKind = Literal["durable", "github-actions-ephemeral", "explicit-scratch"]


@dataclass(frozen=True, slots=True)
class CheckoutCustody:
    """Typed separation between source location and execution custody.

    A durable checkout family owns long-lived Molt state. A verified hosted CI
    checkout is source-only: its per-run execution root is issued by the
    workflow under ``RUNNER_TEMP`` and can never become durable authority.
    """

    source_root: Path
    custody_root: Path
    toolchain_root: Path
    kind: CheckoutCustodyKind
    workflow_ref: str | None = None

    @property
    def ephemeral(self) -> bool:
        return self.kind != "durable"

    @property
    def source_only(self) -> bool:
        return self.kind == "github-actions-ephemeral"


# A session's Cargo target (`target/sessions/<component>`) and its backend-daemon
# sidecar label share one path component. It must be injective: two sessions
# that map to one component share one "isolated" build and one daemon label.
_SESSION_COMPONENT_SAFE = re.compile(r"[A-Za-z0-9_-]{1,32}")
_SESSION_COMPONENT_UNSAFE_CHAR = re.compile(r"[^A-Za-z0-9_-]")
_SESSION_COMPONENT_PREFIX_CHARS = 15
_SESSION_COMPONENT_DIGEST_CHARS = 16
_SESSION_COMPONENT_DIGEST_FORM = re.compile(
    rf"[A-Za-z0-9_-]{{0,{_SESSION_COMPONENT_PREFIX_CHARS}}}"
    rf"-[0-9a-f]{{{_SESSION_COMPONENT_DIGEST_CHARS}}}"
)


def session_artifact_component(session_id: str) -> str:
    """Return the path component that names one session's artifacts.

    An ID of 1 to 32 ASCII letters, digits, `-` or `_` is its own component.
    Any other ID becomes its first 15 characters, each unsafe one replaced by
    `_`, then `-` and 16 hex digits of the SHA-256 of the whole ID. An ID that
    already has that digest shape also takes the digest form, so the two forms
    never meet. Every component has at most 32 characters.
    """

    if _SESSION_COMPONENT_SAFE.fullmatch(
        session_id
    ) and not _SESSION_COMPONENT_DIGEST_FORM.fullmatch(session_id):
        return session_id
    digest = hashlib.sha256(session_id.encode("utf-8", "surrogatepass")).hexdigest()
    prefix = _SESSION_COMPONENT_UNSAFE_CHAR.sub(
        "_", session_id[:_SESSION_COMPONENT_PREFIX_CHARS]
    )
    return f"{prefix}-{digest[:_SESSION_COMPONENT_DIGEST_CHARS]}"


def generated_session_id(env: Mapping[str, str]) -> bool:
    return env.get("MOLT_SESSION_ID_GENERATED", "").strip().lower() in TRUE_VALUES


def uv_project_env_component(value: str) -> str:
    component = re.sub(r"[^A-Za-z0-9_.-]+", "-", value.strip()).strip("-._")
    return component or "default"


def checkout_component(source_root: Path) -> str:
    """A short, stable path component that names one checkout.

    Per-checkout state under a shared family root (a uv environment, a pytest
    cache) keys on it, so sibling worktrees never share it.
    """
    source = source_root.expanduser().resolve()
    digest = hashlib.sha256(os.path.normcase(str(source)).encode()).hexdigest()[:12]
    return f"{uv_project_env_component(source.name)[:24]}-{digest}"


def stable_uv_project_env_dir(
    artifact_root: Path,
    *,
    purpose: str,
    python: str,
    source_root: Path,
) -> Path:
    name = (
        f"{uv_project_env_component(purpose)}__py{uv_project_env_component(python)}"
        f"__src-{checkout_component(source_root)}"
    )
    return (artifact_root.expanduser().resolve() / "uv-project-envs" / name).resolve()


# The uv project environment (installed deps + editable molt) is a pure function
# of (project source, purpose, python) — NOT of the session — so it is stable
# within one checkout and cannot be overwritten by a sibling worktree. It is shared
# across sessions; only the Cargo target dir may be session-scoped for build
# isolation. Managed uv environments are durable caches. Callers that need a
# different environment must set an explicit `UV_PROJECT_ENVIRONMENT`.
DEFAULT_UV_PROJECT_PURPOSE = "dx"
DEFAULT_UV_PROJECT_PYTHON = "3.12"


def stable_uv_project_env_from_env(
    env: Mapping[str, str], artifact_root: Path, source_root: Path
) -> Path:
    return stable_uv_project_env_dir(
        artifact_root,
        purpose=env.get("MOLT_UV_PROJECT_PURPOSE") or DEFAULT_UV_PROJECT_PURPOSE,
        python=env.get("MOLT_UV_PROJECT_PYTHON") or DEFAULT_UV_PROJECT_PYTHON,
        source_root=source_root,
    )


def session_scoped_target_dir(target_root: Path, session_id: str | None) -> Path:
    if session_id:
        return target_root / "sessions" / session_artifact_component(session_id)
    return target_root


def cargo_target_dir_for_artifact_root(
    artifact_root: Path,
    session_id: str | None,
) -> Path:
    return session_scoped_target_dir(artifact_root / "target", session_id)


def cargo_target_dir_for_environment(
    artifact_root: Path,
    env: Mapping[str, str],
) -> Path:
    """Resolve Cargo custody from the canonical session provenance markers."""

    session_id = env.get("MOLT_SESSION_ID", "").strip()
    if not session_id or generated_session_id(env):
        session_id = None
    return cargo_target_dir_for_artifact_root(artifact_root, session_id)


# Peak resident memory a single rustc codegen job for the heavy molt-runtime (and
# source-recompiled numpy/scipy) build can reach. Cargo's default `-j<num_cpus>`
# runs that many rustc processes in parallel, so on a small box (8GB) an unbounded
# job count thrashes swap. Bounding jobs to available memory keeps a build inside a
# memory ceiling AND — critically for throughput — scales UP to the CPU count on a
# capable box instead of a fixed handful of jobs.
_BYTES_PER_CARGO_JOB = 2 * 1024 * 1024 * 1024
# Reserve headroom for the OS, the linker (peaks separately from parallel rustc),
# sccache, and the driving Python before dividing the rest among parallel jobs.
_CARGO_JOB_MEMORY_HEADROOM = 2 * 1024 * 1024 * 1024


def _windows_system_memory_bytes() -> tuple[int | None, int | None]:
    if os.name != "nt":
        return None, None
    import ctypes

    class _MemoryStatusEx(ctypes.Structure):
        _fields_ = [
            ("dwLength", ctypes.c_ulong),
            ("dwMemoryLoad", ctypes.c_ulong),
            ("ullTotalPhys", ctypes.c_ulonglong),
            ("ullAvailPhys", ctypes.c_ulonglong),
            ("ullTotalPageFile", ctypes.c_ulonglong),
            ("ullAvailPageFile", ctypes.c_ulonglong),
            ("ullTotalVirtual", ctypes.c_ulonglong),
            ("ullAvailVirtual", ctypes.c_ulonglong),
            ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
        ]

    status = _MemoryStatusEx()
    status.dwLength = ctypes.sizeof(_MemoryStatusEx)
    try:
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GlobalMemoryStatusEx.restype = ctypes.c_int
        kernel32.GlobalMemoryStatusEx.argtypes = [ctypes.POINTER(_MemoryStatusEx)]
        if kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
            return int(status.ullTotalPhys), int(status.ullAvailPhys)
    except (OSError, AttributeError, ValueError):
        pass
    return None, None


def _read_memory_integer(path: Path, *, allow_zero: bool = False) -> int | None:
    try:
        raw = path.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    if not raw.isdigit():
        return None
    value = int(raw)
    return value if value > 0 or (allow_zero and value == 0) else None


def _darwin_sysctl_integer(name: str) -> int | None:
    """Read an integer sysctl without spawning the ``sysctl`` executable."""

    if sys.platform != "darwin":
        return None
    import ctypes

    try:
        libc = ctypes.CDLL(None, use_errno=True)
        sysctlbyname = libc.sysctlbyname
        sysctlbyname.restype = ctypes.c_int
        sysctlbyname.argtypes = [
            ctypes.c_char_p,
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p,
            ctypes.c_size_t,
        ]
        encoded = name.encode("ascii")
        size = ctypes.c_size_t()
        if sysctlbyname(encoded, None, ctypes.byref(size), None, 0) != 0:
            return None
        if size.value <= 0 or size.value > ctypes.sizeof(ctypes.c_uint64):
            return None
        storage = ctypes.create_string_buffer(size.value)
        if sysctlbyname(encoded, storage, ctypes.byref(size), None, 0) != 0:
            return None
    except (AttributeError, OSError, ValueError):
        return None
    return int.from_bytes(storage.raw[: size.value], byteorder=sys.byteorder)


def _darwin_system_memory_bytes() -> tuple[int | None, int | None]:
    """Sample macOS physical capacity and immediately reclaimable memory."""

    total = _darwin_sysctl_integer("hw.memsize")
    page_size = _darwin_sysctl_integer("hw.pagesize")
    page_counts = (
        _darwin_sysctl_integer("vm.page_free_count"),
        _darwin_sysctl_integer("vm.page_inactive_count"),
        _darwin_sysctl_integer("vm.page_speculative_count"),
    )
    if page_size is None or any(value is None for value in page_counts):
        return total, None
    available = page_size * sum(value for value in page_counts if value is not None)
    return total, available


def _read_cgroup_inactive_file_bytes(path: Path) -> int:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return 0
    for line in lines:
        name, separator, raw = line.partition(" ")
        if separator and name in {"inactive_file", "total_inactive_file"}:
            return int(raw) if raw.isdigit() else 0
    return 0


def _linux_cgroup_memory_directories(
    *,
    cgroup_root: Path,
    membership_path: Path,
) -> tuple[Path, Path]:
    unified_relative: Path | None = None
    legacy_relative: Path | None = None
    try:
        memberships = membership_path.read_text(encoding="utf-8").splitlines()
    except OSError:
        memberships = []
    for line in memberships:
        _hierarchy, separator, suffix = line.partition(":")
        controllers, separator2, relative = suffix.partition(":")
        if not separator or not separator2:
            continue
        candidate = Path(relative.lstrip("/"))
        if not controllers:
            unified_relative = candidate
        elif "memory" in controllers.split(","):
            legacy_relative = candidate
    unified = cgroup_root / (unified_relative or Path())
    if not (unified / "memory.max").exists():
        unified = cgroup_root
    legacy_base = cgroup_root / "memory"
    legacy = legacy_base / (legacy_relative or Path())
    if not (legacy / "memory.limit_in_bytes").exists():
        legacy = legacy_base
    return unified, legacy


def _linux_system_memory_bytes(
    *,
    meminfo_path: Path = Path("/proc/meminfo"),
    cgroup_root: Path = Path("/sys/fs/cgroup"),
    cgroup_membership_path: Path = Path("/proc/self/cgroup"),
) -> tuple[int | None, int | None]:
    """Sample Linux host memory constrained by the active cgroup, if any."""

    fields: dict[str, int] = {}
    try:
        meminfo = meminfo_path.read_text(encoding="utf-8")
    except OSError:
        meminfo = ""
    for line in meminfo.splitlines():
        name, separator, value = line.partition(":")
        if not separator or name not in {"MemTotal", "MemAvailable"}:
            continue
        parts = value.split()
        if parts and parts[0].isdigit():
            fields[name] = int(parts[0]) * 1024

    total = fields.get("MemTotal")
    available = fields.get("MemAvailable")
    unified, legacy = _linux_cgroup_memory_directories(
        cgroup_root=cgroup_root,
        membership_path=cgroup_membership_path,
    )
    cgroup_limit = _read_memory_integer(unified / "memory.max")
    cgroup_usage = _read_memory_integer(unified / "memory.current", allow_zero=True)
    cgroup_inactive_file = _read_cgroup_inactive_file_bytes(unified / "memory.stat")
    if cgroup_limit is None:
        cgroup_limit = _read_memory_integer(legacy / "memory.limit_in_bytes")
        cgroup_usage = _read_memory_integer(
            legacy / "memory.usage_in_bytes", allow_zero=True
        )
        cgroup_inactive_file = _read_cgroup_inactive_file_bytes(legacy / "memory.stat")
    if cgroup_limit is not None:
        total = cgroup_limit if total is None else min(total, cgroup_limit)
        if cgroup_usage is not None:
            reclaimable = min(cgroup_usage, cgroup_inactive_file)
            cgroup_available = max(0, cgroup_limit - cgroup_usage + reclaimable)
            available = (
                cgroup_available
                if available is None
                else min(available, cgroup_available)
            )
    return total, available


def _system_memory_bytes() -> tuple[int | None, int | None]:
    """Sample total and live available physical memory as one host snapshot."""

    if os.name == "nt":
        return _windows_system_memory_bytes()
    if sys.platform.startswith("linux"):
        total, available = _linux_system_memory_bytes()
        if total is not None or available is not None:
            return total, available
    if sys.platform == "darwin":
        total, available = _darwin_system_memory_bytes()
        if total is not None or available is not None:
            return total, available
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        total_pages = os.sysconf("SC_PHYS_PAGES")
        available_pages = os.sysconf("SC_AVPHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None, None
    if page_size <= 0:
        return None, None
    total = int(page_size) * int(total_pages) if total_pages > 0 else None
    available = int(page_size) * int(available_pages) if available_pages > 0 else None
    return total, available


def _memory_bounded_worker_count(
    *,
    bytes_per_worker: int,
    headroom_bytes: int,
    cpu_count: int | None = None,
) -> int:
    """CPU and live-memory bounded worker ceiling shared by build phases."""

    if bytes_per_worker <= 0 or headroom_bytes < 0:
        raise ValueError("worker memory policy must be positive")
    total_memory_bytes, available_memory_bytes = _system_memory_bytes()
    return _memory_bounded_worker_count_from_samples(
        bytes_per_worker=bytes_per_worker,
        headroom_bytes=headroom_bytes,
        total_memory_bytes=total_memory_bytes,
        available_memory_bytes=available_memory_bytes,
        cpu_count=cpu_count,
    )


def _memory_bounded_worker_count_from_samples(
    *,
    bytes_per_worker: int,
    headroom_bytes: int,
    total_memory_bytes: int | None,
    available_memory_bytes: int | None,
    cpu_count: int | None = None,
) -> int:
    """Compute a worker ceiling from one coherent resource snapshot."""

    if bytes_per_worker <= 0 or headroom_bytes < 0:
        raise ValueError("worker memory policy must be positive")
    cpus = max(1, cpu_count if cpu_count is not None else (os.cpu_count() or 1))
    memory_samples = [
        sample
        for sample in (total_memory_bytes, available_memory_bytes)
        if sample is not None
    ]
    if not memory_samples:
        return cpus
    usable = max(0, min(memory_samples) - headroom_bytes)
    memory_workers = max(1, usable // bytes_per_worker)
    return max(1, min(cpus, memory_workers))


def _memory_bounded_cargo_jobs() -> int | None:
    """Cargo ``--jobs`` ceiling from the canonical live resource snapshot.

    Caps parallel rustc jobs by CPU, total physical memory, live available
    memory, and active Linux cgroup capacity. Returns ``None`` only when neither
    memory dimension can be observed, leaving Cargo's default untouched.
    """
    total_memory_bytes, available_memory_bytes = _system_memory_bytes()
    if total_memory_bytes is None and available_memory_bytes is None:
        return None
    return _memory_bounded_worker_count_from_samples(
        bytes_per_worker=_BYTES_PER_CARGO_JOB,
        headroom_bytes=_CARGO_JOB_MEMORY_HEADROOM,
        total_memory_bytes=total_memory_bytes,
        available_memory_bytes=available_memory_bytes,
    )


def _running_under_pytest(env: Mapping[str, str] | None = None) -> bool:
    source = os.environ if env is None else env
    return any(
        source.get(key)
        for key in (
            "PYTEST_CURRENT_TEST",
            "PYTEST_VERSION",
            "MOLT_PYTEST_OUTER_GUARD_REEXEC",
        )
    )


def _maybe_register_lane_target(ext_root: Path, target_dir: Path) -> None:
    """Best-effort: register an isolated per-lane target dir for TTL GC.

    So a completed lane's isolated ``CARGO_TARGET_DIR`` is garbage-collected by
    ``disk_guard --gc`` once it ages past the TTL, killing the accumulation at
    the source (the orchestrator never has to ``rm`` by hand). Never raises;
    gated by the same independent ``MOLT_DISABLE_DISK_GUARD`` flag.
    """
    try:
        if os.environ.get("MOLT_DISABLE_DISK_GUARD", "").strip().lower() in (
            "1",
            "true",
            "yes",
            "on",
        ):
            return
        if _running_under_pytest():
            return
        tools_dir = compiler_source_root() / "tools"
        if str(tools_dir.parent) not in sys.path:
            sys.path.insert(0, str(tools_dir.parent))
        from tools import disk_guard  # noqa: PLC0415 - lazy, best-effort

        disk_guard.register_lane_target(target_dir, root=str(ext_root))
    except Exception:
        pass


def _env_bool(
    env: Mapping[str, str],
    names: Collection[str],
    *,
    default: bool,
) -> bool:
    for name in names:
        raw = env.get(name)
        if raw is None:
            continue
        normalized = raw.strip().lower()
        if normalized in TRUE_VALUES:
            return True
        if normalized in FALSE_VALUES:
            return False
    return default


def _env_float(
    env: Mapping[str, str],
    name: str,
    *,
    default: float,
) -> float:
    raw = env.get(name, "").strip()
    if not raw:
        return default
    try:
        parsed = float(raw)
    except ValueError:
        return default
    return parsed if parsed >= 0 else default


def development_artifacts_requested(env: Mapping[str, str]) -> bool:
    """Return whether a development wrapper requested guarded artifact custody.

    This is intentionally a development control-plane predicate. Public compile
    paths keep Cargo/default output behavior unless the operator set one of
    these Molt development knobs or an explicit output/target flag.
    """

    return _env_bool(env, DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS, default=False)


def _looks_like_ambient_tmpdir(raw: str) -> bool:
    spelling = raw.strip().replace("\\", "/")
    if spelling in {"/tmp", "/var/tmp"} or spelling.startswith("/var/folders/"):
        return True
    lowered = spelling.lower()
    if (
        lowered.endswith("/appdata/local/temp")
        or "/appdata/local/temp/" in lowered
        or lowered in {"c:/windows/temp", "c:/temp", "c:/tmp"}
        or lowered.startswith("c:/windows/temp/")
        or lowered.startswith("c:/temp/")
        or lowered.startswith("c:/tmp/")
    ):
        return True
    normalized = str(Path(raw).expanduser()).rstrip(os.sep)
    return normalized in {"/tmp", "/var/tmp"} or normalized.startswith("/var/folders/")


def _drop_ambient_tmpdir(env: dict[str, str], *, prefer_external: bool) -> None:
    if not prefer_external:
        return
    if _env_bool(env, ("MOLT_PRESERVE_AMBIENT_TMPDIR",), default=False):
        return
    for key in ("TMPDIR", "TMP", "TEMP"):
        raw = env.get(key)
        if raw and _looks_like_ambient_tmpdir(raw):
            env.pop(key, None)


def _dedupe_paths(paths: list[Path]) -> tuple[Path, ...]:
    seen: set[str] = set()
    deduped: list[Path] = []
    for path in paths:
        key = os.path.normcase(str(path))
        if key in seen:
            continue
        seen.add(key)
        deduped.append(path)
    return tuple(deduped)


def _default_external_artifact_roots(
    repo_root: Path, env: Mapping[str, str] | None = None
) -> tuple[Path, ...]:
    """Return the one automatic artifact root: the checkout family's custody root.

    The rule is the same on every OS. A checkout family (`<root>/molt-src` and
    `<root>/worktrees/<name>`) keeps build artifacts under `<root>`. Any other
    location is an explicit `MOLT_EXTERNAL_ARTIFACT_ROOTS` choice: volume names,
    labels, and free-space ranking never select a root by themselves, and never
    promote a removable volume into source, worktree, or toolchain authority.
    """
    custody = checkout_custody(repo_root, env, require_exists=False)
    root = custody.custody_root
    return (root,) if custody.source_only or root.is_dir() else ()


def _windows_volume_info(drive_root: Path) -> tuple[str | None, str | None]:
    if os.name != "nt":
        return None, None
    try:
        import ctypes

        label = ctypes.create_unicode_buffer(261)
        fs_name = ctypes.create_unicode_buffer(261)
        serial = ctypes.c_ulong()
        max_component_len = ctypes.c_ulong()
        flags = ctypes.c_ulong()
        ok = ctypes.windll.kernel32.GetVolumeInformationW(
            str(drive_root),
            label,
            len(label),
            ctypes.byref(serial),
            ctypes.byref(max_component_len),
            ctypes.byref(flags),
            fs_name,
            len(fs_name),
        )
    except (AttributeError, OSError, ValueError):
        return None, None
    if not ok:
        return None, None
    return label.value, fs_name.value


def _windows_volume_label(drive_root: Path) -> str | None:
    return _windows_volume_info(drive_root)[0]


def _windows_volume_filesystem(drive_root: Path) -> str | None:
    return _windows_volume_info(drive_root)[1]


def _path_drive(path: Path) -> str:
    return path.drive.upper()


def _windows_drive_root_for_path(path: Path) -> Path:
    drive = _path_drive(path)
    if drive:
        return Path(f"{drive}\\")
    parent = _nearest_existing_parent(path) or path
    return Path(parent.anchor) if parent.anchor else parent


def _artifact_root_is_windows_exfat(artifact_root: Path) -> bool:
    if os.name != "nt":
        return False
    filesystem = _windows_volume_filesystem(_windows_drive_root_for_path(artifact_root))
    return filesystem is not None and filesystem.casefold() == "exfat"


def _path_is_within(path: Path, parent: Path) -> bool:
    return host_path_is_within(path, parent)


def git_checkout_head(repo_root: Path) -> str | None:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            timeout=30,
            encoding="utf-8",
        )
    except (OSError, subprocess.SubprocessError):
        return None
    head = proc.stdout.strip().lower()
    return (
        head if proc.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", head) else None
    )


def _github_actions_checkout_custody(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    require_exists: bool,
) -> CheckoutCustody | None:
    """Verify the complete hosted-checkout contract, or return ``None``.

    ``GITHUB_ACTIONS=true`` is deliberately insufficient. The workflow must
    issue a per-run custody root under GitHub's runner temp directory, and the
    reserved runner facts, event payload, workflow identity, workspace, and
    checked-out commit must agree. Any partial contract fails closed.
    """

    contract_raw = env.get(GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV, "").strip()
    if not contract_raw:
        return None

    required_exact = {
        "GITHUB_ACTIONS": "true",
        "CI": "true",
        "GITHUB_SERVER_URL": "https://github.com",
        "GITHUB_API_URL": "https://api.github.com",
    }
    for key, expected in required_exact.items():
        if env.get(key, "").strip() != expected:
            raise DxConfigError(
                f"{GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV} requires verified {key}={expected!r}"
            )

    source_root = repo_root.expanduser().resolve()
    verify_checkout_files = require_exists or source_root.is_dir()
    workspace_raw = env.get("GITHUB_WORKSPACE", "").strip()
    runner_temp_raw = env.get("RUNNER_TEMP", "").strip()
    if workspace_raw and not same_host_path(workspace_raw, source_root):
        # A hosted job's environment is process-global, but unit/integration
        # tests legitimately create synthetic projects beneath RUNNER_TEMP.
        # The hosted checkout contract belongs only to GITHUB_WORKSPACE; nested
        # runner scratch is explicitly non-canonical and resolves normally.
        if runner_temp_raw and host_path_is_within(source_root, runner_temp_raw):
            return None
    if not workspace_raw or not same_host_path(workspace_raw, source_root):
        raise DxConfigError(
            "GitHub Actions custody requires GITHUB_WORKSPACE to equal the source checkout"
        )

    github_repository = env.get("GITHUB_REPOSITORY", "").strip()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", github_repository):
        raise DxConfigError("GitHub Actions custody requires GITHUB_REPOSITORY")
    workflow_ref = env.get("GITHUB_WORKFLOW_REF", "").strip()
    prefix = f"{github_repository}/.github/workflows/"
    if not workflow_ref.startswith(prefix) or "@" not in workflow_ref[len(prefix) :]:
        raise DxConfigError("GitHub Actions custody requires a checked-in workflow ref")
    workflow_name, workflow_revision = workflow_ref[len(prefix) :].rsplit("@", 1)
    workflow_path = source_root / ".github" / "workflows" / workflow_name
    if (
        not workflow_revision.strip()
        or not re.fullmatch(r"[A-Za-z0-9_.-]+\.ya?ml", workflow_name)
        or (verify_checkout_files and not workflow_path.is_file())
    ):
        raise DxConfigError(f"invalid GitHub Actions workflow ref: {workflow_ref!r}")
    workflow_sha = env.get("GITHUB_WORKFLOW_SHA", "").strip().lower()
    if not re.fullmatch(r"[0-9a-f]{40}", workflow_sha):
        raise DxConfigError(
            "GitHub Actions custody requires a full GITHUB_WORKFLOW_SHA"
        )

    event_path_raw = env.get("GITHUB_EVENT_PATH", "").strip()
    try:
        event = json.loads(Path(event_path_raw).read_text(encoding="utf-8"))
        event_repository = event["repository"]["full_name"]
    except (KeyError, OSError, TypeError, ValueError) as exc:
        raise DxConfigError("GitHub Actions event provenance is unreadable") from exc
    if event_repository != github_repository:
        raise DxConfigError(
            f"GitHub Actions event repository mismatch: {event_repository!r}"
        )

    github_sha = env.get("GITHUB_SHA", "").strip().lower()
    if not re.fullmatch(r"[0-9a-f]{40}", github_sha):
        raise DxConfigError("GitHub Actions custody requires a full GITHUB_SHA")
    checkout_head = git_checkout_head(source_root)
    if verify_checkout_files and checkout_head != github_sha:
        raise DxConfigError(
            f"GitHub Actions checkout HEAD mismatch: expected {github_sha}, got {checkout_head}"
        )

    runner_temp = Path(runner_temp_raw).expanduser()
    custody_root = Path(contract_raw).expanduser()
    if not runner_temp_raw or not runner_temp.is_absolute():
        raise DxConfigError("GitHub Actions custody requires an absolute RUNNER_TEMP")
    if not custody_root.is_absolute():
        raise DxConfigError(
            f"{GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV} must be an absolute path"
        )
    runner_temp = runner_temp.resolve()
    custody_root = custody_root.resolve()
    if verify_checkout_files and not runner_temp.is_dir():
        raise DxConfigError(f"GitHub Actions RUNNER_TEMP does not exist: {runner_temp}")
    if custody_root == runner_temp or not _path_is_within(custody_root, runner_temp):
        raise DxConfigError(
            f"{GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV} must be a child of RUNNER_TEMP"
        )
    if _path_is_within(custody_root, source_root) or _path_is_within(
        source_root, custody_root
    ):
        raise DxConfigError(
            "GitHub Actions source checkout and custody roots must be disjoint"
        )

    for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"):
        if not env.get(key, "").strip().isdigit():
            raise DxConfigError(f"GitHub Actions custody requires numeric {key}")
    if not env.get("GITHUB_JOB", "").strip():
        raise DxConfigError("GitHub Actions custody requires GITHUB_JOB")
    for key in ("GITHUB_EVENT_NAME", "GITHUB_REF"):
        if not env.get(key, "").strip():
            raise DxConfigError(f"GitHub Actions custody requires {key}")
    expected_runner_os = {
        "nt": "Windows",
        "posix": "macOS" if sys.platform == "darwin" else "Linux",
    }.get(os.name)
    if env.get("RUNNER_OS", "").strip() != expected_runner_os:
        raise DxConfigError(
            f"GitHub Actions RUNNER_OS does not match this host: {env.get('RUNNER_OS')!r}"
        )
    expected_runner_arch = {
        "amd64": "X64",
        "x86_64": "X64",
        "aarch64": "ARM64",
        "arm64": "ARM64",
        "x86": "X86",
        "i386": "X86",
        "i686": "X86",
    }.get(platform.machine().lower())
    if (
        expected_runner_arch is None
        or env.get("RUNNER_ARCH", "").strip() != expected_runner_arch
    ):
        raise DxConfigError(
            "GitHub Actions RUNNER_ARCH does not match this host: "
            f"{env.get('RUNNER_ARCH')!r}"
        )

    # One per-run execution authority on every hosted OS.  RUNNER_TOOL_CACHE is
    # a runner-managed shared cache, not Molt custody; deriving Windows tools
    # from it created a second platform-only authority and incorrectly treated
    # its drive letter as durable project identity.
    toolchain_root = custody_root / DEFAULT_TARGET_ROOT_DIRNAME

    return CheckoutCustody(
        source_root=source_root,
        custody_root=custody_root,
        toolchain_root=toolchain_root,
        kind="github-actions-ephemeral",
        workflow_ref=workflow_ref,
    )


def canonical_molt_root(repo_root: str | Path, *, require_exists: bool = True) -> Path:
    """Return the single durable Molt custody root for this platform.

    The authority is derived from the invoking checkout family (``molt-src`` or
    a sibling under ``worktrees``). It never consults artifact-output
    environment, volume labels, free-space policy, or preservation switches.
    The same checkout-family rule applies on every filesystem; drive letters
    and directory names carry no custody authority.
    """
    root = custody_layout.custody_root(repo_root)
    if require_exists and not root.is_dir():
        raise DxConfigError(f"canonical Molt custody root does not exist: {root}")
    return root


def _host_scratch_roots() -> tuple[Path, ...]:
    """Return scratch roots issued by this host process, not child env input.

    ``RunContext`` accepts an explicit child environment, but that mapping is
    configuration rather than custody proof: it must neither erase the hosted
    runner's real temp root nor fabricate a scratch classification.  The Python
    temp authority is always host-local.  ``RUNNER_TEMP`` is additionally
    trusted only when the current process is itself running under GitHub
    Actions; a caller-supplied mapping cannot self-attest that fact.
    """

    roots = [Path(tempfile.gettempdir()).expanduser().resolve()]
    if (
        os.environ.get("GITHUB_ACTIONS", "").strip() == "true"
        and os.environ.get("CI", "").strip() == "true"
    ):
        raw = os.environ.get("RUNNER_TEMP", "").strip()
        candidate = Path(raw).expanduser() if raw else None
        if candidate is not None and candidate.is_absolute():
            resolved = candidate.resolve()
            if resolved not in roots:
                roots.append(resolved)
    return tuple(roots)


def checkout_custody(
    repo_root: Path,
    env: Mapping[str, str] | None = None,
    *,
    require_exists: bool = True,
) -> CheckoutCustody:
    """Resolve durable local or verified ephemeral hosted execution custody."""

    source_root = repo_root.expanduser().resolve()
    env_view = os.environ if env is None else env
    hosted = _github_actions_checkout_custody(
        source_root, env_view, require_exists=require_exists
    )
    if hosted is not None:
        return hosted
    scratch_roots = _host_scratch_roots()
    if any(host_path_is_within(source_root, root) for root in scratch_roots):
        # Projects beneath the OS-issued temp root have explicit scratch custody.
        return CheckoutCustody(
            source_root=source_root,
            custody_root=source_root,
            toolchain_root=source_root / DEFAULT_TARGET_ROOT_DIRNAME,
            kind="explicit-scratch",
        )
    durable_root = canonical_molt_root(source_root, require_exists=require_exists)
    return CheckoutCustody(
        source_root=source_root,
        custody_root=durable_root,
        toolchain_root=durable_root / DEFAULT_TARGET_ROOT_DIRNAME,
        kind="durable",
    )


def canonical_toolchain_root(repo_root: Path, *, require_exists: bool = True) -> Path:
    return (
        canonical_molt_root(repo_root, require_exists=require_exists)
        / DEFAULT_TARGET_ROOT_DIRNAME
    )


def _requires_external_artifacts(env: Mapping[str, str]) -> bool:
    return _env_bool(env, ("MOLT_REQUIRE_EXTERNAL_ARTIFACTS",), default=False)


def _require_external_path(
    key: str,
    path: Path,
    env: Mapping[str, str],
    *,
    repo_root: Path,
) -> None:
    if _requires_external_artifacts(env):
        if host_path_is_within(path, repo_root):
            raise DxConfigError(
                f"{key} must be outside the checkout when MOLT_REQUIRE_EXTERNAL_ARTIFACTS=1: {path}"
            )


def _candidate_roots(repo_root: Path, env: Mapping[str, str]) -> tuple[Path, ...]:
    raw = env.get(EXTERNAL_ARTIFACT_ROOTS_ENV, "")
    candidates = raw.split(os.pathsep) if raw.strip() else ()
    roots: list[Path] = []
    for candidate in candidates:
        text = candidate.strip()
        if not text:
            continue
        roots.append(Path(text).expanduser())
    return (
        _dedupe_paths(roots)
        if roots
        else _default_external_artifact_roots(repo_root, env)
    )


def _nearest_existing_parent(path: Path) -> Path | None:
    current = path
    while not current.exists():
        parent = current.parent
        if parent == current:
            return None
        current = parent
    return current if current.is_dir() else current.parent


def _artifact_root_accepts_child_dirs(path: Path, *, create_dirs: bool) -> bool:
    if not create_dirs:
        parent = _nearest_existing_parent(path)
        return parent is not None and os.access(parent, os.W_OK)
    probe = path / f".molt-write-probe-{os.getpid()}-{uuid.uuid4().hex}"
    try:
        path.mkdir(parents=True, exist_ok=True)
        probe.mkdir()
        list(probe.iterdir())
    except OSError:
        return False
    finally:
        try:
            shutil.rmtree(probe)
        except OSError:
            pass
    return True


def select_external_artifact_root(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    create_dirs: bool,
    prefer_external: bool,
) -> Path | None:
    """Return the first healthy external artifact root, or None for repo-local."""

    if env.get(ARTIFACT_ROOT_ENV, "").strip():
        return None
    require_external = _requires_external_artifacts(env)
    if (
        not _env_bool(
            env,
            (PREFER_EXTERNAL_ARTIFACTS_ENV,),
            default=prefer_external,
        )
        and not require_external
    ):
        return None

    min_free_gb = _env_float(env, "MOLT_EXTERNAL_MIN_FREE_GB", default=20.0)
    repo_root = repo_root.resolve()
    for raw_candidate in _candidate_roots(repo_root, env):
        candidate = (
            raw_candidate if raw_candidate.is_absolute() else repo_root / raw_candidate
        )
        candidate = candidate.resolve()
        if candidate == repo_root or repo_root in candidate.parents:
            continue
        parent = _nearest_existing_parent(candidate)
        if parent is None:
            continue
        try:
            usage = shutil.disk_usage(parent)
        except OSError:
            continue
        if usage.free < min_free_gb * 1024 * 1024 * 1024:
            continue
        if not _artifact_root_accepts_child_dirs(
            candidate,
            create_dirs=create_dirs,
        ):
            continue
        return candidate
    if require_external:
        raise DxConfigError(
            "no healthy Molt artifact root was found (each candidate needs "
            f"{min_free_gb:g} GiB free outside the checkout). Use a checkout family "
            "(`<root>/molt-src`, whose artifacts live under `<root>`) or set "
            "MOLT_EXTERNAL_ARTIFACT_ROOTS to an explicit root; see "
            "docs/agent/ORCHESTRATION.md canonical paths."
        )
    return None


def require_external_artifact_root(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    create_dirs: bool,
    prefer_external: bool,
) -> Path | None:
    selected = select_external_artifact_root(
        repo_root,
        env,
        create_dirs=create_dirs,
        prefer_external=prefer_external,
    )
    if selected is not None:
        return selected
    if _requires_external_artifacts(env):
        candidates = (
            ", ".join(str(path) for path in _candidate_roots(repo_root, env))
            or "<none>"
        )
        raise DxConfigError(
            "Molt build artifacts must be outside the checkout. Configure a healthy "
            "root with MOLT_EXTERNAL_ARTIFACT_ROOTS or MOLT_EXT_ROOT. "
            f"Checked candidates: {candidates}"
        )
    return None


def configured_artifact_root_text(env: Mapping[str, str] | None = None) -> str | None:
    """Return the operator's ``MOLT_EXT_ROOT`` text exactly, or None when unset.

    For cache keys on hot paths: it touches no filesystem. The cached consumer
    anchors and resolves the value once per distinct key.
    """

    view = os.environ if env is None else env
    raw = view.get(ARTIFACT_ROOT_ENV, "").strip()
    return raw or None


def configured_artifact_root(
    env: Mapping[str, str] | None = None, *, relative_to: Path
) -> Path | None:
    """Return the operator's ``MOLT_EXT_ROOT``, resolved, or None when unset.

    A relative value is anchored at ``relative_to``. Consumers with their own
    non-checkout default (the user cache home) use this; consumers that need
    the canonical root use `artifact_root`.
    """

    view = os.environ if env is None else env
    raw = view.get(ARTIFACT_ROOT_ENV, "").strip()
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = Path(relative_to) / path
    return path.resolve()


def _select_artifact_root(
    source_root: Path,
    env: Mapping[str, str],
    custody: CheckoutCustody,
    *,
    prefer_external: bool,
    create_dirs: bool,
) -> Path:
    if custody.source_only:
        return custody.custody_root
    return (
        require_external_artifact_root(
            source_root,
            env,
            create_dirs=create_dirs,
            prefer_external=prefer_external,
        )
        or custody.custody_root
    )


def artifact_root(
    repo_root: Path,
    env: Mapping[str, str] | None = None,
    *,
    prefer_external: bool = False,
) -> Path:
    """Return the artifact root `RunContext.canonical_env` exports for a run.

    That is ``MOLT_EXT_ROOT`` when set, else a healthy external root when the
    caller or the environment asks for one, else the checkout custody root:
    the family root of a checkout family, the clone itself for a plain clone.
    It creates nothing.
    """

    source = Path(repo_root).expanduser().resolve()
    view = os.environ if env is None else env
    root = configured_artifact_root(view, relative_to=source)
    if root is None:
        root = _select_artifact_root(
            source,
            view,
            checkout_custody(source, view, require_exists=False),
            prefer_external=prefer_external,
            create_dirs=False,
        )
    _require_external_path(ARTIFACT_ROOT_ENV, root, view, repo_root=source)
    return root


@dataclass(frozen=True, slots=True)
class ScratchStorage:
    """Where run scratch lives: disk, or an operator's memory-backed root."""

    memory_root: Path | None = None

    @property
    def mode(self) -> Literal["disk", "memory"]:
        return "disk" if self.memory_root is None else "memory"


def scratch_storage(env: Mapping[str, str] | None = None) -> ScratchStorage:
    """Parse ``MOLT_SCRATCH_STORAGE`` into the selected scratch storage."""

    view = os.environ if env is None else env
    raw = view.get(SCRATCH_STORAGE_ENV, "").strip()
    if raw in {"", "disk"}:
        return ScratchStorage()
    if raw == "memory":
        if not SHARED_MEMORY_ROOT.is_dir():
            raise DxConfigError(
                f"{SCRATCH_STORAGE_ENV}=memory uses {SHARED_MEMORY_ROOT}, which this "
                "host does not have. Mount a RAM disk and set "
                f"{SCRATCH_STORAGE_ENV} to its absolute path, or use disk."
            )
        return ScratchStorage(SHARED_MEMORY_ROOT.resolve())
    path = Path(raw).expanduser()
    if not path.is_absolute():
        raise DxConfigError(
            f"{SCRATCH_STORAGE_ENV} must be disk, memory, or the absolute path of "
            f"a memory-backed directory; got {raw!r}"
        )
    if not path.is_dir():
        raise DxConfigError(
            f"{SCRATCH_STORAGE_ENV} names {path}, which is not a directory. Molt "
            "never creates or mounts a RAM disk; mount it first."
        )
    return ScratchStorage(path.resolve())


def _scratch_root_for(
    artifact: Path, source_root: Path, env: Mapping[str, str]
) -> Path:
    storage = scratch_storage(env)
    if storage.memory_root is not None and host_path_is_within(
        storage.memory_root, source_root
    ):
        raise DxConfigError(
            f"{SCRATCH_STORAGE_ENV} must be outside the checkout: {storage.memory_root}"
        )
    return custody_layout.scratch_root(
        artifact, source_root, memory_root=storage.memory_root
    )


def scratch_root(repo_root: Path, env: Mapping[str, str] | None = None) -> Path:
    """Return the run scratch root for a checkout, never inside it.

    Disk storage puts scratch at ``<artifact root>/tmp``; memory storage puts
    it under the operator's memory-backed root. `RunContext.canonical_env`
    exports the same root as ``TMPDIR``.
    """

    source = Path(repo_root).expanduser().resolve()
    view = os.environ if env is None else env
    return _scratch_root_for(artifact_root(source, view), source, view)


def _purpose_parts(purpose: str) -> tuple[str, ...]:
    parts = tuple(purpose.split("/"))
    if not purpose or any(part in {"", ".", ".."} or "\\" in part for part in parts):
        raise ValueError(f"scratch purpose must be a relative name: {purpose!r}")
    return parts


def scratch_dir(
    repo_root: Path, purpose: str, env: Mapping[str, str] | None = None
) -> Path:
    """Return the scratch directory for one purpose, such as ``"bench"``.

    ``purpose`` is a relative POSIX path of plain names. The directory follows
    the selected scratch storage and is not created.
    """

    return scratch_root(repo_root, env).joinpath(*_purpose_parts(purpose))


def control_state_dir(
    repo_root: Path, purpose: str, env: Mapping[str, str] | None = None
) -> Path:
    """Return the directory for control state every process must agree on.

    Locks, guard markers and build control stay in the artifact root's disk
    scratch whatever scratch storage is selected, so a process with memory
    scratch and one with disk scratch see the same locks and markers. The
    directory is not created.
    """

    source = Path(repo_root).expanduser().resolve()
    view = os.environ if env is None else env
    disk_scratch = custody_layout.scratch_root(artifact_root(source, view), source)
    return disk_scratch.joinpath(*_purpose_parts(purpose))


def _backend_daemon_socket_root(env: Mapping[str, str]) -> Path:
    raw = env.get("MOLT_BACKEND_DAEMON_SOCKET_ROOT", "").strip()
    if raw:
        return Path(raw).expanduser()
    for key in ("TMPDIR", "TMP", "TEMP"):
        raw = env.get(key, "").strip()
        if raw:
            return Path(raw).expanduser()
    if os.name == "nt":
        return Path(tempfile.gettempdir())
    return Path("/tmp")


def backend_daemon_socket_dir(repo_root: Path, env: Mapping[str, str]) -> Path:
    """Resolve the short local backend-daemon socket directory for this checkout."""

    root_hash = hashlib.sha256(str(repo_root.resolve()).encode()).hexdigest()[:12]
    return (_backend_daemon_socket_root(env) / f"molt-backend-{root_hash}").resolve()


# The sccache release is pinned in config/tool_releases.toml. Molt never
# downloads a tool on its own: the pinned release is used only after
# `python -m molt.tool_releases provision sccache` installed it.
_sccache_degrade_warned = False


def pinned_sccache(env: Mapping[str, str]) -> str | None:
    """The pinned sccache already provisioned under ``MOLT_TARGET_ROOT``."""
    raw_target_root = env.get("MOLT_TARGET_ROOT", "").strip()
    if not raw_target_root:
        return None
    from molt import tool_releases

    discovery = tool_releases.discover_tool(
        tool_releases.tool_release("sccache"), Path(raw_target_root).expanduser()
    )
    return None if discovery is None else str(discovery.executable)


def _ensure_sccache_wrapper(env: dict[str, str]) -> None:
    """Wire ``RUSTC_WRAPPER=sccache`` for content-addressed, cross-worktree-shared
    rustc caching across EVERY DX/proof build path. When the pinned release is
    not provisioned, DEGRADE LOUDLY (cold builds saturate memory under parallel
    lanes) and name the command that provisions it; never download it here.
    Respects an explicit pre-set RUSTC_WRAPPER (e.g. benchmarks)."""
    global _sccache_degrade_warned
    if env.get("RUSTC_WRAPPER"):
        return
    mode = env.get("MOLT_USE_SCCACHE", "auto").strip().lower()
    if mode in {"0", "false", "no", "off"}:
        return
    # Windows: sccache delivers 0 cache hits here and crashes builds mid-compile
    # (os error 10054), so "auto" must NOT provision/wire it (that would be a
    # NEGATIVE-leverage cache). Only an explicit MOLT_USE_SCCACHE=1 forces it.
    if os.name == "nt" and mode not in {"1", "true", "yes", "on"}:
        if not _sccache_degrade_warned:
            _sccache_degrade_warned = True
            print(
                "molt: sccache disabled by default on Windows (0 cache hits + "
                "mid-compile crashes here); using direct rustc. Set "
                "MOLT_USE_SCCACHE=1 to force.",
                file=sys.stderr,
                flush=True,
            )
        return
    sccache = pinned_sccache(env)
    if sccache is None:
        if not _sccache_degrade_warned:
            _sccache_degrade_warned = True
            print(
                "molt: WARNING the pinned sccache is not provisioned under "
                "MOLT_TARGET_ROOT, so the Rust compilation cache is OFF; builds "
                "will be COLD and memory-heavy (every worktree recompiles the full "
                "crate graph, which saturates memory under parallel lanes). "
                "Provision it with `python -m molt.tool_releases provision sccache` "
                "or set MOLT_USE_SCCACHE=0 to silence.",
                file=sys.stderr,
                flush=True,
            )
        return
    env["RUSTC_WRAPPER"] = sccache
    # sccache silently SKIPS incremental compilation units — without this the
    # wrapper we just wired would cache nothing on paths that default incremental
    # on (e.g. the proof-queue lane). Force it off wherever sccache is enabled.
    env["CARGO_INCREMENTAL"] = "0"


def _install_dx_defaults(repo_root: Path, env: dict[str, str]) -> None:
    ext_root = Path(env[ARTIFACT_ROOT_ENV]).expanduser()
    env.setdefault(
        "MOLT_BACKEND_DAEMON_SOCKET_DIR",
        str(backend_daemon_socket_dir(repo_root, env)),
    )
    # sccache off-by-default on Windows: measured 0 cache hits + mid-compile
    # crashes (os error 10054) that time builds out. Power users force it with
    # MOLT_USE_SCCACHE=1; cargo_execution also treats "auto" as off-on-Windows.
    env.setdefault("MOLT_USE_SCCACHE", "0" if os.name == "nt" else "1")
    env.setdefault("MOLT_DIFF_ALLOW_RUSTC_WRAPPER", "1")
    env.setdefault("SCCACHE_DIR", str((ext_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env.setdefault("MOLT_CACHE_MAX_GB", DEFAULT_MOLT_CACHE_MAX_GB)
    env.setdefault("MOLT_CACHE_MAX_AGE_DAYS", DEFAULT_MOLT_CACHE_MAX_AGE_DAYS)
    _ensure_sccache_wrapper(env)
    if _artifact_root_is_windows_exfat(ext_root):
        env.setdefault("UV_LINK_MODE", "copy")


def _host_facts() -> dict[str, str]:
    return {
        "os": platform.system().lower() or os.name,
        "platform": sys.platform,
        "arch": platform.machine().lower(),
        "python": platform.python_version(),
    }


def dx_env_payload(env: Mapping[str, str], keys: Sequence[str]) -> dict[str, object]:
    return {
        "schema_version": "1.0",
        "kind": "molt_dx_env",
        "host": _host_facts(),
        "keys": list(keys),
        "env": {key: env[key] for key in keys if key in env},
    }


def _posix_quote(value: str) -> str:
    escaped = (
        value.replace("\\", "\\\\")
        .replace('"', '\\"')
        .replace("$", "\\$")
        .replace("`", "\\`")
    )
    return f'"{escaped}"'


def _powershell_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _cmd_quote(value: str) -> str:
    return value.replace("^", "^^").replace("&", "^&").replace("|", "^|")


EnvRenderFormat = Literal["dotenv", "posix", "powershell", "cmd", "json"]


_POSIX_ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def render_env(
    env: Mapping[str, str], keys: Sequence[str], fmt: EnvRenderFormat
) -> str:
    present = [(key, env[key]) for key in keys if key in env]
    if fmt == "json":
        return json.dumps(dx_env_payload(env, keys), indent=2, sort_keys=True)
    if fmt == "posix":
        # A POSIX shell cannot name a variable such as `CC_wasm32-wasip1`.
        # Such a key is exported only through its underscore spelling, which
        # its consumers (cc-rs) also read; a key without that twin is refused
        # rather than silently dropped.
        names = {key for key, _ in present}
        exported = []
        for key, value in present:
            if _POSIX_ENV_NAME.fullmatch(key):
                exported.append(f"export {key}={_posix_quote(value)}")
            elif key.replace("-", "_") not in names:
                raise DxConfigError(
                    f"{key} cannot be exported by a POSIX shell and has no "
                    "underscore spelling to carry its value"
                )
        return "\n".join(exported)
    if fmt == "powershell":
        # The braced form names any environment variable, hyphens included.
        return "\n".join(
            f"${{env:{key}}} = {_powershell_quote(value)}"
            if not _POSIX_ENV_NAME.fullmatch(key)
            else f"$env:{key} = {_powershell_quote(value)}"
            for key, value in present
        )
    if fmt == "cmd":
        return "\n".join(f'set "{key}={_cmd_quote(value)}"' for key, value in present)
    return "\n".join(f"{key}={value}" for key, value in present)


class RunContext:
    """Canonical artifact roots and session identity for dev subprocesses."""

    def __init__(
        self,
        root: Path,
        *,
        session_prefix: str = "dev",
        prefer_external_artifacts: bool = False,
    ) -> None:
        self.root = root.expanduser().resolve()
        self.session_prefix = session_prefix
        self.prefer_external_artifacts = prefer_external_artifacts

    def _resolve_env_path(self, raw: str) -> Path:
        path = Path(raw).expanduser()
        if not path.is_absolute():
            path = self.root / path
        return path.resolve()

    def uv_project_env_dir(self, env: Mapping[str, str]) -> Path:
        explicit = env.get("UV_PROJECT_ENVIRONMENT", "").strip()
        if explicit:
            return self._resolve_env_path(explicit)
        ext_root = artifact_root(
            self.root, env, prefer_external=self.prefer_external_artifacts
        )
        return stable_uv_project_env_from_env(env, ext_root, self.root)

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
        force_default_keys: Collection[str] = (),
    ) -> dict[str, str]:
        env = dict(os.environ if base is None else base)
        try:
            check_process_environment(env, program="molt-dev")
        except EnvironmentRegistryError as exc:
            raise DxConfigError(str(exc)) from exc
        _drop_ambient_tmpdir(env, prefer_external=self.prefer_external_artifacts)
        forced = set(force_default_keys)
        custody = checkout_custody(self.root, env)

        if custody.source_only:
            for key in CANONICAL_ROOT_ENV_KEYS:
                raw = env.get(key, "").strip()
                if raw and _path_is_within(self._resolve_env_path(raw), self.root):
                    raise DxConfigError(
                        f"verified ephemeral checkout cannot own {key}: {raw}. "
                        f"Use {GITHUB_ACTIONS_EPHEMERAL_ROOT_ENV} custody instead."
                    )

        if ARTIFACT_ROOT_ENV in forced:
            ext_root = custody.custody_root
        else:
            ext_root = configured_artifact_root(
                env, relative_to=self.root
            ) or _select_artifact_root(
                self.root,
                env,
                custody,
                prefer_external=self.prefer_external_artifacts,
                create_dirs=create_dirs,
            )
        _require_external_path(
            ARTIFACT_ROOT_ENV,
            ext_root,
            env,
            repo_root=self.root,
        )
        env[ARTIFACT_ROOT_ENV] = str(ext_root)

        def install_default(key: str, value: Path | str) -> None:
            if key in forced or not env.get(key):
                env[key] = str(value)

        # Session-scope the Cargo target dir ONLY when the caller PINNED an explicit
        # MOLT_SESSION_ID (perf/bench/test-shard isolation, e.g. perf_scoreboard,
        # bench_*, molt_dev difftest, development_artifact_env(session_id=...)). The
        # common interactive/CI build path leaves it unset -> a STABLE persistent
        # target dir on the fast external volume, so incremental compilation
        # artifacts SURVIVE across sessions/processes instead of paying a full cold
        # compile every invocation. Cargo's own build lock plus the compiler-build
        # resource mutex serialize concurrent writers safely. This is the
        # cold-every-session killer; do not reintroduce a per-PID default here.
        session_pinned = "MOLT_SESSION_ID" in forced or (
            bool(env.get("MOLT_SESSION_ID")) and not generated_session_id(env)
        )
        if "MOLT_SESSION_ID" in forced or not env.get("MOLT_SESSION_ID"):
            env["MOLT_SESSION_ID"] = f"{self.session_prefix}-{os.getpid()}"
            env["MOLT_SESSION_ID_GENERATED"] = "1"
        elif session_pinned:
            env.pop("MOLT_SESSION_ID_GENERATED", None)
        target_session_id = env["MOLT_SESSION_ID"] if session_pinned else None
        install_default(
            "CARGO_TARGET_DIR",
            cargo_target_dir_for_artifact_root(ext_root, target_session_id),
        )
        if create_dirs and target_session_id is not None:
            # This is an ISOLATED per-lane target dir; register it so a completed
            # lane's dir is TTL-garbage-collected without a manual rm (item 4).
            _maybe_register_lane_target(Path(ext_root), Path(env["CARGO_TARGET_DIR"]))
        install_default("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
        # Incremental ON by default (fast warm rebuilds against the persistent
        # per-artifact-root CARGO_TARGET_DIR above). _ensure_sccache_wrapper forces
        # it back to "0" wherever it actually enables sccache (mutually exclusive).
        install_default("CARGO_INCREMENTAL", "1")
        install_default("MOLT_CACHE", ext_root / ".molt_cache")
        run_scratch = _scratch_root_for(ext_root, self.root, env)
        install_default("MOLT_DIFF_ROOT", run_scratch / "diff")
        install_default("MOLT_DIFF_TMPDIR", run_scratch)
        install_default("UV_CACHE_DIR", ext_root / ".uv-cache")
        install_default("UV_PROJECT_ENVIRONMENT", self.uv_project_env_dir(env))
        install_default("PIP_CACHE_DIR", ext_root / ".pip-cache")
        install_default("RUFF_CACHE_DIR", ext_root / ".ruff-cache")
        # MOLT_TARGET_ROOT is durable toolchain custody, not scratch capacity.
        # Keep it on the canonical Molt root even when build outputs are routed
        # elsewhere explicitly.
        default_toolchain_root = custody.toolchain_root
        raw_target_root = env.get("MOLT_TARGET_ROOT")
        if not raw_target_root:
            env["MOLT_TARGET_ROOT"] = str(default_toolchain_root)
        install_default("PYTHONPYCACHEPREFIX", run_scratch / "pycache")
        install_default("TMPDIR", run_scratch)
        install_default("TMP", env["TMPDIR"])
        install_default("TEMP", env["TMPDIR"])

        for key in CANONICAL_ROOT_ENV_KEYS:
            value = env.get(key)
            if value:
                env[key] = str(self._resolve_env_path(value))
                value = env[key]
                if key == "MOLT_TARGET_ROOT":
                    continue  # Toolchain custody is independent of artifact placement.
                _require_external_path(
                    key,
                    Path(value).expanduser(),
                    env,
                    repo_root=self.root,
                )

        if create_dirs:
            for key in CANONICAL_ROOT_ENV_KEYS:
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env

    def dx_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
        force_default_keys: Collection[str] = (),
    ) -> dict[str, str]:
        env = self.canonical_env(
            base,
            create_dirs=create_dirs,
            force_default_keys=force_default_keys,
        )
        _install_dx_defaults(self.root, env)
        if create_dirs:
            for key in ("MOLT_BACKEND_DAEMON_SOCKET_DIR", "SCCACHE_DIR"):
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env


def development_artifact_env(
    repo_root: Path,
    base: Mapping[str, str] | None = None,
    *,
    session_prefix: str = "dev",
    session_id: str | None = None,
    create_dirs: bool = True,
) -> dict[str, str]:
    """Resolve Molt developer build/cache/temp roots through the DX authority."""

    env = dict(os.environ if base is None else base)
    if session_id:
        inherited_generated_session = generated_session_id(env) and (
            env.get("MOLT_SESSION_ID", "").strip() == session_id
        )
        env["MOLT_SESSION_ID"] = session_id
        if not inherited_generated_session:
            # A genuinely different explicit API argument supersedes provenance
            # inherited from an outer guard.  Re-passing that guard's identical
            # generated ID is propagation, not a request for shard isolation.
            env.pop("MOLT_SESSION_ID_GENERATED", None)
    env = RunContext(
        repo_root,
        session_prefix=session_prefix,
        prefer_external_artifacts=True,
    ).dx_env(env, create_dirs=create_dirs)
    ensure_repo_src_pythonpath(repo_root, env)
    return env


def ensure_repo_src_pythonpath(repo_root: Path, env: dict[str, str]) -> None:
    src = repo_root.resolve() / "src"
    existing = env.get("PYTHONPATH", "")
    parts = [part for part in existing.split(os.pathsep) if part]
    if str(src) not in parts:
        env["PYTHONPATH"] = str(src) if not existing else f"{src}{os.pathsep}{existing}"


def bind_repo_src_pythonpath(repo_root: Path, env: dict[str, str]) -> None:
    """Make one repository source tree the complete import-path authority."""

    env["PYTHONPATH"] = str(repo_root.resolve() / "src")


class DxProject:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()

    @classmethod
    def from_current_repo(cls) -> "DxProject":
        return cls(compiler_source_root())

    def load_config(self) -> dict[str, object]:
        pyproject = self.root / "pyproject.toml"
        if not pyproject.exists():
            return {}
        with pyproject.open("rb") as fh:
            data = tomllib.load(fh)
        tool = data.get("tool", {})
        if not isinstance(tool, dict):
            return {}
        molt = tool.get("molt", {})
        if not isinstance(molt, dict):
            return {}
        dx = molt.get("dx", {})
        return dx if isinstance(dx, dict) else {}

    def commands(self) -> dict[str, object]:
        commands = self.load_config().get("commands", {})
        return cast(dict[str, object], commands) if isinstance(commands, dict) else {}

    def project_env_dir(self) -> Path:
        return self.root / ".venv"

    def uv_project_env_dir(self, env: Mapping[str, str]) -> Path:
        explicit = env.get("UV_PROJECT_ENVIRONMENT", "").strip()
        if explicit:
            path = Path(explicit).expanduser()
            if not path.is_absolute():
                path = self.root / path
            return path.resolve()
        ext_root = artifact_root(
            self.root,
            env,
            prefer_external=bool(self.load_config().get("prefer_external_artifacts")),
        )
        return stable_uv_project_env_from_env(env, ext_root, self.root)

    def project_python(self, env: Mapping[str, str] | None = None) -> Path:
        if env is not None:
            project_env = self.uv_project_env_dir(env)
            if os.name == "nt":
                return project_env / "Scripts" / "python.exe"
            return project_env / "bin" / "python3"
        if os.name == "nt":
            return self.project_env_dir() / "Scripts" / "python.exe"
        return self.project_env_dir() / "bin" / "python3"

    def normalized_uv_run_env(
        self,
        env: Mapping[str, str],
        *,
        python: str | None,
        project_env_matches_python: bool | None = None,
    ) -> dict[str, str]:
        run_env = dict(env)
        run_env.setdefault("PYTHONUNBUFFERED", "1")
        run_env["UV_PROJECT_ENVIRONMENT"] = str(self.uv_project_env_dir(run_env))
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            run_env.pop(name, None)
        if run_env.get("UV_NO_SYNC") == "1":
            env_matches = project_env_matches_python
            if env_matches is None:
                raise DxConfigError(
                    "UV_NO_SYNC normalization requires a guarded project "
                    "Python version probe result"
                )
            if not env_matches:
                run_env.pop("UV_NO_SYNC", None)
        return run_env

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
    ) -> dict[str, str]:
        dx = self.load_config()
        env = dict(os.environ if base is None else base)
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            env.pop(name, None)
        prefer_external = bool(dx.get("prefer_external_artifacts"))
        _drop_ambient_tmpdir(env, prefer_external=prefer_external)
        ext_root = configured_artifact_root(env, relative_to=self.root) or (
            require_external_artifact_root(
                self.root,
                env,
                create_dirs=create_dirs,
                prefer_external=prefer_external,
            )
            or self.root
        )
        _require_external_path(
            ARTIFACT_ROOT_ENV,
            ext_root,
            env,
            repo_root=self.root,
        )
        env_cfg = dx.get("env", {})
        if isinstance(env_cfg, dict):
            for key, raw_value in env_cfg.items():
                if not isinstance(key, str) or not isinstance(raw_value, str):
                    continue
                if key in SCRATCH_ENV_KEYS:
                    raise DxConfigError(
                        f"[tool.molt.dx.env] must not set {key}: scratch roots are "
                        "derived by molt.custody_layout.scratch_root, which keeps "
                        "them out of the checkout. Remove the entry."
                    )
                if key in CANONICAL_RUN_ENV_KEYS and env.get(key):
                    continue
                value = raw_value.format(
                    root=str(self.root),
                    artifact_root=str(ext_root),
                )
                if key in CANONICAL_ROOT_ENV_KEYS or key == "PYTHONPATH":
                    value = str(Path(value).expanduser().resolve())
                env[key] = value
        env = RunContext(
            self.root,
            session_prefix="dev",
            prefer_external_artifacts=prefer_external,
        ).canonical_env(
            env,
            create_dirs=create_dirs,
        )
        ensure_repo_src_pythonpath(self.root, env)
        env.setdefault("MOLT_SESSION_ID", f"dev-{os.getpid()}")
        env.setdefault("MOLT_BACKEND_DAEMON", "1" if dx.get("backend_daemon") else "0")
        # Do NOT hardcode a conservative fixed job count here — it poisons the
        # session env and DEFEATS the adaptive memory-bounded ceiling (a fixed 2
        # ran a 24-core/34GB box at ~2 jobs instead of 14). Honor an explicit
        # config value; otherwise use the memory-fit adaptive count (scales up on
        # capable boxes, still safe on 8GB), leaving it unset only if RAM can't be
        # probed (then the build path's own _apply_memory_bounded_cargo_jobs runs).
        jobs_cfg = dx.get("cargo_build_jobs")
        if jobs_cfg is None:
            jobs_cfg = _memory_bounded_cargo_jobs()
        if jobs_cfg is not None:
            env.setdefault("CARGO_BUILD_JOBS", str(jobs_cfg))
        return env

    def dx_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
    ) -> dict[str, str]:
        env = self.canonical_env(base, create_dirs=create_dirs)
        _install_dx_defaults(self.root, env)
        if create_dirs:
            for key in ("MOLT_BACKEND_DAEMON_SOCKET_DIR", "SCCACHE_DIR"):
                value = env.get(key)
                if value:
                    Path(value).expanduser().mkdir(parents=True, exist_ok=True)
        return env

    def require_project_python(
        self,
        context: str,
        env: Mapping[str, str] | None = None,
    ) -> Path:
        python = self.project_python(env)
        if not python.exists():
            raise DxConfigError(
                f"{python} is missing; run `tools/dev.py install` before {context}"
            )
        return python

    def format_command(
        self,
        command: str,
        env: Mapping[str, str] | None = None,
    ) -> str:
        return command.format(
            root=str(self.root),
            project_python=str(self.project_python(env)),
        )

    def split_command(
        self,
        command: object,
        name: str,
        env: Mapping[str, str] | None = None,
    ) -> list[str]:
        if not isinstance(command, str) or not command.strip():
            raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")
        return shlex.split(self.format_command(command, env), posix=os.name != "nt")

    def split_command_sequence(
        self,
        command: object,
        name: str,
        *,
        env: Mapping[str, str] | None = None,
        commands: dict[str, object] | None = None,
        stack: tuple[str, ...] = (),
    ) -> list[list[str]]:
        commands = self.commands() if commands is None else commands

        def split_item(item: str, item_name: str) -> list[list[str]]:
            stripped = item.strip()
            if stripped.startswith("@"):
                ref = stripped[1:]
                if not ref or any(ch.isspace() for ch in ref):
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{item_name} reference: {item!r}"
                    )
                if ref in stack:
                    chain = " -> ".join((*stack, ref))
                    raise DxConfigError(
                        f"Cyclic [tool.molt.dx.commands] reference: {chain}"
                    )
                if ref not in commands:
                    raise DxConfigError(
                        f"Missing [tool.molt.dx.commands].{ref} referenced by {item_name}"
                    )
                return self.split_command_sequence(
                    commands[ref],
                    ref,
                    env=env,
                    commands=commands,
                    stack=(*stack, ref),
                )
            return [self.split_command(item, item_name, env)]

        if isinstance(command, str):
            return split_item(command, name)
        if isinstance(command, list) and command:
            split: list[list[str]] = []
            for idx, item in enumerate(command):
                if not isinstance(item, str) or not item.strip():
                    raise DxConfigError(
                        f"Invalid [tool.molt.dx.commands].{name}[{idx}]: "
                        "expected command string"
                    )
                split.extend(split_item(item, f"{name}[{idx}]"))
            return split
        raise DxConfigError(f"Missing [tool.molt.dx.commands].{name}")


def proof_scratch_root(repo_root: Path, env: Mapping[str, str] | None = None) -> Path:
    """Where a tool run from ``repo_root`` may write its outputs.

    The proof queue gives every guarded run one fresh scratch root in
    ``MOLT_PROOF_SCRATCH_ROOT``; a direct run uses `scratch_root`. Neither is
    under the source checkout, so a tool's outputs never register as
    mutations of the inputs it is proven from.
    """
    source = os.environ if env is None else env
    scratch = str(source.get(PROOF_SCRATCH_ROOT_ENV, "")).strip()
    if scratch:
        return Path(scratch).expanduser().resolve()
    return scratch_root(repo_root, source)
