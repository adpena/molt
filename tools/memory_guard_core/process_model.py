from __future__ import annotations

from collections.abc import Callable, Collection, Mapping, Sequence
import contextlib
from dataclasses import dataclass
from datetime import datetime
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
from typing import Any, cast

from tools.memory_guard_core.windows_snapshot import (
    ProcessSnapshotError,
    _windows_process_snapshot_rows,
)


HOST_CONTROL_PLANE_TOKENS = (
    "/Applications/Codex.app/",
    "Codex.app/Contents/",
    "Codex (Renderer)",
    "Codex Helper",
    "OpenAI.Codex_",
    "/codex.app/",
    "\\app\\Codex.exe",
    "\\app\\resources\\codex.exe",
    "codex.cmd",
    "codex --",
    'codex.exe" app-server',
    "codex app-server",
    "codex-app-server",
    "codex-linux-sandbox",
    "codex-macos-sandbox",
    "codex-win32-sandbox",
    "codex.ps1",
    "codex_chronicle",
    "/.codex/",
    "/appdata/local/codex/",
    "/appdata/local/openai/codex/",
    "/appdata/local/temp/codex/",
    "/appdata/roaming/codex/",
    "/node_modules/@openai/codex/",
    "\\node_modules\\@openai\\codex\\",
    "@openai/codex",
    "/cua_node/bin/node_repl",
    "\\runtimes\\cua_node\\",
    "node_repl",
    "node_repl.exe",
    "/Applications/Claude.app/",
    "claude --",
    "\\claude.exe",
    "\\claude.cmd",
    "\\claude-code.exe",
    "\\node_modules\\@anthropic-ai\\claude-code\\",
    "Claude.app/Contents/",
    "/.claude/",
    "/appdata/local/temp/claude/",
    "@anthropic-ai/claude-code",
    "CLAUDE_PLUGIN_DATA=",
)
HOST_CONTROL_PLANE_EXECUTABLE_NAMES = frozenset(
    {
        "claude",
        "claude-code",
        "claude-code.exe",
        "claude.cmd",
        "claude.exe",
        "codex",
        "codex.appimage",
        "codex-cli",
        "codex-cli.exe",
        "codex.cmd",
        "codex.exe",
        "codex.ps1",
        "codex-app-server",
        "codex-linux-sandbox",
        "codex-macos-sandbox",
        "codex-win32-sandbox",
        "node_repl",
        "node_repl.exe",
    }
)
HOST_CONTROL_PLANE_ARG_EXECUTABLE_NAMES = (
    HOST_CONTROL_PLANE_EXECUTABLE_NAMES
    | frozenset(
        {
            "claude.js",
            "codex.js",
        }
    )
)
HOST_CONTROL_PLANE_LAUNCHER_NAMES = frozenset(
    {
        "bun",
        "bun.exe",
        "bash",
        "cmd",
        "cmd.exe",
        "deno",
        "deno.exe",
        "env",
        "fish",
        "node",
        "node.exe",
        "npm",
        "npm.cmd",
        "npx",
        "npx.cmd",
        "powershell",
        "powershell.exe",
        "pwsh",
        "pwsh.exe",
        "sh",
        "zsh",
    }
)
HOST_CONTROL_PLANE_LINEAGE_PROTECTED_EXECUTABLE_NAMES = (
    HOST_CONTROL_PLANE_LAUNCHER_NAMES
    | frozenset(
        {
            "conhost.exe",
            "git",
            "git.exe",
            "git-remote-https",
            "git-remote-https.exe",
            "openconsole.exe",
        }
    )
)


@dataclass(frozen=True, slots=True)
class ProcessSample:
    pid: int
    ppid: int
    rss_kb: int
    command: str
    pgid: int | None = None
    elapsed_sec: int | None = None
    started_at_ns: int | None = None


@dataclass(frozen=True, slots=True)
class ProcessIdentity:
    """Stable process-instance identity, independent of mutable execution state."""

    started_at_ns: int | None


def process_identity(sample: ProcessSample) -> ProcessIdentity:
    return ProcessIdentity(started_at_ns=sample.started_at_ns)


def process_identity_has_creation_marker(identity: ProcessIdentity) -> bool:
    """Return whether an identity can distinguish PID reuse by construction."""

    return identity.started_at_ns is not None


