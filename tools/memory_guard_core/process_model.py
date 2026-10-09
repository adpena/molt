from __future__ import annotations

from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from functools import lru_cache
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import threading
import time
from typing import Any, cast

from tools.memory_guard_core.windows_snapshot import (
    ProcessSnapshotError,
    _windows_process_snapshot_rows_hard_timeout,
    windows_current_process_started_at_ns,
)


# These are conservative host-protection signatures, never Molt ownership.
# Agent home directories also contain ordinary project checkouts and data: only
# their known helper locations belong here. Retained instance/ancestry custody
# and the independent executable protections still decide cleanup eligibility.
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
    "/.codex/plugins/",
    "/.codex/runtimes/",
    "/.codex/vendor_imports/",
    "/.codex/shell_snapshots/",
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
    "/.claude/plugins/",
    "/.claude/runtimes/",
    "/.claude/shell-snapshots/",
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
    # None uses the native sampler's command source; () explicitly means unknown.
    argv: tuple[str, ...] | None = None
    command_kind: str = "full"


@dataclass(frozen=True, slots=True)
class ProcessIdentity:
    """Stable process-instance identity, independent of mutable execution state."""

    started_at_ns: int | None


def process_identity(sample: ProcessSample) -> ProcessIdentity:
    return ProcessIdentity(started_at_ns=sample.started_at_ns)


def process_identity_has_creation_marker(identity: ProcessIdentity) -> bool:
    """Return whether an identity can distinguish PID reuse by construction."""

    return identity.started_at_ns is not None


def process_births_are_ordered(
    parent_started_at_ns: object, child_started_at_ns: object
) -> bool:
    """Admit an ancestry edge only between positive exact births in time order.

    Windows retains a dead parent's PID in its children. A live PID match is
    insufficient if that number now identifies a process younger than the child.
    Equal births are valid because native creation clocks have finite precision.
    """
    return (
        type(parent_started_at_ns) is int
        and type(child_started_at_ns) is int
        and 0 < parent_started_at_ns <= child_started_at_ns
    )


@dataclass(frozen=True, slots=True)
class ProcessAncestry:
    identity: ProcessIdentity
    admitted_at_ns: int
    ancestors: tuple[tuple[int, ProcessIdentity], ...]


