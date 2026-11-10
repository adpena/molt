"""Instructions retired on main's head: measure, store, compare (HF-222, HF-616).

``run`` is the farm cell ``performance.instructions`` (family
``instruction_signal`` in tools/proof_plan.toml). It measures every workload of
one tier, writes a write-once record to the farm store (the benchmark-evidence
custody class), compares the record with the record of the nearest measured
first-parent ancestor and writes a verdict. ``measure`` counts without a store
or a verdict, for lane A/B. ``status`` reads the verdicts trunk_loop collected.
tools/PERF_AUTHORITY.md owns the contract and the instruction driver.

The primitive programs are declared in tools/perf_primitives.py.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import StrEnum
import hashlib
import json
import math
import multiprocessing
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import bench, harness_memory_guard  # noqa: E402
from tools.bench_evidence import (  # noqa: E402
    AdmittedBenchmarkWorkload,
    admit_benchmark_workload,
    workloads_comparable,
)
from tools.bench_suites import (  # noqa: E402
    BENCHMARKS,
    application_run_args,
    molt_args_for_benchmark,
)
from tools.compat.comparison import (  # noqa: E402
    ComparisonMode,
    ExitLaw,
    Outputs,
    compare_outputs,
)
from tools.perf_scoreboard_build_profiles import (  # noqa: E402
    profile_binding_problems,
    profile_selection_for_backend,
)
from tools.perf_scoreboard_measure import _cpython_run_env  # noqa: E402
from tools.perf_primitives import EMPTY, PRIMITIVES, Primitive  # noqa: E402
from tools.resource_pressure import plan_resource_pressure  # noqa: E402
from tools.shard_partition import lpt_shards  # noqa: E402

RECORD_SCHEMA = "molt.instruction-record.v1"
VERDICT_SCHEMA = "molt.perf-verdict.v1"
MIN_REPEATS = 5
CPYTHON_VERSION = (3, 12, 16)
CPYTHON_SELECTOR = ".".join(map(str, CPYTHON_VERSION))
# The base is the nearest measured first-parent ancestor inside this window.
BASE_WINDOW = 500
CREEP_DAYS = 7
# RED needs a change beyond max(3 x calibrated bound, 2 x head spread, 1%).
BOUND_FACTOR = 3
SPREAD_FACTOR = 2
THRESHOLD_FLOOR = 0.01
# Step 1 (design section 2): 8 concurrent builds left 8.5 of 24.6 GB free.
MAX_COMPILE_SLOTS = 8
COMPILE_GB_PER_SLOT = 2.0
# The instrument: this file and the tools modules it imports on the
# measurement path. Their bytes key the run identity (design section 3.3);
# tests/tools/test_perf_instructions.py pins this tuple to the import set.
# tools/molt_dev_queue.py is imported only after the record is written, for
# suspect restoration commands, and cannot change a count.
INSTRUMENT = (
    "tools/perf_instructions.py",
    "tools/import_file.py",
    "tools/bench.py",
    "tools/harness_memory_guard.py",
    "tools/bench_evidence.py",
    "tools/bench_suites.py",
    "tools/compat/comparison.py",
    "tools/perf_scoreboard_build_profiles.py",
    "tools/perf_scoreboard_measure.py",
    "tools/perf_primitives.py",
    "tools/resource_pressure.py",
    "tools/shard_partition.py",
)
# The stdlib-heavy programs: every compiler change rebuilds their stdlib
# objects, 1,878 s of build work together after a landing (design section 2).
NIGHTLY_BENCHMARKS = frozenset(
    {
        "tests/benchmarks/bench_async_await.py",
        "tests/benchmarks/bench_itertools_tee.py",
        "tests/benchmarks/bench_parse_msgpack.py",
        "tests/benchmarks/bench_channel_throughput.py",
        "tests/benchmarks/bench_counter_words.py",
    }
)


class Tier(StrEnum):
    HOURLY = "hourly"  # performance.instructions
    NIGHTLY = "nightly"  # performance.instructions-nightly


class Phase(StrEnum):
    STARTUP = "startup"
    PER_OP = "per_op"
    WHOLE = "whole"


class Unmeasured(StrEnum):
    BUILD_FAILED = "build_failed"
    RUN_FAILED = "run_failed"  # after one rerun
    OUTPUT_DIFFERS = "output_differs"


class AxisVerdict(StrEnum):
    SAME = "same"
    IMPROVED = "improved"
    REGRESSED = "regressed"
    LOST = "lost"
    NEW = "new"


class PerfStatus(StrEnum):
    """The verdict a run writes. NOT_MEASURED is the reader's, never written."""

    GREEN = "green"
    RED = "red"
    UNARMED = "unarmed"


class Counter(StrEnum):
    MACOS = "macos-ri_instructions"
    PERF = "linux-perf-instructions:u"
    CALLGRIND = "callgrind-Ir-model"


