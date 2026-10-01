from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
import contextlib
import ctypes
import errno
import json
import os
import platform
import time
from pathlib import Path
import shlex
import stat
import subprocess
import sys
from typing import TYPE_CHECKING, Any, cast
import uuid

from tools.memory_guard_core.common import utc_timestamp as _utc_timestamp


if TYPE_CHECKING:
    from tools.memory_guard_core.process_model import ProcessIdentity, ProcessSample


def _exception_diagnostic(exc: BaseException) -> str:
    """Describe failures without invoking exception-controlled formatting."""
    name = str.__str__(type.__dict__["__name__"].__get__(type(exc), type))
    args = cast(Any, BaseException.args).__get__(exc, type(exc))
    details = [arg for arg in args if type(arg) is str]
    return name + (": " + "; ".join(details) if details else "")


DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP = 5


@dataclass(frozen=True, slots=True)
class CargoIncrementalQuarantineMove:
    original_path: str
    quarantined_path: str


@dataclass(frozen=True, slots=True)
class CargoIncrementalQuarantine:
    reason: str
    recorded_at: str
    target_dir: str
    quarantine_dir: str | None
    command: tuple[str, ...]
    cwd: str
    moved_paths: tuple[CargoIncrementalQuarantineMove, ...] = ()
    pruned_quarantine_dirs: tuple[str, ...] = ()
    errors: tuple[str, ...] = ()
    receipt_path: str | None = None
    ownership_status: str = "unavailable"
    ownership_observations: tuple[CargoIncrementalObservation, ...] = ()
    recovery_observations: tuple[CargoIncrementalObservation, ...] = ()
    admission_telemetry: Mapping[str, int | float] | None = None


@dataclass(frozen=True, slots=True)
class CargoIncrementalObservation:
    rustc_pid: int
    rustc_started_at_ns: int
    incremental_dir: str
    cargo_pid: int
    cargo_started_at_ns: int


def _owned_sample_argv(sample: ProcessSample) -> tuple[str, ...] | None:
    """Native argument boundaries from the observed process instance only."""
    argv = getattr(sample, "argv", None)
    if argv is None and os.name == "nt":
        from molt.backend_daemon_custody import _split_command

        argv = tuple(_split_command(sample.command))
    elif argv is None and sys.platform.startswith("linux"):
        from tools.memory_guard_core.process_model import process_started_at_ns

        try:
            before = process_started_at_ns(sample.pid)
            if before != sample.started_at_ns:
                return None
            raw = (Path("/proc") / str(sample.pid) / "cmdline").read_bytes()
            after = process_started_at_ns(sample.pid)
            if after != before or not raw.endswith(b"\0"):
                return None
            argv = tuple(
                part.decode("utf8", errors="surrogateescape")
                for part in raw[:-1].split(b"\0")
            )
        except OSError:
            return None
    if (
        not isinstance(argv, tuple)
        or not argv
        or any(not isinstance(arg, str) or "\0" in arg for arg in argv)
    ):
        return None
    return argv


def _owned_cargo_ancestor(
    child: ProcessSample,
    samples: Mapping[int, ProcessSample],
    watched: set[int],
    identities: Mapping[int, ProcessIdentity],
) -> ProcessSample | None:
    from tools.memory_guard_core.process_model import process_identity

    # No generic ancestor walk: a Cargo build script/program can manually spawn
    # Rustc. Wrappers need an explicit source-bound protocol before admission.
    if child.ppid not in watched or child.ppid == child.pid:
        return None
    parent = samples.get(child.ppid)
    if (
        parent is None
        or type(child.started_at_ns) is not int
        or type(parent.started_at_ns) is not int
        or parent.started_at_ns <= 0
        or identities.get(parent.pid) != process_identity(parent)
        or parent.started_at_ns > child.started_at_ns
        or parent.ppid == child.pid
    ):
        return None
    argv = _owned_sample_argv(parent)
    return (
        parent
        if argv is not None and _native_executable_name(argv[0]) == "cargo"
        else None
    )