@dataclass(slots=True)
class ProcessTreeTracker:
    root_pid: int
    known_pids: set[int] | None = None
    known_pgids: set[int] | None = None
    known_identities: dict[int, ProcessIdentity] | None = None

    def __post_init__(self) -> None:
        if self.known_pids is None:
            self.known_pids = {self.root_pid}
        else:
            self.known_pids.add(self.root_pid)
        if self.known_pgids is None:
            self.known_pgids = {self.root_pid}
        else:
            self.known_pgids.add(self.root_pid)
        if self.known_identities is None:
            self.known_identities = {}

    def update(self, samples: Mapping[int, ProcessSample]) -> set[int]:
        """Return currently observed members of this process tree."""

        assert self.known_pids is not None
        assert self.known_pgids is not None
        assert self.known_identities is not None
        for pid in list(self.known_pids):
            sample = samples.get(pid)
            if sample is None:
                continue
            identity = process_identity(sample)
            known_identity = self.known_identities.get(pid)
            if known_identity is None:
                if process_identity_has_creation_marker(identity):
                    self.known_identities[pid] = identity
            elif not process_identity_has_creation_marker(identity):
                # An access-degraded sample cannot revoke strong historical
                # custody and cannot refresh it. Keep the last-good identity;
                # signal-time validation will fail closed until sampling
                # recovers the creation marker.
                continue
            elif known_identity != identity:
                self.known_pids.remove(pid)
                self.known_identities.pop(pid, None)
        changed = True
        live_known_pids = {
            pid
            for pid in self.known_pids
            if (sample := samples.get(pid)) is not None
            and (known_identity := self.known_identities.get(pid)) is not None
            and (current_identity := process_identity(sample)) == known_identity
            and process_identity_has_creation_marker(current_identity)
        }
        while changed:
            changed = False
            for sample in samples.values():
                sample_pgid = sample_pgid_or_pid(sample)
                # Historical PIDs remain known so a live reparented descendant
                # stays under custody.  An absent historical PID must not admit
                # new children: Windows can reuse that stale number, otherwise
                # unrelated processes contaminate RSS and termination scope.
                if sample.pid in self.known_pids or sample.ppid in live_known_pids:
                    if sample.pid not in self.known_pids:
                        self.known_pids.add(sample.pid)
                        identity = process_identity(sample)
                        if process_identity_has_creation_marker(identity):
                            self.known_identities[sample.pid] = identity
                        if sample.pid in self.known_identities:
                            live_known_pids.add(sample.pid)
                        changed = True
                    if (
                        sample.pid != self.root_pid or sample_pgid == self.root_pid
                    ) and sample_pgid not in self.known_pgids:
                        self.known_pgids.add(sample_pgid)
                        changed = True
        return {pid for pid in self.known_pids if pid in samples}

    def custody_identities(
        self,
        pids: Collection[int],
    ) -> dict[int, ProcessIdentity]:
        """Return the identities captured when each PID entered custody.

        A fresh sampler row is evidence about what owns a PID *now*; it must not
        replace the historical identity that made the PID part of this tree.
        Termination code compares these captured identities with a fresh sample
        before signaling so PID reuse cannot manufacture ownership.
        """

        assert self.known_identities is not None
        return {
            pid: identity
            for pid in pids
            if (identity := self.known_identities.get(pid)) is not None
        }


@dataclass(frozen=True, slots=True)
class RssViolation:
    pid: int
    rss_kb: int
    command: str
    scope: str = "process"

    @property
    def rss_gb(self) -> float:
        return self.rss_kb / (1024 * 1024)


@dataclass(frozen=True, slots=True)
class ChildExitResourceUsage:
    max_rss_kb: int


def elapsed_seconds_from_ps(value: str) -> int | None:
    raw = value.strip()
    if not raw:
        return None
    if raw.isdigit():
        return int(raw)
    days = 0
    time_part = raw
    if "-" in raw:
        day_part, time_part = raw.split("-", 1)
        if not day_part.isdigit():
            return None
        days = int(day_part)
    fields = time_part.split(":")
    if not 1 <= len(fields) <= 3 or any(not field.isdigit() for field in fields):
        return None
    values = [int(field) for field in fields]
    if len(values) == 3:
        hours, minutes, seconds = values
    elif len(values) == 2:
        hours = 0
        minutes, seconds = values
    else:
        hours = 0
        minutes = 0
        seconds = values[0]
    return (((days * 24) + hours) * 60 + minutes) * 60 + seconds