@dataclass(frozen=True)
class Workload:
    """One program of the registry: a primitive or a core benchmark."""

    name: str
    tier: Tier
    bench: str | None = None  # repository-relative benchmark script
    primitive: Primitive | None = None

    def phases(self) -> tuple[Phase, ...]:
        if self.primitive is None:
            return (Phase.WHOLE,)
        if self.primitive.name == EMPTY.name:
            return (Phase.STARTUP,)
        return (Phase.STARTUP, Phase.PER_OP)

    def arguments(self) -> tuple[int | None, ...]:
        """The N of each run: None runs a benchmark with its declared arguments."""
        if self.primitive is None:
            return (None,)
        if self.primitive.name == EMPTY.name:
            return (0,)
        return (0, self.primitive.iterations)


def workloads(tier: Tier) -> tuple[Workload, ...]:
    """The one registry (design invariant 1)."""
    rows = [Workload(p.name, Tier.HOURLY, primitive=p) for p in (EMPTY, *PRIMITIVES)]
    rows.extend(
        Workload(
            Path(script).stem,
            Tier.NIGHTLY if script in NIGHTLY_BENCHMARKS else Tier.HOURLY,
            bench=script,
        )
        for script in BENCHMARKS
    )
    return tuple(row for row in rows if row.tier is tier)


@dataclass(frozen=True)
class Observation:
    instructions: int
    wall_s: float
    stdout: str
    stderr: str


@dataclass(frozen=True)
class Measured:
    observation: Observation


@dataclass(frozen=True)
class ProgramFailed:
    """The measured program, not the instrument, failed: exit, signal or timeout."""

    returncode: int | None
    timed_out: bool
    detail: str


RunOutcome = Measured | ProgramFailed


def parse_count(counter: Counter, text: str) -> int:
    if counter is Counter.MACOS:
        values = re.findall(r"^\s*(\d+)\s+instructions retired\s*$", text, re.M)
    elif counter is Counter.PERF:
        # perf -x ';' --no-big-num: value;unit;event;runtime;percent-running
        values = []
        for line in text.splitlines():
            fields = line.split(";")
            if len(fields) >= 5 and fields[2] == "instructions:u":
                if float(fields[4].strip().rstrip("%")) != 100:
                    raise ValueError(
                        "multiplexed or invalid instruction count is not admissible"
                    )
                values.append(fields[0].strip())
    else:
        events = re.findall(r"^events:\s*(.+)$", text, re.M)
        summaries = re.findall(r"^summary:\s*(.+)$", text, re.M)
        if len(events) != 1 or len(summaries) != 1 or "Ir" not in events[0].split():
            raise ValueError("Callgrind needs one Ir event summary")
        values = [summaries[0].split()[events[0].split().index("Ir")]]
    if len(values) != 1 or not values[0].isdigit() or int(values[0]) <= 0:
        raise ValueError(f"missing, ambiguous or invalid {counter} instruction count")
    return int(values[0])


def select_counter(request: str) -> Counter:
    if request != "auto":
        chosen = Counter(request)
    elif sys.platform == "darwin":
        chosen = Counter.MACOS
    elif sys.platform == "linux":
        # Select before workload/build launches, never retry a failed measurement
        # with another model. VM PMU/permission absence selects a labelled model.
        paranoid = Path("/proc/sys/kernel/perf_event_paranoid")
        pmus = Path("/sys/bus/event_source/devices")
        exposed = pmus.is_dir() and any(
            p.name == "cpu" or p.name.startswith("armv8_pmuv3") for p in pmus.iterdir()
        )
        permitted = (
            paranoid.is_file()
            and int(paranoid.read_text(encoding="ascii").strip()) <= 2
        )
        chosen = (
            Counter.PERF
            if exposed and permitted and shutil.which("perf")
            else Counter.CALLGRIND
        )
    else:
        raise ValueError(f"no instruction-counter plan for {sys.platform}")
    tool = {
        Counter.MACOS: "/usr/bin/time",
        Counter.PERF: "perf",
        Counter.CALLGRIND: "valgrind",
    }[chosen]
    if not shutil.which(tool):
        raise ValueError(
            f"{chosen} requires {tool}; no counter substitution is allowed"
        )
    if chosen is Counter.MACOS and sys.platform != "darwin":
        raise ValueError("ri_instructions requires macOS")
    if chosen is not Counter.MACOS and sys.platform != "linux":
        raise ValueError("perf/Callgrind plans require Linux")
    return chosen