def observe_owned_incremental_state(
    samples: Mapping[int, ProcessSample],
    watched: set[int],
    identities: Mapping[int, ProcessIdentity],
) -> set[CargoIncrementalObservation]:
    """Capture compiler arguments plus birth-custodied Cargo producer ancestry.

    A manual Rustc process, token appearing in a shell command, missing ancestor,
    or stale/reused parent instance cannot authorize Cargo cache recovery.
    """
    from tools.memory_guard_core.process_model import process_identity

    observations = set()
    for pid in watched:
        sample = samples.get(pid)
        if (
            sample is None
            or type(sample.started_at_ns) is not int
            or sample.started_at_ns <= 0
            or identities.get(pid) != process_identity(sample)
        ):
            continue
        argv = _owned_sample_argv(sample)
        if argv is None or _native_executable_name(argv[0]) != "rustc":
            continue
        producer = _owned_cargo_ancestor(sample, samples, watched, identities)
        if producer is None or type(producer.started_at_ns) is not int:
            continue
        if any(arg.startswith("@") for arg in argv[1:]):
            # Unknown response-file expansion can override every literal flag.
            # Command text alone grants no authority over its effective path.
            continue
        # rustc applies codegen options in argv order and overwrites the
        # incremental slot. Superseded paths confer no recovery authority.
        effective_incremental = None
        for index, arg in enumerate(argv[1:], start=1):
            if arg == "--":
                break
            option = (
                argv[index + 1]
                if arg in {"-C", "--codegen"} and index + 1 < len(argv)
                else arg[2:]
                if arg.startswith("-C")
                else arg[len("--codegen=") :]
                if arg.startswith("--codegen=")
                else ""
            )
            if option.startswith("incremental="):
                effective_incremental = option[len("incremental=") :]
        if effective_incremental:
            path = Path(effective_incremental)
            if path.is_absolute():
                observations.add(
                    CargoIncrementalObservation(
                        pid,
                        sample.started_at_ns,
                        str(path),
                        producer.pid,
                        producer.started_at_ns,
                    )
                )
    return observations


def _observed_pid_is_definitely_closed(pid: int) -> bool:
    """Non-signaling ordinary OS authority; missing sampler rows are unknown."""
    if type(pid) is not int or pid <= 0:
        return False
    if os.name == "posix":
        if pid > 0x7FFFFFFF:
            return False
        try:
            os.kill(pid, 0)  # Probe existence only; no process is signaled.
        except ProcessLookupError as exc:
            return exc.errno == errno.ESRCH
        except OSError:
            return False  # EPERM and other ambiguous failures stay unknown.
        return False
    if os.name == "nt":
        if pid > 0xFFFFFFFF:
            return False
        from tools.memory_guard_core.windows_snapshot import _windows_snapshot_api

        api = _windows_snapshot_api()  # Shared pointer-width-correct query ABI.
        snapshot = api.create_snapshot(0x00000002, 0)  # TH32CS_SNAPPROCESS
        if snapshot in (None, 0, api.invalid_handle_value):
            return False
        absent = False
        try:
            entry = api.ProcessEntry32W()
            entry.dwSize = api.ctypes.sizeof(api.ProcessEntry32W)
            saw_self = False
            api.ctypes.set_last_error(0)
            ok = api.process_first(snapshot, api.ctypes.byref(entry))
            while ok:
                saw_self |= int(entry.th32ProcessID) == os.getpid()
                if int(entry.th32ProcessID) == pid:
                    return False  # Includes access-degraded live process rows.
                api.ctypes.set_last_error(0)
                ok = api.process_next(snapshot, api.ctypes.byref(entry))
            absent = (
                saw_self and api.ctypes.get_last_error() == 18
            )  # ERROR_NO_MORE_FILES only.
        finally:
            if not api.close_handle(snapshot):
                absent = False
        return absent
    return False