def parse_process_table(text: str) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        pid: int
        ppid: int
        rss_kb: int
        command: str
        pgid: int | None
        elapsed_sec: int | None = None
        parts = line.split(None, 5)
        if len(parts) >= 6:
            try:
                pid = int(parts[0])
                ppid = int(parts[1])
                pgid = int(parts[2])
                rss_kb = int(parts[3])
                elapsed_sec = elapsed_seconds_from_ps(parts[4])
                if elapsed_sec is None:
                    raise ValueError("elapsed process age is not parseable")
                command = parts[5]
            except ValueError:
                legacy_parts = line.split(None, 4)
                if len(legacy_parts) < 5:
                    continue
                try:
                    pid = int(legacy_parts[0])
                    ppid = int(legacy_parts[1])
                    pgid = int(legacy_parts[2])
                    rss_kb = int(legacy_parts[3])
                except ValueError:
                    fallback_parts = line.split(None, 3)
                    if len(fallback_parts) < 4:
                        continue
                    try:
                        pid = int(fallback_parts[0])
                        ppid = int(fallback_parts[1])
                        rss_kb = int(fallback_parts[2])
                    except ValueError:
                        continue
                    command = fallback_parts[3]
                    pgid = None
                else:
                    command = legacy_parts[4]
        elif len(parts) >= 5:
            try:
                pid = int(parts[0])
                ppid = int(parts[1])
                pgid = int(parts[2])
                rss_kb = int(parts[3])
                command = parts[4]
            except ValueError:
                legacy_parts = line.split(None, 3)
                if len(legacy_parts) < 4:
                    continue
                try:
                    pid = int(legacy_parts[0])
                    ppid = int(legacy_parts[1])
                    rss_kb = int(legacy_parts[2])
                except ValueError:
                    continue
                command = legacy_parts[3]
                pgid = None
        else:
            legacy_parts = line.split(None, 3)
            if len(legacy_parts) < 4:
                continue
            try:
                pid = int(legacy_parts[0])
                ppid = int(legacy_parts[1])
                rss_kb = int(legacy_parts[2])
            except ValueError:
                continue
            command = legacy_parts[3]
            pgid = None
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=ppid,
            rss_kb=rss_kb,
            command=command,
            pgid=pgid,
            elapsed_sec=elapsed_sec,
        )
    return samples


def _ps_lstart_ns(value: str) -> int | None:
    try:
        local_start = datetime.strptime(value, "%a %b %d %H:%M:%S %Y")
        return int(local_start.timestamp() * 1_000_000_000)
    except (OverflowError, ValueError):
        return None


def parse_process_table_with_start(text: str) -> dict[int, ProcessSample]:
    """Parse one `ps` snapshot that includes its stable `lstart` field."""

    samples: dict[int, ProcessSample] = {}
    now_ns = time.time_ns()
    for raw_line in text.splitlines():
        parts = raw_line.strip().split(None, 9)
        if len(parts) != 10:
            continue
        try:
            pid = int(parts[0])
            ppid = int(parts[1])
            pgid = int(parts[2])
            rss_kb = int(parts[3])
        except ValueError:
            continue
        started_at_ns = _ps_lstart_ns(" ".join(parts[4:9]))
        if pid <= 0 or started_at_ns is None:
            continue
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=max(0, ppid),
            rss_kb=max(0, rss_kb),
            command=parts[9],
            pgid=pgid,
            elapsed_sec=max(0, (now_ns - started_at_ns) // 1_000_000_000),
            started_at_ns=started_at_ns,
        )
    return samples


def _linux_proc_stat_identity(
    pid: int,
    proc_root: Path = Path("/proc"),
) -> tuple[int, int, int, str] | None:
    """Read parent, group, start marker, and comm from one `/proc` stat row."""

    if pid <= 0 or (
        not sys.platform.startswith("linux") and proc_root == Path("/proc")
    ):
        return None
    try:
        raw = (proc_root / str(pid) / "stat").read_text(encoding="utf-8")
        comm_end = raw.rindex(")")
        comm_start = raw.index("(") + 1
        command = raw[comm_start:comm_end]
        tail = raw[comm_end + 2 :].split()
        ppid = int(tail[1])
        pgid = int(tail[2])
        start_ticks = int(tail[19])
        ticks_per_second = (
            int(os.sysconf("SC_CLK_TCK"))
            if hasattr(os, "sysconf")
            else 100
        )
    except (IndexError, OSError, ValueError):
        return None
    if start_ticks < 0 or ticks_per_second <= 0:
        return None
    return (
        max(0, ppid),
        pgid,
        start_ticks * 1_000_000_000 // ticks_per_second,
        command,
    )


def _linux_proc_started_at_ns(pid: int) -> int | None:
    identity = _linux_proc_stat_identity(pid)
    return None if identity is None else identity[2]


def _linux_proc_command(
    pid: int,
    fallback: str,
    proc_root: Path = Path("/proc"),
) -> str:
    try:
        raw = (proc_root / str(pid) / "cmdline").read_bytes()
    except OSError:
        return fallback
    fields = [part.decode(errors="replace") for part in raw.split(b"\0") if part]
    return " ".join(fields) if fields else fallback


