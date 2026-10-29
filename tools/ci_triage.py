#!/usr/bin/env python3
"""Summarize the failures of one GitHub Actions run in a few lines per job.

For every failed job the report names the failed proof commands, each unique
failing test with its first assertion or panic line, and the Cargo errors. A
failure is marked NEW unless the newest completed run of the same workflow on
`main` fails the same test in a job of the same name, and each failure names
its owning open ledger row (`docs/agent/V1_HANDOFF_FINDINGS.md`). A test whose
expectation names a finding (a `@finding` test, a `finding=` program or a
`finding` manifest entry; `tools/finding_coverage.py`) is owned by that
finding exactly. Any other failure names the open rows whose text mentions it.

Completed job logs are immutable, so the tool caches them by job ID under the
system temporary directory and downloads each log at most once.

`--junit FILE` reads a local or farm pytest JUnit report instead of a CI run
and groups its failures by cause: the most specific error line with paths,
temporary names and numbers removed. Each cause names its owning open rows,
the rows that name its test files, and how many of those files no proof-plan
command runs (`tools/check_test_ownership.py`).

Usage (`uv run --python 3.12 python` as the interpreter):
    tools/ci_triage.py RUN_ID
    tools/ci_triage.py --branch verify/train-32
    tools/ci_triage.py RUN_ID --compare none --json
    tools/ci_triage.py RUN_ID --verify
    tools/ci_triage.py --junit junit.xml [--json]

Exit status: 0 completed successful CI (and verified coverage with --verify),
1 failed CI/proofs, 2 unavailable verification, 3 unfinished CI. NEW/main
classification does not turn a known failure into success. With --junit: 0 no
failures, 1 failures.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from collections.abc import Mapping
from typing import Any
from xml.etree import ElementTree

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402
from tools.finding_status import STATE_OPEN, project  # noqa: E402

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
# A cause groups on its whole normalized line, so two refusals that differ only
# in the operation they name stay apart; reports print a shorter prefix.
_CAUSE_KEY_LIMIT = 600
_CAUSE_LINE_LIMIT = 320


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


_JUNIT_TIMEOUT = re.compile(r"^Failed: Timeout\b")
_JUNIT_CAUSE = re.compile(
    r"\w*(?:Error|Exception)\b|\b(?:error|panicked|refused|rejected)\b"
)
# pytest's own wrappers around a child's failure; the cause is inside them.
_JUNIT_WRAPPERS = ("AssertionError:", "Failed:")
_VOLATILE = (
    (re.compile(r"(?:[A-Za-z]:)?[\\/][^\s'\"`]+"), "<path>"),
    (re.compile(r"\btmp\w+"), "<tmp>"),
    (re.compile(r"\b0x[0-9a-fA-F]+\b|\b[0-9a-f]{12,}\b"), "<hex>"),
    (re.compile(r"\d+(?:\.\d+)?"), "N"),
)


def _junit_test_id(classname: str, name: str) -> str:
    """`tests/a/test_b.py::Class::name` from pytest's dotted classname."""
    parts = classname.split(".")
    stems = [index for index, part in enumerate(parts) if part.startswith("test_")]
    if not stems:
        return "::".join(part for part in (classname, name) if part)
    end = stems[-1] + 1
    return "::".join(["/".join(parts[:end]) + ".py", *parts[end:], name])


def _junit_detail(message: str, text: str) -> str:
    """The most specific cause line of one pytest failure.

    pytest prefixes the failure message with `E` and then prints its rewritten
    `assert` expression. The cause is the last message line that names an error
    outside pytest's own wrappers, else the last message line (a child's stderr
    tail), else the first `E` line.
    """
    first = message.strip().splitlines()[0] if message.strip() else ""
    if _JUNIT_TIMEOUT.match(first):
        return first
    lines = [line[1:].strip() for line in text.splitlines() if line.startswith("E ")]
    body = []
    for line in lines:
        if line.startswith("assert "):
            break
        if line:
            body.append(line)
    named = [
        line
        for line in body
        if _JUNIT_CAUSE.search(line) and not line.startswith(_JUNIT_WRAPPERS)
    ]
    if named:
        return named[-1]
    return body[-1] if body else (lines[0] if lines else first)


def parse_junit(name: str, text: str) -> tuple[JobReport, int]:
    """The failing tests of one pytest JUnit report and its test count. Pure."""
    report = JobReport(name)
    total = 0
    for case in ElementTree.fromstring(text).iter("testcase"):
        total += 1
        for kind in ("failure", "error"):
            element = case.find(kind)
            if element is not None:
                test = _junit_test_id(case.get("classname", ""), case.get("name", ""))
                detail = _junit_detail(element.get("message", ""), element.text or "")
                report.tests.setdefault(test, detail)
    return report, total