def _observed_compilers_closed(
    observations: Sequence[CargoIncrementalObservation],
) -> bool:
    from tools.memory_guard_core.process_model import sample_processes

    try:
        samples = sample_processes()
    except (OSError, RuntimeError, subprocess.SubprocessError):
        return False
    missing_pid_closure: dict[int, bool] = {}
    for observed in observations:
        for pid, birth in (
            (observed.rustc_pid, observed.rustc_started_at_ns),
            (observed.cargo_pid, observed.cargo_started_at_ns),
        ):
            current = samples.get(pid)
            if current is None:
                if pid not in missing_pid_closure:
                    missing_pid_closure[pid] = _observed_pid_is_definitely_closed(pid)
                if not missing_pid_closure[pid]:
                    return False
            if current is not None and (
                type(current.started_at_ns) is not int
                or current.started_at_ns <= 0
                or current.started_at_ns == birth
            ):
                return False
    return True


class _DarwinStatfs64(ctypes.Structure):
    # Apple xnu bsd/sys/mount.h __DARWIN_STRUCT_STATFS64, both LP64 ABIs.
    _fields_ = [
        ("f_bsize", ctypes.c_uint32),
        ("f_iosize", ctypes.c_int32),
        ("f_blocks", ctypes.c_uint64),
        ("f_bfree", ctypes.c_uint64),
        ("f_bavail", ctypes.c_uint64),
        ("f_files", ctypes.c_uint64),
        ("f_ffree", ctypes.c_uint64),
        ("f_fsid", ctypes.c_int32 * 2),
        ("f_owner", ctypes.c_uint32),
        ("f_type", ctypes.c_uint32),
        ("f_flags", ctypes.c_uint32),
        ("f_fssubtype", ctypes.c_uint32),
        ("f_fstypename", ctypes.c_char * 16),
        ("f_mntonname", ctypes.c_char * 1024),
        ("f_mntfromname", ctypes.c_char * 1024),
        ("f_flags_ext", ctypes.c_uint32),
        ("f_reserved", ctypes.c_uint32 * 7),
    ]


def _darwin_local_cargo_lock_filesystem(path: Path) -> bool:
    machine = platform.machine().lower()
    if machine not in {"x86_64", "arm64", "aarch64"}:
        return False
    if ctypes.sizeof(ctypes.c_void_p) != 8 or ctypes.sizeof(_DarwinStatfs64) != 2168:
        return False
    # Apple xnu sys/cdefs.h: macOS x86_64 retains the INODE64 symbol suffix;
    # arm64 has only the 64-bit inode ABI. Never fall back to legacy statfs.
    symbol = "statfs$INODE64" if machine == "x86_64" else "statfs"
    try:
        probe = getattr(ctypes.CDLL(None, use_errno=True), symbol, None)
        if probe is None:
            return False
        probe.argtypes = [ctypes.c_char_p, ctypes.POINTER(_DarwinStatfs64)]
        probe.restype = ctypes.c_int
        result = _DarwinStatfs64()
        if probe(os.fsencode(path), ctypes.byref(result)) != 0:
            return False
        # MNT_LOCAL is kernel authority, not a mount-path/name heuristic.
        # Restrict admitted local types to the native APFS/HFS lock contract.
        return (
            bool(result.f_flags & 0x1000)
            and not bool(result.f_flags & 1)
            and (result.f_fstypename in {b"apfs", b"hfs"})
        )
    except (OSError, ValueError, TypeError, AttributeError):
        return False