def observe(
    counter: Counter,
    command: list[str],
    prefix: Path,
    env: Mapping[str, str],
    timeout: float,
    *,
    cwd: Path | None = None,
) -> RunOutcome:
    """Count one process.

    The measured program's exit status, signal, timeout or memory violation is
    a ProgramFailed, never an exception. A missing or ambiguous count, a guard
    infrastructure failure or orphaned process groups raise: the instrument
    failed (design invariant 5).
    """
    report = prefix.with_suffix(".counter.txt")
    if report.exists():
        raise ValueError(f"measurement output already exists: {report}")
    if counter is Counter.MACOS:
        argv = ["/usr/bin/time", "-l", "-o", str(report), *command]
    elif counter is Counter.PERF:
        argv = [
            "perf",
            "stat",
            "--no-big-num",
            "-x",
            ";",
            "-e",
            "instructions:u",
            "-o",
            str(report),
            "--",
            *command,
        ]
    else:
        argv = [
            "valgrind",
            "--tool=callgrind",
            "--collect-atstart=yes",
            f"--callgrind-out-file={report}",
            f"--log-file={prefix.with_suffix('.tool.txt')}",
            *command,
        ]
    result = harness_memory_guard.guarded_completed_process(
        argv,
        prefix="MOLT_BENCH",
        env=env,
        cwd=cwd or ROOT,
        timeout=timeout,
        text=True,
        errors="surrogateescape",
    )
    if result.infrastructure_failure or result.orphaned_process_groups:
        raise RuntimeError(
            f"instruction measurement infrastructure failed: {argv!r}; "
            f"{result.infrastructure_failure or result.orphaned_process_groups}"
        )
    if result.timed_out or result.violation is not None or result.returncode != 0:
        return ProgramFailed(
            None if result.timed_out else result.returncode,
            bool(result.timed_out),
            f"rc={result.returncode} timed_out={bool(result.timed_out)} "
            f"memory={result.violation is not None}: " + (result.stderr or "")[-1000:],
        )
    count = parse_count(counter, report.read_text(encoding="utf-8"))
    return Measured(
        Observation(
            count,
            result.child_elapsed_s or result.elapsed_s or 0.0,
            result.stdout or "",
            result.stderr or "",
        )
    )


def run_identity(counter: Counter, tier: Tier) -> dict[str, Any]:
    """What makes two records comparable (design section 3.3).

    The instrument's bytes, the native target, the counter, the selected
    profiles and the tier. Not the methodology version: its bumps were compiler
    and runtime landings, which are what the check compares.
    """
    from molt.native_target import resolve_native_target_spec

    lane = profile_selection_for_backend("native", "release-output")
    return {
        "instrument": {
            path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
            for path in INSTRUMENT
        },
        "target": resolve_native_target_spec(None).triple,
        "counter": counter.value,
        "selected_profiles": lane.as_record(),
        "tier": tier.value,
    }


