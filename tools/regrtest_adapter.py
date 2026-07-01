#!/usr/bin/env python3
"""S6 — CPython regrtest corpus adapter (doc 69 §5, §1; doc 69A §A.0/§A.9).

CPython's own regression suite (``Lib/test`` driven by ``libregrtest`` / ``python
-m test``) is simultaneously the project's ultimate PARITY ORACLE (the whole
language + stdlib at scale, the exact tests CPython holds itself to) AND a STRESS
CORPUS (long-running, allocation-heavy, adversarial). This adapter is the
``bench_friends`` lane for the *perf + stress* dimension of that corpus:

  * select a curated, version-tagged subset of ``Lib/test`` (conservative seed),
  * run each module under the HOST CPython (the oracle) to confirm GREEN and to
    capture wall time AND peak RSS via ``tools.perf_calibration.run_and_measure``
    (the C4 cross-platform peak-RSS path that fixes the Windows ``RSS=0`` gap),
  * keep the SAME subset + measurement path runnable under molt (deferred — needs
    a build) so the parity+stress comparison is symmetric, never a second code
    path, and
  * emit the JSON shape ``tools/bench_friends.py`` consumes, carrying a
    ``python_version`` dimension so every cell is gated by ``sys.version_info``
    (3.12 / 3.13 / 3.14) per doc 69 §3a.

Relationship to the EXISTING regrtest tooling (no overlap, complementary):

  * ``tools/cpython_regrtest.py`` + ``tools/molt_regrtest_shim.py`` drive a
    *vendored* CPython ``regrtest.py`` with a molt ``--python`` shim and produce
    JUnit-XML CORRECTNESS reports (tests/failures/errors/skipped) for the parity
    oracle (doc 66). They do not measure per-module time + peak RSS into the
    ``bench_friends`` perf scoreboard, and they do not tag the host interpreter
    version for the perf matrix.
  * THIS adapter is the perf/stress half: time + peak-RSS per module, tagged by
    host version, in the ``bench_friends`` JSON shape. The two share the curated
    module list seed (``tools/cpython_regrtest_core.txt``).

Driving model — regrtest is invoked as a SUBPROCESS (``python -m test <module>``),
never imported in-process, because:
  1. Symmetry with the deferred molt lane: molt compiles a program and cannot
     import the host's ``libregrtest`` the way CPython can. A subprocess per
     module is the only form that maps cleanly onto both the CPython oracle
     (``python -m test``) and a future molt regrtest driver. An in-process
     ``run_single_test`` would lock the design to CPython and force an asymmetric
     molt path later.
  2. Faithful measurement + isolation: regrtest mutates global interpreter state,
     installs signal handlers, and writes temp dirs; CPython itself runs modules
     in forked workers. Measuring time + peak RSS around a per-module subprocess
     (with ``--single-process`` and a private ``--tempdir``) is the honest unit.

No third-party dependency; stdlib + ``perf_calibration`` only.
"""

from __future__ import annotations

import argparse
import enum
import importlib.util
import json
import platform
import shutil
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_calibration as pc  # noqa: E402  (C1-C4 calibration substrate)


# ---------------------------------------------------------------------------
# Curated subset (doc 69 §5 S6: "start conservative").
# ---------------------------------------------------------------------------
# A conservative, dependency-light seed spanning the categories the perf+stress
# lane cares about: numeric kernels, serialization, containers, decimal,
# iterators, binary struct, regex, heap. Each entry is a ``Lib/test`` module
# runnable as ``python -m test <module>``. The set is intentionally small; it
# grows as molt parity lands (the registry `R` row tracks the canonical ids).
CURATED_SUBSET: tuple[str, ...] = (
    "test_math",
    "test_json",
    "test_collections",
    "test_decimal",
    "test_itertools",
    "test_struct",
    "test_re",
    "test_heapq",
)

# Minimum host interpreter the perf+stress lane is gated to (doc 69 §3a: parity
# AND perf expectations are dimensioned and gated by Python version 3.12/3.13/
# 3.14). Older hosts are refused rather than silently producing an off-version
# cell that would bleed into another version's board.
MIN_SUPPORTED_VERSION: tuple[int, int] = (3, 12)
SUPPORTED_MAJOR_MINORS: tuple[str, ...] = ("3.12", "3.13", "3.14")


