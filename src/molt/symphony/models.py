from __future__ import annotations

import math
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Any


@dataclass(frozen=True, slots=True)
class BlockerRef:
    id: str | None
    identifier: str | None
    state: str | None


@dataclass(frozen=True, slots=True)
class Issue:
    id: str
    identifier: str
    title: str
    description: str | None
    priority: int | None
    state: str
    branch_name: str | None
    url: str | None
    labels: tuple[str, ...]
    blocked_by: tuple[BlockerRef, ...]
    created_at: datetime | None
    updated_at: datetime | None

    @property
    def normalized_state(self) -> str:
        return self.state.strip().lower()


@dataclass(frozen=True, slots=True)
class WorkflowDefinition:
    path: Path
    config: dict[str, Any]
    prompt_template: str
    loaded_at: datetime
    mtime_ns: int


@dataclass(frozen=True, slots=True)
class TrackerConfig:
    kind: str
    endpoint: str
    api_key: str | None
    project_slugs: tuple[str, ...]
    active_states: tuple[str, ...]
    terminal_states: tuple[str, ...]

    @property
    def project_slug(self) -> str | None:
        if not self.project_slugs:
            return None
        return self.project_slugs[0]


@dataclass(frozen=True, slots=True)
class PollingConfig:
    interval_ms: int


@dataclass(frozen=True, slots=True)
class WorkspaceHooks:
    after_create: str | None
    before_run: str | None
    after_run: str | None
    before_remove: str | None
    timeout_ms: int


@dataclass(frozen=True, slots=True)
class WorkspaceConfig:
    root: Path


@dataclass(frozen=True, slots=True)
class AgentConfig:
    max_concurrent_agents: int
    max_turns: int
    max_retry_backoff_ms: int
    max_retry_attempts: int
    max_concurrent_agents_by_state: dict[str, int]
    role_pools: dict[str, int]
    default_role: str


@dataclass(frozen=True, slots=True)
class CodexConfig:
    command: str
    approval_policy: Any
    thread_sandbox: Any
    turn_sandbox_policy: Any
    turn_timeout_ms: int
    read_timeout_ms: int
    stall_timeout_ms: int


@dataclass(frozen=True, slots=True)
class ServerConfig:
    port: int | None


@dataclass(frozen=True, slots=True)
class RuntimeConfig:
    tracker: TrackerConfig
    polling: PollingConfig
    workspace: WorkspaceConfig
    hooks: WorkspaceHooks
    agent: AgentConfig
    codex: CodexConfig
    server: ServerConfig


@dataclass(frozen=True, slots=True)
class Workspace:
    path: Path
    workspace_key: str
    created_now: bool


@dataclass(slots=True)
class RetryEntry:
    issue_id: str
    identifier: str
    attempt: int
    due_at_monotonic: float
    error: str | None


@dataclass(slots=True)
class LiveSession:
    session_id: str | None = None
    thread_id: str | None = None
    turn_id: str | None = None
    codex_app_server_pid: int | None = None
    last_codex_event: str | None = None
    last_codex_timestamp: datetime | None = None
    last_codex_message: str | None = None
    codex_input_tokens: int = 0
    codex_output_tokens: int = 0
    codex_total_tokens: int = 0
    last_reported_input_tokens: int = 0
    last_reported_output_tokens: int = 0
    last_reported_total_tokens: int = 0
    turn_count: int = 0
    turn_started_at_monotonic: float | None = None
    last_turn_duration_ms: float | None = None
    max_turn_duration_ms: float = 0.0
    recent_events: list[dict[str, str | None]] = field(default_factory=list)


@dataclass(slots=True)
class RunningEntry:
    issue: Issue
    issue_identifier: str
    worker_name: str
    worker_role: str
    started_at_utc: datetime
    started_at_monotonic: float
    retry_attempt: int
    dispatch_to_first_event_ms: float | None = None
    stop_requested: bool = False
    cleanup_on_exit: bool = False
    cleanup_reason: str | None = None
    session: LiveSession = field(default_factory=LiveSession)


@dataclass(slots=True)
class CodexTotals:
    input_tokens: int = 0
    output_tokens: int = 0
    total_tokens: int = 0
    turns_completed: int = 0
    ended_seconds_running: float = 0.0

    def snapshot(self, active_seconds: float) -> dict[str, Any]:
        seconds_running = max(self.ended_seconds_running + active_seconds, 0.0)
        tokens_per_second = (
            float(self.total_tokens) / seconds_running if seconds_running > 0 else 0.0
        )
        return {
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "total_tokens": self.total_tokens,
            "turns_completed": self.turns_completed,
            "seconds_running": round(seconds_running, 3),
            "tokens_per_second": round(tokens_per_second, 3),
        }