def _linux_proc_rss_kb(pid: int, proc_root: Path = Path("/proc")) -> int:
    try:
        lines = (proc_root / str(pid) / "status").read_text(
            encoding="utf-8"
        ).splitlines()
    except OSError:
        return 0
    for line in lines:
        if not line.startswith("VmRSS:"):
            continue
        fields = line.split()
        if len(fields) >= 2:
            with contextlib.suppress(ValueError):
                return max(0, int(fields[1]))
    return 0


def sample_processes_linux_proc(
    proc_root: Path = Path("/proc"),
    *,
    stat_reader: Callable[[int, Path], tuple[int, int, int, str] | None]
    | None = None,
    uptime_sec: float | None = None,
) -> dict[int, ProcessSample]:
    """Sample Linux processes with instance-bound ancestry and identity."""

    if stat_reader is None:
        stat_reader = _linux_proc_stat_identity
    samples: dict[int, ProcessSample] = {}
    try:
        pids = [
            int(entry.name)
            for entry in proc_root.iterdir()
            if entry.name.isdigit()
        ]
    except OSError as exc:
        raise ProcessSnapshotError(f"Linux /proc enumeration failed: {exc}") from exc
    if uptime_sec is None:
        try:
            uptime_sec = time.clock_gettime(time.CLOCK_BOOTTIME)
        except (AttributeError, OSError):
            try:
                uptime_sec = float(
                    (proc_root / "uptime")
                    .read_text(encoding="utf-8")
                    .split()[0]
                )
            except (IndexError, OSError, ValueError) as exc:
                raise ProcessSnapshotError(
                    f"Linux boot-time clock is unavailable: {exc}"
                ) from exc
    for pid in pids:
        before = stat_reader(pid, proc_root)
        if before is None:
            continue
        ppid, pgid, started_at_ns, comm = before
        command = _linux_proc_command(pid, comm, proc_root)
        rss_kb = _linux_proc_rss_kb(pid, proc_root)
        after = stat_reader(pid, proc_root)
        if after != before:
            continue
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=ppid,
            rss_kb=rss_kb,
            command=command,
            pgid=pgid,
            elapsed_sec=max(0, int(uptime_sec - started_at_ns / 1_000_000_000)),
            started_at_ns=started_at_ns,
        )
    if not samples:
        raise ProcessSnapshotError("Linux /proc snapshot contained no stable rows")
    return samples


@dataclass(frozen=True, slots=True)
class _DarwinProcessAuthority:
    """Process-wide Darwin FFI bindings shared by every sampler pass."""

    ctypes: Any
    libproc: Any
    libsystem: Any
    proc_bsd_info_type: type[Any]
    proc_pidinfo: Callable[..., int]
    sysctl: Callable[..., int]

    def metadata(self, pid: int) -> tuple[int, int, int, str] | None:
        info = self.proc_bsd_info_type()
        size = self.ctypes.sizeof(info)
        returned = self.proc_pidinfo(
            pid,
            3,
            0,
            self.ctypes.byref(info),
            size,
        )
        if returned != size or info.pbi_start_tvsec <= 0:
            return None
        started_at_ns = int(info.pbi_start_tvsec) * 1_000_000_000 + int(
            info.pbi_start_tvusec
        ) * 1_000
        raw_name = bytes(info.pbi_name).split(b"\0", 1)[0]
        if not raw_name:
            raw_name = bytes(info.pbi_comm).split(b"\0", 1)[0]
        command = raw_name.decode(errors="replace") or f"pid:{pid}"
        return int(info.pbi_ppid), int(info.pbi_pgid), started_at_ns, command

    def command(self, pid: int) -> str | None:
        mib = (self.ctypes.c_int * 3)(1, 49, pid)
        size = self.ctypes.c_size_t(0)
        if (
            self.sysctl(mib, 3, None, self.ctypes.byref(size), None, 0) != 0
            or size.value <= 4
        ):
            return None
        buffer = self.ctypes.create_string_buffer(size.value)
        if (
            self.sysctl(
                mib,
                3,
                buffer,
                self.ctypes.byref(size),
                None,
                0,
            )
            != 0
        ):
            return None
        raw = bytes(buffer.raw[: size.value])
        argc = int.from_bytes(raw[:4], sys.byteorder, signed=True)
        if argc <= 0:
            return None
        offset = raw.find(b"\0", 4)
        if offset < 0:
            return None
        offset += 1
        while offset < len(raw) and raw[offset] == 0:
            offset += 1
        argv: list[str] = []
        while offset < len(raw) and len(argv) < argc:
            end = raw.find(b"\0", offset)
            if end < 0:
                break
            argv.append(raw[offset:end].decode(errors="replace"))
            offset = end + 1
        return " ".join(argv) if len(argv) == argc else None


