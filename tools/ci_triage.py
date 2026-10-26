#!/usr/bin/env python3
"""Summarize the failures of one GitHub Actions run in a few lines per job.

For every failed job the report names the failed proof commands, each unique
failing test with its first assertion or panic line, and the Cargo errors. A
failure is marked NEW unless the newest completed run of the same workflow on
`main` fails the same test in a job of the same name, and each failure names
the open ledger rows (`docs/agent/V1_HANDOFF_FINDINGS.md`) that mention it.

Completed job logs are immutable, so the tool caches them by job ID under the
system temporary directory and downloads each log at most once.

Usage:
    python3 tools/ci_triage.py RUN_ID
    python3 tools/ci_triage.py --branch verify/train-32
    python3 tools/ci_triage.py RUN_ID --compare none --json
    python3 tools/ci_triage.py RUN_ID --verify

Exit status: 0 completed successful CI (and verified coverage with --verify),
1 failed CI/proofs, 2 unavailable verification, 3 unfinished CI. NEW/main
classification does not turn a known failure into success.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
LEDGER = ROOT / "docs" / "agent" / "V1_HANDOFF_FINDINGS.md"
CACHE = Path(tempfile.gettempdir()) / "molt-ci-triage"
_COMMANDS = CommandExecutor.for_file(__file__)

_ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
_TIMESTAMP = re.compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z ?")
_PROOF_FAILED = re.compile(r"^proof-plan: failed (\S+): (.+)$")
_PYTEST_FAILED = re.compile(
    r"^(?:\[gw\d+\] \[\s*\d+%\] )?FAILED (\S+::\S+?)(?: - (.+))?\s*$"
)
_RUST_SECTION = re.compile(r"^failures:$|^\d+ failing test\(s\):$")
_RUST_NAME = re.compile(r"^ {4}([A-Za-z_][\w:]*)$")
_PANIC = re.compile(r"^thread '([^']*)' (?:\(\d+\) )?panicked at (\S+?):?$")
_CARGO_ERROR = re.compile(r"^error(\[[\w-]+\])?: (.+)$")
_PYTEST_HEADER = re.compile(
    r"^(?:_{3,} (\S+) _{3,}|=+ Molt immediate pytest failure(?: \[gw\d+\])?: (\S+) \(\w+\) =+)$"
)
_CARGO_NOISE = (
    "test failed, to rerun",
    "aborting due to",
    "could not compile",
    "target failed",
)
_TEST_LINE_LIMIT = 240


def clean_log(text: str) -> list[str]:
    """Log lines without ANSI escapes, runner timestamps or trailing space."""
    lines = []
    for raw in text.splitlines():
        line = _TIMESTAMP.sub("", _ANSI.sub("", raw)).rstrip()
        lines.append(line)
    return lines


@dataclass
class JobReport:
    name: str
    proof_commands: list[str] = field(default_factory=list)
    tests: dict[str, str] = field(default_factory=dict)
    errors: list[str] = field(default_factory=list)

    def failure_keys(self) -> set[str]:
        return set(self.tests) | set(self.errors) | set(self.proof_commands)


def _first_detail(lines: list[str], start: int) -> str:
    """The first assertion or message line after a panic or test header."""
    for line in lines[start : start + 12]:
        text = line.strip()
        if not text or text.startswith(("note:", "stack backtrace", "----")):
            continue
        return text[:_TEST_LINE_LIMIT]
    return ""


def parse_job_log(name: str, text: str) -> JobReport:
    """Extract the failures one job log reports. Pure: no I/O."""
    lines = clean_log(text)
    report = JobReport(name)
    panics: dict[str, str] = {}
    assertions: dict[str, str] = {}
    pytest_section: str | None = None
    in_rust_list = False
    for index, line in enumerate(lines):
        if match := _PYTEST_HEADER.match(line.strip()):
            pytest_section = match.group(1) or match.group(2)
            continue
        if (
            pytest_section
            and line.startswith("E ")
            and pytest_section not in assertions
        ):
            assertions[pytest_section] = line[1:].strip()[:_TEST_LINE_LIMIT]
        if match := _PROOF_FAILED.match(line):
            entry = f"{match.group(1)}: {match.group(2)}"
            if entry not in report.proof_commands:
                report.proof_commands.append(entry)
            continue
        if match := _PYTEST_FAILED.match(line):
            test, message = match.group(1), (match.group(2) or "").strip()
            if message or test not in report.tests:
                report.tests[test] = message[:_TEST_LINE_LIMIT]
            continue
        stripped = line.strip()
        if _RUST_SECTION.match(stripped):
            in_rust_list = True
            continue
        if in_rust_list:
            if match := _RUST_NAME.match(line.rstrip()):
                report.tests.setdefault(match.group(1), "")
                continue
            if stripped:
                in_rust_list = False
        if match := _PANIC.match(stripped):
            # libtest names each test thread after its test.
            detail = _first_detail(lines, index + 1)
            panics.setdefault(match.group(1), f"{match.group(2)} {detail}".strip())
            continue
        if match := _CARGO_ERROR.match(stripped):
            message = f"error{match.group(1) or ''}: {match.group(2)}"
            if not any(noise in message for noise in _CARGO_NOISE):
                if message not in report.errors:
                    report.errors.append(message[:_TEST_LINE_LIMIT])
    for test, detail in panics.items():
        if test in report.tests and not report.tests[test]:
            report.tests[test] = detail[:_TEST_LINE_LIMIT]
    # A pytest section header names the test with or without its path.
    for test in report.tests:
        if not report.tests[test]:
            short = test.rsplit("::", 1)[-1]
            report.tests[test] = assertions.get(test) or assertions.get(short, "")
    return report


def open_ledger_rows(ledger_text: str) -> list[tuple[str, str]]:
    """(ID, row text) for every open finding row; a "Fixed" section closes rows."""
    rows: list[tuple[str, str]] = []
    open_section = True
    for line in ledger_text.splitlines():
        if line.startswith("## "):
            open_section = not line[3:].startswith("Fixed")
            continue
        if open_section and (match := re.match(r"^\| ((?:HF|V1)-F?\d+) \|", line)):
            rows.append((match.group(1), line))
    return rows


def ledger_owners(failure: str, rows: list[tuple[str, str]]) -> list[str]:
    """Open rows that mention the failure's test function or proof command."""
    needle = failure.split(": ", 1)[0]
    needle = needle.rsplit("::", 1)[-1].split("[", 1)[0]
    if len(needle) < 8:
        return []
    return [row_id for row_id, text in rows if needle in text]


