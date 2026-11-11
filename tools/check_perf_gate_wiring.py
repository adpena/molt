#!/usr/bin/env python3
"""Fail-closed audit for the controlled candidate-cost workflow.

`.github/workflows/perf-gate.yml` keeps one job, `candidate-runtime-costs`:
explicit-dispatch evidence with no comparative or release claim. A prior push
trigger combined with cancel-in-progress consumed hours of runner time while
repeated main updates cancelled every result. This audit keeps one honest
contract: only an explicit dispatch allocates the job, an active run is never
cancelled, and the job executes its one fixed proof family blocking. The
instruction signal on main runs on the farm (tools/perf_instructions.py).

The proof plan runs it as a landing gate and as ``repository.perf-gate-wiring``.
"""

from __future__ import annotations

import re
import sys
from collections.abc import Mapping
from pathlib import Path


REPO = Path(__file__).resolve().parent.parent
PERF_GATE = REPO / ".github" / "workflows" / "perf-gate.yml"
CANDIDATE_JOB = "candidate-runtime-costs"
CANDIDATE_RUN = "--run-family runtime_candidate_costs --receipt"
ADMITTED_EVENTS = {"workflow_dispatch"}


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
    if (value.startswith('"') and value.endswith('"')) or (
        value.startswith("'") and value.endswith("'")
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
        elif trigger == "workflow_dispatch":
            inputs: dict[str, dict] = {}
            current: dict | None = None
            while idx < end and _indent(lines[idx]) > 2:
                child = lines[idx]
                if child.strip() and _indent(child) == 6:
                    name, _ = _split_key_value(child.strip())
                    current = inputs.setdefault(name, {})
                elif child.strip() and _indent(child) == 8 and current is not None:
                    key, scalar = _split_key_value(child.strip())
                    current[key] = scalar
                idx += 1
            trigger_doc = {"inputs": inputs} if inputs else {}
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
            if _indent(child) == 4 and child_stripped == "strategy:":
                matrix: dict[str, object] = {}
                idx += 1
                while idx < end and (not lines[idx].strip() or _indent(lines[idx]) > 4):
                    nested = lines[idx]
                    if nested.strip() and _indent(nested) == 8:
                        key, scalar = _split_key_value(nested.strip())
                        matrix[key] = scalar
                    idx += 1
                job["strategy"] = {"matrix": matrix}
                continue
            if _indent(child) == 4:
                key, scalar = _split_key_value(child_stripped)
                if key in {"if", "continue-on-error", "needs"}:
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
                if (
                    child_stripped
                    and not child_stripped.startswith("#")
                    and child_indent == 8
                ):
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


def _triggers(doc: object) -> dict[str, object]:
    # YAML 1.1 coerces the bare key `on` to the boolean True; handle both spellings
    # so the audit is not itself fooled by the parse (a meta-meta-bug).
    if isinstance(doc, Mapping):
        value = doc.get("on", doc.get(True))
        if isinstance(value, Mapping):
            return {str(key): item for key, item in value.items()}
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


def candidate_problems(doc: object) -> list[str]:
    """The one candidate job runs blocking on every explicit dispatch."""
    jobs = doc.get("jobs") if isinstance(doc, Mapping) else None
    if not isinstance(jobs, Mapping):
        return ["perf-gate.yml declares no jobs"]
    problems: list[str] = []
    if set(jobs) != {CANDIDATE_JOB}:
        problems.append(
            f"perf-gate.yml runs only {CANDIDATE_JOB!r}; found {sorted(jobs)!r}"
        )
    candidate = jobs.get(CANDIDATE_JOB)
    if not isinstance(candidate, Mapping):
        return problems
    if candidate.get("if") is not None:
        problems.append(
            "candidate costs run on every explicit dispatch, never under a condition"
        )
    if _nonblocking_continue_on_error(candidate.get("continue-on-error")):
        problems.append(
            "candidate cost failures must remain blocking for that invocation"
        )
    captures = [
        step
        for step in candidate.get("steps", [])
        if isinstance(step, Mapping) and CANDIDATE_RUN in str(step.get("run", ""))
    ]
    if len(captures) != 1:
        problems.append("candidate mode must execute its one fixed proof family")
    elif (
        _nonblocking_continue_on_error(captures[0].get("continue-on-error"))
        or captures[0].get("if") is not None
        or "||" in str(captures[0].get("run", ""))
    ):
        problems.append(
            "candidate proof execution must propagate failure without conditional skipping"
        )
    return problems


def check(path: Path | None = None) -> list[str]:
    """Return a list of wiring problems; empty list == correctly wired."""
    path = PERF_GATE if path is None else path
    if not path.exists():
        return [f"{path} is missing -- the candidate cost workflow does not exist"]
    text = path.read_text(encoding="utf-8")
    try:
        doc = _load_yaml(path)
    except Exception as exc:  # noqa: BLE001 - any parse failure is a wiring failure
        return [f"perf-gate.yml does not parse as YAML: {exc}"]
    problems = candidate_problems(doc)
    # Push/PR/schedule events allocate noisy shared runners and cancel before
    # producing evidence; only an explicit dispatch admits a measurement.
    triggers = set(map(str, _triggers(doc)))
    if triggers != ADMITTED_EVENTS:
        problems.append(
            "perf-gate.yml admits only workflow_dispatch; observed triggers are "
            f"{sorted(triggers)!r}"
        )
    # Pending work may debounce before allocation, but an active measurement
    # must finish and publish its evidence.
    if not re.search(r"(?m)^\s*cancel-in-progress:\s*false\s*$", text):
        problems.append(
            "perf-gate.yml must set cancel-in-progress: false so active "
            "measurements finish and publish evidence"
        )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("perf-gate-wiring: FAIL -- controlled candidate measurement drifted:")
        for p in problems:
            print(f"  - {p}")
        print("  (controlled measurement must run to completion and fail honestly.)")
        return 1
    print(
        "perf-gate-wiring: OK -- explicit dispatch runs the blocking candidate "
        "family without active cancellation."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
