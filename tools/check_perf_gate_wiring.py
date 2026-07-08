#!/usr/bin/env python3
"""Fail-closed audit: the canonical perf gate MUST be wired to fire on main.

The meta-bug this kills (the TIER-0 / sharpest finding of the verification-machinery
audit): .github/workflows/perf-gate.yml is the ONE release-blocking CPython-floor
scoreboard, but its triggers were workflow_dispatch + a weekly cron only -- it never
ran on a PR or a merge to main. So every perf-green was vacuous and the
perf-authority drift-gate certified nothing. ci.yml running tests/tools/
test_perf_authority.py is a *unit test of the authority module* -- a PROXY for
"the gate ran", which is the master meta-bug class: PROXY-MEASUREMENT SUBSTITUTION
(a verifier measures a cheap proxy correlated with the real invariant on the happy
path and decorrelated exactly where the bug lives).

This checker replaces the proxy with a check on the real thing: the canonical gate
must (a) invoke the real scoreboard and (b) actually fire on main. It is wired into
ci_gate tier-1 (via tests/tools/test_check_perf_gate_wiring.py) so an un-wiring
cannot silently regress.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PERF_GATE = REPO / ".github" / "workflows" / "perf-gate.yml"
SCOREBOARD_CMD = "perf_scoreboard.py"
REQUIRED_EVENTS = {"push", "pull_request", "pull_request_target"}


def _load_yaml(path: Path):
    text = path.read_text(encoding="utf-8")
    try:
        import yaml  # pyyaml is a dev dependency

        return yaml.safe_load(text)
    except ImportError:
        return _load_workflow_yaml_subset(text)


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def _parse_scalar(text: str) -> object:
    value = text.strip()
    if value in {"", "null", "Null", "NULL", "~"}:
        return {}
    lower = value.lower()
    if lower == "true":
        return True
    if lower == "false":
        return False
    if value.startswith("[") and value.endswith("]"):
        inner = value[1:-1].strip()
        if not inner:
            return []
        return [_parse_scalar(part) for part in inner.split(",")]
    if (
        (value.startswith('"') and value.endswith('"'))
        or (value.startswith("'") and value.endswith("'"))
    ):
        return value[1:-1]
    return value


def _split_key_value(stripped_line: str) -> tuple[str, object]:
    key, _, value = stripped_line.partition(":")
    return key.strip(), _parse_scalar(value)


def _find_top_level_block(lines: list[str], key: str) -> tuple[int, int] | None:
    start: int | None = None
    for idx, line in enumerate(lines):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if _indent(line) == 0 and line.strip() == f"{key}:":
            start = idx + 1
            break
    if start is None:
        return None
    end = len(lines)
    for idx in range(start, len(lines)):
        line = lines[idx]
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if _indent(line) == 0 and line.rstrip().endswith(":"):
            end = idx
            break
    return start, end


def _load_workflow_yaml_subset(text: str) -> dict:
    """Parse the workflow topology this audit owns when PyYAML is unavailable."""
    lines = text.splitlines()
    return {
        "on": _parse_triggers(lines),
        "jobs": _parse_jobs(lines),
    }


def _parse_triggers(lines: list[str]) -> dict:
    block = _find_top_level_block(lines, "on")
    if block is None:
        return {}
    start, end = block
    triggers: dict[str, object] = {}
    idx = start
    while idx < end:
        line = lines[idx]
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or _indent(line) != 2:
            idx += 1
            continue
        trigger, value = _split_key_value(stripped)
        trigger_doc: object = value if value != {} else {}
        idx += 1
        if trigger == "push":
            push: dict[str, object] = {}
            while idx < end and _indent(lines[idx]) > 2:
                child = lines[idx].strip()
                if child and not child.startswith("#") and _indent(lines[idx]) == 4:
                    key, scalar = _split_key_value(child)
                    push[key] = scalar
                idx += 1
            trigger_doc = push
        else:
            while idx < end and _indent(lines[idx]) > 2:
                idx += 1
        triggers[trigger] = trigger_doc
    return triggers


def _parse_jobs(lines: list[str]) -> dict:
    block = _find_top_level_block(lines, "jobs")
    if block is None:
        return {}
    start, end = block
    jobs: dict[str, dict] = {}
    idx = start
    while idx < end:
        line = lines[idx]
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or _indent(line) != 2:
            idx += 1
            continue
        job_name, _, value = stripped.partition(":")
        if value.strip():
            idx += 1
            continue
        job: dict[str, object] = {}
        idx += 1
        while idx < end and _indent(lines[idx]) > 2:
            child = lines[idx]
            child_stripped = child.strip()
            if not child_stripped or child_stripped.startswith("#"):
                idx += 1
                continue
            if _indent(child) == 4 and child_stripped.startswith("steps:"):
                steps, idx = _parse_steps(lines, idx + 1, end)
                job["steps"] = steps
                continue
            if _indent(child) == 4:
                key, scalar = _split_key_value(child_stripped)
                if key in {"if", "continue-on-error"}:
                    job[key] = scalar
            idx += 1
        jobs[job_name] = job
    return jobs


def _parse_steps(lines: list[str], start: int, end: int) -> tuple[list[dict], int]:
    steps: list[dict] = []
    idx = start
    while idx < end:
        line = lines[idx]
        if not line.strip() or line.lstrip().startswith("#"):
            idx += 1
            continue
        indent = _indent(line)
        if indent <= 4:
            break
        stripped = line.strip()
        if indent == 6 and stripped.startswith("- "):
            step: dict[str, object] = {}
            first = stripped[2:].strip()
            if first:
                key, scalar = _split_key_value(first)
                step[key] = scalar
            idx += 1
            while idx < end:
                child = lines[idx]
                child_stripped = child.strip()
                child_indent = _indent(child)
                if child_indent <= 4 or (
                    child_indent == 6 and child_stripped.startswith("- ")
                ):
                    break
                if child_stripped and not child_stripped.startswith("#") and child_indent == 8:
                    key, scalar = _split_key_value(child_stripped)
                    if key == "run" and scalar in ({}, "|", ">"):
                        run_lines: list[str] = []
                        idx += 1
                        while idx < end and _indent(lines[idx]) > 8:
                            run_lines.append(lines[idx].strip())
                            idx += 1
                        step["run"] = "\n".join(run_lines)
                        continue
                    step[key] = scalar
                idx += 1
            steps.append(step)
            continue
        idx += 1
    return steps, idx


def _triggers(doc: object) -> dict:
    # YAML 1.1 coerces the bare key `on` to the boolean True; handle both spellings
    # so the audit is not itself fooled by the parse (a meta-meta-bug).
    if isinstance(doc, dict):
        if "on" in doc:
            return doc["on"] or {}
        if True in doc:
            return doc[True] or {}
    return {}


def _expr(value: object) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return ""
    text = str(value).strip()
    if text.startswith("${{") and text.endswith("}}"):
        text = text[3:-2].strip()
    return text


def _nonblocking_continue_on_error(value: object) -> bool:
    if value is None or value is False:
        return False
    normalized = _expr(value).lower()
    return normalized not in {"", "false", "0"}


def _if_condition_problem(value: object) -> str | None:
    if value is None:
        return None
    normalized = _expr(value).strip().lower()
    compact = re.sub(r"\s+", "", normalized)
    if compact in {"", "true", "always()"}:
        return None
    if compact in {"false", "0"} or compact.startswith("false&&"):
        return "has a trivially false if condition"

    event_matches = re.findall(
        r"github\.event_name\s*==\s*['\"]([^'\"]+)['\"]", normalized
    )
    if event_matches and REQUIRED_EVENTS.isdisjoint(event_matches):
        return (
            "is gated away from push/pull_request events, so the scoreboard "
            "does not block the required main/PR path"
        )
    return None


def _scoreboard_steps(doc: object) -> list[tuple[str, int, dict, dict]]:
    if not isinstance(doc, dict):
        return []
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        return []

    hits: list[tuple[str, int, dict, dict]] = []
    for job_name, job in jobs.items():
        if not isinstance(job, dict):
            continue
        steps = job.get("steps")
        if not isinstance(steps, list):
            continue
        for idx, step in enumerate(steps):
            if not isinstance(step, dict):
                continue
            run = step.get("run")
            if isinstance(run, str) and SCOREBOARD_CMD in run:
                hits.append((str(job_name), idx, job, step))
    return hits


def check() -> list[str]:
    """Return a list of wiring problems; empty list == correctly wired."""
    if not PERF_GATE.exists():
        return [f"{PERF_GATE} is missing -- the canonical perf gate does not exist"]
    text = PERF_GATE.read_text(encoding="utf-8")
    try:
        doc = _load_yaml(PERF_GATE)
    except Exception as exc:  # noqa: BLE001 - any parse failure is a wiring failure
        return [f"perf-gate.yml does not parse as YAML: {exc}"]

    problems: list[str] = []
    triggers = _triggers(doc)

    # (1) It must invoke the REAL scoreboard in an executable workflow step,
    # not merely mention it in prose or comments.
    scoreboard_steps = _scoreboard_steps(doc)
    if SCOREBOARD_CMD not in text:
        problems.append(
            f"perf-gate.yml never invokes {SCOREBOARD_CMD} -- it is not the canonical gate"
        )
    elif not scoreboard_steps:
        problems.append(
            f"perf-gate.yml mentions {SCOREBOARD_CMD} but no executable run step invokes it"
        )

    # (2) It must actually FIRE on main: a push to main (release gate) and/or a PR.
    push = triggers.get("push") or {}
    push_branches = push.get("branches") if isinstance(push, dict) else None
    fires_on_main_push = bool(push_branches) and "main" in push_branches
    fires_on_pr = "pull_request" in triggers
    if not (fires_on_main_push or fires_on_pr):
        problems.append(
            "perf-gate.yml does not fire on a pull_request or a push to main -- its "
            f"only triggers are {sorted(map(str, triggers))!r}. The canonical perf gate "
            "certifies NOTHING on any merge; every perf-green is vacuous. Add "
            "`push: {branches: [main]}` (and/or pull_request) to its `on:` block."
        )

    # (3) The scoreboard invocation itself must be blocking. A workflow that
    # invokes the scoreboard under continue-on-error or a dead `if` is still a
    # proxy: it observes the gate without letting the gate block a merge.
    for job_name, step_idx, job, step in scoreboard_steps:
        job_label = f"job {job_name!r}"
        step_name = step.get("name") or f"step #{step_idx + 1}"
        step_label = f"{job_label} scoreboard step {step_name!r}"

        if _nonblocking_continue_on_error(job.get("continue-on-error")):
            problems.append(
                f"{job_label} has continue-on-error set; the canonical perf gate is non-blocking"
            )
        if _nonblocking_continue_on_error(step.get("continue-on-error")):
            problems.append(
                f"{step_label} has continue-on-error set; the canonical perf gate is non-blocking"
            )

        job_if_problem = _if_condition_problem(job.get("if"))
        if job_if_problem:
            problems.append(f"{job_label} {job_if_problem}")
        step_if_problem = _if_condition_problem(step.get("if"))
        if step_if_problem:
            problems.append(f"{step_label} {step_if_problem}")

    return problems


def main() -> int:
    problems = check()
    if problems:
        print("perf-gate-wiring: FAIL -- the canonical perf gate is not wired to main:")
        for p in problems:
            print(f"  - {p}")
        print(
            "  (a gate that never fires certifies nothing -- proxy-measurement substitution.)"
        )
        return 1
    print(
        "perf-gate-wiring: OK -- canonical perf gate fires on main and runs the scoreboard."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
