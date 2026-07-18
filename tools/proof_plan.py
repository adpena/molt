#!/usr/bin/env python3
"""Select and validate Molt proofs from the canonical proof-plan manifest.

This is policy-neutral execution code.  Path ownership, proof metadata, and
local gate commands live only in ``tools/proof_plan.toml``.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
from dataclasses import dataclass
from pathlib import Path
import subprocess
import sys
import time
import tomllib
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "tools" / "proof_plan.toml"
NULL_SHA = "0" * 40
REQUIRED_FAMILY_FIELDS = (
    "description",
    "inputs",
    "executor",
    "workflow",
    "job",
    "tiers",
    "required",
    "timeout_minutes",
    "memory_class",
    "cache_domain",
    "artifact_schema",
    "zero_work_policy",
    "dependencies",
    "resource_class",
    "metadata_mode",
    "targets",
    "backends",
    "profiles",
    "python_versions",
    "operating_systems",
    "architectures",
)


@dataclass(frozen=True, slots=True)
class ProofFamily:
    name: str
    data: dict[str, Any]

    @property
    def inputs(self) -> tuple[str, ...]:
        return tuple(self.data["inputs"])


@dataclass(frozen=True, slots=True)
class Selection:
    changed_paths: tuple[str, ...]
    selected: tuple[ProofFamily, ...]
    reasons: dict[str, tuple[str, ...]]
    fail_closed_reason: str | None = None


@dataclass(frozen=True, slots=True)
class ProofPlan:
    path: Path
    authority_inputs: tuple[str, ...]
    families: tuple[ProofFamily, ...]
    local_rules: tuple[dict[str, Any], ...]
    always: tuple[str, ...]

    @classmethod
    def load(cls, path: Path = DEFAULT_MANIFEST) -> "ProofPlan":
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        if data.get("schema") != "molt.proof-plan.v1":
            raise ValueError(f"{path}: expected schema molt.proof-plan.v1")
        families = tuple(
            ProofFamily(str(entry.get("name", "")), dict(entry))
            for entry in data.get("ci_family", [])
        )
        plan = cls(
            path=path,
            authority_inputs=tuple(data.get("authority_inputs", [])),
            families=families,
            local_rules=tuple(dict(entry) for entry in data.get("rule", [])),
            always=tuple(data.get("always", [])),
        )
        errors = plan.validate()
        if errors:
            raise ValueError("invalid proof plan:\n- " + "\n- ".join(errors))
        return plan

    def validate(self) -> list[str]:
        errors: list[str] = []
        names = [family.name for family in self.families]
        if not names:
            errors.append("at least one [[ci_family]] is required")
        if len(names) != len(set(names)):
            errors.append("ci_family names must be unique")
        for family in self.families:
            if not family.name:
                errors.append("ci_family.name must be non-empty")
            for field in REQUIRED_FAMILY_FIELDS:
                if field not in family.data:
                    errors.append(f"{family.name}: missing {field}")
            if family.data.get("zero_work_policy") not in {"fail", "advisory"}:
                errors.append(f"{family.name}: invalid zero_work_policy")
            if family.data.get("metadata_mode") != "descriptive":
                errors.append(
                    f"{family.name}: execution metadata must explicitly be descriptive"
                )
            if (
                family.data.get("required")
                and family.data.get("zero_work_policy") != "fail"
            ):
                errors.append(f"{family.name}: required proof must fail on zero work")
            workflow = ROOT / str(family.data.get("workflow", ""))
            if not workflow.is_file():
                errors.append(f"{family.name}: workflow does not exist: {workflow}")
            elif family.data.get("executor") == "github-job":
                job = str(family.data.get("job", ""))
                if f"  {job}:" not in workflow.read_text(encoding="utf-8"):
                    errors.append(f"{family.name}: workflow job {job!r} is missing")
        known = set(names)
        dependency_graph: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            dependencies = family.data.get("dependencies", [])
            if not isinstance(dependencies, list) or not all(
                isinstance(dependency, str) for dependency in dependencies
            ):
                errors.append(f"{family.name}: dependencies must be a list of names")
                continue
            dependency_graph[family.name] = tuple(dependencies)
            unknown = set(dependencies) - known
            if unknown:
                errors.append(
                    f"{family.name}: unknown dependencies {sorted(unknown)!r}"
                )
        visiting: list[str] = []
        visited: set[str] = set()

        def visit(name: str) -> None:
            if name in visited:
                return
            if name in visiting:
                start = visiting.index(name)
                cycle = (*visiting[start:], name)
                errors.append(f"dependency cycle: {' -> '.join(cycle)}")
                return
            visiting.append(name)
            for dependency in dependency_graph.get(name, ()):
                if dependency in dependency_graph:
                    visit(dependency)
            visiting.pop()
            visited.add(name)

        for name in names:
            visit(name)
        local_names: set[str] = set()
        for rule in self.local_rules:
            name = rule.get("name")
            if not isinstance(name, str) or not name:
                errors.append("local [[rule]] missing non-empty name")
                continue
            if name in local_names:
                errors.append(f"duplicate local rule {name!r}")
            local_names.add(name)
            for field in ("globs", "gates"):
                value = rule.get(field)
                if not isinstance(value, list) or not all(
                    isinstance(item, str) for item in value
                ):
                    errors.append(f"{name}: {field} must be a list of strings")
        return errors

    def all_selected(self, *, reason: str) -> Selection:
        return Selection(
            changed_paths=(),
            selected=self.families,
            reasons={family.name: (reason,) for family in self.families},
            fail_closed_reason=reason,
        )

    def select(self, paths: list[str] | tuple[str, ...]) -> Selection:
        normalized = tuple(_normalize_path(path) for path in paths if path.strip())
        authority_matches = tuple(
            path
            for path in normalized
            if any(_matches(path, pattern) for pattern in self.authority_inputs)
        )
        direct_reasons: dict[str, tuple[str, ...]] = {}
        for family in self.families:
            matched = tuple(
                path
                for path in normalized
                if any(_matches(path, pattern) for pattern in family.inputs)
            )
            family_reasons = tuple(dict.fromkeys((*authority_matches, *matched)))
            if family_reasons:
                direct_reasons[family.name] = family_reasons

        selected_names = set(direct_reasons)
        dependency_reasons: dict[str, list[str]] = {}
        pending = [
            family.name for family in self.families if family.name in selected_names
        ]
        family_by_name = {family.name: family for family in self.families}
        for name in pending:
            for dependency in family_by_name[name].data["dependencies"]:
                reason = f"dependency:{name}"
                if dependency not in selected_names:
                    dependency_reasons.setdefault(dependency, []).append(reason)
                    selected_names.add(dependency)
                    pending.append(dependency)
                elif dependency not in direct_reasons:
                    dependency_reasons.setdefault(dependency, [])
                    if reason not in dependency_reasons[dependency]:
                        dependency_reasons[dependency].append(reason)

        selected = tuple(
            family for family in self.families if family.name in selected_names
        )
        reasons = {
            family.name: tuple(
                (
                    *direct_reasons.get(family.name, ()),
                    *dependency_reasons.get(family.name, ()),
                )
            )
            for family in selected
        }
        return Selection(normalized, selected, reasons)


def _normalize_path(path: str) -> str:
    return path.replace("\\", "/").removeprefix("./")


def _matches(path: str, pattern: str) -> bool:
    normalized = _normalize_path(pattern)
    if normalized.endswith("/**"):
        prefix = normalized[:-3].rstrip("/")
        return path == prefix or path.startswith(prefix + "/")
    return fnmatch.fnmatchcase(path, normalized)


def _run_git(args: list[str]) -> str:
    return subprocess.check_output(
        ["git", *args], cwd=ROOT, text=True, stderr=subprocess.STDOUT
    )


def _diff_paths(base: str, head: str, *, three_dot: bool = False) -> list[str]:
    if not base or not head or base == NULL_SHA or head == NULL_SHA:
        raise RuntimeError("event does not provide two non-null commit identities")
    separator = "..." if three_dot else ".."
    output = _run_git(
        [
            "diff",
            "--name-status",
            "-z",
            "--diff-filter=ACDMRTUXB",
            f"{base}{separator}{head}",
        ]
    )
    tokens = output.split("\0")
    paths: list[str] = []
    index = 0
    while index < len(tokens):
        status = tokens[index]
        index += 1
        if not status:
            continue
        path_count = 2 if status[0] in {"R", "C"} else 1
        if index + path_count > len(tokens):
            raise RuntimeError(f"malformed git diff record for status {status!r}")
        for path in tokens[index : index + path_count]:
            if path and path not in paths:
                paths.append(path)
        index += path_count
    return paths


def _pull_request_paths(base_ref: str) -> list[str]:
    if not base_ref:
        raise RuntimeError("GITHUB_BASE_REF is not set")
    remote_ref = f"origin/{base_ref}"
    try:
        _run_git(["rev-parse", "--verify", remote_ref])
    except subprocess.CalledProcessError:
        _run_git(
            [
                "fetch",
                "--no-tags",
                "--prune",
                "origin",
                f"+{base_ref}:refs/remotes/{remote_ref}",
            ]
        )
    return _diff_paths(remote_ref, "HEAD", three_dot=True)


def _event_payload(path: str) -> dict[str, Any]:
    if not path:
        return {}
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("GitHub event payload is not an object")
    return value


def selection_for_event(
    plan: ProofPlan,
    *,
    event_name: str,
    base_ref: str,
    event_path: str,
    before: str,
    after: str,
) -> Selection:
    try:
        payload = _event_payload(event_path)
        if event_name in {"pull_request", "pull_request_target"}:
            return plan.select(_pull_request_paths(base_ref))
        if event_name == "push":
            if bool(payload.get("forced")):
                raise RuntimeError("forced push has no trustworthy incremental base")
            base = before or str(payload.get("before", ""))
            head = after or str(payload.get("after", "")) or "HEAD"
            return plan.select(_diff_paths(base, head))
        if event_name in {"schedule", "workflow_dispatch", "workflow_call"}:
            return plan.all_selected(reason=f"{event_name}: full proof plan")
        raise RuntimeError(f"unsupported or missing event {event_name!r}")
    except Exception as exc:
        return plan.all_selected(reason=f"fail-closed event selection: {exc}")


def family_outputs(plan: ProofPlan, selection: Selection) -> dict[str, str]:
    selected = {family.name for family in selection.selected}
    outputs = {
        family.name: "true" if family.name in selected else "false"
        for family in plan.families
    }
    matrix = [
        {
            "name": family.name,
            **{
                key: family.data[key]
                for key in (
                    "executor",
                    "workflow",
                    "job",
                    "required",
                    "timeout_minutes",
                    "memory_class",
                    "cache_domain",
                    "resource_class",
                    "metadata_mode",
                    "targets",
                    "backends",
                    "profiles",
                    "python_versions",
                    "operating_systems",
                    "architectures",
                )
            },
            "selected_by": list(selection.reasons.get(family.name, ())),
        }
        for family in selection.selected
    ]
    outputs["matrix"] = json.dumps({"include": matrix}, separators=(",", ":"))
    outputs["selected"] = json.dumps(sorted(selected), separators=(",", ":"))
    outputs["changed_paths"] = json.dumps(
        selection.changed_paths, separators=(",", ":")
    )
    return outputs


def verify_required_results(
    plan: ProofPlan,
    selected_names: list[str] | tuple[str, ...],
    results: dict[str, tuple[str, int]],
) -> list[str]:
    """Return fail-closed result errors for selected required proof families.

    The execution count is the number of completed proof partitions reported by
    the executor.  A selected required family cannot be absent, skipped, or
    successful with zero completed partitions.
    """
    known = {family.name for family in plan.families}
    unknown_selected = set(selected_names) - known
    errors = [
        f"unknown selected proof family {name!r}" for name in sorted(unknown_selected)
    ]
    selected = set(selected_names) & known
    for family in plan.families:
        if family.name not in selected or not family.data["required"]:
            continue
        result = results.get(family.name)
        if result is None:
            errors.append(f"{family.name}: required result is missing")
            continue
        status, executed = result
        if status != "success":
            errors.append(f"{family.name}: required executor status is {status!r}")
        if family.data["zero_work_policy"] == "fail" and executed <= 0:
            errors.append(f"{family.name}: zero proof partitions executed")
    return errors


def _parse_result(value: str) -> tuple[str, tuple[str, int]]:
    try:
        name, payload = value.split("=", 1)
        status, raw_count = payload.rsplit(":", 1)
        count = int(raw_count)
    except (ValueError, TypeError) as exc:
        raise ValueError(
            f"invalid --result {value!r}; expected FAMILY=STATUS:COUNT"
        ) from exc
    if not name or not status or count < 0:
        raise ValueError(f"invalid --result {value!r}")
    return name, (status, count)


def replay_recent_commits(plan: ProofPlan, count: int) -> dict[str, Any]:
    """Replay first-parent commit diffs and quantify avoided family launches."""
    if count <= 0:
        raise ValueError("replay commit count must be positive")
    started = time.perf_counter()
    commits = _run_git(
        ["rev-list", "--first-parent", f"--max-count={count}", "HEAD"]
    ).splitlines()
    launches = {family.name: 0 for family in plan.families}
    path_total = 0
    for commit in commits:
        paths = _diff_paths(f"{commit}^", commit)
        path_total += len(paths)
        for family in plan.select(paths).selected:
            launches[family.name] += 1
    total = len(commits)
    return {
        "schema": "molt.proof-plan-replay.v1",
        "commits": total,
        "changed_paths_examined": path_total,
        "wall_time_ms": round((time.perf_counter() - started) * 1000, 2),
        "families": {
            name: {
                "selected": selected,
                "avoided": total - selected,
                "avoidable_percent": round(
                    100.0 * (total - selected) / total if total else 0.0, 1
                ),
            }
            for name, selected in launches.items()
        },
    }


def write_github_outputs(path: Path, outputs: dict[str, str]) -> None:
    with path.open("a", encoding="utf-8") as handle:
        for name, value in outputs.items():
            print(f"{name}={value}", file=handle)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument("--path", action="append", default=[])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--replay-commits", type=int)
    parser.add_argument(
        "--verify-selected",
        help="JSON array of selected family names whose required results must pass.",
    )
    parser.add_argument(
        "--result",
        action="append",
        default=[],
        help="Executor result as FAMILY=STATUS:EXECUTED_COUNT.",
    )
    parser.add_argument("--event-name", default=os.environ.get("GITHUB_EVENT_NAME", ""))
    parser.add_argument("--base-ref", default=os.environ.get("GITHUB_BASE_REF", ""))
    parser.add_argument("--event-path", default=os.environ.get("GITHUB_EVENT_PATH", ""))
    parser.add_argument("--before", default="")
    parser.add_argument("--after", default=os.environ.get("GITHUB_SHA", ""))
    args = parser.parse_args(argv)

    try:
        plan = ProofPlan.load(args.manifest)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"proof-plan: {exc}", file=sys.stderr)
        return 2
    if args.check:
        print(
            f"proof-plan: OK families={len(plan.families)} "
            f"local_rules={len(plan.local_rules)} authority={plan.path}"
        )
        return 0
    if args.replay_commits is not None:
        try:
            replay = replay_recent_commits(plan, args.replay_commits)
        except (RuntimeError, subprocess.CalledProcessError, ValueError) as exc:
            print(f"proof-plan replay: {exc}", file=sys.stderr)
            return 2
        print(json.dumps(replay, indent=2, sort_keys=True))
        return 0
    if args.verify_selected is not None:
        try:
            selected = json.loads(args.verify_selected)
            if not isinstance(selected, list) or not all(
                isinstance(name, str) for name in selected
            ):
                raise ValueError("--verify-selected must be a JSON string array")
            parsed_results = dict(_parse_result(value) for value in args.result)
            errors = verify_required_results(plan, selected, parsed_results)
        except (ValueError, json.JSONDecodeError) as exc:
            print(f"proof-plan verdict: {exc}", file=sys.stderr)
            return 2
        if errors:
            for error in errors:
                print(f"proof-plan verdict: {error}", file=sys.stderr)
            return 1
        print(
            f"proof-plan verdict: OK selected={len(selected)} "
            f"required={sum(1 for family in plan.families if family.name in selected and family.data['required'])}"
        )
        return 0
    selection = (
        plan.select(args.path)
        if args.path
        else selection_for_event(
            plan,
            event_name=args.event_name,
            base_ref=args.base_ref,
            event_path=args.event_path,
            before=args.before,
            after=args.after,
        )
    )
    outputs = family_outputs(plan, selection)
    if args.json:
        print(json.dumps(outputs, indent=2, sort_keys=True))
    else:
        for family in plan.families:
            print(f"{family.name}={outputs[family.name]}")
        print(f"matrix={outputs['matrix']}")
        if selection.fail_closed_reason:
            print(f"proof-plan: {selection.fail_closed_reason}", file=sys.stderr)
    if args.github_output is not None:
        write_github_outputs(args.github_output, outputs)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