def cause_key(detail: str) -> str:
    """A failure detail without paths, temporary names or numbers."""
    for pattern, token in _VOLATILE:
        detail = pattern.sub(token, detail)
    return detail[:_CAUSE_KEY_LIMIT]


@dataclass
class Cause:
    cause: str
    tests: list[str]
    owners: list[str]
    files: list[str]
    files_without_proof_cell: int


def group_causes(
    items: list[Classified],
    rows: list[tuple[str, str]],
    unowned_files: set[str],
) -> list[Cause]:
    """Failures grouped by cause, largest first, with test and file owners."""
    groups: dict[str, list[Classified]] = {}
    for item in items:
        groups.setdefault(cause_key(item.detail), []).append(item)
    causes = []
    for cause, members in groups.items():
        files = sorted({item.key.split("::", 1)[0] for item in members})
        owners = {owner for item in members for owner in item.owners}
        owners.update(owner for path in files for owner in ledger_owners(path, rows))
        causes.append(
            Cause(
                cause,
                sorted(item.key for item in members),
                sorted(owners),
                files,
                sum(path in unowned_files for path in files),
            )
        )
    return sorted(causes, key=lambda cause: (-len(cause.tests), cause.cause))


def render_causes(source: str, total: int, causes: list[Cause]) -> str:
    failing = sum(len(cause.tests) for cause in causes)
    lines = [f"{source}: {failing} failing of {total} tests, {len(causes)} causes"]
    for cause in causes:
        owners = f" ({', '.join(cause.owners)})" if cause.owners else ""
        text = (cause.cause or "(no detail)")[:_CAUSE_LINE_LIMIT]
        lines.append(f"  {len(cause.tests)} {text}{owners}")
        shown = ", ".join(cause.files[:3])
        more = f" +{len(cause.files) - 3}" if len(cause.files) > 3 else ""
        bare = (
            f"; {cause.files_without_proof_cell} without a proof cell"
            if cause.files_without_proof_cell
            else ""
        )
        lines.append(f"    files: {shown}{more}{bare}")
        lines.append(f"    e.g. {cause.tests[0]}")
    return "\n".join(lines)


def open_ledger_rows(ledger_text: str) -> list[tuple[str, str]]:
    """(key, row text) for every open finding row, by the ledger's one reader."""
    lines = ledger_text.splitlines()
    return [
        (row.key, lines[row.line - 1])
        for row in project(ledger_text).rows
        if row.state == STATE_OPEN
    ]


def expectation_owners() -> dict[str, str]:
    """The finding that each expectation-carrying test names, keyed by test id."""
    from tools.finding_coverage import citations

    return {citation.test: citation.finding for citation in citations()}


def ledger_owners(
    failure: str,
    rows: list[tuple[str, str]],
    owners: Mapping[str, str] | None = None,
) -> list[str]:
    """The finding a failure's expectation names, else open rows mentioning it."""
    if owners and (owner := owners.get(failure.split(" - ", 1)[0])):
        return [owner]
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
    owners: Mapping[str, str] | None = None,
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
                    report.name,
                    kind,
                    key,
                    detail,
                    new,
                    ledger_owners(key, ledger_rows, owners),
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


def _triage_junit(path: Path, *, as_json: bool) -> int:
    from tools.check_test_ownership import unowned

    report, total = parse_junit(path.name, path.read_text(encoding="utf-8"))
    ledger = LEDGER.read_text(encoding="utf-8") if LEDGER.is_file() else ""
    rows = open_ledger_rows(ledger)
    items = classify([report], None, rows, expectation_owners())
    causes = group_causes(items, rows, unowned(ROOT))
    if as_json:
        payload = {
            "source": str(path),
            "total": total,
            "causes": [asdict(cause) for cause in causes],
        }
        print(json.dumps(payload, indent=2))
    else:
        print(render_causes(str(path), total, causes))
    return 1 if items else 0


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
    parser.add_argument(
        "--junit",
        type=Path,
        help="group the failures of this pytest JUnit report by cause instead",
    )
    args = parser.parse_args(argv)
    if args.junit is not None:
        if args.run_id is not None or args.branch or args.verify:
            parser.error("--junit takes no RUN_ID, --branch or --verify")
        return _triage_junit(args.junit, as_json=args.json)
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
    items = classify(reports, baseline, open_ledger_rows(ledger), expectation_owners())
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