@dataclass(slots=True)
class ProcessTreeTracker:
    root_pid: int
    known_pids: set[int] | None = None
    known_pgids: set[int] | None = None
    known_identities: dict[int, ProcessIdentity] | None = None
    released_identities: dict[int, ProcessIdentity] | None = None
    # Live admission evidence, frozen with the admitted process birth. It is
    # descriptive custody only, never an additional termination authority.
    known_ancestry: dict[int, ProcessAncestry] | None = None

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
        if self.released_identities is None:
            self.released_identities = {}
        if self.known_ancestry is None:
            self.known_ancestry = {}

    def update(
        self,
        samples: Mapping[int, ProcessSample],
        *,
        observed_at_ns: int | None = None,
    ) -> set[int]:
        """Return currently observed members of this process tree."""

        assert self.known_pids is not None
        assert self.known_pgids is not None
        assert self.known_identities is not None
        assert self.released_identities is not None
        assert self.known_ancestry is not None
        admitted_at_ns = (
            time.monotonic_ns() if observed_at_ns is None else observed_at_ns
        )
        for pid in list(self.known_pids):
            sample = samples.get(pid)
            if sample is None:
                continue
            identity = process_identity(sample)
            ancestry = self.known_ancestry.get(pid)
            if (
                ancestry is not None
                and process_identity_has_creation_marker(identity)
                and ancestry.identity != identity
            ):
                self.known_ancestry.pop(pid)
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
                self.known_ancestry.pop(pid, None)
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
                released = self.released_identities.get(sample.pid)
                current = process_identity(sample)
                if released is not None and (
                    not process_identity_has_creation_marker(current)
                    or released == current
                ):
                    continue  # Exact receiver-owned instances cannot be re-adopted.
                # Historical PIDs remain known so a live reparented descendant
                # stays under custody.  An absent historical PID must not admit
                # new children: Windows can reuse that stale number, otherwise
                # unrelated processes contaminate RSS and termination scope.
                parent = samples.get(sample.ppid)
                parent_admitted = (
                    sample.ppid in live_known_pids
                    and sample.ppid != sample.pid
                    and parent is not None
                    and process_births_are_ordered(
                        parent.started_at_ns, sample.started_at_ns
                    )
                )
                if sample.pid in self.known_pids or parent_admitted:
                    if sample.pid not in self.known_pids:
                        self.known_pids.add(sample.pid)
                        identity = process_identity(sample)
                        if process_identity_has_creation_marker(identity):
                            self.known_identities[sample.pid] = identity
                        if sample.pid in self.known_identities:
                            live_known_pids.add(sample.pid)
                        changed = True
                    if (
                        sample.pid != self.root_pid
                        and sample.pid not in self.known_ancestry
                        and sample.pid in live_known_pids
                        and parent_admitted
                    ):
                        parent_chain = self.known_ancestry.get(sample.ppid)
                        # Wait for an in-tree parent to acquire its chain first.
                        # This keeps row ordering from truncating live lineage.
                        if sample.ppid == self.root_pid or parent_chain is not None:
                            self.known_ancestry[sample.pid] = ProcessAncestry(
                                current,
                                admitted_at_ns,
                                ((sample.ppid, self.known_identities[sample.ppid]),)
                                + (
                                    ()
                                    if parent_chain is None
                                    else parent_chain.ancestors
                                ),
                            )
                            changed = True
                    if (
                        sample.pid != self.root_pid or sample_pgid == self.root_pid
                    ) and sample_pgid not in self.known_pgids:
                        self.known_pgids.add(sample_pgid)
                        changed = True
        return {pid for pid in self.known_pids if pid in samples}

    def transfer_process_group(
        self,
        pgid: int,
        *,
        samples: Mapping[int, ProcessSample],
        identities: Mapping[int, ProcessIdentity],
    ) -> bool:
        """Release exactly one explicitly adopted, birth-verified child group.

        The receiving suite must acknowledge ownership before this is called.
        A transfer never grants an exemption to this guard's own root group.
        """
        watched = self.update(samples)
        members = {
            pid for pid, sample in samples.items() if sample_pgid_or_pid(sample) == pgid
        }
        if (
            pgid == self.root_pid
            or self.root_pid in identities
            or not members
            or not members <= set(identities)
            or not set(identities) <= watched
        ):
            return False
        if any(
            not process_identity_has_creation_marker(identities[pid])
            or process_identity(samples[pid]) != identities[pid]
            or self.custody_identities(identities).get(pid) != identities[pid]
            for pid in identities
        ):
            return False
        assert self.released_identities is not None
        self.released_identities.update(identities)
        assert self.known_pids is not None and self.known_identities is not None
        assert self.known_pgids is not None
        self.known_pids.difference_update(identities)
        assert self.known_ancestry is not None
        for pid in identities:
            self.known_identities.pop(pid, None)
            self.known_ancestry.pop(pid, None)
        self.known_pgids.discard(pgid)
        return True

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

    def cut_ancestry_at(
        self, identities: Mapping[int, ProcessIdentity], *, observed_at_ns: int
    ) -> None:
        """Retire earlier ancestry when exact births transfer to another owner.

        Membership/termination custody is unchanged. A receiving suite must not
        attribute its adopted daemon, or that daemon's children, to a former
        request owner even after the daemon disappears from a later snapshot.
        """
        assert self.known_ancestry is not None
        for pid, identity in self.custody_identities(identities).items():
            if identities[pid] == identity:
                previous = self.known_ancestry.get(pid)
                self.known_ancestry[pid] = ProcessAncestry(
                    identity,
                    observed_at_ns if previous is None else previous.admitted_at_ns,
                    (),
                )
        for pid, chain in tuple(self.known_ancestry.items()):
            for index, (ancestor, birth) in enumerate(chain.ancestors):
                if identities.get(ancestor) == birth:
                    self.known_ancestry[pid] = ProcessAncestry(
                        chain.identity,
                        chain.admitted_at_ns,
                        chain.ancestors[: index + 1],
                    )
                    break

    def custody_ancestry_payload(
        self,
        samples: Mapping[int, ProcessSample],
        *,
        excluded_roots: Collection[int] = (),
    ) -> list[dict[str, object]]:
        """Serialize only birth-bound chains admitted by this live tree tracker."""
        assert self.known_ancestry is not None
        records = []
        for pid, identity in self.custody_identities(samples).items():
            chain = self.known_ancestry.get(pid)
            if (
                chain is None
                or not chain.ancestors
                or chain.identity != identity
                or process_identity(samples[pid]) != identity
                or pid in excluded_roots
                or any(parent in excluded_roots for parent, _birth in chain.ancestors)
            ):
                continue
            records.append(
                {
                    "pid": pid,
                    "started_at_ns": identity.started_at_ns,
                    "admitted_at_ns": chain.admitted_at_ns,
                    "ancestors": [
                        {"pid": ancestor, "started_at_ns": born.started_at_ns}
                        for ancestor, born in chain.ancestors
                    ],
                }
            )
        return records


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


