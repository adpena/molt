from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import time
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path

from molt.symphony.dlq import DeadLetterQueue, dead_letter_fingerprint
from molt.symphony.loop_hooks import HookDecision, LoopHookRunner
from molt.symphony.paths import (
    symphony_dlq_events_file,
    resolve_molt_ext_root,
    symphony_perf_reports_dir,
    symphony_recursive_loop_dir,
    symphony_taste_memory_distillations_dir,
    symphony_taste_memory_events_file,
    symphony_tool_promotion_distillations_dir,
    symphony_tool_promotion_events_file,
)
from molt.symphony.taste_memory import TasteMemoryStore
from molt.symphony.tool_promotion import ToolPromotionStore

DEFAULT_EXT_ROOT = "/Volumes/APDataStore/Molt"
DEFAULT_TEAM = "Moltlang"
DEFAULT_ENV_FILE = Path("ops/linear/runtime/symphony.env")


@dataclass(slots=True)
class StepResult:
    name: str
    command: list[str]
    returncode: int
    duration_seconds: float
    stdout_path: str
    stderr_path: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0


def _utc_now() -> datetime:
    return datetime.now(tz=UTC)


def _stamp(ts: datetime) -> str:
    return ts.strftime("%Y%m%dT%H%M%SZ")


def _env_with_external_defaults(env: dict[str, str], ext_root: Path) -> dict[str, str]:
    result = dict(env)
    result.setdefault("MOLT_EXT_ROOT", str(ext_root))
    result.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    result.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", result["CARGO_TARGET_DIR"])
    result.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    result.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    result.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    result.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    result.setdefault("TMPDIR", str(ext_root / "tmp"))
    result.setdefault("PYTHONPATH", "src")
    return result


def _load_env_file(path: Path) -> dict[str, str]:
    loaded: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        if key:
            loaded[key] = value
    return loaded


def _run_step(
    *,
    name: str,
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    cycle_dir: Path,
    shell: bool = False,
) -> StepResult:
    start = time.perf_counter()
    safe_name = "".join(ch if ch.isalnum() or ch in {"_", "-"} else "_" for ch in name)
    stdout_path = cycle_dir / f"{safe_name}.stdout.log"
    stderr_path = cycle_dir / f"{safe_name}.stderr.log"
    if shell:
        shell_cmd = " ".join(shlex.quote(part) for part in command)
        proc = subprocess.run(
            shell_cmd,
            cwd=cwd,
            env=env,
            shell=True,
            text=True,
            capture_output=True,
            check=False,
        )
    else:
        proc = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
    duration = time.perf_counter() - start
    stdout_path.write_text(proc.stdout or "", encoding="utf-8")
    stderr_path.write_text(proc.stderr or "", encoding="utf-8")
    return StepResult(
        name=name,
        command=command,
        returncode=int(proc.returncode),
        duration_seconds=round(duration, 3),
        stdout_path=str(stdout_path),
        stderr_path=str(stderr_path),
    )


def _load_next_tranche_commands(path: Path) -> list[str]:
    if not path.exists():
        return []
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    if not isinstance(payload, dict):
        return []
    actions = payload.get("actions")
    if not isinstance(actions, list):
        return []
    commands: list[str] = []
    for action in actions:
        if not isinstance(action, dict):
            continue
        action_commands = action.get("commands")
        if not isinstance(action_commands, list):
            continue
        for command in action_commands:
            text = str(command or "").strip()
            if text:
                commands.append(text)
    return commands


def _load_next_tranche_actions(path: Path) -> list[dict[str, object]]:
    if not path.exists():
        return []
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    actions = payload.get("actions") if isinstance(payload, dict) else None
    if not isinstance(actions, list):
        return []
    return [item for item in actions if isinstance(item, dict)]


def _parse_command(raw: str) -> list[str]:
    return [part for part in shlex.split(raw) if part]


