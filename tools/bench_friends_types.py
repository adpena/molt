import re
import signal
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

SUPPORTED_SEMANTIC_MODES = {
    "runs_unmodified",
    "requires_adapter",
    "unsupported_by_molt",
}

SUPPORTED_RUNNER_ROLES = {
    "workload",
    "custody_audit",
    "c_api_scan",
}

RUNNER_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+$")
MAX_FAILURE_DETAIL_RECORDS = 32
MAX_FAILURE_MESSAGE_CHARS = 4000


@dataclass(frozen=True)
class RunnerSpec:
    name: str
    role: str
    build_cmd: list[str] | None
    run_cmd: list[str] | None
    env: dict[str, str]
    skip_reason: str | None
    json_stdout: bool


@dataclass(frozen=True)
class SourceCustody:
    source: str
    requested_ref: str | None
    expected_ref: str | None
    head_ref: str | None
    ref_verified: bool | None
    git_clean: bool | None
    git_status_porcelain: str | None
    git_ignored_artifacts: str | None
    suite_root_overridden: bool
    verification: str


@dataclass(frozen=True)
class SuiteAcquisition:
    suite_root: Path
    suite_workdir: Path
    custody: SourceCustody


@dataclass(frozen=True)
class SuiteSpec:
    id: str
    friend: str
    display_name: str
    enabled: bool
    source: str
    repo_url: str | None
    repo_ref: str | None
    local_path: str | None
    workdir: str | None
    semantic_mode: str
    adapter_notes: str | None
    tags: list[str]
    timeout_sec: int
    repeat: int
    env: dict[str, str]
    prepare_cmds: list[list[str]]
    runners: dict[str, RunnerSpec]


@dataclass
class PhaseResult:
    cmd: list[str]
    returncode: int
    elapsed_s: float
    timed_out: bool
    stdout_path: str
    stderr_path: str
    stdout_json: Any | None = None
    stdout_json_error: str | None = None
    guard_status: str | None = None
    guard_violation: dict[str, Any] | None = None
    guard_limit_at_violation: dict[str, Any] | None = None
    guard_orphaned_process_groups: list[int] = field(default_factory=list)
    guard_exit_signal: dict[str, Any] | None = None
    guard_cargo_incremental_quarantine: dict[str, Any] | None = None
    molt_failure: dict[str, Any] | None = None

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and not self.timed_out


@dataclass
class RunnerResult:
    name: str
    role: str
    status: str
    reason: str | None = None
    build: PhaseResult | None = None
    runs: list[PhaseResult] = field(default_factory=list)
    run_samples_s: list[float] = field(default_factory=list)
    run_median_s: float | None = None
    run_mean_s: float | None = None
    run_stdev_s: float | None = None
    structured_outputs: list[Any] = field(default_factory=list)
    structured_samples_s: dict[str, list[float]] = field(default_factory=dict)
    structured_median_s: dict[str, float] = field(default_factory=dict)
    molt_failure: dict[str, Any] | None = None


@dataclass
class SuiteResult:
    id: str
    friend: str
    display_name: str
    semantic_mode: str
    source: str
    suite_root: str
    suite_workdir: str
    resolved_ref: str | None
    requested_ref: str | None
    source_custody: SourceCustody
    status: str
    reason: str | None
    adapter_notes: str | None
    tags: list[str]
    runners: dict[str, RunnerResult]
    metrics: dict[str, Any]


class BenchInterrupted(BaseException):
    def __init__(self, signum: int) -> None:
        self.signum = signum
        self.signame = signal.Signals(signum).name
        super().__init__(f"interrupted by {self.signame}")


class BenchSignalScope:
    def __init__(self, signals: tuple[int, ...] = (signal.SIGTERM, signal.SIGINT)):
        self._signals = signals
        self._previous: dict[int, Any] = {}

    def __enter__(self) -> "BenchSignalScope":
        for signum in self._signals:
            self._previous[signum] = signal.getsignal(signum)
            signal.signal(signum, self._handle_signal)
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        for signum, previous in self._previous.items():
            signal.signal(signum, previous)

    def _handle_signal(self, signum: int, _frame: object) -> None:
        raise BenchInterrupted(signum)