@dataclass
class Classified:
    job: str
    kind: str
    key: str
    detail: str
    new: bool | None
    owners: list[str]


def classify(
    reports: list[JobReport],
    baseline: list[JobReport] | None,
    ledger_rows: list[tuple[str, str]],
) -> list[Classified]:
    """Mark each failure NEW unless the baseline job of that name has it too."""
    known = {report.name: report.failure_keys() for report in baseline or []}
    out: list[Classified] = []
    for report in reports:
        seen = known.get(report.name, set())
        items = (
            [("proof", entry, "") for entry in report.proof_commands]
            + [("test", test, detail) for test, detail in report.tests.items()]
            + [("error", error, "") for error in report.errors]
        )
        for kind, key, detail in items:
            new = None if baseline is None else key not in seen
            out.append(
                Classified(
                    report.name, kind, key, detail, new, ledger_owners(key, ledger_rows)
                )
            )
    return out


def render(run: dict[str, Any], items: list[Classified], pending: list[str]) -> str:
    header = [
        f"run {run['databaseId']}",
        run.get("headBranch") or "",
        str(run.get("headSha") or "")[:9],
        run.get("status") or "",
        run.get("conclusion") or "",
    ]
    lines = [" ".join(part for part in header if part)]
    if pending:
        lines.append(f"  still running: {', '.join(pending)}")
    current = None
    for item in items:
        if item.job != current:
            current = item.job
            lines.append(f"[{current}]")
        mark = "" if item.new is None else ("NEW " if item.new else "main ")
        owners = f" ({', '.join(item.owners)})" if item.owners else ""
        detail = f" -- {item.detail}" if item.detail else ""
        lines.append(f"  {mark}{item.kind}: {item.key}{owners}{detail}")
    if not items and not pending:
        lines.append("  no failures")
    return "\n".join(lines)


def _gh_json(args: list[str]) -> Any:
    output = _COMMANDS.check_output(
        ["gh", *args], cwd=ROOT, text=True, encoding="utf-8", timeout=120
    )
    return json.loads(output)