def _local_cargo_lock_filesystem(path: Path) -> bool:
    """Fail closed where Cargo can ignore locks or locality is unverified."""
    if os.name == "nt":
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        drive_type = kernel.GetDriveTypeW
        drive_type.argtypes = [ctypes.c_wchar_p]
        drive_type.restype = ctypes.c_uint
        return drive_type(path.resolve().anchor) == 3  # local fixed drive
    if sys.platform.startswith("linux"):
        libc = ctypes.CDLL(None, use_errno=True)
        probe = getattr(libc, "statfs", None)
        if probe is None:
            return False
        buffer = ctypes.create_string_buffer(512)
        probe.argtypes = [ctypes.c_char_p, ctypes.c_void_p]
        probe.restype = ctypes.c_int
        if probe(os.fsencode(path), buffer) != 0:
            return False
        kind = ctypes.c_long.from_buffer(buffer).value & 0xFFFFFFFF
        return kind in {
            0xEF53,
            0x58465342,
            0x9123683E,
            0x01021994,
            0x794C7630,
            0x2FC12FC1,
        }
    if sys.platform == "darwin":
        return _darwin_local_cargo_lock_filesystem(path)
    return False  # additional native filesystem witnesses are required


def _observed_incremental_units(
    target_dir: Path, observations: Sequence[CargoIncrementalObservation]
) -> dict[Path, Path]:
    root = target_dir.resolve(strict=True)
    units: dict[Path, Path] = {}
    for observed in observations:
        if (
            type(observed.rustc_pid) is not int
            or observed.rustc_pid <= 0
            or type(observed.rustc_started_at_ns) is not int
            or observed.rustc_started_at_ns <= 0
            or type(observed.cargo_pid) is not int
            or observed.cargo_pid <= 0
            or observed.cargo_pid == observed.rustc_pid
            or type(observed.cargo_started_at_ns) is not int
            or observed.cargo_started_at_ns <= 0
            or observed.cargo_started_at_ns > observed.rustc_started_at_ns
        ):
            raise ValueError("incremental observation lacks process birth authority")
        path = Path(observed.incremental_dir)
        if not path.is_absolute():
            raise ValueError("incremental observation is not absolute")
        source = path.resolve(strict=True)
        parts = source.relative_to(root).parts
        if (
            len(parts) not in {2, 3}
            or parts[-1] != "incremental"
            or any(part in {".molt_state", "sessions"} for part in parts)
        ):
            raise ValueError("incremental observation lacks a Cargo coordinate")
        if (
            not source.is_dir()
            or path.is_symlink()
            or source != path.parent.resolve(strict=True) / "incremental"
            or getattr(path, "is_junction", lambda: False)()
        ):
            raise ValueError(
                "incremental observation is not a regular profile directory"
            )
        units[source] = source.parent
    return units


_CARGO_BUILD_STATE_EXECUTABLES = frozenset({"cargo", "rustc", "rustdoc"})


def _command_tokens(fragment: str) -> list[str]:
    try:
        return shlex.split(fragment)
    except ValueError:
        return fragment.split()


def _native_executable_name(token: str) -> str:
    """Native argv elements are already delimited; literal quotes are data."""
    name = token.replace("\\", "/").rsplit("/", 1)[-1]
    suffix = Path(name).suffix.casefold()
    if suffix in {".exe", ".cmd", ".bat"}:
        name = name[: -len(suffix)]
    return name.casefold()


def _token_executable_name(token: str) -> str:
    return _native_executable_name(token.strip().strip("\"'"))


def _command_invokes_cargo_build_state(command: Sequence[str]) -> bool:
    for item in command:
        for token in (item, *_command_tokens(item)):
            if _token_executable_name(token) in _CARGO_BUILD_STATE_EXECUTABLES:
                return True
    return False


def _samples_include_cargo_build_state(
    samples: Mapping[int, object],
    watched: set[int],
) -> bool:
    for pid in watched:
        sample = samples.get(pid)
        if sample is None:
            continue
        argv = getattr(sample, "argv", None)
        if isinstance(argv, tuple):
            if argv and all(isinstance(arg, str) and "\0" not in arg for arg in argv):
                if _native_executable_name(argv[0]) in _CARGO_BUILD_STATE_EXECUTABLES:
                    return True
            continue  # Explicitly unknown native argv grants no role.
        command = getattr(sample, "command", None)
        if isinstance(command, str) and _command_invokes_cargo_build_state(
            _command_tokens(command)
        ):
            return True
    return False