def _linux_proc_stat_row(
    pid: int,
    proc_root: Path = Path("/proc"),
) -> tuple[tuple[int, int, int, str], int] | None:
    """Read one `/proc` stat row: the instance identity plus resident kB.

    The identity is (parent, group, start marker, comm). Field 24 carries the
    same resident-set counter that `status` reports as `VmRSS`, so one read
    serves both the ancestry sample and the memory accounting.
    """

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
        rss_pages = int(tail[21])
        ticks_per_second = (
            int(os.sysconf("SC_CLK_TCK")) if hasattr(os, "sysconf") else 100
        )
    except (IndexError, OSError, ValueError):
        return None
    if start_ticks < 0 or ticks_per_second <= 0:
        return None
    identity = (
        max(0, ppid),
        pgid,
        start_ticks * 1_000_000_000 // ticks_per_second,
        command,
    )
    return identity, max(0, rss_pages) * _LINUX_PAGE_KB


_LINUX_PAGE_KB = max(
    1, (os.sysconf("SC_PAGE_SIZE") if hasattr(os, "sysconf") else 4096) // 1024
)


def _linux_proc_stat_identity(
    pid: int,
    proc_root: Path = Path("/proc"),
) -> tuple[int, int, int, str] | None:
    """Read parent, group, start marker, and comm from one `/proc` stat row."""

    row = _linux_proc_stat_row(pid, proc_root)
    return None if row is None else row[0]


def _linux_proc_started_at_ns(pid: int) -> int | None:
    identity = _linux_proc_stat_identity(pid)
    return None if identity is None else identity[2]


def _linux_proc_argv(
    pid: int, proc_root: Path = Path("/proc")
) -> tuple[str, ...] | None:
    """Preserve kernel argv boundaries, including empty and whitespace arguments."""
    try:
        raw = (proc_root / str(pid) / "cmdline").read_bytes()
    except OSError:
        return None
    if not raw or not raw.endswith(b"\0"):
        return None
    fields = tuple(
        part.decode(errors="surrogateescape") for part in raw.split(b"\0")[:-1]
    )
    return fields if fields and fields[0] else None


def _linux_proc_command(
    pid: int, fallback: str, proc_root: Path = Path("/proc")
) -> str:
    argv = _linux_proc_argv(pid, proc_root)
    return shlex.join(argv) if argv is not None else fallback


def _linux_proc_rss_kb(pid: int, proc_root: Path = Path("/proc")) -> int:
    row = _linux_proc_stat_row(pid, proc_root)
    return 0 if row is None else row[1]


def sample_processes_linux_proc(
    proc_root: Path = Path("/proc"),
    *,
    stat_reader: Callable[[int, Path], tuple[int, int, int, str] | None] | None = None,
    uptime_sec: float | None = None,
) -> dict[int, ProcessSample]:
    """Sample Linux processes with instance-bound ancestry and identity."""

    samples: dict[int, ProcessSample] = {}
    try:
        pids = [
            int(entry.name) for entry in proc_root.iterdir() if entry.name.isdigit()
        ]
    except OSError as exc:
        raise ProcessSnapshotError(f"Linux /proc enumeration failed: {exc}") from exc
    if uptime_sec is None:
        try:
            uptime_sec = time.clock_gettime(time.CLOCK_BOOTTIME)
        except (AttributeError, OSError):
            try:
                uptime_sec = float(
                    (proc_root / "uptime").read_text(encoding="utf-8").split()[0]
                )
            except (IndexError, OSError, ValueError) as exc:
                raise ProcessSnapshotError(
                    f"Linux boot-time clock is unavailable: {exc}"
                ) from exc
    injected_reader = stat_reader is not None
    if stat_reader is None:
        stat_reader = _linux_proc_stat_identity
    for pid in pids:
        if injected_reader:
            before = stat_reader(pid, proc_root)
            rss_kb = _linux_proc_rss_kb(pid, proc_root)
        else:
            row = _linux_proc_stat_row(pid, proc_root)
            before, rss_kb = (None, 0) if row is None else row
        if before is None:
            continue
        ppid, pgid, started_at_ns, comm = before
        argv = _linux_proc_argv(pid, proc_root)
        command = shlex.join(argv) if argv is not None else comm
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
            argv=argv,
        )
    if not samples:
        raise ProcessSnapshotError("Linux /proc snapshot contained no stable rows")
    return samples


# ``kern.proc`` ``p_stat`` of a process that exited and awaits its parent's
# ``wait()``. XNU keeps it listed under ``proc_listallpids`` with its birth,
# parent and group intact, while every ``proc_pidinfo`` flavor answers ESRCH.
_DARWIN_SZOMB = 5
_DARWIN_KINFO_PROC_SIZE = 648


@dataclass(frozen=True, slots=True)
class _DarwinKernelProcRow:
    """One ``kern.proc.pid`` row: the kernel's own state and instance identity."""

    status: int
    ppid: int
    pgid: int
    started_at_ns: int
    command: str