def identity_digest(identity: Mapping[str, Any]) -> str:
    return hashlib.sha256(
        json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def relative_spread(samples: Sequence[float]) -> float:
    """The largest relative deviation of a sample from the median."""
    if len(samples) < MIN_REPEATS:
        raise ValueError(f"need at least {MIN_REPEATS} samples to measure noise")
    if any(
        isinstance(s, bool) or not isinstance(s, (int, float)) or not math.isfinite(s)
        for s in samples
    ):
        raise ValueError("samples must be finite numbers")
    middle = statistics.median(samples)
    if middle <= 0:
        raise ValueError("an instruction axis must have a positive median")
    return max(abs(s / middle - 1) for s in samples)


def calibrated_bound(samples: Sequence[float]) -> float:
    """2 x the relative spread, rounded up to 0.001; zero is exact.

    The spread is rounded to 1e-9 first, so float noise in an exact ratio
    (104 / 100 - 1) never rounds a bound up by a whole step.
    """
    return math.ceil(round(2 * relative_spread(samples) * 1000, 6)) / 1000


def axis_key(axis: Mapping[str, Any]) -> str:
    """The axis wire name with its admitted workload identity."""
    return json.dumps(
        {"axis": axis["axis"], "workload": axis["workload"]},
        sort_keys=True,
        separators=(",", ":"),
    )


def compare(
    base: Mapping[str, Any],
    head: Mapping[str, Any],
    bounds: Mapping[str, float],
) -> dict[str, list[dict[str, Any]]]:
    """Classify every head axis against the base (design section 3.4).

    Requires equal run identities. An axis compares only when its admitted
    workload identities are comparable; otherwise, or when the base did not
    measure it, it is NEW. A measured base axis that the head could not
    measure is LOST.
    """
    if base["identity_digest"] != head["identity_digest"]:
        raise ValueError("records of different run identities never compare")
    prior = {axis["axis"]: axis for axis in base["axes"]}
    result: dict[str, list[dict[str, Any]]] = {kind.value: [] for kind in AxisVerdict}
    for axis in head["axes"]:
        name = axis["axis"]
        before = prior.get(name)
        if (
            before is None
            or "samples" not in before
            or not workloads_comparable(before["workload"], axis["workload"])
        ):
            result[AxisVerdict.NEW].append({"axis": name})
            continue
        if "samples" not in axis:
            result[AxisVerdict.LOST].append(
                {
                    "axis": name,
                    "reason": axis["unmeasured"],
                    "detail": axis.get("detail", ""),
                }
            )
            continue
        old = statistics.median(before["samples"])
        new = statistics.median(axis["samples"])
        if old <= 0:
            raise ValueError(f"{name}: base median must be positive")
        threshold = max(
            BOUND_FACTOR * bounds.get(axis_key(axis), 0.0),
            SPREAD_FACTOR * relative_spread(axis["samples"]),
            THRESHOLD_FLOOR,
        )
        row = {
            "axis": name,
            "base": old,
            "head": new,
            "change": new / old - 1,
            "threshold": threshold,
        }
        if new > old * (1 + threshold):
            result[AxisVerdict.REGRESSED].append(row)
        elif new < old * (1 - threshold):
            result[AxisVerdict.IMPROVED].append(row)
        else:
            result[AxisVerdict.SAME].append(row)
    return result


def _git_process(
    repo: Path, argv: Sequence[str], *, check: bool = True
) -> harness_memory_guard.GuardedCompletedProcess[str]:
    """One bounded Git metadata read through the harness guard."""
    result = harness_memory_guard.guarded_completed_process(
        ["git", "-C", str(repo), *argv],
        prefix="MOLT_BENCH",
        timeout=120,
        text=True,
        encoding="utf-8",
        errors="strict",
    )
    if check and result.returncode:
        raise subprocess.CalledProcessError(
            result.returncode, result.args, result.stdout, result.stderr
        )
    return result


def _git(repo: Path, *argv: str) -> str:
    return (_git_process(repo, argv).stdout or "").strip()


class RecordStore:
    """The farm store, the one authority: write-once records by (commit, identity)."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.records = root / "records"

    def path(self, commit: str, digest: str) -> Path:
        if not re.fullmatch(r"[0-9a-f]{40}", commit) or not re.fullmatch(
            r"[0-9a-f]{64}", digest
        ):
            raise ValueError("records are keyed by a full commit and identity digest")
        return self.records / f"{commit}.{digest}.json"

    def get(self, commit: str, digest: str) -> dict[str, Any] | None:
        path = self.path(commit, digest)
        if not path.is_file():
            return None
        record = json.loads(path.read_text(encoding="utf-8"))
        if (
            record.get("schema") != RECORD_SCHEMA
            or record.get("commit") != commit
            or record.get("identity_digest") != digest
        ):
            raise ValueError(f"stored record does not bind its key: {path}")
        return record

    def put(self, record: Mapping[str, Any]) -> Path:
        """Write once (design invariant 7). Raises FileExistsError on a second write."""
        from molt.file_publication import atomic_write_bytes

        path = self.path(record["commit"], record["identity_digest"])
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_write_bytes(path, _json_bytes(record), exclusive=True)
        return path

    def of_identity(self, digest: str) -> list[dict[str, Any]]:
        records = [
            json.loads(path.read_text(encoding="utf-8"))
            for path in sorted(self.records.glob(f"*.{digest}.json"))
        ]
        return sorted(records, key=lambda record: record["recorded_at"])

    def nearest_measured_ancestor(
        self, repo: Path, head: str, digest: str, *, window: int = BASE_WINDOW
    ) -> dict[str, Any] | None:
        """The first first-parent ancestor of head, excluding head, with a record."""
        commits = _git(
            repo, "rev-list", "--first-parent", f"--max-count={window + 1}", head
        ).split()
        for commit in commits[1:]:
            record = self.get(commit, digest)
            if record is not None:
                return record
        return None

    def nearest_before(
        self, repo: Path, head: str, digest: str, *, days: int = CREEP_DAYS
    ) -> dict[str, Any] | None:
        """The record nearest to `days` before head's commit time (creep)."""
        stamp = int(_git(repo, "show", "-s", "--format=%ct", head)) - days * 86400
        commits = _git(
            repo,
            "rev-list",
            "--first-parent",
            f"--until={stamp}",
            f"--max-count={BASE_WINDOW}",
            head,
        ).split()
        for commit in commits:
            record = self.get(commit, digest)
            if record is not None:
                return record
        return None

    def first_bounds(self, digest: str) -> dict[str, float]:
        """Each axis's bound comes from the first record that measured its key."""
        bounds: dict[str, float] = {}
        for record in self.of_identity(digest):
            for axis in record["axes"]:
                if "samples" in axis:
                    bounds.setdefault(axis_key(axis), calibrated_bound(axis["samples"]))
        return bounds


def default_store() -> Path:
    from molt.dx import storage_path

    return storage_path(ROOT, "benchmark-evidence", os.environ) / "instructions"


def _json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=1, sort_keys=True) + "\n").encode()


class _Git:
    """The `git._run` shape that molt_dev_queue._postsubmit_reverts reads."""

    def __init__(self, repo: Path) -> None:
        self.repo = repo

    def _run(
        self, argv: list[str], check: bool = True
    ) -> harness_memory_guard.GuardedCompletedProcess[str]:
        return _git_process(self.repo, argv, check=check)