def _load_darwin_process_authority() -> _DarwinProcessAuthority:
    import ctypes

    class ProcBsdInfo(ctypes.Structure):
        _fields_ = [
            ("pbi_flags", ctypes.c_uint32),
            ("pbi_status", ctypes.c_uint32),
            ("pbi_xstatus", ctypes.c_uint32),
            ("pbi_pid", ctypes.c_uint32),
            ("pbi_ppid", ctypes.c_uint32),
            ("pbi_uid", ctypes.c_uint32),
            ("pbi_gid", ctypes.c_uint32),
            ("pbi_ruid", ctypes.c_uint32),
            ("pbi_rgid", ctypes.c_uint32),
            ("pbi_svuid", ctypes.c_uint32),
            ("pbi_svgid", ctypes.c_uint32),
            ("pbi_rfu_1", ctypes.c_uint32),
            ("pbi_comm", ctypes.c_char * 16),
            ("pbi_name", ctypes.c_char * 32),
            ("pbi_nfiles", ctypes.c_uint32),
            ("pbi_pgid", ctypes.c_uint32),
            ("pbi_pjobc", ctypes.c_uint32),
            ("e_tdev", ctypes.c_uint32),
            ("e_tpgid", ctypes.c_uint32),
            ("pbi_nice", ctypes.c_int32),
            ("pbi_start_tvsec", ctypes.c_uint64),
            ("pbi_start_tvusec", ctypes.c_uint64),
        ]

    libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    proc_pidinfo = libproc.proc_pidinfo
    proc_pidinfo.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    proc_pidinfo.restype = ctypes.c_int

    libsystem = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
    sysctl = libsystem.sysctl
    sysctl.argtypes = [
        ctypes.POINTER(ctypes.c_int),
        ctypes.c_uint,
        ctypes.c_void_p,
        ctypes.POINTER(ctypes.c_size_t),
        ctypes.c_void_p,
        ctypes.c_size_t,
    ]
    sysctl.restype = ctypes.c_int
    return _DarwinProcessAuthority(
        ctypes=ctypes,
        libproc=libproc,
        libsystem=libsystem,
        proc_bsd_info_type=ProcBsdInfo,
        proc_pidinfo=proc_pidinfo,
        sysctl=sysctl,
    )


_DARWIN_PROCESS_AUTHORITY_UNSET = object()
_darwin_process_authority_cache: _DarwinProcessAuthority | None | object = (
    _DARWIN_PROCESS_AUTHORITY_UNSET
)
_darwin_process_authority_lock = threading.Lock()


def _darwin_process_authority() -> _DarwinProcessAuthority | None:
    """Return the one cached Darwin authority, including cached unavailability."""

    global _darwin_process_authority_cache
    cached = _darwin_process_authority_cache
    if cached is _DARWIN_PROCESS_AUTHORITY_UNSET:
        with _darwin_process_authority_lock:
            cached = _darwin_process_authority_cache
            if cached is _DARWIN_PROCESS_AUTHORITY_UNSET:
                try:
                    cached = _load_darwin_process_authority()
                except (AttributeError, OSError, TypeError, ValueError):
                    cached = None
                _darwin_process_authority_cache = cached
    return None if cached is None else cast(_DarwinProcessAuthority, cached)


def _darwin_proc_metadata(pid: int) -> tuple[int, int, int, str] | None:
    """Return instance-bound Darwin parent, group, start marker, and name."""

    if sys.platform != "darwin" or pid <= 0:
        return None
    authority = _darwin_process_authority()
    if authority is None:
        return None
    try:
        return authority.metadata(pid)
    except (AttributeError, OSError, TypeError, ValueError):
        return None


def _darwin_proc_started_at_ns(pid: int) -> int | None:
    metadata = _darwin_proc_metadata(pid)
    return None if metadata is None else metadata[2]


def _darwin_proc_command(pid: int) -> str | None:
    """Read Darwin argv from KERN_PROCARGS2 for one process instance."""

    if sys.platform != "darwin" or pid <= 0:
        return None
    authority = _darwin_process_authority()
    if authority is None:
        return None
    try:
        return authority.command(pid)
    except (AttributeError, OSError, TypeError, ValueError):
        return None


def process_started_at_ns(pid: int) -> int | None:
    """Read one process creation marker without authorizing a whole snapshot."""

    if pid <= 0 or os.name == "nt":
        return None
    if sys.platform.startswith("linux"):
        return _linux_proc_started_at_ns(pid)
    if sys.platform == "darwin":
        return _darwin_proc_started_at_ns(pid)
    return None