@dataclass(slots=True)
class LatencyStats:
    count: int = 0
    total_ms: float = 0.0
    max_ms: float = 0.0
    recent_ms: list[float] = field(default_factory=list)

    def observe(self, value_ms: float, *, keep: int = 256) -> None:
        value = max(float(value_ms), 0.0)
        self.count += 1
        self.total_ms += value
        self.max_ms = max(self.max_ms, value)
        self.recent_ms.append(value)
        if len(self.recent_ms) > keep:
            del self.recent_ms[: len(self.recent_ms) - keep]

    def snapshot(self) -> dict[str, float | int]:
        avg_ms = (self.total_ms / self.count) if self.count else 0.0
        p95_ms = 0.0
        if self.recent_ms:
            p95_ms = _percentile(self.recent_ms, ratio=0.95)
        return {
            "count": self.count,
            "total_ms": round(self.total_ms, 3),
            "avg_ms": round(avg_ms, 3),
            "p95_ms": round(p95_ms, 3),
            "max_ms": round(self.max_ms, 3),
        }

    def recent_snapshot(self, *, window: int = 48) -> dict[str, float | int]:
        size = max(int(window), 1)
        sample = self.recent_ms[-size:]
        if not sample:
            return {
                "count": 0,
                "avg_ms": 0.0,
                "p95_ms": 0.0,
                "max_ms": 0.0,
            }
        avg_ms = math.fsum(sample) / len(sample)
        return {
            "count": len(sample),
            "avg_ms": round(avg_ms, 3),
            "p95_ms": round(_percentile(sample, ratio=0.95), 3),
            "max_ms": round(max(sample), 3),
        }