class RunnerMode(enum.Enum):
    """Which engine drives the regrtest module subprocess.

    ``CPYTHON`` is the oracle lane, active now. ``MOLT`` is deferred (needs a
    build); it is a first-class member of this enum precisely so the selection +
    measurement + JSON-emission code path is SHARED — the molt lane is not a
    forked second adapter, it is the same path with a different launch argv.
    """

    CPYTHON = "cpython"
    MOLT = "molt"


@dataclass(frozen=True)
class ModuleResult:
    module: str
    returncode: int
    passed: bool
    elapsed_s: float
    peak_rss_bytes: Optional[int]
    timed_out: bool
    tail: str  # last non-empty stdout line (regrtest verdict line)

    def to_json(self) -> dict[str, object]:
        # `benchmark` + `elapsed_s` are the keys bench_friends'
        # _extract_structured_elapsed() reads from each `results` entry.
        return {
            "benchmark": self.module,
            "elapsed_s": self.elapsed_s,
            "passed": self.passed,
            "returncode": self.returncode,
            "peak_rss_bytes": self.peak_rss_bytes,
            "timed_out": self.timed_out,
            "verdict": self.tail,
        }


@dataclass
class RunReport:
    runner: str
    python_version: str
    python_version_info: list[int]
    python_executable: str
    host_os: str
    host_arch: str
    host_fingerprint: str
    test_dir: str
    requested_modules: list[str]
    results: list[ModuleResult] = field(default_factory=list)

    @property
    def all_passed(self) -> bool:
        return all(r.passed for r in self.results) if self.results else False

    @property
    def total_elapsed_s(self) -> float:
        return sum(r.elapsed_s for r in self.results)

    @property
    def peak_rss_bytes_max(self) -> Optional[int]:
        rss = [r.peak_rss_bytes for r in self.results if r.peak_rss_bytes]
        return max(rss) if rss else None

    def to_json(self) -> dict[str, object]:
        # Top-level shape consumed by bench_friends:
        #   - `status`: must be absent or "ok" for the runner to be accepted
        #     (bench_friends rejects any other status). It is "ok" only when
        #     every selected module ran GREEN under the oracle — a regrtest
        #     failure is a corpus failure, surfaced honestly, never hidden.
        #   - `results`: list of {benchmark, elapsed_s, ...} -> per-module
        #     structured timings on the scoreboard.
        #   - `total_elapsed_s`: bench_friends maps this to the "total" metric.
        #   - `python_version` + `host_*`: the version/host dimensioning (§3a).
        status = "ok" if self.all_passed else "failed"
        payload: dict[str, object] = {
            "schema": "molt.regrtest_adapter.v1",
            "status": status,
            "runner": self.runner,
            "suite": "regrtest",
            "python_version": self.python_version,
            "python_version_info": self.python_version_info,
            "python_executable": self.python_executable,
            "host_os": self.host_os,
            "host_arch": self.host_arch,
            "host_fingerprint": self.host_fingerprint,
            "test_dir": self.test_dir,
            "requested_modules": self.requested_modules,
            "module_count": len(self.results),
            "passed_count": sum(1 for r in self.results if r.passed),
            "failed_modules": [r.module for r in self.results if not r.passed],
            "results": [r.to_json() for r in self.results],
            "total_elapsed_s": self.total_elapsed_s,
            "peak_rss_bytes_max": self.peak_rss_bytes_max,
        }
        return payload


# ---------------------------------------------------------------------------
# Test-directory discovery (host CPython OR a target checkout).
# ---------------------------------------------------------------------------
def discover_test_dir(explicit: Optional[Path] = None) -> Path:
    """Locate the ``Lib/test`` directory.

    Priority: an explicit ``--test-dir`` (e.g. a vendored ``third_party/cpython/
    Lib/test`` for cross-version gating), else the host interpreter's own
    ``test`` package via importlib (no import side effects — we read the spec
    origin only)."""
    if explicit is not None:
        test_dir = explicit.expanduser().resolve()
        if not test_dir.is_dir():
            raise FileNotFoundError(f"--test-dir does not exist: {test_dir}")
        return test_dir
    spec = importlib.util.find_spec("test")
    if spec is None or not spec.origin:
        raise RuntimeError(
            "could not locate the CPython 'test' package on the host interpreter; "
            "pass --test-dir <path-to-Lib/test> explicitly"
        )
    test_dir = Path(spec.origin).resolve().parent
    if not (test_dir / "__init__.py").exists():
        raise RuntimeError(f"resolved test dir is not a package: {test_dir}")
    return test_dir


def available_modules(
    test_dir: Path, modules: Sequence[str]
) -> tuple[list[str], list[str]]:
    """Split requested modules into (present, missing) for this test dir.

    A module is "present" if ``<test_dir>/<module>.py`` exists or it is a test
    subpackage directory (``<test_dir>/<module>/`` with an ``__init__.py``),
    mirroring how regrtest resolves a test name."""
    present: list[str] = []
    missing: list[str] = []
    for module in modules:
        name = module[len("test.") :] if module.startswith("test.") else module
        as_file = test_dir / f"{name}.py"
        as_pkg = test_dir / name / "__init__.py"
        if as_file.exists() or as_pkg.exists():
            present.append(module)
        else:
            missing.append(module)
    return present, missing


# ---------------------------------------------------------------------------
# Launch argv per runner mode.
# ---------------------------------------------------------------------------
def _cpython_argv(python_exe: str, module: str, tempdir: Path) -> list[str]:
    """Argv to run one ``Lib/test`` module under the host CPython oracle.

    ``--single-process`` keeps the module in ONE process so the measured wall
    time + peak RSS are the module's, not a worker-pool's. ``--quiet`` trims the
    output; ``--tempdir`` gives the module a private scratch dir so concurrent
    adapter runs and the repo tree are never polluted."""
    return [
        python_exe,
        "-I",  # isolated: ignore env/user site so the measurement is hermetic
        "-m",
        "test",
        "--single-process",
        "--quiet",
        "--tempdir",
        str(tempdir),
        module,
    ]


def _molt_argv(molt_cmd: Sequence[str], module: str, tempdir: Path) -> list[str]:
    """Argv to run one ``Lib/test`` module under molt (DEFERRED).

    Intentionally NOT wired to a runnable command yet: the molt regrtest lane
    needs a compiled driver and the vendored-CPython ``Lib/test`` module set
    routed through molt's import boundary (the mechanism the existing
    ``tools/molt_regrtest_shim.py`` already implements for the correctness
    harness). When enabled, this returns the molt launch argv for the SAME
    module so the selection/measurement/emission path above is reused verbatim.
    """
    raise NotImplementedError(
        "molt regrtest perf lane is deferred (needs a build + the libregrtest "
        "driver routed through molt's import boundary); the CPython oracle lane "
        "is the active S6 deliverable. See tools/molt_regrtest_shim.py for the "
        "module-routing mechanism the molt lane will reuse."
    )


def build_argv(
    mode: RunnerMode,
    *,
    python_exe: str,
    molt_cmd: Optional[Sequence[str]],
    module: str,
    tempdir: Path,
) -> list[str]:
    if mode is RunnerMode.CPYTHON:
        return _cpython_argv(python_exe, module, tempdir)
    if mode is RunnerMode.MOLT:
        if not molt_cmd:
            raise ValueError("molt runner requires --molt-cmd")
        return _molt_argv(molt_cmd, module, tempdir)
    raise ValueError(f"unknown runner mode: {mode!r}")


# ---------------------------------------------------------------------------
# Measurement (shared by every runner mode).
# ---------------------------------------------------------------------------
def _verdict_tail(stdout: str) -> str:
    for line in reversed(stdout.splitlines()):
        stripped = line.strip()
        if stripped:
            return stripped
    return ""


def run_module(
    mode: RunnerMode,
    module: str,
    *,
    python_exe: str,
    molt_cmd: Optional[Sequence[str]],
    timeout_s: Optional[float],
    env: Optional[dict[str, str]],
) -> ModuleResult:
    """Run ONE module under ``mode`` and capture time + peak RSS.

    Time + peak RSS come from ``perf_calibration.run_and_measure`` — the single
    cross-platform measurement primitive (Windows Job-Object peak, macOS
    ``ru_maxrss``, Linux ``VmHWM``). The module gets a fresh private tempdir that
    is removed after the run regardless of outcome."""
    tempdir = Path(tempfile.mkdtemp(prefix=f"molt_regrtest_{module}_"))
    try:
        argv = build_argv(
            mode,
            python_exe=python_exe,
            molt_cmd=molt_cmd,
            module=module,
            tempdir=tempdir,
        )
        measurement = pc.run_and_measure(argv, timeout=timeout_s, env=env)
        return ModuleResult(
            module=module,
            returncode=measurement.returncode,
            passed=(measurement.returncode == 0 and not measurement.timed_out),
            elapsed_s=measurement.elapsed_s,
            peak_rss_bytes=measurement.peak_rss_bytes,
            timed_out=measurement.timed_out,
            tail=_verdict_tail(measurement.stdout),
        )
    finally:
        shutil.rmtree(tempdir, ignore_errors=True)


# ---------------------------------------------------------------------------
# Version gating (doc 69 §3a).
# ---------------------------------------------------------------------------
def host_major_minor() -> str:
    return f"{sys.version_info.major}.{sys.version_info.minor}"


def assert_version_supported() -> None:
    if sys.version_info[:2] < MIN_SUPPORTED_VERSION:
        raise RuntimeError(
            f"host CPython {platform.python_version()} is below the regrtest "
            f"perf-lane floor {MIN_SUPPORTED_VERSION[0]}.{MIN_SUPPORTED_VERSION[1]}; "
            "the version-gated corpus targets 3.12 / 3.13 / 3.14 (doc 69 §3a)"
        )


# ---------------------------------------------------------------------------
# Top-level run.
# ---------------------------------------------------------------------------
def run(
    *,
    mode: RunnerMode,
    modules: Sequence[str],
    test_dir: Path,
    python_exe: str,
    molt_cmd: Optional[Sequence[str]],
    timeout_s: Optional[float],
    skip_missing: bool,
    env: Optional[dict[str, str]],
) -> RunReport:
    present, missing = available_modules(test_dir, modules)
    if missing and not skip_missing:
        raise FileNotFoundError(
            f"requested modules not found under {test_dir}: {', '.join(missing)} "
            "(pass --skip-missing to ignore, or --test-dir to point at the right "
            "checkout)"
        )

    fp = pc.host_fingerprint()
    report = RunReport(
        runner=mode.value,
        python_version=platform.python_version(),
        python_version_info=list(sys.version_info[:3]),
        python_executable=python_exe,
        host_os=fp.os,
        host_arch=fp.arch,
        host_fingerprint=fp.key(),
        test_dir=str(test_dir),
        requested_modules=list(present),
    )
    for module in present:
        report.results.append(
            run_module(
                mode,
                module,
                python_exe=python_exe,
                molt_cmd=molt_cmd,
                timeout_s=timeout_s,
                env=env,
            )
        )
    return report


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def _parse_modules_csv(raw: Optional[str]) -> tuple[str, ...]:
    if not raw:
        return CURATED_SUBSET
    cleaned = tuple(value.strip() for value in raw.split(",") if value.strip())
    return cleaned or CURATED_SUBSET


def _cmd_list(args: argparse.Namespace) -> int:
    test_dir = discover_test_dir(args.test_dir)
    modules = _parse_modules_csv(args.modules)
    present, missing = available_modules(test_dir, modules)
    payload = {
        "schema": "molt.regrtest_adapter.v1",
        "test_dir": str(test_dir),
        "python_version": platform.python_version(),
        "python_version_info": list(sys.version_info[:3]),
        "supported_major_minors": list(SUPPORTED_MAJOR_MINORS),
        "curated_subset": list(CURATED_SUBSET),
        "requested_modules": list(modules),
        "present": present,
        "missing": missing,
    }
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        print(f"test_dir={test_dir}")
        print(f"python_version={platform.python_version()}")
        print("present=" + ",".join(present))
        if missing:
            print("missing=" + ",".join(missing))
    return 0


def _parse_env_kv(pairs: Sequence[str]) -> dict[str, str]:
    env: dict[str, str] = {}
    for pair in pairs:
        if "=" not in pair:
            raise ValueError(f"--env expects KEY=VALUE, got {pair!r}")
        key, value = pair.split("=", 1)
        env[key] = value
    return env


def _cmd_run(args: argparse.Namespace) -> int:
    assert_version_supported()
    mode = RunnerMode(args.runner)
    test_dir = discover_test_dir(args.test_dir)
    modules = _parse_modules_csv(args.modules)
    molt_cmd = args.molt_cmd if args.molt_cmd else None
    env = _parse_env_kv(args.env) if args.env else None
    report = run(
        mode=mode,
        modules=modules,
        test_dir=test_dir,
        python_exe=args.python,
        molt_cmd=molt_cmd,
        timeout_s=args.timeout,
        skip_missing=args.skip_missing,
        env=env,
    )
    if args.json:
        print(json.dumps(report.to_json(), sort_keys=True))
    else:
        for result in report.results:
            rss_mb = (
                f"{result.peak_rss_bytes / 1048576:.1f}MB"
                if result.peak_rss_bytes
                else "?"
            )
            verdict = "PASS" if result.passed else "FAIL"
            print(
                f"[{verdict}] {result.module} "
                f"elapsed_s={result.elapsed_s:.3f} peak_rss={rss_mb} "
                f"({result.tail})"
            )
        print(
            f"runner={report.runner} python={report.python_version} "
            f"passed={report.all_passed} total_elapsed_s={report.total_elapsed_s:.3f} "
            f"peak_rss_max={report.peak_rss_bytes_max}"
        )
    # Non-zero exit when the oracle (or molt) did not run the corpus GREEN, so a
    # CI lane wrapping this adapter fails loudly rather than recording a silent
    # red. bench_friends additionally rejects a non-"ok" JSON status.
    return 0 if report.all_passed else 1


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "S6 CPython regrtest corpus adapter: run a version-tagged Lib/test "
            "subset under the CPython oracle (or, deferred, molt), capturing time "
            "+ peak RSS in the bench_friends JSON shape (doc 69 §5)."
        )
    )
    sub = parser.add_subparsers(dest="command", required=True)

    list_cmd = sub.add_parser(
        "list",
        help="Show the curated subset and which modules are present in the test dir.",
    )
    list_cmd.add_argument("--test-dir", type=Path, default=None)
    list_cmd.add_argument(
        "--modules",
        default=None,
        help="Comma-separated module list (default: curated subset).",
    )
    list_cmd.add_argument("--json", action="store_true")
    list_cmd.set_defaults(func=_cmd_list)

    run_cmd = sub.add_parser(
        "run",
        help="Run the subset under a runner and emit time + peak RSS as JSON.",
    )
    run_cmd.add_argument(
        "--runner",
        choices=[m.value for m in RunnerMode],
        default=RunnerMode.CPYTHON.value,
        help="Engine driving regrtest (default: cpython oracle; molt deferred).",
    )
    run_cmd.add_argument(
        "--python",
        default=sys.executable,
        help="Host CPython executable for the cpython runner (default: sys.executable).",
    )
    run_cmd.add_argument(
        "--molt-cmd",
        nargs="+",
        default=None,
        help="Command for the molt runner (deferred; e.g. 'molt run').",
    )
    run_cmd.add_argument("--test-dir", type=Path, default=None)
    run_cmd.add_argument(
        "--modules",
        default=None,
        help="Comma-separated module list (default: curated subset).",
    )
    run_cmd.add_argument(
        "--timeout",
        type=float,
        default=900.0,
        help="Per-module wall-time cap in seconds (default: 900).",
    )
    run_cmd.add_argument(
        "--skip-missing",
        action="store_true",
        help="Skip requested modules absent from the test dir instead of failing.",
    )
    run_cmd.add_argument(
        "--env",
        action="append",
        default=[],
        help="Extra KEY=VALUE env var for the child (repeatable).",
    )
    run_cmd.add_argument("--json", action="store_true")
    run_cmd.set_defaults(func=_cmd_run)

    return parser


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