def parse_windows_process_snapshot_rows(
    rows: Sequence[
        tuple[int, int, int, str, int | None]
        | tuple[int, int, int, str, int | None, int | None]
    ],
) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for row in rows:
        if len(row) == 5:
            pid, ppid, rss_kb, command, elapsed_sec = row
            started_at_ns = None
        else:
            pid, ppid, rss_kb, command, elapsed_sec, started_at_ns = row
        if pid <= 0:
            continue
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=max(0, ppid),
            rss_kb=max(0, rss_kb),
            command=command.strip() or f"pid:{pid}",
            pgid=None,
            elapsed_sec=elapsed_sec,
            started_at_ns=started_at_ns,
        )
    return samples


def sample_processes_posix() -> dict[int, ProcessSample]:
    if sys.platform.startswith("linux"):
        return sample_processes_linux_proc()
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,pgid=,rss=,lstart=,command="],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
            env={**os.environ, "LC_ALL": "C"},
        )
    except (OSError, subprocess.TimeoutExpired, TypeError) as exc:
        raise ProcessSnapshotError(f"POSIX process snapshot failed: {exc}") from exc
    if result.returncode != 0:
        raise ProcessSnapshotError(
            f"POSIX process snapshot failed with exit code {result.returncode}"
        )
    samples = parse_process_table_with_start(result.stdout)
    if sys.platform == "darwin":
        bound_samples: dict[int, ProcessSample] = {}
        for pid, sample in samples.items():
            before = _darwin_proc_metadata(pid)
            command = _darwin_proc_command(pid)
            after = _darwin_proc_metadata(pid)
            if before is None or before != after or command is None:
                bound_samples[pid] = ProcessSample(
                    pid=pid,
                    ppid=0,
                    rss_kb=sample.rss_kb,
                    command=sample.command,
                    pgid=sample.pgid,
                    elapsed_sec=sample.elapsed_sec,
                    started_at_ns=None,
                )
                continue
            ppid, pgid, started_at_ns, _native_name = before
            bound_samples[pid] = ProcessSample(
                pid=pid,
                ppid=max(0, ppid),
                rss_kb=sample.rss_kb,
                command=command,
                pgid=pgid,
                elapsed_sec=sample.elapsed_sec,
                started_at_ns=started_at_ns,
            )
        samples = bound_samples
    else:
        # Other BSDs retain observability but not signal authority until a
        # native subsecond creation marker is implemented for that kernel.
        samples = {
            pid: ProcessSample(
                pid=sample.pid,
                ppid=sample.ppid,
                rss_kb=sample.rss_kb,
                command=sample.command,
                pgid=sample.pgid,
                elapsed_sec=sample.elapsed_sec,
                started_at_ns=None,
            )
            for pid, sample in samples.items()
        }
    if not samples:
        raise ProcessSnapshotError("POSIX process snapshot contained no usable rows")
    return samples


def sample_processes_windows(
    snapshot_rows: Callable[
        [],
        Sequence[
            tuple[int, int, int, str, int | None]
            | tuple[int, int, int, str, int | None, int | None]
        ],
    ] = _windows_process_snapshot_rows,
) -> dict[int, ProcessSample]:
    try:
        rows = snapshot_rows()
    except ProcessSnapshotError:
        raise
    except (OSError, TypeError, AttributeError, TimeoutError) as exc:
        raise ProcessSnapshotError(f"Windows process snapshot failed: {exc}") from exc
    samples = parse_windows_process_snapshot_rows(rows)
    if not samples:
        raise ProcessSnapshotError("Windows process snapshot contained no usable rows")
    return samples


def sample_processes() -> dict[int, ProcessSample]:
    if os.name == "nt":
        return sample_processes_windows()
    return sample_processes_posix()


def sample_pgid_or_pid(sample: ProcessSample) -> int:
    return sample.pgid if sample.pgid is not None else sample.pid


def command_executable_name(command: str) -> str:
    text = command.strip()
    if not text:
        return ""
    if text[0] in {"'", '"'}:
        quote = text[0]
        end = text.find(quote, 1)
        token = text[1:end] if end > 0 else text[1:]
    elif re.match(r"(?i)^[a-z]:[\\/]", text) or text.startswith(("\\\\", "//")):
        match = re.match(r"(?is)^(.+?\.(?:exe|cmd|bat|com))(?:\s|$)", text)
        token = match.group(1) if match else text.split(None, 1)[0]
    else:
        token = text.split(None, 1)[0]
    return token.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def command_arg_executable_names(command: str) -> tuple[str, ...]:
    names: list[str] = []
    for match in re.finditer(r"""(?:"([^"]+)"|'([^']+)'|(\S+))""", command.strip()):
        token = next(group for group in match.groups() if group is not None)
        normalized = token.replace("\\", "/").rstrip("/")
        name = normalized.rsplit("/", 1)[-1].casefold()
        if name:
            names.append(name)
    return tuple(names)