def _step_payload(step: StepResult) -> dict[str, object]:
    return {
        "name": step.name,
        "command": list(step.command),
        "returncode": step.returncode,
        "duration_seconds": step.duration_seconds,
        "stdout_path": step.stdout_path,
        "stderr_path": step.stderr_path,
        "ok": step.ok,
    }


def _write_log(path: Path, content: str) -> str:
    path.write_text(content, encoding="utf-8")
    return str(path)


def _decision_payload(decision: HookDecision) -> dict[str, object]:
    return {
        "action": decision.action,
        "reason": decision.reason,
        "command": list(decision.command or []),
        "metadata": dict(decision.metadata or {}),
    }


def _blocked_step_result(
    *,
    name: str,
    command: list[str],
    cycle_dir: Path,
    reason: str,
) -> StepResult:
    safe_name = "".join(ch if ch.isalnum() or ch in {"_", "-"} else "_" for ch in name)
    stdout_path = cycle_dir / f"{safe_name}.stdout.log"
    stderr_path = cycle_dir / f"{safe_name}.stderr.log"
    _write_log(stdout_path, "")
    _write_log(stderr_path, f"blocked_by_hook: {reason}\n")
    return StepResult(
        name=name,
        command=command,
        returncode=90,
        duration_seconds=0.0,
        stdout_path=str(stdout_path),
        stderr_path=str(stderr_path),
    )


def _failure_codes_from_readiness(path: Path) -> list[str]:
    if not path.exists():
        return []
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []
    findings = payload.get("findings") if isinstance(payload, dict) else None
    if not isinstance(findings, list):
        return []
    codes: list[str] = []
    for row in findings:
        if not isinstance(row, dict):
            continue
        if str(row.get("severity") or "") not in {"warn", "fail"}:
            continue
        code = str(row.get("code") or "").strip()
        if code:
            codes.append(code)
    return codes


def _record_dead_letter(
    *,
    queue: DeadLetterQueue,
    cycle_name: str,
    cycle_dir: Path,
    step: StepResult,
    phase: str,
) -> dict[str, object]:
    row = {
        "kind": "recursive_loop_step_failure",
        "phase": phase,
        "cycle_name": cycle_name,
        "cycle_dir": str(cycle_dir),
        "name": step.name,
        "command": list(step.command),
        "fingerprint": dead_letter_fingerprint(
            kind="recursive_loop_step_failure",
            name=step.name,
            command=list(step.command),
        ),
        "returncode": step.returncode,
        "stdout_path": step.stdout_path,
        "stderr_path": step.stderr_path,
    }
    return queue.append(row)


def _record_taste_memory(
    *,
    store: TasteMemoryStore,
    cycle_name: str,
    cycle_dir: Path,
    status: str,
    failure_codes: list[str],
    executed_commands: list[StepResult],
    steps: list[StepResult],
) -> dict[str, object]:
    successful_actions = [
        " ".join(step.command) for step in executed_commands if step.ok and step.command
    ]
    tools_used = [step.name for step in steps]
    payload = {
        "kind": "recursive_loop_cycle",
        "cycle_name": cycle_name,
        "cycle_dir": str(cycle_dir),
        "cycle_status": status,
        "failure_codes": list(failure_codes),
        "successful_actions": successful_actions,
        "tools_used": tools_used,
    }
    return store.record(payload)


def _run_step_with_hooks(
    *,
    name: str,
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    cycle_dir: Path,
    hook_runner: LoopHookRunner,
    cycle_name: str,
    phase: str,
    shell: bool = False,
) -> tuple[StepResult, dict[str, object], dict[str, object]]:
    before_decision = hook_runner.run(
        event="before_step",
        payload={
            "cycle_name": cycle_name,
            "phase": phase,
            "name": name,
            "command": list(command),
            "shell": shell,
        },
        cwd=cwd,
        env=env,
    )
    effective_command = list(before_decision.command or command)
    if before_decision.action == "block":
        step = _blocked_step_result(
            name=name,
            command=effective_command,
            cycle_dir=cycle_dir,
            reason=before_decision.reason or "blocked",
        )
    else:
        if before_decision.action != "replace":
            effective_command = list(command)
        step = _run_step(
            name=name,
            command=effective_command,
            cwd=cwd,
            env=env,
            cycle_dir=cycle_dir,
            shell=shell,
        )
    after_decision = hook_runner.run(
        event="after_step",
        payload={
            "cycle_name": cycle_name,
            "phase": phase,
            "step": _step_payload(step),
        },
        cwd=cwd,
        env=env,
    )
    return step, _decision_payload(before_decision), _decision_payload(after_decision)