def _job_log(job_id: int) -> str:
    CACHE.mkdir(parents=True, exist_ok=True)
    cached = CACHE / f"{job_id}.log"
    if cached.is_file():
        return cached.read_text(encoding="utf-8", errors="replace")
    text = _COMMANDS.check_output(
        [
            "gh",
            "api",
            f"repos/{{owner}}/{{repo}}/actions/jobs/{job_id}/logs",
            "--allow-escape-sequences",
        ],
        text=True,
        cwd=ROOT,
        encoding="utf-8",
        errors="replace",
        timeout=300,
    )
    assert isinstance(text, str)
    cached.write_text(text, encoding="utf-8")
    return text


def _run(run_id: int) -> dict[str, Any]:
    return _gh_json(
        [
            "run",
            "view",
            str(run_id),
            "--json",
            "databaseId,headBranch,headSha,status,conclusion,workflowName,jobs,event,attempt,url",
        ]
    )


def _failed_reports(run: dict[str, Any]) -> tuple[list[JobReport], list[str]]:
    reports, pending = [], []
    for job in run["jobs"]:
        if job.get("status") != "completed":
            pending.append(job["name"])
        elif job.get("conclusion") == "failure":
            reports.append(parse_job_log(job["name"], _job_log(job["databaseId"])))
    return reports, pending


def _latest_run(
    branch: str, workflow: str, completed: bool, *, exclude_run_id: int | None = None
) -> int | None:
    args = ["run", "list", "--branch", branch, "--workflow", workflow, "--limit", "10"]
    runs = _gh_json([*args, "--json", "databaseId,status"])
    for run in runs:
        if run["databaseId"] != exclude_run_id and (
            not completed or run["status"] == "completed"
        ):
            return int(run["databaseId"])
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("run_id", nargs="?", type=int)
    parser.add_argument("--branch", help="triage the newest run on this branch")
    parser.add_argument("--workflow", default="ci.yml")
    parser.add_argument(
        "--compare",
        default="main",
        help="baseline: 'main' (newest completed main run), a run ID, or 'none'",
    )
    parser.add_argument("--json", action="store_true")
    parser.add_argument(
        "--verify",
        action="store_true",
        help="retrieve and verify selected proof coverage for this exact committed "
        "source (requires a completed successful CI run with selection artifact)",
    )
    args = parser.parse_args(argv)
    run_id = args.run_id
    if run_id is None:
        if not args.branch:
            parser.error("give a RUN_ID or --branch")
        run_id = _latest_run(args.branch, args.workflow, completed=False)
        if run_id is None:
            parser.error(f"no {args.workflow} run on {args.branch}")
    run = _run(run_id)
    reports, pending = _failed_reports(run)
    baseline = None
    baseline_run = None
    if args.compare != "none":
        base_id = (
            _latest_run("main", args.workflow, completed=True, exclude_run_id=run_id)
            if args.compare == "main"
            else int(args.compare)
        )
        if base_id is not None and base_id != run_id:
            baseline_run = _run(base_id)
            baseline, _ = _failed_reports(baseline_run)
    ledger = LEDGER.read_text(encoding="utf-8") if LEDGER.is_file() else ""
    items = classify(reports, baseline, open_ledger_rows(ledger))
    verification = None
    if args.verify:
        from tools.ci_proof_evidence import verify_run

        verification = verify_run(
            run, repo_root=ROOT, commands=_COMMANDS, read_run=_run
        )
    if args.json:
        payload = {
            "run": {
                key: run.get(key)
                for key in (
                    "databaseId",
                    "headBranch",
                    "headSha",
                    "status",
                    "conclusion",
                    "event",
                    "attempt",
                    "url",
                )
            },
            "pending": pending,
            "baseline": None
            if baseline_run is None
            else {
                key: baseline_run.get(key)
                for key in ("databaseId", "headSha", "conclusion", "url")
            },
            "failures": [item.__dict__ for item in items],
            "verification": None if verification is None else asdict(verification),
        }
        print(json.dumps(payload, indent=2))
    else:
        print(render(run, items, pending))
        if baseline_run is not None:
            print(
                f"  baseline: run {baseline_run['databaseId']} "
                f"{baseline_run.get('headSha')}"
            )
        if verification is not None:
            print(f"  proof coverage: {verification.status} ({verification.scope})")
            if verification.evidence_dir:
                print(f"  evidence: {verification.evidence_dir}")
            for error in verification.errors:
                print(f"  {error}")
    if run.get("status") != "completed" or pending:
        return 3
    if run.get("conclusion") != "success" or items:
        return 1
    if verification is not None and verification.status != "verified":
        return 2 if verification.status == "unavailable" else 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