def _host_control_plane_launcher_command(command: str) -> bool:
    names = command_arg_executable_names(command)
    if len(names) < 2 or names[0] not in HOST_CONTROL_PLANE_LAUNCHER_NAMES:
        return False
    return any(name in HOST_CONTROL_PLANE_ARG_EXECUTABLE_NAMES for name in names[1:])


def is_host_control_plane_process(sample: ProcessSample) -> bool:
    command = sample.command.casefold()
    normalized_command = command.replace("\\", "/")
    return (
        any(
            token.casefold() in command
            or token.casefold().replace("\\", "/") in normalized_command
            for token in HOST_CONTROL_PLANE_TOKENS
        )
        or command_executable_name(sample.command)
        in HOST_CONTROL_PLANE_EXECUTABLE_NAMES
        or _host_control_plane_launcher_command(sample.command)
    )


def host_control_plane_ancestor_pids(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
    *,
    include_self: bool = False,
) -> set[int]:
    ancestors = ancestor_pids(samples, pid)
    if not include_self and pid is not None:
        ancestors.discard(pid)
    return {
        ancestor
        for ancestor in ancestors
        if (sample := samples.get(ancestor)) is not None
        and is_host_control_plane_process(sample)
    }


def has_host_control_plane_ancestor(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
    *,
    include_self: bool = False,
) -> bool:
    return bool(
        host_control_plane_ancestor_pids(
            samples,
            pid,
            include_self=include_self,
        )
    )


def has_external_host_control_plane_lineage(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
    *,
    current_pid: int | None = None,
    include_self: bool = True,
    owned_pids: Collection[int] = (),
) -> bool:
    """Return true when pid belongs to protected host-control lineage.

    Codex/Claude/app-server/renderer/node-repl processes are the operator control
    plane. Their descendants are protected unless a caller proves Molt ownership
    by passing an explicit owned PID set. Being a descendant of the currently
    running guard process is not ownership by itself: Codex-launched shell/Git/
    launcher helpers remain protected even under the guard.
    """

    if pid is None or pid <= 0:
        return False
    sample = samples.get(pid)
    if sample is None:
        return False
    host_lineage = has_host_control_plane_ancestor(
        samples,
        pid,
        include_self=include_self,
    )
    if not host_lineage:
        return False
    if pid not in owned_pids:
        return True
    if current_pid is None or current_pid <= 0:
        return True
    if pid not in descendant_pids(samples, current_pid):
        return True
    executable = command_executable_name(sample.command)
    if executable in HOST_CONTROL_PLANE_LINEAGE_PROTECTED_EXECUTABLE_NAMES:
        return True
    return is_host_control_plane_process(sample)


_ORPHAN_ROOT_PPIDS = frozenset({0, 1})


def ancestry_resolves_to_confirmed_orphan(
    samples: Mapping[int, ProcessSample],
    pid: int,
    *,
    host_control_plane_pids: set[int] | None = None,
) -> bool:
    """Return true only when ancestry is fully observed to a non-host orphan root.

    Heuristic repo-scope process matching is intentionally weaker than explicit
    guard custody. If a real parent PID is absent from the snapshot, especially
    on Windows, that missing link could hide a Codex or Claude ancestor. Treat
    that uncertainty as protected unless a caller supplies explicit ownership.
    """

    if pid <= 0:
        return False
    if host_control_plane_pids is None:
        host_control_plane_pids = {
            sample.pid
            for sample in samples.values()
            if is_host_control_plane_process(sample)
        }
    seen: set[int] = set()
    current = pid
    while True:
        if current in host_control_plane_pids:
            return False
        if current in seen:
            return True
        seen.add(current)
        sample = samples.get(current)
        if sample is None:
            return False
        ppid = sample.ppid
        if ppid in _ORPHAN_ROOT_PPIDS or ppid == current:
            return True
        if ppid <= 0:
            return True
        if ppid not in samples:
            return False
        current = ppid


def ancestor_pids(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
) -> set[int]:
    if pid is None or pid <= 0:
        return set()
    ancestors: set[int] = set()
    current = pid
    while current > 0 and current not in ancestors:
        ancestors.add(current)
        sample = samples.get(current)
        if sample is None or sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return ancestors