def _effective_guard_cwd(
    cwd: str | Path | None,
    environ: Mapping[str, str],
) -> Path:
    if cwd is not None:
        cwd_path = Path(cwd).expanduser()
        if cwd_path.is_absolute():
            return cwd_path.resolve(strict=False)
        return (Path.cwd() / cwd_path).resolve(strict=False)
    pwd = environ.get("PWD", "")
    if pwd:
        pwd_path = Path(pwd).expanduser()
        if pwd_path.is_absolute():
            return pwd_path.resolve(strict=False)
    return Path.cwd().resolve(strict=False)


def _cargo_target_dir(
    environ: Mapping[str, str],
    cwd: str | Path | None,
) -> Path:
    base = _effective_guard_cwd(cwd, environ)
    raw_target = environ.get("CARGO_TARGET_DIR", "").strip()
    if raw_target:
        target = Path(raw_target).expanduser()
        if target.is_absolute():
            return target.resolve(strict=False)
        return (base / target).resolve(strict=False)
    return (base / "target").resolve(strict=False)


def _cargo_incremental_dirs(target_dir: Path) -> tuple[Path, ...]:
    """Unowned target-wide discovery grants no recovery authority."""
    return ()


def _cargo_quarantine_parent(target_dir: Path) -> Path:
    return target_dir / ".molt_state" / "quarantine" / "cargo_incremental"


def _cargo_quarantine_id(recorded_at: str, pid: int, reason: str) -> str:
    safe_time = (
        recorded_at.replace(":", "").replace("-", "").replace("T", "-").replace("Z", "")
    )
    safe_reason = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in reason)
    return f"{safe_time}-pid{pid}-{safe_reason}"


