from __future__ import annotations

from collections.abc import Collection
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
import time
import tomllib
import uuid
from pathlib import Path
from typing import Literal, Mapping, Sequence, cast


TEST_PYTHONS = ["3.12", "3.13", "3.14"]
CANONICAL_ROOT_ENV_KEYS = (
    "MOLT_EXT_ROOT",
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
CANONICAL_RUN_ENV_KEYS = (
    *CANONICAL_ROOT_ENV_KEYS,
    "CARGO_INCREMENTAL",
    "MOLT_SESSION_ID",
    "MOLT_ALLOW_C_DRIVE_ARTIFACTS",
)
DX_ENV_KEYS = (
    *CANONICAL_RUN_ENV_KEYS,
    "MOLT_BACKEND_DAEMON_SOCKET_DIR",
    "MOLT_USE_SCCACHE",
    "MOLT_DIFF_ALLOW_RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "MOLT_CACHE_MAX_GB",
    "MOLT_CACHE_MAX_AGE_DAYS",
    "UV_LINK_MODE",
)
DEFAULT_POSIX_EXTERNAL_ARTIFACT_ROOTS = (
    "/Volumes/APDataStore/Molt",
    "/Volumes/VertigoDataTier/Molt",
)
DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME = "Molt"
DEFAULT_WINDOWS_PRIMARY_ARTIFACT_ROOT = Path("C:/Molt")
LEGACY_WINDOWS_MOLT_ROOTS = (
    r"D:\Molt",
    r"E:\Molt",
    r"D:\molt-target",
    r"E:\molt-target",
)
# The maintainer/agent artifact root prefers C:\Molt, then fallback volumes
# selected by VOLUME LABEL, never by drive-letter order. The deleted
# letter-order scan silently picked E:\Molt (the contended legacy drive).
# Explicit MOLT_EXTERNAL_ARTIFACT_ROOTS remains the escape hatch for any other
# fallback volume.
DEFAULT_WINDOWS_PREFERRED_VOLUME_LABELS = ("APDataStore",)
# Toolchain root (wasi-sysroot / binaryen / zig) is DERIVED from the selected
# artifact root so MOLT_TARGET_ROOT follows the one root authority instead of
# stranding a legacy E:\molt-target or D:\molt-target.
DEFAULT_TARGET_ROOT_DIRNAME = "target-root"
LEGACY_WINDOWS_TARGET_ROOT_DIRNAME = "molt-target"
DEFAULT_SCCACHE_CACHE_SIZE = "10G"
DEFAULT_MOLT_CACHE_MAX_GB = "30"
DEFAULT_MOLT_CACHE_MAX_AGE_DAYS = "30"
DEVELOPMENT_ARTIFACT_REQUEST_ENV_KEYS = (
    "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
    "MOLT_PREFER_EXTERNAL_ARTIFACTS",
    "MOLT_USE_EXTERNAL_ARTIFACTS",
)
TRUE_VALUES = {"1", "true", "yes", "on"}
FALSE_VALUES = {"0", "false", "no", "off"}


class DxConfigError(RuntimeError):
    pass


def session_artifact_component(session_id: str) -> str:
    return "".join(c if c.isalnum() or c in "-_" else "_" for c in session_id)[:32]


def uv_project_env_component(value: str) -> str:
    component = re.sub(r"[^A-Za-z0-9_.-]+", "-", value.strip()).strip("-._")
    return component or "default"


def stable_uv_project_env_dir(
    artifact_root: Path,
    *,
    purpose: str,
    python: str,
) -> Path:
    name = f"{uv_project_env_component(purpose)}__py{uv_project_env_component(python)}"
    return (
        artifact_root.expanduser().resolve() / "tmp" / "uv-project-envs" / name
    ).resolve()


# The uv project environment (installed deps + editable molt) is a pure function
# of (project source, python) — NOT of the session — so it is stable and shared
# across sessions by default; only the Cargo target dir is session-scoped (build
# isolation). Session-scoping the uv env too churns a fresh `.venv` per proof and
# was the DX lock-churn source. `MOLT_UV_PROJECT_ENV_SESSION_SCOPED` is the opt-in
# for the rare case that genuinely needs an isolated uv env.
DEFAULT_UV_PROJECT_PURPOSE = "dx"
DEFAULT_UV_PROJECT_PYTHON = "3.12"


def uv_project_env_session_scoped(env: Mapping[str, str]) -> bool:
    return env.get("MOLT_UV_PROJECT_ENV_SESSION_SCOPED", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }


def stable_uv_project_env_from_env(env: Mapping[str, str], artifact_root: Path) -> Path:
    return stable_uv_project_env_dir(
        artifact_root,
        purpose=env.get("MOLT_UV_PROJECT_PURPOSE") or DEFAULT_UV_PROJECT_PURPOSE,
        python=env.get("MOLT_UV_PROJECT_PYTHON") or DEFAULT_UV_PROJECT_PYTHON,
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


def _total_system_memory_bytes() -> int | None:
    """Best-effort total physical memory in bytes, or ``None`` if unknown.

    Uses only stdlib probes so the resource authority does not depend on
    ``psutil`` or the ``tools/`` memory-guard package (a layering boundary).
    """
    if os.name == "nt":
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
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
                return int(status.ullTotalPhys)
        except (OSError, AttributeError, ValueError):
            return None
        return None
    try:
        page_size = os.sysconf("SC_PAGE_SIZE")
        phys_pages = os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None
    if page_size <= 0 or phys_pages <= 0:
        return None
    return int(page_size) * int(phys_pages)


def _memory_bounded_cargo_jobs() -> int | None:
    """Cargo ``--jobs`` ceiling that fits total memory, or ``None`` if unknown.

    Caps parallel rustc jobs to roughly one per ``_BYTES_PER_CARGO_JOB`` of total
    RAM, never exceeding the CPU count. Returns ``None`` when memory can't be
    probed so callers leave cargo's default job count untouched.
    """
    total = _total_system_memory_bytes()
    if total is None:
        return None
    cpu_count = os.cpu_count() or 1
    usable = max(0, total - _CARGO_JOB_MEMORY_HEADROOM)
    mem_jobs = max(1, usable // _BYTES_PER_CARGO_JOB)
    return max(1, min(cpu_count, mem_jobs))


# Fire the SSD janitor at most once per this many hours per artifact root, so the
# molt volume stays tidy BY DEFAULT wherever it runs — stale per-session cargo
# targets / tmp / scratch never pile up again (they hit 881 dirs before this).
_JANITOR_THROTTLE_HOURS = 6.0


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


def _maybe_sweep_stale_artifacts(ext_root: Path) -> None:
    """Opportunistically reclaim stale build artifacts. Best-effort + throttled.

    Spawns ``tools/molt_ssd_janitor.py --apply`` DETACHED (never blocks or slows a
    build) at most once per :data:`_JANITOR_THROTTLE_HOURS` per artifact root. The
    janitor is age-based and protects anything live (registered worktrees, the
    current session, recently-touched dirs), so it only removes obsolete cruft.
    Set ``MOLT_DISABLE_AUTO_JANITOR=1`` to opt out. Never raises.
    """
    try:
        if os.environ.get("MOLT_DISABLE_AUTO_JANITOR", "").strip().lower() in (
            "1",
            "true",
            "yes",
            "on",
        ):
            return
        if _running_under_pytest():
            return
        marker = ext_root / ".molt_janitor_last_run"
        now = time.time()
        try:
            last = marker.stat().st_mtime
        except OSError:
            last = 0.0
        if now - last < _JANITOR_THROTTLE_HOURS * 3600:
            return
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(str(now), encoding="utf-8")  # claim the slot first
        janitor = Path(__file__).resolve().parent.parent.parent / "tools" / "molt_ssd_janitor.py"
        if not janitor.exists():
            return
        creationflags = 0
        if os.name == "nt":
            # DETACHED_PROCESS | CREATE_NO_WINDOW — outlive this process, no console.
            creationflags = 0x00000008 | 0x08000000
        subprocess.Popen(
            [
                sys.executable,
                str(janitor),
                "--root",
                str(ext_root),
                "--apply",
                "--no-sizes",
                "--free-below-gb",
                "80",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            creationflags=creationflags,
            close_fds=True,
            cwd=str(Path(__file__).resolve().parent.parent.parent),
        )
    except Exception:
        # Cleanup is best-effort — never break a build over it.
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


def _default_external_artifact_roots(env: Mapping[str, str]) -> tuple[Path, ...]:
    roots: list[Path] = []
    if os.name == "nt":
        roots.extend(_default_windows_external_artifact_roots())
    else:
        roots.extend(
            Path(path).expanduser() for path in DEFAULT_POSIX_EXTERNAL_ARTIFACT_ROOTS
        )
    return _dedupe_paths(roots)


def _default_windows_external_artifact_roots() -> tuple[Path, ...]:
    """Windows artifact roots, with C:\\Molt primary when present.

    APDataStore-labeled volumes remain fallback candidates. The legacy
    drive-letter-order scan is deleted: it silently returned ``E:\\Molt`` (the
    contended legacy volume) ahead of the intended root whenever labels went
    unchecked. A volume without a preferred label is not a default candidate;
    set MOLT_EXTERNAL_ARTIFACT_ROOTS explicitly for any other fallback.
    """
    preferred_labels = {
        label.casefold() for label in DEFAULT_WINDOWS_PREFERRED_VOLUME_LABELS
    }
    roots: list[Path] = list(_default_windows_primary_artifact_roots())
    for drive_root in _windows_drive_roots():
        label = _windows_volume_label(drive_root)
        if label is not None and label.casefold() in preferred_labels:
            roots.append(drive_root / DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME)
    return _dedupe_paths(roots)


def _default_windows_primary_artifact_roots() -> tuple[Path, ...]:
    root = DEFAULT_WINDOWS_PRIMARY_ARTIFACT_ROOT
    return (root,) if root.is_dir() else ()


def _windows_drive_roots() -> tuple[Path, ...]:
    roots: list[Path] = []
    for letter in "DEFGHIJKLMNOPQRSTUVWXYZ":
        drive_root = Path(f"{letter}:\\")
        if drive_root.exists():
            roots.append(drive_root)
    return tuple(roots)


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


def _default_toolchain_root_for_artifact_root(artifact_root: Path) -> Path:
    """Toolchain root (MOLT_TARGET_ROOT) derived from the artifact root.

    ``C:\\Molt`` -> ``C:\\Molt\\target-root`` on the canonical workstation
    root, and similarly for any explicitly selected artifact root.
    """
    return artifact_root / DEFAULT_TARGET_ROOT_DIRNAME


def _legacy_sibling_toolchain_root_for_artifact_root(
    artifact_root: Path,
) -> Path | None:
    if (
        artifact_root.name.casefold()
        == DEFAULT_WINDOWS_EXTERNAL_ARTIFACT_DIRNAME.casefold()
    ):
        parent = artifact_root.parent
        if parent != artifact_root:
            return parent / LEGACY_WINDOWS_TARGET_ROOT_DIRNAME
    return None


def _same_path_text(left: Path, right: Path) -> bool:
    return _path_text_key(left) == _path_text_key(right)


def _path_text_key(path: Path) -> str:
    return os.path.normcase(str(path.expanduser())).rstrip("\\/")


def _windows_path_text_key(raw: str | Path) -> str:
    return str(raw).strip().replace("/", "\\").rstrip("\\").casefold()


def _windows_path_is_under(raw: str | Path, root: str) -> bool:
    path_key = _windows_path_text_key(raw)
    root_key = _windows_path_text_key(root)
    return path_key == root_key or path_key.startswith(root_key + "\\")


def _stale_windows_molt_root_value(raw: str, env: Mapping[str, str]) -> bool:
    if os.name != "nt":
        return False
    if _env_bool(env, ("MOLT_PRESERVE_LEGACY_ARTIFACT_ROOTS",), default=False):
        return False
    if not _default_windows_primary_artifact_roots():
        return False
    return any(_windows_path_is_under(raw, root) for root in LEGACY_WINDOWS_MOLT_ROOTS)


def _drop_stale_windows_root_env(env: dict[str, str]) -> None:
    for key in CANONICAL_ROOT_ENV_KEYS:
        raw = env.get(key, "")
        if raw and _stale_windows_molt_root_value(raw, env):
            env.pop(key, None)


def _should_rehome_toolchain_root(
    raw: str,
    artifact_root: Path,
    env: Mapping[str, str],
) -> bool:
    """True when an inherited MOLT_TARGET_ROOT is stale for the selected root.

    Rehome off-volume roots (for example ``E:\\molt-target`` when the authority
    chose ``D:\\Molt``) and the old same-volume sibling default
    (``D:\\molt-target``). Set ``MOLT_PRESERVE_TARGET_ROOT=1`` to keep an
    intentional non-default toolchain root.
    """
    if _env_bool(env, ("MOLT_PRESERVE_TARGET_ROOT",), default=False):
        return False
    if os.name != "nt":
        return False
    target_path = Path(raw).expanduser()
    target_drive = _path_drive(target_path)
    artifact_drive = _path_drive(artifact_root)
    if bool(target_drive and artifact_drive) and target_drive != artifact_drive:
        return True
    legacy_sibling = _legacy_sibling_toolchain_root_for_artifact_root(artifact_root)
    return legacy_sibling is not None and _same_path_text(target_path, legacy_sibling)


def _requires_external_artifacts(
    repo_root: Path,
    env: Mapping[str, str],
    *,
    prefer_external: bool,
) -> bool:
    del repo_root, prefer_external
    if _env_bool(env, ("MOLT_ALLOW_C_DRIVE_ARTIFACTS",), default=False):
        return False
    return _env_bool(env, ("MOLT_REQUIRE_EXTERNAL_ARTIFACTS",), default=False)


def _allow_c_drive_artifacts(env: Mapping[str, str]) -> bool:
    return _env_bool(env, ("MOLT_ALLOW_C_DRIVE_ARTIFACTS",), default=False)


def _is_windows_c_drive_path(path: Path) -> bool:
    return os.name == "nt" and path.drive.upper() == "C:"


def _reject_c_drive_artifact_path(
    key: str,
    path: Path,
    env: Mapping[str, str],
    *,
    repo_root: Path,
    prefer_external: bool,
) -> None:
    if not _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        return
    if _allow_c_drive_artifacts(env):
        return
    if _is_windows_c_drive_path(path.resolve()):
        raise DxConfigError(
            f"{key} resolved to {path}; Molt build artifacts must live on an "
            "approved artifact root. Prefer C:\\Molt on this workstation; set "
            "MOLT_ALLOW_C_DRIVE_ARTIFACTS=1 for the canonical C:\\Molt root "
            "or MOLT_EXTERNAL_ARTIFACT_ROOTS for an explicit fallback."
        )


def _candidate_roots(env: Mapping[str, str]) -> tuple[Path, ...]:
    raw = (
        env.get("MOLT_EXTERNAL_ARTIFACT_ROOTS")
        or env.get("MOLT_EXTERNAL_ARTIFACT_CANDIDATES")
        or ""
    )
    candidates = raw.split(os.pathsep) if raw.strip() else ()
    roots: list[Path] = []
    for candidate in candidates:
        text = candidate.strip()
        if not text:
            continue
        if _stale_windows_molt_root_value(text, env):
            continue
        roots.append(Path(text).expanduser())
    return _dedupe_paths(roots) if roots else _default_external_artifact_roots(env)


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

    if env.get("MOLT_EXT_ROOT"):
        return None
    require_external = _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    )
    if (
        not _env_bool(
            env,
            ("MOLT_PREFER_EXTERNAL_ARTIFACTS", "MOLT_USE_EXTERNAL_ARTIFACTS"),
            default=prefer_external,
        )
        and not require_external
    ):
        return None

    min_free_gb = _env_float(env, "MOLT_EXTERNAL_MIN_FREE_GB", default=20.0)
    repo_root = repo_root.resolve()
    for raw_candidate in _candidate_roots(env):
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
            "no healthy Molt artifact root was found. Prefer C:\\Molt on this "
            "workstation; set MOLT_ALLOW_C_DRIVE_ARTIFACTS=1 for the canonical "
            "C:\\Molt root or MOLT_EXTERNAL_ARTIFACT_ROOTS for an explicit "
            "fallback with sufficient free space."
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
    if _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        candidates = ", ".join(str(path) for path in _candidate_roots(env)) or "<none>"
        raise DxConfigError(
            "Molt build artifacts must not be placed on C:. Configure a healthy "
            "non-C artifact root with MOLT_EXTERNAL_ARTIFACT_ROOTS or MOLT_EXT_ROOT. "
            f"Checked candidates: {candidates}"
        )
    return None


def _is_onedrive_path(path: Path) -> bool:
    """True if *path* is under a OneDrive-synced tree — forbidden for molt.

    OneDrive continuously syncs the `.git` + build tree (thousands of tiny objects),
    throttling every git/build op and corrupting the working set; it was the root of
    the drift retired 2026-07-08. The canonical checkout is `C:\\Molt\\molt-src` and
    artifacts live on `C:\\Molt` — nothing may drift back onto OneDrive.
    """
    try:
        parts = path.resolve().parts
    except (OSError, ValueError):
        parts = path.parts
    return any("onedrive" in str(p).lower() for p in parts)


def _reject_onedrive(path: Path, kind: str) -> None:
    if _is_onedrive_path(path):
        raise DxConfigError(
            f"Molt {kind} must NOT be under OneDrive (it throttles/corrupts git + "
            f"builds and was the retired drift root). Rejected: {path}. Use the "
            f"canonical checkout C:\\Molt\\molt-src and artifacts C:\\Molt "
            f"(see docs/agent/ORCHESTRATION.md canonical paths)."
        )


def _validate_windows_artifact_root(
    artifact_root: Path,
    *,
    repo_root: Path,
    env: Mapping[str, str],
    prefer_external: bool,
) -> None:
    # Fail closed against OneDrive re-drift — the checkout AND the artifact root.
    _reject_onedrive(repo_root, "checkout / repo root")
    _reject_onedrive(artifact_root, "build-artifact root")
    if not _requires_external_artifacts(
        repo_root,
        env,
        prefer_external=prefer_external,
    ):
        return
    if not _is_windows_c_drive_path(artifact_root.resolve()):
        return
    raise DxConfigError(
        "Molt build artifacts must not be placed on C:. "
        f"Rejected artifact root: {artifact_root}"
    )


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


# Pinned for reproducible custody (R73.3). A missing compilation-cache binary is a
# missing PRIMITIVE that gets COMPLETED here, never a silent fallback to cold builds.
_SCCACHE_VERSION = "v0.16.0"
_sccache_degrade_warned = False
# Provisioning is attempted at most ONCE per process: a failed network download
# must never re-run on every _install_dx_defaults call (that would hang every
# build's env setup by the download timeout on an offline host).
_sccache_download_failed = False


def _sccache_asset_url() -> str | None:
    machine = platform.machine().lower()
    if machine in {"amd64", "x86_64", "x64"}:
        arch = "x86_64"
    elif machine in {"arm64", "aarch64"}:
        arch = "aarch64"
    else:
        return None
    system = platform.system().lower()
    if system == "windows" or os.name == "nt":
        stem, ext = f"sccache-{_SCCACHE_VERSION}-{arch}-pc-windows-msvc", "zip"
    elif system == "darwin":
        stem, ext = f"sccache-{_SCCACHE_VERSION}-{arch}-apple-darwin", "tar.gz"
    else:
        stem, ext = f"sccache-{_SCCACHE_VERSION}-{arch}-unknown-linux-musl", "tar.gz"
    return (
        "https://github.com/mozilla/sccache/releases/download/"
        f"{_SCCACHE_VERSION}/{stem}.{ext}"
    )


def _provision_sccache() -> str | None:
    """Provision the pinned sccache binary into ``~/.cargo/bin`` (already on PATH for
    any cargo/rust host). Idempotent, bounded (network timeout), and NEVER raises:
    returns the resolved path or ``None``. The R73.3 custody primitive that keeps
    Rust compilation content-address-cached and shared across worktrees."""
    found = shutil.which("sccache")
    if found:
        return found
    exe = "sccache.exe" if os.name == "nt" else "sccache"
    dest = Path.home() / ".cargo" / "bin" / exe
    if dest.exists():
        return str(dest)
    global _sccache_download_failed
    if _sccache_download_failed:
        return None  # already tried and failed this process; do not re-hang
    url = _sccache_asset_url()
    if url is None:
        _sccache_download_failed = True
        return None
    import stat as _stat
    import tarfile
    import urllib.request
    import zipfile

    ok = False
    try:
        dest.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory() as td:
            archive = Path(td) / url.rsplit("/", 1)[-1]
            with urllib.request.urlopen(url, timeout=90) as resp, open(
                archive, "wb"
            ) as out:
                shutil.copyfileobj(resp, out)
            if archive.suffix == ".zip":
                with zipfile.ZipFile(archive) as zf:
                    member = next(n for n in zf.namelist() if n.endswith(exe))
                    with zf.open(member) as src, open(dest, "wb") as out:
                        shutil.copyfileobj(src, out)
            else:
                with tarfile.open(archive) as tf:
                    member = next(m for m in tf.getmembers() if m.name.endswith(exe))
                    src = tf.extractfile(member)
                    if src is not None:
                        with src, open(dest, "wb") as out:
                            shutil.copyfileobj(src, out)
        if os.name != "nt" and dest.exists():
            dest.chmod(
                dest.stat().st_mode | _stat.S_IEXEC | _stat.S_IXGRP | _stat.S_IXOTH
            )
        ok = dest.exists()
    except Exception:
        ok = False
    if not ok:
        _sccache_download_failed = True
        return None
    return str(dest)


def _ensure_sccache_wrapper(env: dict[str, str]) -> None:
    """Wire ``RUSTC_WRAPPER=sccache`` for content-addressed, cross-worktree-shared
    rustc caching across EVERY DX/proof build path. Provisions sccache when absent;
    if it genuinely cannot be made available, DEGRADE LOUDLY (cold builds saturate
    memory under parallel lanes) instead of the historical silent skip. Single
    authority — respects an explicit pre-set RUSTC_WRAPPER (e.g. benchmarks)."""
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
    sccache = _provision_sccache()
    if sccache is None:
        if not _sccache_degrade_warned:
            _sccache_degrade_warned = True
            print(
                "molt: WARNING sccache unavailable and could not be provisioned — "
                "Rust compilation cache is OFF; builds will be COLD and "
                "memory-heavy (every worktree recompiles the full crate graph, "
                "which saturates memory under parallel lanes). Install it "
                "(`cargo install sccache`) or set MOLT_USE_SCCACHE=0 to silence.",
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
    artifact_root = Path(env["MOLT_EXT_ROOT"]).expanduser()
    env.setdefault(
        "MOLT_BACKEND_DAEMON_SOCKET_DIR",
        str(backend_daemon_socket_dir(repo_root, env)),
    )
    # sccache off-by-default on Windows: measured 0 cache hits + mid-compile
    # crashes (os error 10054) that time builds out. Power users force it with
    # MOLT_USE_SCCACHE=1; cargo_execution also treats "auto" as off-on-Windows.
    env.setdefault("MOLT_USE_SCCACHE", "0" if os.name == "nt" else "1")
    env.setdefault("MOLT_DIFF_ALLOW_RUSTC_WRAPPER", "1")
    env.setdefault("SCCACHE_DIR", str((artifact_root / ".sccache").resolve()))
    env.setdefault("SCCACHE_CACHE_SIZE", DEFAULT_SCCACHE_CACHE_SIZE)
    env.setdefault("MOLT_CACHE_MAX_GB", DEFAULT_MOLT_CACHE_MAX_GB)
    env.setdefault("MOLT_CACHE_MAX_AGE_DAYS", DEFAULT_MOLT_CACHE_MAX_AGE_DAYS)
    _ensure_sccache_wrapper(env)
    if _artifact_root_is_windows_exfat(artifact_root):
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


def render_env(
    env: Mapping[str, str], keys: Sequence[str], fmt: EnvRenderFormat
) -> str:
    present = [(key, env[key]) for key in keys if key in env]
    if fmt == "json":
        return json.dumps(dx_env_payload(env, keys), indent=2, sort_keys=True)
    if fmt == "posix":
        return "\n".join(
            f"export {key}={_posix_quote(value)}" for key, value in present
        )
    if fmt == "powershell":
        return "\n".join(
            f"$env:{key} = {_powershell_quote(value)}" for key, value in present
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
        ext_root = self._resolve_env_path(env.get("MOLT_EXT_ROOT", str(self.root)))
        if uv_project_env_session_scoped(env):
            session = env.get("MOLT_SESSION_ID", f"{self.session_prefix}-{os.getpid()}")
            return (ext_root / "tmp" / "uv-project-envs" / session).resolve()
        return stable_uv_project_env_from_env(env, ext_root)

    def canonical_env(
        self,
        base: Mapping[str, str] | None = None,
        *,
        create_dirs: bool = True,
        force_default_keys: Collection[str] = (),
    ) -> dict[str, str]:
        env = dict(os.environ if base is None else base)
        _drop_stale_windows_root_env(env)
        _drop_ambient_tmpdir(env, prefer_external=self.prefer_external_artifacts)
        forced = set(force_default_keys)

        if "MOLT_EXT_ROOT" in forced or not env.get("MOLT_EXT_ROOT"):
            ext_root = (
                None
                if "MOLT_EXT_ROOT" in forced
                else require_external_artifact_root(
                    self.root,
                    env,
                    create_dirs=create_dirs,
                    prefer_external=self.prefer_external_artifacts,
                )
            ) or self.root
        else:
            ext_root = self._resolve_env_path(env["MOLT_EXT_ROOT"])
        _validate_windows_artifact_root(
            ext_root,
            repo_root=self.root,
            env=env,
            prefer_external=self.prefer_external_artifacts,
        )
        env["MOLT_EXT_ROOT"] = str(ext_root)
        if _is_windows_c_drive_path(ext_root.resolve()):
            # RunContext is the artifact-root authority. Once it has accepted a
            # Windows C: root, downstream guards must receive the same policy
            # attestation instead of re-litigating the old non-C default.
            env["MOLT_ALLOW_C_DRIVE_ARTIFACTS"] = "1"
        if create_dirs:
            # Keep the artifact volume tidy BY DEFAULT — throttled, detached,
            # best-effort. Only in real (create_dirs) contexts, never in tests.
            _maybe_sweep_stale_artifacts(Path(ext_root))

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
        session_pinned = "MOLT_SESSION_ID" in forced or bool(env.get("MOLT_SESSION_ID"))
        install_default("MOLT_SESSION_ID", f"{self.session_prefix}-{os.getpid()}")
        target_session_id = env["MOLT_SESSION_ID"] if session_pinned else None
        install_default(
            "CARGO_TARGET_DIR",
            cargo_target_dir_for_artifact_root(ext_root, target_session_id),
        )
        install_default("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
        # Incremental ON by default (fast warm rebuilds against the persistent
        # per-artifact-root CARGO_TARGET_DIR above). _ensure_sccache_wrapper forces
        # it back to "0" wherever it actually enables sccache (mutually exclusive).
        install_default("CARGO_INCREMENTAL", "1")
        install_default("MOLT_CACHE", ext_root / ".molt_cache")
        install_default("MOLT_DIFF_ROOT", ext_root / "tmp" / "diff")
        install_default("MOLT_DIFF_TMPDIR", ext_root / "tmp")
        install_default("UV_CACHE_DIR", ext_root / ".uv-cache")
        install_default("UV_PROJECT_ENVIRONMENT", self.uv_project_env_dir(env))
        install_default("PIP_CACHE_DIR", ext_root / ".pip-cache")
        install_default("RUFF_CACHE_DIR", ext_root / ".ruff-cache")
        # MOLT_TARGET_ROOT (wasi-sysroot/binaryen/zig toolchains) flows through
        # the one root authority: derive it from the selected artifact root,
        # and rehome stale inherited defaults rather than honoring legacy roots.
        default_toolchain_root = _default_toolchain_root_for_artifact_root(ext_root)
        raw_target_root = env.get("MOLT_TARGET_ROOT")
        if not raw_target_root or _should_rehome_toolchain_root(
            raw_target_root, ext_root, env
        ):
            env["MOLT_TARGET_ROOT"] = str(default_toolchain_root)
        install_default("PYTHONPYCACHEPREFIX", ext_root / "tmp" / "pycache")
        install_default("TMPDIR", ext_root / "tmp")
        install_default("TMP", env["TMPDIR"])
        install_default("TEMP", env["TMPDIR"])

        for key in CANONICAL_ROOT_ENV_KEYS:
            value = env.get(key)
            if value:
                env[key] = str(self._resolve_env_path(value))
                value = env[key]
                _reject_c_drive_artifact_path(
                    key,
                    Path(value).expanduser(),
                    env,
                    repo_root=self.root,
                    prefer_external=self.prefer_external_artifacts,
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
        env["MOLT_SESSION_ID"] = session_id
    env = RunContext(
        repo_root,
        session_prefix=session_prefix,
        prefer_external_artifacts=True,
    ).dx_env(env, create_dirs=create_dirs)
    src = repo_root.resolve() / "src"
    existing = env.get("PYTHONPATH", "")
    parts = [part for part in existing.split(os.pathsep) if part]
    if str(src) not in parts:
        env["PYTHONPATH"] = str(src) if not existing else f"{src}{os.pathsep}{existing}"
    return env


class DxProject:
    def __init__(self, root: Path) -> None:
        self.root = root.resolve()

    @classmethod
    def from_current_repo(cls) -> "DxProject":
        return cls(Path(__file__).resolve().parents[2])

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
        artifact_root = Path(env.get("MOLT_EXT_ROOT", str(self.root))).expanduser()
        if not artifact_root.is_absolute():
            artifact_root = self.root / artifact_root
        artifact_root = artifact_root.resolve()
        if uv_project_env_session_scoped(env):
            session = env.get("MOLT_SESSION_ID", f"dev-{os.getpid()}")
            return (artifact_root / "tmp" / "uv-project-envs" / session).resolve()
        return stable_uv_project_env_from_env(env, artifact_root)

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
        _drop_stale_windows_root_env(env)
        for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
            env.pop(name, None)
        prefer_external = bool(dx.get("prefer_external_artifacts"))
        _drop_ambient_tmpdir(env, prefer_external=prefer_external)
        if env.get("MOLT_EXT_ROOT"):
            artifact_root = Path(env["MOLT_EXT_ROOT"]).expanduser()
            if not artifact_root.is_absolute():
                artifact_root = self.root / artifact_root
            artifact_root = artifact_root.resolve()
        else:
            artifact_root = (
                require_external_artifact_root(
                    self.root,
                    env,
                    create_dirs=create_dirs,
                    prefer_external=prefer_external,
                )
                or self.root
            )
        _validate_windows_artifact_root(
            artifact_root,
            repo_root=self.root,
            env=env,
            prefer_external=prefer_external,
        )
        env_cfg = dx.get("env", {})
        if isinstance(env_cfg, dict):
            for key, raw_value in env_cfg.items():
                if not isinstance(key, str) or not isinstance(raw_value, str):
                    continue
                if key in CANONICAL_RUN_ENV_KEYS and env.get(key):
                    continue
                value = raw_value.format(
                    root=str(self.root),
                    artifact_root=str(artifact_root),
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