def _render_summary_markdown(
    *,
    started_at: str,
    finished_at: str,
    status: str,
    steps: list[StepResult],
    executed_commands: list[StepResult],
    cycle_dir: Path,
) -> str:
    lines = [
        "# Symphony Recursive Loop",
        "",
        f"- Status: `{status}`",
        f"- Started: `{started_at}`",
        f"- Finished: `{finished_at}`",
        f"- Cycle dir: `{cycle_dir}`",
        "",
        "## Steps",
    ]
    for step in steps:
        marker = "ok" if step.ok else "fail"
        lines.append(
            f"- `{step.name}`: `{marker}` (`rc={step.returncode}`,"
            f" `{step.duration_seconds:.3f}s`)"
        )
    if executed_commands:
        lines.extend(["", "## Executed Next-Tranche Commands"])
        for step in executed_commands:
            marker = "ok" if step.ok else "fail"
            lines.append(
                f"- `{step.name}`: `{marker}` (`rc={step.returncode}`,"
                f" `{step.duration_seconds:.3f}s`)"
            )
    lines.append("")
    return "\n".join(lines)


def _build_readiness_command(
    *,
    args: argparse.Namespace,
    readiness_json: Path,
    readiness_md: Path,
    next_tranche_json: Path,
    next_tranche_md: Path,
) -> list[str]:
    command = [
        "uv",
        "run",
        "--group",
        "dev",
        "--python",
        "3.12",
        "python3",
        "tools/symphony_readiness_audit.py",
        "--team",
        str(args.team),
        "--formal-suite",
        str(args.formal_suite),
        "--output-json",
        str(readiness_json),
        "--output-md",
        str(readiness_md),
        "--output-next-tranche-json",
        str(next_tranche_json),
        "--output-next-tranche-md",
        str(next_tranche_md),
    ]
    if args.strict_autonomy:
        command.extend(["--strict-autonomy", "--fail-on", str(args.fail_on)])
    return command


def _build_linear_hygiene_command(args: argparse.Namespace) -> list[str]:
    command = [
        "uv",
        "run",
        "--group",
        "dev",
        "--python",
        "3.12",
        "python3",
        "tools/linear_hygiene.py",
        "full-pass",
        "--team",
        str(args.team),
        "--formal-suite",
        str(args.formal_suite),
    ]
    if args.apply_linear:
        command.append("--apply")
    return command


def _build_harness_trend_command(
    *, args: argparse.Namespace, trend_json: Path
) -> list[str]:
    return [
        "uv",
        "run",
        "--python",
        "3.12",
        "python3",
        "tools/symphony_harness_trend.py",
        "--ext-root",
        str(args.ext_root),
        "--days",
        str(args.trend_days),
        "--json-out",
        str(trend_json),
    ]


def _build_perf_guard_command(
    *,
    args: argparse.Namespace,
    perf_json: Path,
    perf_verdict_json: Path,
) -> list[str]:
    command = [
        "uv",
        "run",
        "--group",
        "dev",
        "--python",
        "3.12",
        "python3",
        "tools/symphony_perf.py",
        str(args.workflow),
        "--iterations",
        str(args.perf_iterations),
        "--auto-compare-latest",
        "--output-json",
        str(perf_json),
        "--verdict-json",
        str(perf_verdict_json),
        "--reports-dir",
        str(args.perf_reports_dir),
    ]
    if args.fail_on_regression:
        command.append("--fail-on-regression")
    return command