def verdict(store: RecordStore, repo: Path, head: Mapping[str, Any]) -> dict[str, Any]:
    """The verdict of one record against the nearest measured ancestor."""
    digest = head["identity_digest"]
    base = store.nearest_measured_ancestor(repo, head["commit"], digest)
    bounds = store.first_bounds(digest)
    unmeasured = [
        {"axis": a["axis"], "reason": a["unmeasured"], "detail": a.get("detail", "")}
        for a in head["axes"]
        if "samples" not in a
    ]
    payload: dict[str, Any] = {
        "schema": VERDICT_SCHEMA,
        "commit": head["commit"],
        "identity_digest": digest,
        "tier": head["identity"]["tier"],
        "status": PerfStatus.UNARMED.value,
        "base": None,
        **{kind.value: [] for kind in AxisVerdict if kind is not AxisVerdict.SAME},
        "unmeasured": unmeasured,
        "suspects": [],
        "revert_commands": [],
        "creep": None,
    }
    if base is not None:
        comparison = compare(base, head, bounds)
        red = bool(comparison[AxisVerdict.REGRESSED] or comparison[AxisVerdict.LOST])
        payload.update(
            {
                kind: rows
                for kind, rows in comparison.items()
                if kind != AxisVerdict.SAME
            }
        )
        payload["base"] = base["commit"]
        payload["status"] = (PerfStatus.RED if red else PerfStatus.GREEN).value
        if red:
            # Only a RED verdict implicates commits, as postsubmit does.
            from tools.molt_dev_queue import _postsubmit_reverts

            suspects, reverts = _postsubmit_reverts(
                _Git(repo), head["commit"], base["commit"]
            )
            payload["suspects"] = suspects
            payload["revert_commands"] = reverts
    creep = store.nearest_before(repo, head["commit"], digest)
    if creep is not None and creep["commit"] != head["commit"]:
        comparison = compare(creep, head, bounds)
        payload["creep"] = {
            "base": creep["commit"],
            "regressed": comparison[AxisVerdict.REGRESSED],
            "improved": comparison[AxisVerdict.IMPROVED],
        }
    return payload


# --- measurement -------------------------------------------------------------


def _resolve_cpython(env: Mapping[str, str]) -> Path:
    from molt.python_environment_identity import PythonIdentityVerifier
    from molt.python_interpreter import (
        PythonInterpreterRequest,
        resolve_python_interpreter,
    )

    with PythonIdentityVerifier() as verifier:
        interpreter = resolve_python_interpreter(
            PythonInterpreterRequest(*CPYTHON_VERSION),
            override=None,
            verifier=verifier,
            env=dict(env),
            cwd=ROOT,
        )
    if len(interpreter.command) != 1:
        raise ValueError("the output reference requires one physical executable")
    # uv discovery may select a virtualenv symlink; use the physical base.
    oracle = Path(interpreter.command[0]).resolve(strict=True)
    probe = harness_memory_guard.guarded_completed_process(
        [
            str(oracle),
            "-I",
            "-S",
            "-c",
            "import platform; print(platform.python_implementation(), platform.python_version())",
        ],
        prefix="MOLT_BENCH",
        env=env,
        timeout=30,
    )
    if probe.returncode or probe.stdout.strip() != f"CPython {CPYTHON_SELECTOR}":
        raise ValueError(
            f"output reference must be CPython {CPYTHON_SELECTOR}, got "
            f"{probe.stdout!r} {probe.stderr!r}"
        )
    return oracle


def _run_env() -> dict[str, str]:
    env = dict(_cpython_run_env())
    env.update(PYTHONHASHSEED="0", LC_ALL="C")
    return env


def _reference(
    workload: Workload,
    admitted: AdmittedBenchmarkWorkload,
    cpython: str,
    n: int | None,
    env: Mapping[str, str],
    timeout: float,
) -> Outputs:
    """CPython's observable result; its count is not taken (design invariant 8)."""
    if n is None:
        invocation = admitted.reference_invocation([cpython, "-I"], env)
        command, child_env, cwd = (
            list(invocation.command),
            dict(invocation.environment),
            invocation.cwd,
        )
    else:
        command = [cpython, "-I", "-S", str(admitted.execution_script), str(n)]
        child_env, cwd = dict(env), None
    result = harness_memory_guard.guarded_completed_process(
        command,
        prefix="MOLT_BENCH",
        env=child_env,
        cwd=cwd or ROOT,
        timeout=timeout,
        text=True,
        errors="surrogateescape",
    )
    if result.timed_out or result.infrastructure_failure:
        raise RuntimeError(f"{workload.name}: the CPython output reference failed")
    return Outputs(result.stdout or "", result.stderr or "", result.returncode)


def primitive_build_env(
    base: Mapping[str, str], lane_env: Mapping[str, str], program_dir: Path
) -> dict[str, str]:
    """The build environment of one primitive program.

    The program's own directory is its project. Otherwise the build adopts the
    nearest enclosing project (a checkout holding ``--out-dir``) and compiles
    the primitive against that project's sources and environment.
    """
    env = dict(base, PYTHONHASHSEED="0", MOLT_KEEP_SYMBOLS="1")
    env.update(lane_env)
    env["MOLT_PROJECT_ROOT"] = str(program_dir)
    return env


def workload_inputs(
    workload: Workload, programs: Path
) -> tuple[Path, list[str], list[str]]:
    """The script, run arguments and build arguments a workload is admitted with.

    Every workload is admitted against the repository root, so its entry is
    the repository-relative path; a primitive's program lives in `programs`.
    """
    if workload.primitive is not None:
        return (
            programs / f"{workload.name}.py",
            [str(workload.primitive.iterations)],
            [],
        )
    assert workload.bench is not None
    script = ROOT / workload.bench
    return script, application_run_args(script), molt_args_for_benchmark(script)