@dataclass(slots=True)
class ProfilingStats:
    started_at_monotonic: float = field(default_factory=time.monotonic)
    counters: dict[str, int] = field(default_factory=dict)
    latencies_ms: dict[str, LatencyStats] = field(default_factory=dict)
    queue_depth_peak: int = 0
    process_cpu_user_s: float = 0.0
    process_cpu_system_s: float = 0.0
    process_rss_high_water_kb: int = 0
    latest: dict[str, float] = field(default_factory=dict)

    def incr(self, counter: str, delta: int = 1) -> None:
        self.counters[counter] = self.counters.get(counter, 0) + int(delta)

    def observe_latency(self, label: str, value_ms: float) -> None:
        stats = self.latencies_ms.get(label)
        if stats is None:
            stats = LatencyStats()
            self.latencies_ms[label] = stats
        stats.observe(value_ms)
        self.latest[f"{label}_ms"] = round(max(float(value_ms), 0.0), 3)

    def observe_queue_depth(self, depth: int) -> None:
        self.queue_depth_peak = max(self.queue_depth_peak, int(depth))
        self.latest["event_queue_depth"] = float(depth)

    def observe_resource_usage(
        self,
        *,
        cpu_user_s: float,
        cpu_system_s: float,
        rss_high_water_kb: int,
    ) -> None:
        self.process_cpu_user_s = max(self.process_cpu_user_s, float(cpu_user_s))
        self.process_cpu_system_s = max(self.process_cpu_system_s, float(cpu_system_s))
        self.process_rss_high_water_kb = max(
            self.process_rss_high_water_kb, int(rss_high_water_kb)
        )

    def hotspots(self, *, limit: int = 8) -> list[dict[str, float | int | str]]:
        rows: list[dict[str, float | int | str]] = []
        for label, stats in self.latencies_ms.items():
            snap = stats.snapshot()
            rows.append(
                {
                    "label": label,
                    "count": int(snap["count"]),
                    "avg_ms": float(snap["avg_ms"]),
                    "p95_ms": float(snap["p95_ms"]),
                    "max_ms": float(snap["max_ms"]),
                    "total_ms": float(snap["total_ms"]),
                }
            )
        rows.sort(
            key=lambda row: (
                float(row["p95_ms"]),
                float(row["max_ms"]),
                float(row["total_ms"]),
            ),
            reverse=True,
        )
        return rows[: max(limit, 1)]

    def snapshot(
        self,
        *,
        now_monotonic: float | None = None,
        hotspot_limit: int = 8,
    ) -> dict[str, Any]:
        now = time.monotonic() if now_monotonic is None else now_monotonic
        return {
            "uptime_seconds": round(max(now - self.started_at_monotonic, 0.0), 3),
            "queue_depth_peak": self.queue_depth_peak,
            "counters": dict(self.counters),
            "latencies_ms": {
                label: stats.snapshot() for label, stats in self.latencies_ms.items()
            },
            "hotspots": self.hotspots(limit=hotspot_limit),
            "latest": dict(self.latest),
            "process": {
                "cpu_user_s": round(self.process_cpu_user_s, 3),
                "cpu_system_s": round(self.process_cpu_system_s, 3),
                "rss_high_water_kb": self.process_rss_high_water_kb,
            },
        }

    def compare_against_baseline(
        self,
        baseline_by_label: dict[str, dict[str, Any]],
        *,
        recent_window: int = 48,
        limit: int = 8,
        min_delta_ms: float = 0.5,
        min_delta_ratio: float = 0.05,
    ) -> dict[str, list[dict[str, float | int | str]]]:
        regressions: list[dict[str, float | int | str]] = []
        improvements: list[dict[str, float | int | str]] = []
        for label, stats in self.latencies_ms.items():
            baseline_row = baseline_by_label.get(label)
            if not isinstance(baseline_row, dict):
                continue
            baseline_avg = _to_float(baseline_row.get("avg_ms"))
            baseline_p95 = _to_float(baseline_row.get("p95_ms"))
            if baseline_avg <= 0 and baseline_p95 <= 0:
                continue
            recent = stats.recent_snapshot(window=recent_window)
            current_avg = _to_float(recent.get("avg_ms"))
            current_p95 = _to_float(recent.get("p95_ms"))
            if current_avg <= 0 and current_p95 <= 0:
                continue
            avg_delta = current_avg - baseline_avg
            p95_delta = current_p95 - baseline_p95
            avg_delta_ratio = avg_delta / baseline_avg if baseline_avg > 0 else 0.0
            p95_delta_ratio = p95_delta / baseline_p95 if baseline_p95 > 0 else 0.0
            impact_ms = max(avg_delta, 0.0) * max(stats.count, 1)
            row = {
                "label": label,
                "samples": int(stats.count),
                "recent_samples": int(recent.get("count") or 0),
                "baseline_avg_ms": round(baseline_avg, 3),
                "baseline_p95_ms": round(baseline_p95, 3),
                "current_avg_ms": round(current_avg, 3),
                "current_p95_ms": round(current_p95, 3),
                "avg_delta_ms": round(avg_delta, 3),
                "p95_delta_ms": round(p95_delta, 3),
                "avg_delta_ratio": round(avg_delta_ratio, 4),
                "p95_delta_ratio": round(p95_delta_ratio, 4),
                "impact_ms": round(impact_ms, 3),
            }
            if (avg_delta >= min_delta_ms and avg_delta_ratio >= min_delta_ratio) or (
                p95_delta >= min_delta_ms and p95_delta_ratio >= min_delta_ratio
            ):
                regressions.append(row)
                continue
            if avg_delta <= -min_delta_ms or p95_delta <= -min_delta_ms:
                improvements.append(row)
        regressions.sort(
            key=lambda item: (
                _to_float(item.get("impact_ms")),
                _to_float(item.get("p95_delta_ms")),
                _to_float(item.get("avg_delta_ms")),
            ),
            reverse=True,
        )
        improvements.sort(
            key=lambda item: (
                abs(_to_float(item.get("avg_delta_ms"))),
                abs(_to_float(item.get("p95_delta_ms"))),
            ),
            reverse=True,
        )
        return {
            "regressions": regressions[: max(limit, 1)],
            "improvements": improvements[: max(limit, 1)],
        }


def _percentile(values: list[float], *, ratio: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(max(float(value), 0.0) for value in values)
    idx = max(math.ceil(len(ordered) * ratio) - 1, 0)
    return ordered[idx]


def _to_float(value: Any) -> float:
    try:
        parsed = float(value)
    except (TypeError, ValueError):
        return 0.0
    if not math.isfinite(parsed):
        return 0.0
    return parsed


@dataclass(slots=True)
class OrchestratorState:
    poll_interval_ms: int
    max_concurrent_agents: int
    running: dict[str, RunningEntry] = field(default_factory=dict)
    claimed: set[str] = field(default_factory=set)
    retry_attempts: dict[str, RetryEntry] = field(default_factory=dict)
    completed: set[str] = field(default_factory=set)
    last_errors: dict[str, str] = field(default_factory=dict)
    issue_identifiers: dict[str, str] = field(default_factory=dict)
    codex_totals: CodexTotals = field(default_factory=CodexTotals)
    codex_rate_limits: dict[str, Any] | None = None
    profiling: ProfilingStats = field(default_factory=ProfilingStats)
    manual_actions: list[dict[str, Any]] = field(default_factory=list)
    suspension_kind: str | None = None
    suspension_message: str | None = None
    suspension_since_utc: datetime | None = None
    suspension_resume_at_monotonic: float | None = None
    suspension_resume_at_epoch_utc: float | None = None
    suspension_resume_source: str | None = None
    suspension_resume_reason: str | None = None
    suspension_auto_resume: bool = False


def now_utc() -> datetime:
    return datetime.now(UTC)
