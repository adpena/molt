from __future__ import annotations

from dataclasses import dataclass
import hashlib
import itertools
import json
from typing import Any, Callable, Mapping, Sequence

from .reduce import oracle_matches


@dataclass(frozen=True)
class ProbeSupervisorAttemptConfig:
    attempt: int
    jobs: int
    timeout_sec: int


def _sha256(value: Any) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def build_probe_supervisor_attempts(
    *, jobs: int, retry_jobs: int, run_timeout: int
) -> tuple[ProbeSupervisorAttemptConfig, ProbeSupervisorAttemptConfig]:
    timeout = max(1, run_timeout)
    return (
        ProbeSupervisorAttemptConfig(attempt=1, jobs=max(1, jobs), timeout_sec=timeout),
        ProbeSupervisorAttemptConfig(
            attempt=2, jobs=max(1, retry_jobs), timeout_sec=timeout
        ),
    )


def should_retry_probe_statuses(statuses: Mapping[str, str]) -> bool:
    retry_states = {"build_timeout", "run_timeout", "build_failed"}
    return any(status in retry_states for status in statuses.values())


def render_probe_supervisor_markdown(
    *,
    started_at: str,
    finished_at: str,
    required_probes: Sequence[str],
    attempts: list[dict[str, Any]],
    final_ok: bool,
    final_message: str,
) -> str:
    lines = [
        "# IR Probe Supervisor Checkpoint",
        "",
        f"- Started: `{started_at}`",
        f"- Finished: `{finished_at}`",
        f"- Required probes: `{len(required_probes)}`",
        f"- Final status: `{'ok' if final_ok else 'failed'}`",
        f"- Note: {final_message}",
        "",
        "## Attempts",
        "",
        "| Attempt | Jobs | Timeout | Diff RC | Gate RC | Timed Out | Run ID |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for item in attempts:
        lines.append(
            "| {attempt} | {jobs} | {timeout}s | {diff_rc} | {gate_rc} | {timed_out} | {run_id} |".format(
                attempt=item.get("attempt"),
                jobs=item.get("jobs"),
                timeout=item.get("timeout_sec"),
                diff_rc=item.get("diff_rc"),
                gate_rc=item.get("gate_rc"),
                timed_out="yes" if item.get("timed_out") else "no",
                run_id=item.get("run_id") or "",
            )
        )
    lines.append("")
    return "\n".join(lines)


def _diff_signature(oracle: dict[str, Any], evaluation: dict[str, Any]) -> str:
    return _sha256({"oracle": oracle, "evaluation": evaluation})


def bisect_first_bad_pass(
    passes: Sequence[str],
    *,
    oracle: dict[str, Any],
    evaluator: Callable[[tuple[str, ...]], dict[str, Any]],
) -> dict[str, Any]:
    decisions: list[dict[str, Any]] = []
    first_bad_index: int | None = None
    final_eval: dict[str, Any] | None = None
    for index in range(len(passes)):
        prefix = tuple(passes[: index + 1])
        evaluation = evaluator(prefix)
        matched = oracle_matches(oracle, evaluation)
        decisions.append({"candidate": list(prefix), "matched": matched})
        if matched:
            first_bad_index = index
            final_eval = evaluation
            break
    if first_bad_index is None:
        return {
            "mode": "first_bad_pass",
            "status": "clean",
            "decisions": decisions,
            "failure_signature": _diff_signature(oracle, {"matched": False}),
        }
    first_bad_pass = passes[first_bad_index]
    return {
        "mode": "first_bad_pass",
        "status": "ok",
        "first_bad_index": first_bad_index,
        "first_bad_pass": first_bad_pass,
        "pass_window": {
            "start": first_bad_index,
            "end": first_bad_index,
            "passes": [first_bad_pass],
        },
        "decisions": decisions,
        "failure_signature": _diff_signature(oracle, final_eval or {"matched": True}),
    }


def bisect_backend_profile_ic(
    *,
    baseline: Mapping[str, Any],
    failing: Mapping[str, Any],
    oracle: dict[str, Any],
    evaluator: Callable[[dict[str, Any]], dict[str, Any]],
) -> dict[str, Any]:
    differing = [key for key in failing if baseline.get(key) != failing.get(key)]
    decisions: list[dict[str, Any]] = []
    minimal_bad_dimensions: list[str] = []
    minimal_bad_config: dict[str, Any] | None = None
    final_eval: dict[str, Any] | None = None

    for size in range(1, len(differing) + 1):
        for subset in itertools.combinations(differing, size):
            candidate = dict(baseline)
            for key in subset:
                candidate[key] = failing[key]
            evaluation = evaluator(candidate)
            matched = oracle_matches(oracle, evaluation)
            decisions.append(
                {
                    "dimensions": list(subset),
                    "candidate": candidate,
                    "matched": matched,
                }
            )
            if matched:
                minimal_bad_dimensions = list(subset)
                minimal_bad_config = candidate
                final_eval = evaluation
                break
        if minimal_bad_config is not None:
            break

    return {
        "mode": "config_toggle_bisect",
        "status": "ok" if minimal_bad_config is not None else "clean",
        "baseline": dict(baseline),
        "failing": dict(failing),
        "minimal_bad_dimensions": minimal_bad_dimensions,
        "minimal_bad_config": minimal_bad_config,
        "decisions": decisions,
        "failure_signature": _diff_signature(
            oracle,
            final_eval or {"matched": False, "candidate": dict(baseline)},
        ),
    }