def _measure(workload: Workload, ctx: Mapping[str, Any]) -> dict[str, Any]:
    """Build once, run MIN_REPEATS times per axis, check output; one record entry."""
    counter = Counter(ctx["counter"])
    scratch = Path(ctx["scratch"])
    timeout = float(ctx["timeout"])
    repeats = int(ctx["repeats"])
    lane = profile_selection_for_backend("native", "release-output")
    if lane.backend != "native":
        raise ValueError("instruction builds must use the native backend")
    programs = Path(ctx["programs"])
    script, run_args, molt_args = workload_inputs(workload, programs)
    if workload.primitive is not None:
        build_env = primitive_build_env(os.environ, lane.environment(), programs)
    else:
        build_env = dict(os.environ, PYTHONHASHSEED="0", MOLT_KEEP_SYMBOLS="1")
        build_env.update(lane.environment())
    entry: dict[str, Any] = {"name": workload.name, "build_s": None, "run_s": None}
    with admit_benchmark_workload(
        script, project_root=ROOT, run_args=run_args, molt_args=molt_args
    ) as admitted:
        identity = admitted.record

        def unmeasured(reason: Unmeasured, detail: str) -> list[dict[str, Any]]:
            return [
                {
                    "axis": f"{workload.name}/{phase}",
                    "workload": identity,
                    "unmeasured": reason.value,
                    "detail": detail[-2000:],
                }
                for phase in workload.phases()
            ]

        started = time.monotonic()
        binary = bench.prepare_molt_binary(
            str(script),
            extra_args=[
                "--python-version",
                "3.12",
                "--backend",
                lane.codegen_backend,
                *molt_args,
            ],
            env=build_env,
            build_profile="release",
            source_cwd=admitted.source_cwd,
        )
        entry["build_s"] = round(time.monotonic() - started, 3)
        if isinstance(binary, bench.MoltFailure):
            payload = bench.molt_failure_payload(binary)
            entry["axes"] = unmeasured(
                Unmeasured.BUILD_FAILED, json.dumps(payload, sort_keys=True)
            )
            return entry
        try:
            problems = profile_binding_problems(
                binary.build_observation, backend="native", profile=lane.runtime_profile
            )
            if problems:
                raise ValueError(f"{workload.name}: {problems}")
            env = _run_env()
            started = time.monotonic()
            samples: dict[int | None, list[int]] = {}
            first: dict[int | None, Observation] = {}
            failure = None
            for repeat in range(repeats):
                for n in workload.arguments():
                    args = run_args if n is None else [str(n)]
                    prefix = scratch / f"{workload.name}-{repeat}-{n}"
                    command = [str(binary.path), *args]
                    outcome = observe(
                        counter, command, prefix, env, timeout, cwd=admitted.source_cwd
                    )
                    if isinstance(outcome, ProgramFailed):
                        outcome = observe(
                            counter,
                            command,
                            prefix.with_name(prefix.name + "-rerun"),
                            env,
                            timeout,
                            cwd=admitted.source_cwd,
                        )
                    if isinstance(outcome, ProgramFailed):
                        failure = outcome.detail
                        break
                    samples.setdefault(n, []).append(outcome.observation.instructions)
                    first.setdefault(n, outcome.observation)
                if failure is not None:
                    break
            entry["run_s"] = round(time.monotonic() - started, 3)
            if failure is not None:
                entry["axes"] = unmeasured(Unmeasured.RUN_FAILED, failure)
                return entry
            for n in workload.arguments():
                reference = _reference(
                    workload, admitted, ctx["cpython"], n, env, timeout
                )
                observed = first[n]
                check = compare_outputs(
                    reference,
                    Outputs(observed.stdout, observed.stderr, 0),
                    mode=ComparisonMode.EXACT,
                    stderr_mode="exact",
                    exit_law=ExitLaw.EXACT,
                )
                if not check.equal:
                    entry["axes"] = unmeasured(
                        Unmeasured.OUTPUT_DIFFERS, f"N={n}: {check.detail}"
                    )
                    return entry
            if workload.primitive is None:
                values = {Phase.WHOLE: samples[None]}
            else:
                values = {Phase.STARTUP: samples[0]}
                if Phase.PER_OP in workload.phases():
                    n = workload.primitive.iterations
                    operations = workload.primitive.operations()
                    values[Phase.PER_OP] = [
                        (high - low) / operations
                        for low, high in zip(samples[0], samples[n], strict=True)
                    ]
            entry["axes"] = [
                {
                    "axis": f"{workload.name}/{phase}",
                    "workload": identity,
                    "samples": list(values[phase]),
                }
                for phase in workload.phases()
            ]
        finally:
            binary.temp_dir.cleanup()
    return entry


def _measure_shard(
    names: Sequence[str], ctx: Mapping[str, Any]
) -> list[dict[str, Any]]:
    """One build slot: a worker process measures its workloads in turn.

    A process per slot reproduces step 1's measured configuration, and each
    process gets its own generated bench session, so one build's daemon
    pruning never reaches another's.
    """
    os.environ.pop("MOLT_SESSION_ID", None)
    os.environ.pop("MOLT_SESSION_ID_GENERATED", None)
    registry = {w.name: w for tier in Tier for w in workloads(tier)}
    rows = []
    for name in names:
        print(f"[instructions] {name}", flush=True)
        rows.append(_measure(registry[name], ctx))
    return rows


def measure_record(
    selected: Sequence[Workload],
    *,
    counter: Counter,
    identity: Mapping[str, Any],
    commit: str,
    out: Path,
    base: Mapping[str, Any] | None,
    repeats: int,
    timeout: float,
) -> dict[str, Any]:
    started = time.monotonic()
    env = _run_env()
    cpython = _resolve_cpython(env)
    with tempfile.TemporaryDirectory(prefix="molt-instructions-") as raw:
        scratch = Path(raw)
        # Prove the counter with the actual child environment before any build.
        preflight = observe(
            counter,
            [str(cpython), "-I", "-S", "-c", "pass"],
            scratch / "counter-preflight",
            env,
            min(timeout, 30),
        )
        if isinstance(preflight, ProgramFailed):
            raise RuntimeError(f"counter preflight failed: {preflight.detail}")
        programs = out / "programs"
        programs.mkdir(parents=True, exist_ok=True)
        for workload in selected:
            if workload.primitive is not None:
                (programs / f"{workload.name}.py").write_text(
                    workload.primitive.program(), encoding="utf-8"
                )
        # Admit every workload before any build: a malformed workload stops
        # the run in seconds, not after the build slots drain.
        for workload in selected:
            script, run_args, molt_args = workload_inputs(workload, programs)
            with admit_benchmark_workload(
                script, project_root=ROOT, run_args=run_args, molt_args=molt_args
            ):
                pass
        plan = plan_resource_pressure(
            prefix="MOLT_BENCH",
            max_compile_slots=MAX_COMPILE_SLOTS,
            compile_gb_per_slot=COMPILE_GB_PER_SLOT,
        )
        slots = max(1, min(plan.compile_max_slots, len(selected)))
        seconds = {
            row["name"]: (row.get("build_s") or 0) + (row.get("run_s") or 0)
            for row in (base or {}).get("workloads", ())
        }
        shards = lpt_shards(
            [
                {"path": w.name, "weight": max(1, int(1000 * seconds.get(w.name, 0)))}
                for w in selected
            ],
            slots,
        )
        ctx = {
            "counter": counter.value,
            "cpython": str(cpython),
            "programs": str(programs),
            "scratch": str(scratch),
            "timeout": timeout,
            "repeats": repeats,
        }
        print(
            f"[instructions] {len(selected)} workloads on {slots} build slots",
            flush=True,
        )
        measured_at = time.monotonic()
        with ProcessPoolExecutor(
            max_workers=slots, mp_context=multiprocessing.get_context("spawn")
        ) as pool:
            futures = [
                pool.submit(_measure_shard, shard["entries"], ctx) for shard in shards
            ]
            entries = [row for future in futures for row in future.result()]
        measured_s = time.monotonic() - measured_at
    order = {w.name: index for index, w in enumerate(selected)}
    entries.sort(key=lambda row: order[row["name"]])
    return {
        "schema": RECORD_SCHEMA,
        "commit": commit,
        "identity": dict(identity),
        "identity_digest": identity_digest(identity),
        "recorded_at": datetime.now(UTC).isoformat(),
        "repeats": repeats,
        "build_slots": slots,
        "phase_seconds": {
            "preflight": round(measured_at - started, 3),
            "workloads": round(measured_s, 3),
            "build_work": round(sum(row["build_s"] or 0 for row in entries), 3),
            "run_work": round(sum(row["run_s"] or 0 for row in entries), 3),
        },
        "workloads": [
            {k: row[k] for k in ("name", "build_s", "run_s")} for row in entries
        ],
        "axes": [axis for row in entries for axis in row["axes"]],
    }


def _select(tier: Tier, programs: Sequence[str] | None) -> tuple[Workload, ...]:
    selected = workloads(tier)
    if programs:
        unknown = set(programs) - {w.name for w in selected}
        if unknown:
            raise ValueError(f"unknown {tier} programs: {sorted(unknown)}")
        selected = tuple(w for w in selected if w.name in set(programs))
    return selected


def _summary(payload: Mapping[str, Any]) -> str:
    lines = [
        f"instructions {payload['tier']} {payload['commit'][:12]}: "
        f"{payload['status'].upper()} against {(payload['base'] or 'no base')[:12]}"
    ]
    for kind in ("regressed", "lost", "improved"):
        for row in payload[kind]:
            change = (
                f" {row['change']:+.2%} (threshold {row['threshold']:.2%})"
                if "change" in row
                else f" {row['reason']}"
            )
            lines.append(f"  {kind.upper()} {row['axis']}{change}")
    for row in payload["unmeasured"]:
        lines.append(f"  unmeasured {row['axis']}: {row['reason']}")
    if payload["revert_commands"]:
        lines.append("  suspects: " + ", ".join(c[:12] for c in payload["suspects"]))
        lines.extend("  " + command for command in payload["revert_commands"])
    if payload["creep"]:
        for row in payload["creep"]["regressed"]:
            lines.append(
                f"  creep since {payload['creep']['base'][:12]}: {row['axis']} "
                f"{row['change']:+.2%}"
            )
    return "\n".join(lines)


def run(args: argparse.Namespace) -> int:
    """Measure, store, compare. Returns 0 whenever it wrote a verdict, RED too."""
    tier = Tier(args.tier)
    counter = select_counter(args.counter)
    commit = _git(ROOT, "rev-parse", "HEAD")
    identity = run_identity(counter, tier)
    digest = identity_digest(identity)
    store = RecordStore(args.store or default_store())
    out = args.out_dir.expanduser().absolute()
    out.mkdir(parents=True, exist_ok=False)
    record = store.get(commit, digest)
    if record is None:
        base = store.nearest_measured_ancestor(ROOT, commit, digest)
        record = measure_record(
            _select(tier, args.programs),
            counter=counter,
            identity=identity,
            commit=commit,
            out=out,
            base=base,
            repeats=MIN_REPEATS,
            timeout=args.timeout,
        )
        store.put(record)
    else:
        # Main has not moved since this record: answer from it (invariant 7).
        print(f"[instructions] {commit[:12]} already measured; no new record")
    payload = verdict(store, ROOT, record)
    (out / "record.json").write_bytes(_json_bytes(record))
    (out / "verdict.json").write_bytes(_json_bytes(payload))
    print(_summary(payload))
    return 0


def measure(args: argparse.Namespace) -> int:
    """Count only: no store, no verdict (lane A/B)."""
    tier = Tier(args.tier)
    counter = select_counter(args.counter)
    identity = run_identity(counter, tier)
    out = args.out_dir.expanduser().absolute()
    out.mkdir(parents=True, exist_ok=False)
    record = measure_record(
        _select(tier, args.programs),
        counter=counter,
        identity=identity,
        commit=_git(ROOT, "rev-parse", "HEAD"),
        out=out,
        base=None,
        repeats=args.repeats,
        timeout=args.timeout,
    )
    (out / "record.json").write_bytes(_json_bytes(record))
    for axis in record["axes"]:
        if "samples" in axis:
            print(
                f"{axis['axis']}: {statistics.median(axis['samples']):,.2f} "
                f"(spread {relative_spread(axis['samples']):.3%})"
            )
        else:
            print(f"{axis['axis']}: {axis['unmeasured']}")
    return 0


def status(args: argparse.Namespace) -> int:
    """Print collected verdicts for recent first-parent commits of main."""
    from molt.dx import artifact_root

    evidence = artifact_root(ROOT) / "evidence" / "trunk-loop"
    commits = _git(
        ROOT, "rev-list", "--first-parent", f"--max-count={args.commits}", args.ref
    ).split()
    for commit in commits:
        found = sorted(
            evidence.glob(
                f"{commit}/*/performance.instructions*/*/evidence/"
                "proof-receipts/evidence/instructions*/verdict.json"
            )
        )
        if found:
            for path in found:
                print(_summary(json.loads(path.read_text(encoding="utf-8"))))
            continue
        runs = [
            path
            for path in evidence.glob(f"{commit}/*/runs/*/run.json")
            if any(
                str(cell).startswith("performance.instructions")
                for cell in json.loads(path.read_text(encoding="utf-8")).get(
                    "cells", ()
                )
            )
        ]
        # A run without a verdict stopped on infrastructure; no run at all is
        # a skipped slot. Neither is ever RED (design invariant 5).
        print(
            f"instructions {commit[:12]}: "
            + ("INCONCLUSIVE " + str(runs[0].parent) if runs else "NOT_MEASURED")
        )
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="action", required=True)
    for name in ("run", "measure"):
        mode = sub.add_parser(name)
        mode.add_argument("--out-dir", required=True, type=Path)
        mode.add_argument("--tier", choices=[t.value for t in Tier], default="hourly")
        mode.add_argument("--programs", nargs="+")
        mode.add_argument("--counter", choices=["auto", *Counter], default="auto")
        mode.add_argument("--timeout", type=float, default=600)
    sub.choices["run"].add_argument(
        "--store", type=Path, help="scratch store (default: the farm custody store)"
    )
    sub.choices["measure"].add_argument("--repeats", type=int, default=MIN_REPEATS)
    reader = sub.add_parser("status")
    reader.add_argument("--commits", type=int, default=24)
    reader.add_argument("--ref", default="HEAD")
    args = parser.parse_args(argv)
    if args.action == "run" and args.programs and args.store is None:
        parser.error("--programs writes a partial record; it requires --store")
    if args.action == "measure" and args.repeats < MIN_REPEATS:
        parser.error(f"--repeats must be at least {MIN_REPEATS}")
    try:
        return {"run": run, "measure": measure, "status": status}[args.action](args)
    except (ValueError, RuntimeError, OSError, subprocess.CalledProcessError) as exc:
        print(f"instructions: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    # Delegate to the package module, so spawned build workers import the
    # same functions they are handed.
    from tools.perf_instructions import main as _main

    raise SystemExit(_main(sys.argv[1:]))