@dataclass(frozen=True, slots=True)
class _DarwinProcessAuthority:
    """Process-wide Darwin FFI bindings shared by every sampler pass."""

    ctypes: Any
    libproc: Any
    libsystem: Any
    proc_bsd_info_type: type[Any]
    proc_task_info_type: type[Any]
    kinfo_proc_type: type[Any]
    proc_pidinfo: Callable[..., int]
    proc_listallpids: Callable[..., int]
    sysctl: Callable[..., int]

    def kernel_row(self, pid: int) -> _DarwinKernelProcRow | None:
        """Read one ``kern.proc.pid`` row; None once the pid has been reaped.

        This is the table ``ps`` reads. Unlike ``proc_pidinfo`` it still
        answers for a process that exited and awaits ``wait()``, and its
        ``p_starttime`` is the same birth clock as ``pbi_start_tvsec``, so an
        identity bound here equals one bound through libproc.
        """
        info = self.kinfo_proc_type()
        expected = self.ctypes.sizeof(info)
        size = self.ctypes.c_size_t(expected)
        # CTL_KERN, KERN_PROC, KERN_PROC_PID
        mib = (self.ctypes.c_int * 4)(1, 14, 1, pid)
        if (
            self.sysctl(
                mib, 4, self.ctypes.byref(info), self.ctypes.byref(size), None, 0
            )
            != 0
            or size.value != expected
            or int(info.kp_proc.p_pid) != pid
        ):
            return None
        start = info.kp_proc.p_starttime
        started_at_ns = int(start.tv_sec) * 1_000_000_000 + int(start.tv_usec) * 1_000
        if started_at_ns <= 0:
            return None
        raw_name = bytes(info.kp_proc.p_comm).split(b"\0", 1)[0]
        return _DarwinKernelProcRow(
            status=int(info.kp_proc.p_stat),
            ppid=int(info.kp_eproc.e_ppid),
            pgid=int(info.kp_eproc.e_pgid),
            started_at_ns=started_at_ns,
            command=raw_name.decode(errors="replace") or f"pid:{pid}",
        )

    def pids(self) -> list[int]:
        """Enumerate every pid the kernel lists, exited-unreaped ones included."""
        count = self.proc_listallpids(None, 0)
        if count <= 0:
            raise OSError("proc_listallpids reported no processes")
        buffer = (self.ctypes.c_int * (count + 64))()
        returned = self.proc_listallpids(buffer, self.ctypes.sizeof(buffer))
        if returned <= 0:
            raise OSError("proc_listallpids failed")
        return [int(buffer[index]) for index in range(returned) if buffer[index] > 0]

    def resident_kb(self, pid: int) -> int | None:
        """Resident set in kB; None when the kernel withholds task info."""
        info = self.proc_task_info_type()
        size = self.ctypes.sizeof(info)
        returned = self.proc_pidinfo(pid, 4, 0, self.ctypes.byref(info), size)
        if returned != size:
            return None
        return int(info.pti_resident_size) // 1024

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
        started_at_ns = (
            int(info.pbi_start_tvsec) * 1_000_000_000
            + int(info.pbi_start_tvusec) * 1_000
        )
        raw_name = bytes(info.pbi_name).split(b"\0", 1)[0]
        if not raw_name:
            raw_name = bytes(info.pbi_comm).split(b"\0", 1)[0]
        command = raw_name.decode(errors="replace") or f"pid:{pid}"
        return int(info.pbi_ppid), int(info.pbi_pgid), started_at_ns, command

    def argv(self, pid: int) -> tuple[str, ...] | None:
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
            argv.append(raw[offset:end].decode(errors="surrogateescape"))
            offset = end + 1
        return tuple(argv) if len(argv) == argc else None

    def command(self, pid: int) -> str | None:
        argv = self.argv(pid)
        return shlex.join(argv) if argv is not None else None


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

    class ProcTaskInfo(ctypes.Structure):
        _fields_ = [
            ("pti_virtual_size", ctypes.c_uint64),
            ("pti_resident_size", ctypes.c_uint64),
            ("pti_total_user", ctypes.c_uint64),
            ("pti_total_system", ctypes.c_uint64),
            ("pti_threads_user", ctypes.c_uint64),
            ("pti_threads_system", ctypes.c_uint64),
            ("pti_policy", ctypes.c_int32),
            ("pti_faults", ctypes.c_int32),
            ("pti_pageins", ctypes.c_int32),
            ("pti_cow_faults", ctypes.c_int32),
            ("pti_messages_sent", ctypes.c_int32),
            ("pti_messages_received", ctypes.c_int32),
            ("pti_syscalls_mach", ctypes.c_int32),
            ("pti_syscalls_unix", ctypes.c_int32),
            ("pti_csw", ctypes.c_int32),
            ("pti_threadnum", ctypes.c_int32),
            ("pti_numrunning", ctypes.c_int32),
            ("pti_priority", ctypes.c_int32),
        ]

    # <sys/sysctl.h> struct kinfo_proc, the row ``kern.proc`` returns, laid
    # out for the 64-bit ABI. Only p_starttime, p_stat, p_pid, p_comm, e_ppid
    # and e_pgid are read; every other field exists to keep those offsets
    # exact, and the total size is checked against the kernel's.
    class Timeval(ctypes.Structure):
        _fields_ = [("tv_sec", ctypes.c_int64), ("tv_usec", ctypes.c_int32)]

    class Itimerval(ctypes.Structure):
        _fields_ = [("it_interval", Timeval), ("it_value", Timeval)]

    class ExternProc(ctypes.Structure):
        _fields_ = [
            ("p_starttime", Timeval),
            ("p_vmspace", ctypes.c_void_p),
            ("p_sigacts", ctypes.c_void_p),
            ("p_flag", ctypes.c_int32),
            ("p_stat", ctypes.c_int8),
            ("p_pid", ctypes.c_int32),
            ("p_oppid", ctypes.c_int32),
            ("p_dupfd", ctypes.c_int32),
            ("user_stack", ctypes.c_void_p),
            ("exit_thread", ctypes.c_void_p),
            ("p_debugger", ctypes.c_int32),
            ("sigwait", ctypes.c_int32),
            ("p_estcpu", ctypes.c_uint32),
            ("p_cpticks", ctypes.c_int32),
            ("p_pctcpu", ctypes.c_uint32),
            ("p_wchan", ctypes.c_void_p),
            ("p_wmesg", ctypes.c_void_p),
            ("p_swtime", ctypes.c_uint32),
            ("p_slptime", ctypes.c_uint32),
            ("p_realtimer", Itimerval),
            ("p_rtime", Timeval),
            ("p_uticks", ctypes.c_uint64),
            ("p_sticks", ctypes.c_uint64),
            ("p_iticks", ctypes.c_uint64),
            ("p_traceflag", ctypes.c_int32),
            ("p_tracep", ctypes.c_void_p),
            ("p_siglist", ctypes.c_int32),
            ("p_textvp", ctypes.c_void_p),
            ("p_holdcnt", ctypes.c_int32),
            ("p_sigmask", ctypes.c_uint32),
            ("p_sigignore", ctypes.c_uint32),
            ("p_sigcatch", ctypes.c_uint32),
            ("p_priority", ctypes.c_uint8),
            ("p_usrpri", ctypes.c_uint8),
            ("p_nice", ctypes.c_int8),
            ("p_comm", ctypes.c_char * 17),
            ("p_pgrp", ctypes.c_void_p),
            ("p_addr", ctypes.c_void_p),
            ("p_xstat", ctypes.c_uint16),
            ("p_acflag", ctypes.c_uint16),
            ("p_ru", ctypes.c_void_p),
        ]

    class Pcred(ctypes.Structure):
        _fields_ = [
            ("pc_lock", ctypes.c_char * 72),
            ("pc_ucred", ctypes.c_void_p),
            ("p_ruid", ctypes.c_uint32),
            ("p_svuid", ctypes.c_uint32),
            ("p_rgid", ctypes.c_uint32),
            ("p_svgid", ctypes.c_uint32),
            ("p_refcnt", ctypes.c_int32),
        ]

    class Ucred(ctypes.Structure):
        _fields_ = [
            ("cr_ref", ctypes.c_int32),
            ("cr_uid", ctypes.c_uint32),
            ("cr_ngroups", ctypes.c_int16),
            ("cr_groups", ctypes.c_uint32 * 16),
        ]

    class Vmspace(ctypes.Structure):
        _fields_ = [
            ("dummy", ctypes.c_int32),
            ("dummy2", ctypes.c_void_p),
            ("dummy3", ctypes.c_int32 * 5),
            ("dummy4", ctypes.c_void_p * 3),
        ]

    class Eproc(ctypes.Structure):
        _fields_ = [
            ("e_paddr", ctypes.c_void_p),
            ("e_sess", ctypes.c_void_p),
            ("e_pcred", Pcred),
            ("e_ucred", Ucred),
            ("e_vm", Vmspace),
            ("e_ppid", ctypes.c_int32),
            ("e_pgid", ctypes.c_int32),
            ("e_jobc", ctypes.c_int16),
            ("e_tdev", ctypes.c_int32),
            ("e_tpgid", ctypes.c_int32),
            ("e_tsess", ctypes.c_void_p),
            ("e_wmesg", ctypes.c_char * 8),
            ("e_xsize", ctypes.c_int32),
            ("e_xrssize", ctypes.c_int16),
            ("e_xccount", ctypes.c_int16),
            ("e_xswrss", ctypes.c_int16),
            ("e_flag", ctypes.c_int32),
            ("e_login", ctypes.c_char * 12),
            ("e_spare", ctypes.c_int32 * 4),
        ]

    class KinfoProc(ctypes.Structure):
        _fields_ = [("kp_proc", ExternProc), ("kp_eproc", Eproc)]

    if ctypes.sizeof(KinfoProc) != _DARWIN_KINFO_PROC_SIZE:
        raise OSError(
            f"kinfo_proc layout is {ctypes.sizeof(KinfoProc)} bytes, "
            f"kernel ABI needs {_DARWIN_KINFO_PROC_SIZE}"
        )

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
    proc_listallpids = libproc.proc_listallpids
    proc_listallpids.argtypes = [ctypes.c_void_p, ctypes.c_int]
    proc_listallpids.restype = ctypes.c_int

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
        proc_task_info_type=ProcTaskInfo,
        kinfo_proc_type=KinfoProc,
        proc_pidinfo=proc_pidinfo,
        proc_listallpids=proc_listallpids,
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


def _darwin_proc_table() -> dict[int, int]:
    """Enumerate every live pid with its resident kB through libproc.

    One ``proc_listallpids`` call replaces the ``ps`` subprocess and its hard
    timeout. ``PROC_PIDTASKINFO`` is uid-restricted, so another user's
    process carries zero resident kB; the guard never sizes those, because
    global RSS sums only Molt-owned process groups.
    """

    authority = _darwin_process_authority()
    if authority is None:
        raise ProcessSnapshotError("Darwin process authority is unavailable")
    try:
        pids = authority.pids()
    except (AttributeError, OSError, TypeError, ValueError) as exc:
        raise ProcessSnapshotError(f"Darwin process enumeration failed: {exc}") from exc
    table: dict[int, int] = {}
    for pid in pids:
        if type(pid) is not int or pid <= 0:
            continue
        try:
            resident_kb = authority.resident_kb(pid)
        except (AttributeError, OSError, TypeError, ValueError):
            resident_kb = None
        table[pid] = 0 if resident_kb is None else max(0, int(resident_kb))
    if not table:
        raise ProcessSnapshotError("Darwin process enumeration contained no rows")
    return table


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


def _darwin_proc_argv(pid: int) -> tuple[str, ...] | None:
    """Preserve native KERN_PROCARGS2 boundaries; permission/unknown fails closed."""
    if sys.platform != "darwin" or pid <= 0:
        return None
    authority = _darwin_process_authority()
    if authority is None:
        return None
    try:
        argv = authority.argv(pid)
    except (AttributeError, OSError, TypeError, ValueError):
        return None
    if (
        not isinstance(argv, tuple)
        or not argv
        or any(not isinstance(arg, str) or "\0" in arg for arg in argv)
    ):
        return None
    return argv


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

    if type(pid) is not int or pid <= 0:
        return None
    if os.name == "nt":
        return windows_current_process_started_at_ns() if pid == os.getpid() else None
    if sys.platform.startswith("linux"):
        return _linux_proc_started_at_ns(pid)
    if sys.platform == "darwin":
        return _darwin_proc_started_at_ns(pid)
    return None


def parse_windows_process_snapshot_rows(
    rows: Sequence[
        tuple[int, int, int, str, int | None]
        | tuple[int, int, int, str, int | None, int | None]
        | tuple[int, int, int, str, int | None, int | None, str]
    ],
) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for row in rows:
        command_kind = "full"
        if len(row) == 7:
            pid, ppid, rss_kb, command, elapsed_sec, started_at_ns, command_kind = row
        elif len(row) == 5:
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
            command_kind=command_kind,
        )
    return samples


def sample_processes_posix() -> dict[int, ProcessSample]:
    if sys.platform.startswith("linux"):
        return sample_processes_linux_proc()
    if sys.platform == "darwin":
        return _sample_processes_darwin()
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,pgid=,rss=,lstart=,command="],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
            env={**os.environ, "LC_ALL": "C"},
            encoding="utf-8",
        )
    except (OSError, subprocess.TimeoutExpired, TypeError) as exc:
        raise ProcessSnapshotError(f"POSIX process snapshot failed: {exc}") from exc
    if result.returncode != 0:
        raise ProcessSnapshotError(
            f"POSIX process snapshot failed with exit code {result.returncode}"
        )
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
        for pid, sample in parse_process_table_with_start(result.stdout).items()
    }
    if not samples:
        raise ProcessSnapshotError("POSIX process snapshot contained no usable rows")
    return samples