def _prune_cargo_incremental_quarantine(
    parent: Path,
    *,
    keep: int = DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP,
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    return (), ("historical quarantine cleanup requires separate explicit custody",)


def _cargo_incremental_quarantine_payload(
    receipt: CargoIncrementalQuarantine | None,
) -> dict[str, object] | None:
    if receipt is None:
        return None
    return {
        "reason": receipt.reason,
        "recorded_at": receipt.recorded_at,
        "target_dir": receipt.target_dir,
        "quarantine_dir": receipt.quarantine_dir,
        "command": list(receipt.command),
        "cwd": receipt.cwd,
        "moved_paths": [
            {
                "original_path": move.original_path,
                "quarantined_path": move.quarantined_path,
            }
            for move in receipt.moved_paths
        ],
        "pruned_quarantine_dirs": list(receipt.pruned_quarantine_dirs),
        "errors": list(receipt.errors),
        "receipt_path": receipt.receipt_path,
        "ownership_status": receipt.ownership_status,
        "mutation_scope": "cargo_managed_observed_profile_incremental",
        "admission_telemetry": None
        if receipt.admission_telemetry is None
        else dict(receipt.admission_telemetry),
        "ownership_observations": [
            {
                "rustc_pid": item.rustc_pid,
                "rustc_started_at_ns": item.rustc_started_at_ns,
                "incremental_dir": item.incremental_dir,
                "cargo_pid": item.cargo_pid,
                "cargo_started_at_ns": item.cargo_started_at_ns,
            }
            for item in receipt.ownership_observations
        ],
        "recovery_observations": [
            {
                "rustc_pid": item.rustc_pid,
                "rustc_started_at_ns": item.rustc_started_at_ns,
                "cargo_pid": item.cargo_pid,
                "cargo_started_at_ns": item.cargo_started_at_ns,
                "incremental_dir": item.incremental_dir,
            }
            for item in receipt.recovery_observations
        ],
    }


def _write_cargo_quarantine_receipt(
    *,
    receipt_path: Path,
    payload: Mapping[str, object],
) -> None:
    from molt.file_publication import atomic_write_bytes

    atomic_write_bytes(
        receipt_path,
        (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8"),
    )


def _quarantine_cargo_incremental_state(
    *,
    reason: str,
    target_dir: Path,
    command: Sequence[str],
    cwd: str | Path,
    retention_keep: int = DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP,
    observations: Sequence[CargoIncrementalObservation] = (),
    descendants_closed: bool = False,
    eligible_observations: frozenset[CargoIncrementalObservation] = frozenset(),
    profile_lock_settle_s: float = 0.0,
) -> CargoIncrementalQuarantine:
    """Recover observed inactive profile caches under Cargo-compatible exclusion.

    Cargo passes the profile incremental root; Rustc chooses private sessions
    inside it. This is reproducible whole-profile cache recovery, not proof of
    command-owned units or generations. Other profiles and old evidence remain.
    Unknown process closure, waiting Cargo and unobserved wrappers defer.
    Cargo-compatible exclusion is not authority over unsupported manual Rustc
    writers which bypass Cargo locks; no generic inactive-directory claim.
    """
    recorded_at = _utc_timestamp()
    errors = []
    moved = []
    quarantine_dir = None
    receipt_path = None
    status = "deferred"
    lock_attempts = 0
    closure_attempts = 0
    admission_started = None
    lock_phase_finished = None
    admission_finished = None

    def admission_telemetry():
        if admission_started is None:
            return None
        finished = (
            admission_finished if admission_finished is not None else time.monotonic()
        )
        lock_finished = (
            lock_phase_finished if lock_phase_finished is not None else finished
        )
        return {
            "retry_admission_budget_s": max(0.0, min(profile_lock_settle_s, 1.0)),
            "lock_attempts": lock_attempts,
            "closure_attempts": closure_attempts,
            "lock_elapsed_s": max(0.0, lock_finished - admission_started),
            "closure_elapsed_s": max(0.0, finished - lock_finished),
            "total_admission_elapsed_s": max(0.0, finished - admission_started),
        }

    handles = []
    planned_paths: list[dict[str, str]] = []
    cleanup_abort: BaseException | None = None

    def validate_lock_paths():
        for lock_path, handle in handles:
            actual = lock_path.lstat()
            held = os.fstat(handle.file.fileno())
            if not stat.S_ISREG(actual.st_mode) or (actual.st_dev, actual.st_ino) != (
                held.st_dev,
                held.st_ino,
            ):
                raise ValueError(
                    "Cargo lock path identity changed after closure; recovery deferred"
                )

    try:
        from molt.file_locks import (
            _try_acquire_file_lock,
            _release_file_lock,
            _file_lock_owned_operation,
        )

        if descendants_closed is not True:
            raise ValueError(
                "compiler descendant closure is unverified; recovery deferred"
            )
        if not observations:
            raise ValueError(
                "no birth-custodied rustc incremental observations; recovery deferred"
            )
        if not eligible_observations or not eligible_observations.issubset(
            set(observations)
        ):
            raise ValueError(
                "no freshly observed compiler was interrupted; completed caches retained"
            )
        units = _observed_incremental_units(target_dir, tuple(eligible_observations))
        if not units or any(
            not _local_cargo_lock_filesystem(profile) for profile in units.values()
        ):
            raise ValueError(
                "Cargo-compatible local filesystem authority unavailable; recovery deferred"
            )
        lock_paths = sorted(
            {
                profile / name
                for profile in units.values()
                for name in (".cargo-lock", ".cargo-build-lock")
            },
            key=str,
        )
        # Job active-count zero can precede Windows file-lock release during
        # process rundown. Wait only for native lock admission, never bypass it.
        # This bounds retry admission, not filesystem syscalls or process sampling.
        admission_started = time.monotonic()
        lock_deadline = admission_started + max(0.0, min(profile_lock_settle_s, 1.0))
        for lock_path in lock_paths:
            lock_attempts += 1
            handle = _try_acquire_file_lock(lock_path)
            while handle is None and time.monotonic() < lock_deadline:
                time.sleep(min(0.025, max(0.0, lock_deadline - time.monotonic())))
                lock_attempts += 1
                handle = _try_acquire_file_lock(lock_path)
            if handle is None:
                raise ValueError("Cargo target coordinate is active; recovery deferred")
            handles.append((lock_path, handle))
        lock_phase_finished = time.monotonic()
        with contextlib.ExitStack() as pins:
            for lock_path, handle in handles:
                pins.enter_context(
                    _file_lock_owned_operation(handle, expected_lock_path=lock_path)
                )
                actual = lock_path.lstat()
                held = os.fstat(handle.file.fileno())
                if not stat.S_ISREG(actual.st_mode) or (
                    actual.st_dev,
                    actual.st_ino,
                ) != (held.st_dev, held.st_ino):
                    raise ValueError(
                        "Cargo lock path identity changed; recovery deferred"
                    )
            closure_attempts += 1
            compilers_closed = _observed_compilers_closed(tuple(eligible_observations))
            admission_finished = time.monotonic()
            while not compilers_closed and time.monotonic() < lock_deadline:
                time.sleep(min(0.025, max(0.0, lock_deadline - time.monotonic())))
                closure_attempts += 1
                compilers_closed = _observed_compilers_closed(
                    tuple(eligible_observations)
                )
                admission_finished = time.monotonic()
            if not compilers_closed:
                raise ValueError(
                    "observed compiler remains live or unknown; recovery deferred"
                )
            validate_lock_paths()
            parent = _cargo_quarantine_parent(target_dir)
            if not parent.resolve().is_relative_to(target_dir.resolve()):
                raise ValueError("quarantine destination escapes target authority")
            quarantine_dir = parent / (
                _cargo_quarantine_id(recorded_at, os.getpid(), reason)
                + "-"
                + uuid.uuid4().hex
            )
            quarantine_dir.mkdir(parents=True, exist_ok=False)
            planned_receipt_path = quarantine_dir / "receipt.json"
            planned_paths = [
                {
                    "original_path": str(source),
                    "quarantined_path": str(
                        quarantine_dir / source.relative_to(target_dir.resolve())
                    ),
                }
                for source in sorted(units, key=str)
            ]
            planned_receipt = CargoIncrementalQuarantine(
                reason,
                recorded_at,
                str(target_dir),
                str(quarantine_dir),
                tuple(command),
                str(cwd),
                (),
                (),
                (),
                str(planned_receipt_path),
                "cleanup_pending",
                tuple(observations),
                tuple(
                    sorted(
                        eligible_observations,
                        key=lambda item: (item.rustc_pid, item.rustc_started_at_ns),
                    )
                ),
            )
            planned_receipt = replace(
                planned_receipt, admission_telemetry=admission_telemetry()
            )
            planned_payload = _cargo_quarantine_payload_required(planned_receipt)
            planned_payload["planned_paths"] = planned_paths
            _write_cargo_quarantine_receipt(
                receipt_path=planned_receipt_path,
                payload=planned_payload,
            )
            receipt_path = planned_receipt_path
            # Hold every affected coordinate lock for the complete transaction.
            for source in sorted(units, key=str):
                destination = quarantine_dir / source.relative_to(target_dir.resolve())
                destination.parent.mkdir(parents=True, exist_ok=True)
                validate_lock_paths()
                source.rename(destination)
                moved.append(
                    CargoIncrementalQuarantineMove(str(source), str(destination))
                )
            receipt_path = quarantine_dir / "receipt.json"
            status = "quarantined"
            receipt = CargoIncrementalQuarantine(
                reason,
                recorded_at,
                str(target_dir),
                str(quarantine_dir),
                tuple(command),
                str(cwd),
                tuple(moved),
                (),
                (),
                str(receipt_path),
                "cleanup_pending",
                tuple(observations),
                tuple(
                    sorted(
                        eligible_observations,
                        key=lambda item: (item.rustc_pid, item.rustc_started_at_ns),
                    )
                ),
            )
            receipt = replace(receipt, admission_telemetry=admission_telemetry())
            _write_cargo_quarantine_receipt(
                receipt_path=receipt_path,
                payload={
                    **_cargo_quarantine_payload_required(receipt),
                    "planned_paths": planned_paths,
                },
            )
    except (OSError, ValueError, RuntimeError, ImportError) as exc:
        errors.append(_exception_diagnostic(exc))
        status = "partial" if moved else "deferred"
    finally:
        # A failed unlock/close must not strand other coordinate handles or
        # replace the interrupted command's structured recovery outcome.
        for lock_path, handle in reversed(handles):
            try:
                _release_file_lock(handle)
            except Exception as exc:
                errors.append(
                    f"Cargo coordinate lock release failed: {lock_path}: "
                    f"{_exception_diagnostic(exc)}"
                )
                status = "partial" if moved else "deferred"
            except BaseException as exc:
                # Preserve interruption while still trying every owned handle.
                if cleanup_abort is None:
                    cleanup_abort = exc
        if cleanup_abort is not None:
            raise cleanup_abort
    final_receipt = CargoIncrementalQuarantine(
        reason,
        recorded_at,
        str(target_dir),
        None if quarantine_dir is None else str(quarantine_dir),
        tuple(command),
        str(cwd),
        tuple(moved),
        (),
        tuple(errors),
        None if receipt_path is None else str(receipt_path),
        status,
        tuple(observations),
        tuple(
            sorted(
                eligible_observations,
                key=lambda item: (item.rustc_pid, item.rustc_started_at_ns),
            )
        ),
    )
    final_receipt = replace(final_receipt, admission_telemetry=admission_telemetry())
    if receipt_path is not None:
        try:
            # Cache mutation and its provisional receipt were lock-protected.
            # Only our unique quarantine metadata is finalized after cleanup;
            # pending/corrupt publication cannot be mistaken for completed custody.
            final_payload = _cargo_quarantine_payload_required(final_receipt)
            final_payload["planned_paths"] = planned_paths
            _write_cargo_quarantine_receipt(
                receipt_path=receipt_path,
                payload=final_payload,
            )
        except (OSError, ValueError, RuntimeError) as exc:
            errors.append(
                f"Cargo final receipt publication failed: {_exception_diagnostic(exc)}"
            )
            final_receipt = replace(
                final_receipt,
                ownership_status="partial" if moved else "deferred",
                errors=tuple(errors),
            )
    return final_receipt


def _cargo_quarantine_payload_required(
    receipt: CargoIncrementalQuarantine,
) -> dict[str, object]:
    payload = _cargo_incremental_quarantine_payload(receipt)
    assert payload is not None
    return payload


def _cargo_incremental_quarantine_message(
    receipt: CargoIncrementalQuarantine,
) -> str:
    moved_count = len(receipt.moved_paths)
    error_count = len(receipt.errors)
    if moved_count:
        base = (
            "memory_guard: quarantined Cargo incremental state after "
            f"{receipt.reason}: moved={moved_count} target_dir={receipt.target_dir} "
            f"quarantine_dir={receipt.quarantine_dir}"
        )
    else:
        base = (
            "memory_guard: checked Cargo incremental state after "
            f"{receipt.reason}: moved=0 target_dir={receipt.target_dir}"
        )
    if receipt.pruned_quarantine_dirs:
        base = f"{base} pruned={len(receipt.pruned_quarantine_dirs)}"
    if receipt.receipt_path:
        base = f"{base} receipt={receipt.receipt_path}"
    if error_count:
        base = f"{base} errors={error_count}"
    return base


def _cargo_recovery_next_action(receipt: CargoIncrementalQuarantine) -> str:
    """One evidence-preserving guidance contract for direct and CLI consumers."""
    location = (
        f"inspect retained receipt={receipt.receipt_path} before retrying"
        if receipt.receipt_path
        else "inspect cargo_incremental_quarantine in the guard result/summary JSON; no per-quarantine receipt was created"
    )
    return f"memory_guard: caches and quarantine evidence are retained; {location}."