def _run_cycle(
    *,
    args: argparse.Namespace,
    repo_root: Path,
    env: dict[str, str],
    cycle_index: int,
) -> int:
    start = _utc_now()
    stamp = _stamp(start)
    cycle_name = f"{stamp}-cycle{cycle_index:02d}"
    cycle_dir = Path(str(args.output_root)).expanduser().resolve() / cycle_name
    cycle_dir.mkdir(parents=True, exist_ok=True)

    readiness_json = cycle_dir / "readiness.latest.json"
    readiness_md = cycle_dir / "readiness.latest.md"
    next_tranche_json = cycle_dir / "readiness.next_tranche.json"
    next_tranche_md = cycle_dir / "readiness.next_tranche.md"
    trend_json = cycle_dir / "harness_trend.json"
    perf_json = cycle_dir / "symphony_perf.json"
    perf_verdict_json = cycle_dir / "symphony_perf_verdict.json"
    trace_path = cycle_dir / "trace.json"
    hook_runner = LoopHookRunner(_parse_command(str(args.hook_cmd or "")))
    dlq = DeadLetterQueue(Path(str(args.dlq_file)).expanduser().resolve())
    taste_memory = TasteMemoryStore(
        events_path=Path(str(args.taste_memory_file)).expanduser().resolve(),
        distillations_dir=Path(str(args.taste_distillations_dir))
        .expanduser()
        .resolve(),
    )
    tool_promotion = ToolPromotionStore(
        events_path=Path(str(args.tool_promotion_file)).expanduser().resolve(),
        distillations_dir=Path(str(args.tool_promotion_distillations_dir))
        .expanduser()
        .resolve(),
    )

    steps: list[StepResult] = []
    hook_trace: list[dict[str, object]] = []
    for name, command in (
        (
            "readiness_audit",
            _build_readiness_command(
                args=args,
                readiness_json=readiness_json,
                readiness_md=readiness_md,
                next_tranche_json=next_tranche_json,
                next_tranche_md=next_tranche_md,
            ),
        ),
        ("linear_hygiene", _build_linear_hygiene_command(args)),
        (
            "harness_trend",
            _build_harness_trend_command(args=args, trend_json=trend_json),
        ),
    ):
        step, before_hook, after_hook = _run_step_with_hooks(
            name=name,
            command=command,
            cwd=repo_root,
            env=env,
            cycle_dir=cycle_dir,
            hook_runner=hook_runner,
            cycle_name=cycle_name,
            phase="core",
        )
        steps.append(step)
        hook_trace.append({"step": name, "before": before_hook, "after": after_hook})
    if args.run_perf_guard:
        step, before_hook, after_hook = _run_step_with_hooks(
            name="perf_guard",
            command=_build_perf_guard_command(
                args=args,
                perf_json=perf_json,
                perf_verdict_json=perf_verdict_json,
            ),
            cwd=repo_root,
            env=env,
            cycle_dir=cycle_dir,
            hook_runner=hook_runner,
            cycle_name=cycle_name,
            phase="core",
        )
        steps.append(step)
        hook_trace.append(
            {"step": "perf_guard", "before": before_hook, "after": after_hook}
        )

    executed_commands: list[StepResult] = []
    tranche_actions = _load_next_tranche_actions(next_tranche_json)
    if args.execute_next_tranche:
        for action_index, action in enumerate(tranche_actions, start=1):
            action_id = str(action.get("id") or f"action_{action_index:02d}")
            commands = action.get("commands")
            if not isinstance(commands, list):
                continue
            for command_index, raw_command in enumerate(commands, start=1):
                command_text = str(raw_command or "").strip()
                if not command_text:
                    continue
                step, before_hook, after_hook = _run_step_with_hooks(
                    name=f"next_tranche_{action_index:02d}_{command_index:02d}",
                    command=[command_text],
                    cwd=repo_root,
                    env=env,
                    cycle_dir=cycle_dir,
                    shell=True,
                    hook_runner=hook_runner,
                    cycle_name=cycle_name,
                    phase=f"next_tranche:{action_id}",
                )
                executed_commands.append(step)
                hook_trace.append(
                    {
                        "step": step.name,
                        "action_id": action_id,
                        "before": before_hook,
                        "after": after_hook,
                    }
                )

    failures = [step.name for step in steps if not step.ok] + [
        step.name for step in executed_commands if not step.ok
    ]
    status = "pass" if not failures else "fail"
    finished = _utc_now()
    failure_codes = _failure_codes_from_readiness(readiness_json)

    dead_letters: list[dict[str, object]] = []
    for step in steps:
        if not step.ok:
            dead_letters.append(
                _record_dead_letter(
                    queue=dlq,
                    cycle_name=cycle_name,
                    cycle_dir=cycle_dir,
                    step=step,
                    phase="core",
                )
            )
    for step in executed_commands:
        if not step.ok:
            dead_letters.append(
                _record_dead_letter(
                    queue=dlq,
                    cycle_name=cycle_name,
                    cycle_dir=cycle_dir,
                    step=step,
                    phase="next_tranche",
                )
            )
    taste_row = _record_taste_memory(
        store=taste_memory,
        cycle_name=cycle_name,
        cycle_dir=cycle_dir,
        status=status,
        failure_codes=failure_codes,
        executed_commands=executed_commands,
        steps=steps,
    )
    distillation = taste_memory.distill_recent(
        limit=max(int(args.taste_memory_limit), 20)
    )
    tool_promotion_distillation = tool_promotion.distill_candidates(
        taste_rows=taste_memory.load(limit=max(int(args.taste_memory_limit), 20)),
        limit=max(int(args.taste_memory_limit), 20),
        min_success_count=max(1, int(args.tool_promotion_min_success_count)),
    )
    tool_promotion_row = tool_promotion.record(
        {
            "kind": "tool_promotion_distillation",
            "cycle_name": cycle_name,
            "cycle_status": status,
            "samples": tool_promotion_distillation["samples"],
            "candidate_count": tool_promotion_distillation["candidate_count"],
            "ready_candidate_count": tool_promotion_distillation[
                "ready_candidate_count"
            ],
            "manifest_count": (
                (tool_promotion_distillation.get("manifest_batch") or {}).get(
                    "manifest_count"
                )
                if isinstance(tool_promotion_distillation.get("manifest_batch"), dict)
                else 0
            ),
            "path": tool_promotion_distillation["path"],
        }
    )
    cycle_hook = hook_runner.run(
        event="after_cycle",
        payload={
            "cycle_name": cycle_name,
            "status": status,
            "failures": failures,
            "failure_codes": failure_codes,
            "summary_path": str(cycle_dir / "summary.json"),
            "tool_promotion_ready_candidate_count": tool_promotion_distillation[
                "ready_candidate_count"
            ],
        },
        cwd=repo_root,
        env=env,
    )

    summary = {
        "cycle_name": cycle_name,
        "started_at": start.isoformat().replace("+00:00", "Z"),
        "finished_at": finished.isoformat().replace("+00:00", "Z"),
        "status": status,
        "repo_root": str(repo_root),
        "failures": failures,
        "failure_codes": failure_codes,
        "steps": [asdict(step) for step in steps],
        "executed_next_tranche": [asdict(step) for step in executed_commands],
        "dead_letters": dead_letters,
        "hook_trace": hook_trace,
        "cycle_hook": _decision_payload(cycle_hook),
        "taste_memory": {
            "recorded": taste_row,
            "distillation": distillation,
        },
        "tool_promotion": {
            "recorded": tool_promotion_row,
            "distillation": tool_promotion_distillation,
        },
        "artifacts": {
            "cycle_dir": str(cycle_dir),
            "readiness_json": str(readiness_json),
            "readiness_md": str(readiness_md),
            "next_tranche_json": str(next_tranche_json),
            "next_tranche_md": str(next_tranche_md),
            "harness_trend_json": str(trend_json),
            "perf_json": str(perf_json) if args.run_perf_guard else None,
            "perf_verdict_json": (
                str(perf_verdict_json) if args.run_perf_guard else None
            ),
            "trace_json": str(trace_path),
            "tool_promotion_events_file": str(args.tool_promotion_file),
            "tool_promotion_distillations_dir": str(
                args.tool_promotion_distillations_dir
            ),
        },
    }
    (cycle_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (cycle_dir / "summary.md").write_text(
        _render_summary_markdown(
            started_at=summary["started_at"],
            finished_at=summary["finished_at"],
            status=status,
            steps=steps,
            executed_commands=executed_commands,
            cycle_dir=cycle_dir,
        ),
        encoding="utf-8",
    )
    trace_path.write_text(
        json.dumps(
            {
                "cycle_name": cycle_name,
                "failure_codes": failure_codes,
                "dead_letters": dead_letters,
                "hook_trace": hook_trace,
                "tool_promotion": {
                    "recorded": tool_promotion_row,
                    "distillation": tool_promotion_distillation,
                },
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        json.dumps(
            {
                "cycle": cycle_name,
                "status": status,
                "summary_json": str(cycle_dir / "summary.json"),
                "summary_md": str(cycle_dir / "summary.md"),
            }
        )
    )
    return 0 if status == "pass" else 1


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Run a deterministic Symphony recursive loop cycle (readiness, hygiene, "
            "trend, optional perf guard, optional next-tranche execution)."
        )
    )
    parser.add_argument(
        "--ext-root",
        default=os.environ.get("MOLT_EXT_ROOT", DEFAULT_EXT_ROOT),
        help="External artifact root.",
    )
    parser.add_argument(
        "--env-file",
        default=None,
        help=(
            "Optional env file (KEY=VALUE) loaded before command execution. "
            "Defaults to ops/linear/runtime/symphony.env when present."
        ),
    )
    parser.add_argument(
        "--team",
        default=DEFAULT_TEAM,
        help="Linear team reference for readiness/hygiene.",
    )
    parser.add_argument(
        "--workflow",
        default="WORKFLOW.md",
        help="Workflow file path for optional perf guard run.",
    )
    parser.add_argument(
        "--output-root",
        default=None,
        help="Cycle artifact root (defaults to the canonical Symphony recursive-loop log root).",
    )
    parser.add_argument(
        "--formal-suite",
        choices=("inventory", "all"),
        default="all",
        help="Formal suite mode for readiness and hygiene runs.",
    )
    parser.add_argument(
        "--strict-autonomy",
        action="store_true",
        default=True,
        help="Enable strict-autonomy readiness checks.",
    )
    parser.add_argument(
        "--no-strict-autonomy",
        dest="strict_autonomy",
        action="store_false",
        help="Disable strict-autonomy readiness checks.",
    )
    parser.add_argument(
        "--fail-on",
        choices=("warn", "fail"),
        default="warn",
        help="Readiness failure threshold when strict autonomy is enabled.",
    )
    parser.add_argument(
        "--apply-linear",
        action="store_true",
        help="Apply Linear hygiene updates (default is dry-run).",
    )
    parser.add_argument(
        "--execute-next-tranche",
        action="store_true",
        help="Execute commands emitted by readiness next_tranche.actions.",
    )
    parser.add_argument(
        "--run-perf-guard",
        action="store_true",
        help="Run tools/symphony_perf.py as part of each cycle.",
    )
    parser.add_argument(
        "--perf-iterations",
        type=int,
        default=2,
        help="Iteration count passed to symphony_perf when --run-perf-guard is set.",
    )
    parser.add_argument(
        "--fail-on-regression",
        action="store_true",
        help="Enable symphony_perf fail-on-regression behavior.",
    )
    parser.add_argument(
        "--trend-days",
        type=int,
        default=7,
        help="Window size for tools/symphony_harness_trend.py.",
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=1,
        help="How many recursive cycles to run.",
    )
    parser.add_argument(
        "--interval-seconds",
        type=int,
        default=120,
        help="Pause between cycles when --iterations > 1.",
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help=(
            "Quick mode: inventory formal suite, no perf guard, "
            "and no next-tranche execution."
        ),
    )
    parser.add_argument(
        "--hook-cmd",
        default=os.environ.get("MOLT_SYMPHONY_LOOP_HOOK_CMD", ""),
        help="Optional hook command invoked with typed JSON events on stdin.",
    )
    parser.add_argument(
        "--dlq-file",
        default=None,
        help="Dead-letter queue JSONL path (defaults to the canonical Symphony state root).",
    )
    parser.add_argument(
        "--taste-memory-file",
        default=None,
        help="Taste-memory JSONL path (defaults to the canonical Symphony state root).",
    )
    parser.add_argument(
        "--taste-distillations-dir",
        default=None,
        help="Taste-memory distillation output dir (defaults to the canonical Symphony state root).",
    )
    parser.add_argument(
        "--taste-memory-limit",
        type=int,
        default=200,
        help="Recent taste-memory event window used for deterministic distillation.",
    )
    parser.add_argument(
        "--tool-promotion-file",
        default=None,
        help="Tool-promotion events JSONL path (defaults to the canonical Symphony state root).",
    )
    parser.add_argument(
        "--tool-promotion-distillations-dir",
        default=None,
        help="Tool-promotion distillation output dir (defaults to the canonical Symphony state root).",
    )
    parser.add_argument(
        "--tool-promotion-min-success-count",
        type=int,
        default=3,
        help="Minimum recurring success count required before a tool-promotion candidate becomes ready.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo_root = Path.cwd()
    env = os.environ.copy()
    env_file = (
        Path(str(args.env_file)).expanduser() if args.env_file else DEFAULT_ENV_FILE
    )
    if env_file.exists():
        for key, value in _load_env_file(env_file).items():
            env.setdefault(key, value)

    ext_root = resolve_molt_ext_root({"MOLT_EXT_ROOT": str(args.ext_root)})
    if not ext_root.exists():
        raise RuntimeError(f"external root unavailable: {ext_root}")
    if args.output_root is None:
        args.output_root = str(symphony_recursive_loop_dir(env=env))
    args.perf_reports_dir = str(symphony_perf_reports_dir(env=env))
    if args.dlq_file is None:
        args.dlq_file = str(symphony_dlq_events_file(env=env))
    if args.taste_memory_file is None:
        args.taste_memory_file = str(symphony_taste_memory_events_file(env=env))
    if args.taste_distillations_dir is None:
        args.taste_distillations_dir = str(
            symphony_taste_memory_distillations_dir(env=env)
        )
    if args.tool_promotion_file is None:
        args.tool_promotion_file = str(symphony_tool_promotion_events_file(env=env))
    if args.tool_promotion_distillations_dir is None:
        args.tool_promotion_distillations_dir = str(
            symphony_tool_promotion_distillations_dir(env=env)
        )
    if args.quick:
        args.formal_suite = "inventory"
        args.run_perf_guard = False
        args.execute_next_tranche = False

    env = _env_with_external_defaults(env, ext_root=ext_root)
    Path(str(args.output_root)).expanduser().resolve().mkdir(
        parents=True, exist_ok=True
    )

    iterations = max(1, int(args.iterations))
    interval_seconds = max(0, int(args.interval_seconds))
    exit_code = 0
    for idx in range(1, iterations + 1):
        cycle_rc = _run_cycle(
            args=args,
            repo_root=repo_root,
            env=env,
            cycle_index=idx,
        )
        exit_code = max(exit_code, cycle_rc)
        if idx < iterations and interval_seconds > 0:
            time.sleep(interval_seconds)
    return int(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