def descendant_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for sample in samples.values():
            if sample.pid in descendants:
                continue
            if sample.ppid in descendants:
                descendants.add(sample.pid)
                changed = True
    return descendants


def protected_process_group_ids(
    samples: Mapping[int, ProcessSample],
    *,
    self_pid: int | None = None,
    self_pgid: int | None = None,
    owned_pids: Collection[int] = (),
) -> set[int]:
    protected: set[int] = set()
    if self_pgid is not None and self_pgid > 0:
        protected.add(self_pgid)
    ancestor_ids = ancestor_pids(samples, self_pid)
    self_descendant_ids = descendant_pids(samples, self_pid) if self_pid else set()
    explicitly_owned = set(owned_pids) | self_descendant_ids
    host_control_plane_pids = {
        sample.pid
        for sample in samples.values()
        if is_host_control_plane_process(sample)
    }
    for sample in samples.values():
        if sample.pid in ancestor_ids or is_host_control_plane_process(sample):
            protected.add(sample_pgid_or_pid(sample))
            continue
        sample_ancestors = ancestor_pids(samples, sample.pid)
        if (
            host_control_plane_pids.intersection(sample_ancestors)
            and sample.pid not in self_descendant_ids
        ):
            protected.add(sample_pgid_or_pid(sample))
            continue
        if (
            sample.pid not in explicitly_owned
            and not ancestry_resolves_to_confirmed_orphan(
                samples,
                sample.pid,
                host_control_plane_pids=host_control_plane_pids,
            )
        ):
            protected.add(sample_pgid_or_pid(sample))
    return protected


def root_pid_is_kill_eligible(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    protected_pgids: set[int],
    root_owned: bool,
    current_pid: int,
) -> bool:
    if root_pid <= 0 or root_pid == current_pid:
        return False
    sample = samples.get(root_pid)
    if sample is None:
        return False
    if has_external_host_control_plane_lineage(
        samples,
        root_pid,
        current_pid=current_pid,
        owned_pids={root_pid} if root_owned else (),
    ):
        return False
    return sample_pgid_or_pid(
        sample
    ) not in protected_pgids and not is_host_control_plane_process(sample)


def filter_protected_watched_pids(
    samples: Mapping[int, ProcessSample],
    watched: set[int],
    *,
    protected_pgids: set[int],
    current_pid: int | None = None,
) -> set[int]:
    filtered: set[int] = set()
    owned_pids = frozenset(watched)
    for pid in watched:
        sample = samples.get(pid)
        if sample is None:
            continue
        if has_external_host_control_plane_lineage(
            samples,
            pid,
            current_pid=current_pid,
            owned_pids=owned_pids,
        ):
            continue
        if is_host_control_plane_process(sample):
            continue
        if sample_pgid_or_pid(sample) in protected_pgids:
            continue
        filtered.add(pid)
    return filtered


def watched_pids(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    tracker: ProcessTreeTracker | None = None,
    protected_pgids: set[int] | None = None,
) -> set[int]:
    observed = (
        tracker.update(samples)
        if tracker is not None
        else descendant_pids(
            samples,
            root_pid,
        )
    )
    return filter_protected_watched_pids(
        samples,
        observed,
        protected_pgids=set() if protected_pgids is None else protected_pgids,
        current_pid=os.getpid(),
    )


def peak_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
    protected_pgids: set[int] | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(
            samples,
            root_pid,
            tracker=tracker,
            protected_pgids=protected_pgids,
        )
    )
    candidates = [sample for pid, sample in samples.items() if pid in observed]
    if not candidates:
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def total_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
    protected_pgids: set[int] | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(
            samples,
            root_pid,
            tracker=tracker,
            protected_pgids=protected_pgids,
        )
    )
    candidates = [sample for pid, sample in samples.items() if pid in observed]
    if not candidates:
        return None
    return RssViolation(
        pid=root_pid,
        rss_kb=sum(sample.rss_kb for sample in candidates),
        command="process tree aggregate",
        scope="process_tree",
    )


def find_rss_violation(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
    protected_pgids: set[int] | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(
            samples,
            root_pid,
            tracker=tracker,
            protected_pgids=protected_pgids,
        )
    )
    candidates = [
        sample
        for pid, sample in samples.items()
        if pid in observed and sample.rss_kb > max_rss_kb
    ]
    if not candidates:
        if max_total_rss_kb is None:
            return None
        aggregate = total_rss(samples, root_pid=root_pid, watched=observed)
        if aggregate is not None and aggregate.rss_kb > max_total_rss_kb:
            return aggregate
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )
