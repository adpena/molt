import argparse
import datetime as dt
import json
import os
import platform
import statistics
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


SUPPORTED_SEMANTIC_MODES = {
    "runs_unmodified",
    "requires_adapter",
    "unsupported_by_molt",
}


@dataclass(frozen=True)
class RunnerSpec:
    name: str
    build_cmd: list[str] | None
    run_cmd: list[str] | None
    env: dict[str, str]
    skip_reason: str | None


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

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and not self.timed_out


@dataclass
class RunnerResult:
    name: str
    status: str
    reason: str | None = None
    build: PhaseResult | None = None
    runs: list[PhaseResult] = field(default_factory=list)
    run_samples_s: list[float] = field(default_factory=list)
    run_median_s: float | None = None
    run_mean_s: float | None = None
    run_stdev_s: float | None = None


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
    status: str
    reason: str | None
    adapter_notes: str | None
    tags: list[str]
    runners: dict[str, RunnerResult]
    metrics: dict[str, float | None]


def _git_rev() -> str | None:
    try:
        res = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if res.returncode != 0:
        return None
    return res.stdout.strip() or None


def _external_root() -> Path | None:
    root = Path("/Volumes/APDataStore/Molt")
    return root if root.is_dir() else None


def _default_output_root() -> Path:
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    external = _external_root()
    if external is not None:
        return external / "benchmarks" / "friends" / timestamp
    return Path("bench/results/friends") / timestamp


def _load_manifest(path: Path) -> tuple[dict[str, Any], list[SuiteSpec]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    schema_version = int(data.get("schema_version", 1))
    defaults = data.get("defaults", {})
    suites_raw = data.get("suite", [])
    if not isinstance(suites_raw, list):
        raise ValueError("manifest `suite` must be an array of tables")
    suites = [_parse_suite(raw, defaults) for raw in suites_raw]
    return {"schema_version": schema_version}, suites


def _parse_suite(raw: dict[str, Any], defaults: dict[str, Any]) -> SuiteSpec:
    suite_id = str(raw.get("id", "")).strip()
    if not suite_id:
        raise ValueError("suite id is required")
    friend = str(raw.get("friend", "")).strip()
    if not friend:
        raise ValueError(f"suite {suite_id}: friend is required")
    source = str(raw.get("source", "local")).strip()
    if source not in {"local", "git"}:
        raise ValueError(f"suite {suite_id}: unsupported source {source!r}")

    semantic_mode = str(raw.get("semantic_mode", "requires_adapter")).strip()
    if semantic_mode not in SUPPORTED_SEMANTIC_MODES:
        raise ValueError(
            f"suite {suite_id}: semantic_mode must be one of "
            f"{sorted(SUPPORTED_SEMANTIC_MODES)}"
        )

    timeout_sec = int(raw.get("timeout_sec", defaults.get("timeout_sec", 900)))
    repeat = int(raw.get("repeat", defaults.get("repeat", 3)))
    if timeout_sec <= 0:
        raise ValueError(f"suite {suite_id}: timeout_sec must be positive")
    if repeat <= 0:
        raise ValueError(f"suite {suite_id}: repeat must be positive")

    runners = _parse_runners(suite_id, raw.get("runners", {}))
    if not runners:
        raise ValueError(f"suite {suite_id}: at least one runner is required")

    prepare_cmds = _parse_command_list(raw.get("prepare_cmds", []), "prepare_cmds")
    suite_env = _parse_env(raw.get("env", defaults.get("env", {})))

    return SuiteSpec(
        id=suite_id,
        friend=friend,
        display_name=str(raw.get("display_name", suite_id)),
        enabled=bool(raw.get("enabled", False)),
        source=source,
        repo_url=_optional_str(raw.get("repo_url")),
        repo_ref=_optional_str(raw.get("repo_ref")),
        local_path=_optional_str(raw.get("local_path")),
        workdir=_optional_str(raw.get("workdir")),
        semantic_mode=semantic_mode,
        adapter_notes=_optional_str(raw.get("adapter_notes")),
        tags=[str(v) for v in raw.get("tags", [])],
        timeout_sec=timeout_sec,
        repeat=repeat,
        env=suite_env,
        prepare_cmds=prepare_cmds,
        runners=runners,
    )


def _optional_str(value: Any) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def _parse_env(raw_env: Any) -> dict[str, str]:
    if not raw_env:
        return {}
    if not isinstance(raw_env, dict):
        raise ValueError("env must be a table/object of string values")
    parsed: dict[str, str] = {}
    for key, value in raw_env.items():
        parsed[str(key)] = str(value)
    return parsed


def _parse_command_list(raw: Any, field_name: str) -> list[list[str]]:
    if raw in (None, []):
        return []
    if not isinstance(raw, list):
        raise ValueError(f"{field_name} must be an array")
    parsed: list[list[str]] = []
    for idx, entry in enumerate(raw):
        if not isinstance(entry, list) or not entry:
            raise ValueError(f"{field_name}[{idx}] must be a non-empty command array")
        parsed.append([str(part) for part in entry])
    return parsed


def _parse_runners(suite_id: str, raw_runners: Any) -> dict[str, RunnerSpec]:
    if not isinstance(raw_runners, dict):
        raise ValueError(f"suite {suite_id}: runners must be a table/object")
    runners: dict[str, RunnerSpec] = {}
    for runner_name in ("cpython", "molt", "friend"):
        runner_raw = raw_runners.get(runner_name)
        if runner_raw is None:
            continue
        if not isinstance(runner_raw, dict):
            raise ValueError(
                f"suite {suite_id} runner {runner_name}: runner must be a table/object"
            )
        build_cmd = runner_raw.get("build_cmd")
        run_cmd = runner_raw.get("run_cmd")
        cmd = runner_raw.get("cmd")
        if run_cmd is None and cmd is not None:
            run_cmd = cmd
        parsed_build = _parse_single_command(build_cmd, "build_cmd")
        parsed_run = _parse_single_command(run_cmd, "run_cmd")
        runners[runner_name] = RunnerSpec(
            name=runner_name,
            build_cmd=parsed_build,
            run_cmd=parsed_run,
            env=_parse_env(runner_raw.get("env", {})),
            skip_reason=_optional_str(runner_raw.get("skip_reason")),
        )
    return runners


def _parse_single_command(raw: Any, field_name: str) -> list[str] | None:
    if raw in (None, []):
        return None
    if not isinstance(raw, list) or not raw:
        raise ValueError(f"{field_name} must be a non-empty command array")
    return [str(part) for part in raw]


def _resolve_tokenized(parts: list[str], tokens: dict[str, str]) -> list[str]:
    return [part.format_map(tokens) for part in parts]


def _resolve_env(raw_env: dict[str, str], tokens: dict[str, str]) -> dict[str, str]:
    return {key: value.format_map(tokens) for key, value in raw_env.items()}


def _run_command(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout_sec: int,
    stdout_path: Path,
    stderr_path: Path,
    dry_run: bool,
) -> PhaseResult:
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stderr_path.parent.mkdir(parents=True, exist_ok=True)
    if dry_run:
        stdout_path.write_text(
            f"[dry-run] cwd={cwd}\n$ {' '.join(cmd)}\n", encoding="utf-8"
        )
        stderr_path.write_text("", encoding="utf-8")
        return PhaseResult(
            cmd=cmd,
            returncode=0,
            elapsed_s=0.0,
            timed_out=False,
            stdout_path=str(stdout_path),
            stderr_path=str(stderr_path),
        )

    start = dt.datetime.now(dt.timezone.utc)
    timed_out = False
    try:
        res = subprocess.run(
            cmd,
            cwd=str(cwd),
            env=env,
            capture_output=True,
            text=True,
            timeout=timeout_sec,
            check=False,
        )
        rc = res.returncode
        stdout = res.stdout or ""
        stderr = res.stderr or ""
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        rc = -9
        stdout = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
        stderr = (exc.stderr or "") if isinstance(exc.stderr, str) else ""
        stderr = f"{stderr}\n[timeout] command exceeded {timeout_sec}s\n"
    end = dt.datetime.now(dt.timezone.utc)
    elapsed = (end - start).total_seconds()
    stdout_path.write_text(stdout, encoding="utf-8")
    stderr_path.write_text(stderr, encoding="utf-8")
    return PhaseResult(
        cmd=cmd,
        returncode=rc,
        elapsed_s=elapsed,
        timed_out=timed_out,
        stdout_path=str(stdout_path),
        stderr_path=str(stderr_path),
    )


def _run_git(
    args: list[str], *, cwd: Path | None, timeout_sec: int, dry_run: bool
) -> tuple[int, str, str]:
    cmd = ["git", *args]
    if dry_run:
        return 0, "[dry-run]\n", ""
    res = subprocess.run(
        cmd,
        cwd=str(cwd) if cwd is not None else None,
        capture_output=True,
        text=True,
        timeout=timeout_sec,
        check=False,
    )
    return res.returncode, res.stdout or "", res.stderr or ""


def _acquire_suite(
    suite: SuiteSpec,
    *,
    repos_root: Path,
    checkout: bool,
    fetch: bool,
    timeout_sec: int,
    dry_run: bool,
) -> tuple[Path, Path, str | None]:
    if suite.source == "local":
        if not suite.local_path:
            raise ValueError(
                f"suite {suite.id}: local_path is required for source=local"
            )
        suite_root = Path(suite.local_path).expanduser()
        if not dry_run and not suite_root.exists():
            raise FileNotFoundError(
                f"suite {suite.id}: local path not found: {suite_root}"
            )
        suite_workdir = (
            (suite_root / suite.workdir).resolve()
            if suite.workdir
            else suite_root.resolve()
        )
        return suite_root, suite_workdir, None

    if suite.source != "git":
        raise ValueError(f"suite {suite.id}: unsupported source {suite.source}")
    if not suite.repo_url:
        raise ValueError(f"suite {suite.id}: repo_url is required for source=git")
    if not suite.repo_ref:
        raise ValueError(f"suite {suite.id}: repo_ref is required for source=git")
    if "PINNED" in suite.repo_ref.upper() and not dry_run:
        raise ValueError(
            f"suite {suite.id}: repo_ref must be set to a pinned commit/tag, "
            "not a placeholder"
        )

    repo_dir = repos_root / suite.id
    if checkout:
        if not repo_dir.exists():
            repo_dir.parent.mkdir(parents=True, exist_ok=True)
            rc, _out, err = _run_git(
                ["clone", suite.repo_url, str(repo_dir)],
                cwd=None,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git clone failed: {err.strip()}")
        if fetch:
            rc, _out, err = _run_git(
                ["fetch", "--all", "--tags", "--prune"],
                cwd=repo_dir,
                timeout_sec=timeout_sec,
                dry_run=dry_run,
            )
            if rc != 0:
                raise RuntimeError(f"suite {suite.id}: git fetch failed: {err.strip()}")
        rc, _out, err = _run_git(
            ["checkout", "--detach", suite.repo_ref],
            cwd=repo_dir,
            timeout_sec=timeout_sec,
            dry_run=dry_run,
        )
        if rc != 0:
            raise RuntimeError(
                f"suite {suite.id}: git checkout {suite.repo_ref} failed: {err.strip()}"
            )

    if not dry_run and not repo_dir.exists():
        raise FileNotFoundError(
            f"suite {suite.id}: repo checkout missing at {repo_dir}; "
            "run with --checkout"
        )
    resolved_ref = None
    if not dry_run:
        rc, out, err = _run_git(
            ["rev-parse", "HEAD"],
            cwd=repo_dir,
            timeout_sec=timeout_sec,
            dry_run=False,
        )
        if rc != 0:
            raise RuntimeError(f"suite {suite.id}: git rev-parse failed: {err.strip()}")
        resolved_ref = out.strip() or None
    suite_workdir = (
        (repo_dir / suite.workdir).resolve() if suite.workdir else repo_dir.resolve()
    )
    return repo_dir, suite_workdir, resolved_ref


def _run_prepare_steps(
    suite: SuiteSpec,
    *,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    timeout_sec: int,
    logs_dir: Path,
    dry_run: bool,
) -> tuple[bool, str | None]:
    for idx, prepare_cmd in enumerate(suite.prepare_cmds, start=1):
        resolved_cmd = _resolve_tokenized(prepare_cmd, tokens)
        out = logs_dir / f"prepare_{idx}.stdout.log"
        err = logs_dir / f"prepare_{idx}.stderr.log"
        phase = _run_command(
            resolved_cmd,
            cwd=suite_workdir,
            env=suite_env,
            timeout_sec=timeout_sec,
            stdout_path=out,
            stderr_path=err,
            dry_run=dry_run,
        )
        if not phase.ok:
            return False, f"prepare step {idx} failed"
    return True, None


def _run_runner(
    runner: RunnerSpec,
    *,
    suite: SuiteSpec,
    suite_workdir: Path,
    suite_env: dict[str, str],
    tokens: dict[str, str],
    logs_dir: Path,
    dry_run: bool,
) -> RunnerResult:
    if runner.skip_reason:
        return RunnerResult(
            name=runner.name, status="skipped", reason=runner.skip_reason
        )
    if not runner.run_cmd:
        return RunnerResult(
            name=runner.name,
            status="skipped",
            reason="run_cmd not configured",
        )

    env = suite_env.copy()
    env.update(_resolve_env(runner.env, tokens))
    result = RunnerResult(name=runner.name, status="ok")

    if runner.build_cmd:
        build_cmd = _resolve_tokenized(runner.build_cmd, tokens)
        build = _run_command(
            build_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.build.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.build.stderr.log",
            dry_run=dry_run,
        )
        result.build = build
        if not build.ok:
            result.status = "failed"
            result.reason = "build failed"
            return result

    run_cmd = _resolve_tokenized(runner.run_cmd, tokens)
    for run_idx in range(1, suite.repeat + 1):
        phase = _run_command(
            run_cmd,
            cwd=suite_workdir,
            env=env,
            timeout_sec=suite.timeout_sec,
            stdout_path=logs_dir / f"{runner.name}.run{run_idx}.stdout.log",
            stderr_path=logs_dir / f"{runner.name}.run{run_idx}.stderr.log",
            dry_run=dry_run,
        )
        result.runs.append(phase)
        if not phase.ok:
            result.status = "failed"
            result.reason = f"run {run_idx} failed"
            return result
        result.run_samples_s.append(phase.elapsed_s)

    if result.run_samples_s:
        result.run_median_s = statistics.median(result.run_samples_s)
        result.run_mean_s = statistics.mean(result.run_samples_s)
        if len(result.run_samples_s) > 1:
            result.run_stdev_s = statistics.stdev(result.run_samples_s)
        else:
            result.run_stdev_s = 0.0
    return result


def _suite_metrics(runners: dict[str, RunnerResult]) -> dict[str, float | None]:
    cp = runners.get("cpython")
    mt = runners.get("molt")
    ct = runners.get("friend")
    cp_s = cp.run_median_s if cp and cp.status == "ok" else None
    mt_s = mt.run_median_s if mt and mt.status == "ok" else None
    ct_s = ct.run_median_s if ct and ct.status == "ok" else None

    molt_vs_cpython = cp_s / mt_s if cp_s and mt_s else None
    molt_vs_friend = ct_s / mt_s if ct_s and mt_s else None
    friend_vs_molt = mt_s / ct_s if ct_s and mt_s else None
    return {
        "cpython_median_s": cp_s,
        "molt_median_s": mt_s,
        "friend_median_s": ct_s,
        "molt_vs_cpython_speedup": molt_vs_cpython,
        "molt_vs_friend_speedup": molt_vs_friend,
        "friend_vs_molt_speedup": friend_vs_molt,
    }


def _suite_status(runners: dict[str, RunnerResult]) -> tuple[str, str | None]:
    failed = [name for name, runner in runners.items() if runner.status == "failed"]
    if failed:
        return "failed", f"runner failures: {', '.join(sorted(failed))}"
    ok_count = sum(1 for runner in runners.values() if runner.status == "ok")
    if ok_count == 0:
        return "skipped", "no runnable runners"
    return "ok", None


def _format_optional(value: float | None) -> str:
    if value is None:
        return "-"
    return f"{value:.4f}"


def _render_summary_markdown(
    *,
    run_started_at: str,
    manifest_path: Path,
    json_rel: str,
    suites: list[SuiteResult],
) -> str:
    lines: list[str] = []
    lines.append("# Friend Benchmark Summary")
    lines.append("")
    lines.append(f"Generated: {run_started_at}")
    lines.append(f"Manifest: `{manifest_path}`")
    lines.append(f"JSON: `{json_rel}`")
    lines.append("")
    lines.append(
        "| Suite | Semantic Mode | Status | CPython s | Molt s | "
        "Friend s | Molt/CPython | Molt/Friend |"
    )
    lines.append("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |")
    for suite in suites:
        m = suite.metrics
        lines.append(
            "| "
            f"{suite.id} | {suite.semantic_mode} | {suite.status} | "
            f"{_format_optional(m.get('cpython_median_s'))} | "
            f"{_format_optional(m.get('molt_median_s'))} | "
            f"{_format_optional(m.get('friend_median_s'))} | "
            f"{_format_optional(m.get('molt_vs_cpython_speedup'))} | "
            f"{_format_optional(m.get('molt_vs_friend_speedup'))} |"
        )

    lines.append("")
    lines.append("## Notes")
    lines.append(
        "- Semantic mode values: `runs_unmodified`, `requires_adapter`, "
        "`unsupported_by_molt`."
    )
    lines.append(
        "- `Molt/Friend` values > 1.0 indicate Molt is faster on the suite median."
    )
    lines.append(
        "- Compile-vs-run separation is recorded per runner when build commands "
        "are configured."
    )

    failures = [s for s in suites if s.status != "ok"]
    if failures:
        lines.append("")
        lines.append("## Non-OK Suites")
        for suite in failures:
            reason = suite.reason or "no reason provided"
            lines.append(f"- `{suite.id}`: {suite.status} ({reason})")

    lines.append("")
    lines.append("Generated by `tools/bench_friends.py`.")
    return "\n".join(lines) + "\n"


def _runner_to_dict(result: RunnerResult) -> dict[str, Any]:
    return {
        "name": result.name,
        "status": result.status,
        "reason": result.reason,
        "build": _phase_to_dict(result.build) if result.build else None,
        "runs": [_phase_to_dict(phase) for phase in result.runs],
        "run_samples_s": result.run_samples_s,
        "run_median_s": result.run_median_s,
        "run_mean_s": result.run_mean_s,
        "run_stdev_s": result.run_stdev_s,
    }


def _phase_to_dict(phase: PhaseResult) -> dict[str, Any]:
    return {
        "cmd": phase.cmd,
        "returncode": phase.returncode,
        "elapsed_s": phase.elapsed_s,
        "timed_out": phase.timed_out,
        "stdout_path": phase.stdout_path,
        "stderr_path": phase.stderr_path,
    }


def _suite_to_dict(suite: SuiteResult) -> dict[str, Any]:
    return {
        "id": suite.id,
        "friend": suite.friend,
        "display_name": suite.display_name,
        "semantic_mode": suite.semantic_mode,
        "source": suite.source,
        "suite_root": suite.suite_root,
        "suite_workdir": suite.suite_workdir,
        "resolved_ref": suite.resolved_ref,
        "status": suite.status,
        "reason": suite.reason,
        "adapter_notes": suite.adapter_notes,
        "tags": suite.tags,
        "metrics": suite.metrics,
        "runners": {
            name: _runner_to_dict(result) for name, result in suite.runners.items()
        },
    }


def _select_suites(
    suites: list[SuiteSpec], *, suite_filters: set[str], include_disabled: bool
) -> list[SuiteSpec]:
    if suite_filters:
        selected = [suite for suite in suites if suite.id in suite_filters]
    else:
        selected = suites
    if include_disabled:
        return selected
    return [suite for suite in selected if suite.enabled]


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run Molt against friend-owned benchmark suites using a pinned "
            "manifest and reproducible command protocol."
        )
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("bench/friends/manifest.toml"),
        help="Path to friend benchmark manifest TOML.",
    )
    parser.add_argument(
        "--suite",
        action="append",
        default=[],
        help="Run only selected suite id (repeatable).",
    )
    parser.add_argument(
        "--include-disabled",
        action="store_true",
        help="Include suites marked enabled=false in manifest.",
    )
    parser.add_argument(
        "--output-root",
        type=Path,
        default=None,
        help="Output root directory. Default prefers external volume when present.",
    )
    parser.add_argument(
        "--repos-root",
        type=Path,
        default=Path("bench/friends/repos"),
        help="Local cache root for git-based friend suites.",
    )
    parser.add_argument(
        "--repeat",
        type=int,
        default=None,
        help="Override repeat count for all suites.",
    )
    parser.add_argument(
        "--timeout-sec",
        type=int,
        default=None,
        help="Override command timeout for all suites.",
    )
    parser.add_argument(
        "--checkout",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Clone/checkout/fetch git suites as needed.",
    )
    parser.add_argument(
        "--fetch",
        action="store_true",
        help="Fetch updates before checkout for git suites.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Plan and emit artifacts without executing real commands.",
    )
    parser.add_argument(
        "--fail-fast",
        action="store_true",
        help="Stop after the first suite failure.",
    )
    parser.add_argument(
        "--summary-out",
        type=Path,
        default=None,
        help="Override summary markdown output path.",
    )
    parser.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="Override JSON output path.",
    )
    parser.add_argument(
        "--update-doc",
        action="store_true",
        help="Also write docs/benchmarks/friend_summary.md from this run.",
    )
    return parser


def main() -> int:
    args = _parser().parse_args()
    manifest_path = args.manifest.resolve()
    metadata, suites = _load_manifest(manifest_path)
    selected = _select_suites(
        suites,
        suite_filters=set(args.suite),
        include_disabled=args.include_disabled,
    )
    if not selected:
        print("No suites selected. Use --include-disabled or --suite.", file=sys.stderr)
        return 2

    if args.repeat is not None and args.repeat <= 0:
        print("--repeat must be positive", file=sys.stderr)
        return 2
    if args.timeout_sec is not None and args.timeout_sec <= 0:
        print("--timeout-sec must be positive", file=sys.stderr)
        return 2

    run_started = dt.datetime.now(dt.timezone.utc)
    output_root = (args.output_root or _default_output_root()).resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    repos_root = args.repos_root.resolve()

    run_env = os.environ.copy()
    run_env.setdefault("PYTHONHASHSEED", "0")
    run_env.setdefault("PYTHONUNBUFFERED", "1")

    suite_results: list[SuiteResult] = []
    overall_rc = 0
    for suite in selected:
        suite_timeout = args.timeout_sec or suite.timeout_sec
        suite_repeat = args.repeat or suite.repeat
        suite = SuiteSpec(
            **{
                **suite.__dict__,
                "timeout_sec": suite_timeout,
                "repeat": suite_repeat,
            }
        )
        try:
            suite_root, suite_workdir, resolved_ref = _acquire_suite(
                suite,
                repos_root=repos_root,
                checkout=args.checkout,
                fetch=args.fetch,
                timeout_sec=suite.timeout_sec,
                dry_run=args.dry_run,
            )
            suite_logs = output_root / "logs" / suite.id
            tokens = {
                "repo_root": str(Path.cwd().resolve()),
                "suite_root": str(suite_root.resolve()),
                "suite_workdir": str(suite_workdir.resolve()),
                "output_root": str(output_root),
            }
            suite_env = run_env.copy()
            suite_env.update(_resolve_env(suite.env, tokens))
            prep_ok, prep_reason = _run_prepare_steps(
                suite,
                suite_workdir=suite_workdir,
                suite_env=suite_env,
                tokens=tokens,
                timeout_sec=suite.timeout_sec,
                logs_dir=suite_logs,
                dry_run=args.dry_run,
            )
            runners: dict[str, RunnerResult] = {}
            if prep_ok:
                for runner_name, runner_spec in suite.runners.items():
                    runners[runner_name] = _run_runner(
                        runner_spec,
                        suite=suite,
                        suite_workdir=suite_workdir,
                        suite_env=suite_env,
                        tokens=tokens,
                        logs_dir=suite_logs,
                        dry_run=args.dry_run,
                    )
            else:
                for runner_name in suite.runners:
                    runners[runner_name] = RunnerResult(
                        name=runner_name,
                        status="failed",
                        reason=prep_reason,
                    )
            status, reason = _suite_status(runners)
            if prep_reason and not reason:
                reason = prep_reason
            metrics = _suite_metrics(runners)
            suite_result = SuiteResult(
                id=suite.id,
                friend=suite.friend,
                display_name=suite.display_name,
                semantic_mode=suite.semantic_mode,
                source=suite.source,
                suite_root=str(suite_root),
                suite_workdir=str(suite_workdir),
                resolved_ref=resolved_ref,
                status=status,
                reason=reason,
                adapter_notes=suite.adapter_notes,
                tags=suite.tags,
                runners=runners,
                metrics=metrics,
            )
            suite_results.append(suite_result)
            if status == "failed":
                overall_rc = 1
                if args.fail_fast:
                    break
        except Exception as exc:  # noqa: BLE001
            suite_result = SuiteResult(
                id=suite.id,
                friend=suite.friend,
                display_name=suite.display_name,
                semantic_mode=suite.semantic_mode,
                source=suite.source,
                suite_root="",
                suite_workdir="",
                resolved_ref=None,
                status="failed",
                reason=str(exc),
                adapter_notes=suite.adapter_notes,
                tags=suite.tags,
                runners={},
                metrics=_suite_metrics({}),
            )
            suite_results.append(suite_result)
            overall_rc = 1
            if args.fail_fast:
                break

    json_out = (args.json_out or (output_root / "results.json")).resolve()
    json_out.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "schema_version": 1,
        "manifest_schema_version": metadata["schema_version"],
        "generated_at": run_started.isoformat(),
        "manifest_path": str(manifest_path),
        "git_rev": _git_rev(),
        "dry_run": args.dry_run,
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
        },
        "options": {
            "include_disabled": args.include_disabled,
            "checkout": args.checkout,
            "fetch": args.fetch,
            "repeat_override": args.repeat,
            "timeout_override": args.timeout_sec,
        },
        "suites": [_suite_to_dict(suite) for suite in suite_results],
    }
    json_out.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")

    summary_out = (args.summary_out or (output_root / "summary.md")).resolve()
    summary_out.parent.mkdir(parents=True, exist_ok=True)
    summary_text = _render_summary_markdown(
        run_started_at=run_started.isoformat(),
        manifest_path=manifest_path,
        json_rel=str(json_out),
        suites=suite_results,
    )
    summary_out.write_text(summary_text, encoding="utf-8")

    if args.update_doc:
        doc_out = Path("docs/benchmarks/friend_summary.md").resolve()
        doc_out.parent.mkdir(parents=True, exist_ok=True)
        doc_out.write_text(summary_text, encoding="utf-8")

    print(f"Wrote JSON: {json_out}")
    print(f"Wrote summary: {summary_out}")
    if args.update_doc:
        print("Updated docs/benchmarks/friend_summary.md")
    return overall_rc


if __name__ == "__main__":
    raise SystemExit(main())