def _darwin_proc_kernel_row(pid: int) -> _DarwinKernelProcRow | None:
    """Return the kernel's own row for a pid libproc no longer answers for."""

    if sys.platform != "darwin" or pid <= 0:
        return None
    authority = _darwin_process_authority()
    if authority is None:
        return None
    try:
        return authority.kernel_row(pid)
    except (AttributeError, OSError, TypeError, ValueError):
        return None


def _darwin_sample_from_kernel_row(
    pid: int, rss_kb: int, *, now_ns: int
) -> ProcessSample | None:
    """Sample a pid libproc will not describe, from ``kern.proc.pid``.

    A pid with no row was reaped between two reads. A ``SZOMB`` row has
    exited: it holds no memory, no signal reaches it, and only its parent's
    ``wait()`` remains, so it is no live member of any tree. Any other row is
    a process libproc withholds (a system daemon such as launchd) or one
    still leaving; it binds the exact birth the kernel kept when its argv is
    readable, and otherwise stays unbound like any libproc row without argv.
    """

    before = _darwin_proc_kernel_row(pid)
    if before is None or before.status == _DARWIN_SZOMB:
        return None
    argv = _darwin_proc_argv(pid)
    after = _darwin_proc_kernel_row(pid)
    if after is None or after.status == _DARWIN_SZOMB:
        return None
    if before != after or argv is None:
        return ProcessSample(
            pid=pid,
            ppid=0,
            rss_kb=rss_kb,
            command=before.command,
            pgid=before.pgid,
            elapsed_sec=None,
            started_at_ns=None,
            argv=(),
        )
    return ProcessSample(
        pid=pid,
        ppid=max(0, before.ppid),
        rss_kb=rss_kb,
        command=shlex.join(argv),
        pgid=before.pgid,
        elapsed_sec=max(0, (now_ns - before.started_at_ns) // 1_000_000_000),
        started_at_ns=before.started_at_ns,
        argv=argv,
    )


def _sample_processes_darwin() -> dict[int, ProcessSample]:
    """Instance-bound Darwin samples without a `ps` subprocess.

    ``proc_listallpids`` also lists processes that exited and await their
    parent's ``wait()``, and ``proc_pidinfo`` answers ESRCH for them. Such a
    pid is read from ``kern.proc.pid`` instead, which leaves exited processes
    out and binds a still-leaving one exactly. Each live row reads its BSD
    metadata before and after argv so a pid recycled mid-read cannot bind a
    stale instance; such a row, like one whose argv is unreadable, keeps its
    resident kB but carries no ancestry or creation marker.
    """

    now_ns = time.time_ns()
    samples: dict[int, ProcessSample] = {}
    for pid, rss_kb in _darwin_proc_table().items():
        before = _darwin_proc_metadata(pid)
        if before is None:
            sample = _darwin_sample_from_kernel_row(pid, rss_kb, now_ns=now_ns)
            if sample is not None:
                samples[pid] = sample
            continue
        argv = _darwin_proc_argv(pid)
        after = _darwin_proc_metadata(pid)
        if after is None:
            sample = _darwin_sample_from_kernel_row(pid, rss_kb, now_ns=now_ns)
            if sample is not None:
                samples[pid] = sample
            continue
        if before != after or argv is None:
            samples[pid] = ProcessSample(
                pid=pid,
                ppid=0,
                rss_kb=rss_kb,
                command=before[3],
                pgid=before[1],
                elapsed_sec=None,
                started_at_ns=None,
                argv=(),
            )
            continue
        ppid, pgid, started_at_ns, _native_name = before
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=max(0, ppid),
            rss_kb=rss_kb,
            command=shlex.join(argv),
            pgid=pgid,
            elapsed_sec=max(0, (now_ns - started_at_ns) // 1_000_000_000),
            started_at_ns=started_at_ns,
            argv=argv,
        )
    return samples


def sample_processes_windows(
    snapshot_rows: Callable[
        [],
        Sequence[
            tuple[int, int, int, str, int | None]
            | tuple[int, int, int, str, int | None, int | None]
            | tuple[int, int, int, str, int | None, int | None, str]
        ],
    ] = _windows_process_snapshot_rows_hard_timeout,
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


@lru_cache(maxsize=2048)
def _cached_host_control_plane_command(
    command: str,
    tokens: tuple[str, ...],
    executable_names: frozenset[str],
    launcher_names: frozenset[str],
    argument_executable_names: frozenset[str],
    argv: tuple[str, ...] | None = None,
) -> bool:
    """Cache lexical work only, with every policy input in the value key.

    A process's current command is read on every call. PID, creation identity,
    ancestry, and ownership never enter this cache; those remain live facts.
    """
    folded_command = (" ".join(argv) if argv else command).casefold()
    normalized_command = folded_command.replace("\\", "/")
    if (
        any(
            token.casefold() in folded_command
            or token.casefold().replace("\\", "/") in normalized_command
            for token in tokens
        )
        or (
            argv[0].replace("\\", "/").rsplit("/", 1)[-1].casefold()
            if argv
            else command_executable_name(command)
        )
        in executable_names
    ):
        return True
    names = (
        tuple(
            arg.replace("\\", "/").rstrip("/").rsplit("/", 1)[-1].casefold()
            for arg in argv
        )
        if argv
        else command_arg_executable_names(command)
    )
    return (
        len(names) >= 2
        and names[0] in launcher_names
        and any(name in argument_executable_names for name in names[1:])
    )


def is_host_control_plane_process(sample: ProcessSample) -> bool:
    return _cached_host_control_plane_command(
        sample.command,
        HOST_CONTROL_PLANE_TOKENS,
        HOST_CONTROL_PLANE_EXECUTABLE_NAMES,
        HOST_CONTROL_PLANE_LAUNCHER_NAMES,
        HOST_CONTROL_PLANE_ARG_EXECUTABLE_NAMES,
        sample.argv,
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
    if pid not in owned_pids and any(
        samples[ancestor].command_kind == "unavailable"
        for ancestor in ancestor_pids(samples, pid)
        if ancestor in samples
    ):
        # Unknown command evidence cannot grant permission to terminate an unrelated tree.
        return True
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
    current = samples.get(current_pid)
    if current is None or type(current.started_at_ns) is not int:
        return True
    descendants, _unresolved = birth_fenced_descendants(
        samples, {current_pid: current.started_at_ns}
    )
    if pid not in descendants:
        return True
    executable = (
        sample.argv[0].replace("\\", "/").rsplit("/", 1)[-1].casefold()
        if sample.argv
        else command_executable_name(sample.command)
    )
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
    """Find possible descendants for diagnostics, never positive custody admission."""
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
    self_descendant_ids: set[int] = set()
    current = samples.get(self_pid) if self_pid is not None else None
    if current is not None and type(current.started_at_ns) is int:
        descendants, _unresolved = birth_fenced_descendants(
            samples, {current.pid: current.started_at_ns}
        )
        self_descendant_ids.update(descendants)
    # Possible host ancestry remains conservative. Exempting a current child
    # from that protection, however, requires positive birth-fenced ancestry.
    explicitly_owned = set(owned_pids) | self_descendant_ids
    host_control_plane_pids = {
        sample.pid
        for sample in samples.values()
        if is_host_control_plane_process(sample)
    }
    for sample in samples.values():
        if sample.pid in ancestor_ids or sample.pid in host_control_plane_pids:
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
    if tracker is not None:
        observed = tracker.update(samples)
    else:
        # The caller supplies the root; PPID alone cannot supply its children.
        observed = {root_pid}
        root = samples.get(root_pid)
        if root is not None and type(root.started_at_ns) is int:
            descendants, _unresolved = birth_fenced_descendants(
                samples, {root_pid: root.started_at_ns}
            )
            observed.update(descendants)
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


def birth_fenced_descendants(
    samples: Mapping[int, ProcessSample], observed: Mapping[int, int]
) -> tuple[dict[int, ProcessSample], set[int]]:
    """Extend observed process instances through live birth-verified parents.

    A historical PID without a current matching birth grants no ancestry.
    Unknown or inconsistent child births remain unresolved, never signalable.
    PGID changes do not sever independently verified ancestry.
    """
    owned = {
        pid: samples[pid]
        for pid, born in observed.items()
        if type(born) is int
        and born > 0
        and pid in samples
        and type(samples[pid].started_at_ns) is int
        and samples[pid].started_at_ns == born
    }
    unresolved: set[int] = set()
    changed = True
    while changed:
        changed = False
        for pid, child in samples.items():
            if pid in owned or child.ppid not in owned:
                continue
            parent_born = owned[child.ppid].started_at_ns
            child_born = child.started_at_ns
            if not process_births_are_ordered(parent_born, child_born):
                unresolved.add(pid)
                continue
            owned[pid] = child
            unresolved.discard(pid)
            changed = True
    return owned, unresolved


def process_command_argv(pid: int) -> tuple[str, ...] | None:
    """Read authoritative argv within one native process-birth fence."""
    if pid <= 0:
        return None
    if sys.platform.startswith("linux"):
        before = _linux_proc_stat_identity(pid)
        argv = _linux_proc_argv(pid)
        after = _linux_proc_stat_identity(pid)
    elif sys.platform == "darwin":
        before = _darwin_proc_metadata(pid)
        argv = _darwin_proc_argv(pid)
        after = _darwin_proc_metadata(pid)
    else:
        return None
    return argv if before is not None and before == after else None
