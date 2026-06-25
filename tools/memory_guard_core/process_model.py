from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import os
import re
import subprocess

from tools.memory_guard_core.windows_snapshot import _windows_process_snapshot_rows


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
    "codex.exe\" app-server",
    "codex app-server",
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
        "node_repl.exe",
    }
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


ProcessIdentity = tuple[int | None, str, int | None]


def process_identity(sample: ProcessSample) -> ProcessIdentity:
    return (sample.pgid, sample.command, sample.started_at_ns)


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
                self.known_identities[pid] = identity
            elif known_identity != identity:
                self.known_pids.remove(pid)
                self.known_identities.pop(pid, None)
        changed = True
        while changed:
            changed = False
            for sample in samples.values():
                sample_pgid = sample_pgid_or_pid(sample)
                if sample.pid in self.known_pids or sample.ppid in self.known_pids:
                    if sample.pid not in self.known_pids:
                        self.known_pids.add(sample.pid)
                        self.known_identities[sample.pid] = process_identity(sample)
                        changed = True
                    if (
                        sample.pid != self.root_pid or sample_pgid == self.root_pid
                    ) and sample_pgid not in self.known_pgids:
                        self.known_pgids.add(sample_pgid)
                        changed = True
        return {pid for pid in self.known_pids if pid in samples}


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
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,pgid=,rss=,etime=,command="],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired, TypeError):
        return {}
    if result.returncode != 0:
        return {}
    return parse_process_table(result.stdout)


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
    except (OSError, TypeError, AttributeError, TimeoutError):
        return {}
    return parse_windows_process_snapshot_rows(rows)


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
        if (
            sample := samples.get(ancestor)
        ) is not None
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
) -> bool:
    """Return true when pid belongs to host-control lineage outside this guard.

    Codex/Claude/app-server/renderer/node-repl processes are the operator control
    plane. Their descendants are also protected unless they are descendants of
    the currently running guard process, which is the only process subtree a
    guard is allowed to own and terminate.
    """

    if pid is None or pid <= 0:
        return False
    if current_pid is not None and current_pid > 0:
        current_descendants = descendant_pids(samples, current_pid)
        if pid in current_descendants:
            sample = samples.get(pid)
            return sample is not None and is_host_control_plane_process(sample)
    return has_host_control_plane_ancestor(
        samples,
        pid,
        include_self=include_self,
    )


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
) -> set[int]:
    protected: set[int] = set()
    if self_pgid is not None and self_pgid > 0:
        protected.add(self_pgid)
    ancestor_ids = ancestor_pids(samples, self_pid)
    self_descendant_ids = descendant_pids(samples, self_pid) if self_pid else set()
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
    ):
        return False
    return (
        sample_pgid_or_pid(sample) not in protected_pgids
        and not is_host_control_plane_process(sample)
    )


def filter_protected_watched_pids(
    samples: Mapping[int, ProcessSample],
    watched: set[int],
    *,
    protected_pgids: set[int],
    current_pid: int | None = None,
) -> set[int]:
    filtered: set[int] = set()
    for pid in watched:
        sample = samples.get(pid)
        if sample is None:
            continue
        if has_external_host_control_plane_lineage(
            samples,
            pid,
            current_pid=current_pid,
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
    observed = tracker.update(samples) if tracker is not None else descendant_pids(
        samples,
        root_pid,
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
