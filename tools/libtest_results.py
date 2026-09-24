"""Bounded interpretation of one libtest pretty-text stdout stream.

This is accounting, not authentication: a test can print the same bytes as
libtest. Observable ambiguity fails closed. In particular, standalone results
are bound only with an explicit, unique --test-threads=1 invocation. Stderr is
never joined to stdout; independently captured streams have no common order.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import TextIO

BINARY_RECEIPT_SCHEMA = "molt.cargo-test-binary.v2"
ACCOUNTING_SCHEMA = "molt.libtest-accounting.v1"
MAX_LINE_CHARS = 65_536
MAX_TESTS = 100_000
MAX_IDENTITY_CHARS = 4 * 1024 * 1024
MAX_ISSUES = 16
_START = re.compile(r"running ([0-9]{1,6}) tests?")
_HEADER = re.compile(r"test (.+?) \.\.\.(?: (.*))?")
_RESULT = re.compile(r"(ok|FAILED|ignored)(?:, (.+))?")
_SUMMARY = re.compile(
    r"test result: (ok|FAILED)\. ([0-9]{1,6}) passed; "
    r"([0-9]{1,6}) failed; ([0-9]{1,6}) ignored; "
    r"([0-9]{1,6}) measured; ([0-9]{1,6}) filtered out; "
    r"finished in [0-9]+(?:\.[0-9]+)?s"
)
_STATUS = {"ok": "pass", "FAILED": "fail", "ignored": "ignored"}


def serial_invocation(argv: tuple[str, ...]) -> bool:
    values: list[str] = []
    index = 1
    while index < len(argv):
        arg = argv[index]
        if arg == "--":
            break
        if arg == "--test-threads":
            values.append(argv[index + 1] if index + 1 < len(argv) else "")
            index += 2
        elif arg.startswith("--test-threads="):
            values.append(arg.partition("=")[2])
            index += 1
        elif arg in {"--skip", "--color", "--format", "--logfile", "--shuffle-seed"}:
            index += 2
        else:
            index += 1
    return values == ["1"]


@dataclass(frozen=True)
class LibtestReport:
    serial: bool
    declared: int | None
    observations: tuple[tuple[str, str], ...]
    pending: tuple[str, ...]
    summary: dict[str, object] | None
    issues: tuple[str, ...]

    @property
    def complete(self) -> bool:
        return (
            not self.issues
            and self.declared is not None
            and self.summary is not None
            and not self.pending
            and len(self.observations) == self.declared
        )

    def rows(self) -> list[dict[str, str]]:
        # Keep contradictory observations in the receipt but never promote them
        # into semantic/known-red evidence. Interrupted, unambiguous rows remain
        # available to the bounded abnormal-exit attribution lane.
        if self.issues:
            return []
        return [
            dict(identity=identity, status=status)
            for identity, status in self.observations
        ]

    def payload(self) -> dict[str, object]:
        return {
            "schema": ACCOUNTING_SCHEMA,
            "protocol": "libtest-pretty-text",
            "stream": "stdout",
            "serial_invocation": self.serial,
            "declared_tests": self.declared,
            "observations": [dict(identity=i, status=s) for i, s in self.observations],
            "pending_tests": list(self.pending),
            "summary": self.summary,
            "issues": list(self.issues),
            "complete": self.complete,
        }


def parse_libtest(stream: TextIO, argv: tuple[str, ...]) -> LibtestReport:
    """Consume bounded lines; retain only bounded identities and diagnostics."""
    serial = serial_invocation(argv)
    declared: int | None = None
    summary: dict[str, object] | None = None
    rows: list[tuple[str, str]] = []
    seen: set[str] = set()
    pending: str | None = None
    candidate: str | None = None
    issues: list[str] = []
    identity_chars = 0

    def issue(message: str) -> None:
        if message not in issues and len(issues) < MAX_ISSUES:
            issues.append(message)

    def finish_pending() -> None:
        nonlocal pending, candidate
        if pending is not None and candidate is not None:
            rows.append((pending, candidate))
            pending = candidate = None

    while raw := stream.readline(MAX_LINE_CHARS + 1):
        if len(raw) > MAX_LINE_CHARS:
            issue("line-limit-exceeded")
            while not raw.endswith("\n"):
                raw = stream.readline(MAX_LINE_CHARS + 1)
                if not raw:
                    break
            continue
        line = raw.rstrip("\r\n")
        start = _START.fullmatch(line)
        terminal = _SUMMARY.fullmatch(line)
        header = _HEADER.fullmatch(line)
        result = _RESULT.fullmatch(line)
        if start:
            if declared is not None or summary is not None:
                issue("multiple-run-banners")
            else:
                declared = int(start[1])
                if declared > MAX_TESTS:
                    issue("test-limit-exceeded")
            continue
        if declared is None:
            # Pre-run test-like output is not libtest evidence.
            continue
        if summary is not None:
            if terminal or header or result:
                issue("protocol-output-after-summary")
            continue
        if terminal:
            finish_pending()
            summary = dict(
                outcome=terminal[1],
                passed=int(terminal[2]),
                failed=int(terminal[3]),
                ignored=int(terminal[4]),
                measured=int(terminal[5]),
                filtered_out=int(terminal[6]),
            )
            continue
        if line.startswith("test result:"):
            issue("unsupported-summary")
            continue
        if header:
            finish_pending()
            if pending is not None:
                issue("overlapping-test-headers")
                continue
            identity = header[1].removesuffix(" - should panic")
            if not identity or identity in seen:
                issue("duplicate-or-empty-test-identity")
                continue
            if (
                len(seen) >= MAX_TESTS
                or identity_chars + len(identity) > MAX_IDENTITY_CHARS
            ):
                issue("test-limit-exceeded")
                continue
            identity_chars += len(identity)
            seen.add(identity)
            pending = identity
            suffix = header[2] or ""
            inline = _RESULT.fullmatch(suffix)
            if inline and (inline[2] is None or inline[1] == "ignored"):
                candidate = _STATUS[inline[1]]
            elif not serial:
                issue("split-output-without-proven-serial-invocation")
            continue
        if result and (result[2] is None or result[1] == "ignored"):
            if not serial or pending is None or candidate is not None:
                issue("ambiguous-standalone-result")
            else:
                candidate = _STATUS[result[1]]
        # All other stdout remains raw evidence, not parser state.
    finish_pending()
    if summary is not None:
        counts = {
            status: sum(found == status for _, found in rows)
            for status in _STATUS.values()
        }
        if (
            summary["passed"] != counts["pass"]
            or summary["failed"] != counts["fail"]
            or summary["ignored"] != counts["ignored"]
            or summary["measured"] != 0
            or len(rows) != declared
            or pending is not None
        ):
            issue("summary-count-disagreement")
        if (summary["outcome"] == "ok") != (summary["failed"] == 0):
            issue("summary-outcome-disagreement")
    return LibtestReport(
        serial,
        declared,
        tuple(rows),
        (() if pending is None else (pending,)),
        summary,
        tuple(issues),
    )


def accounting_problem(receipt: dict) -> str | None:
    """Validate the mandatory v2 accounting envelope, not text a second time."""
    if receipt.get("schema") != BINARY_RECEIPT_SCHEMA:
        return "unsupported binary receipt schema; replay required"
    accounting = receipt.get("result_accounting")
    if (
        not isinstance(accounting, dict)
        or accounting.get("schema") != ACCOUNTING_SCHEMA
    ):
        return "missing libtest result accounting"
    complete = accounting.get("complete")
    issues = accounting.get("issues")
    results = receipt.get("test_results")
    observed = accounting.get("observed_results")
    declared = accounting.get("declared_results")
    if (
        type(complete) is not bool
        or not isinstance(issues, list)
        or not all(isinstance(issue, str) for issue in issues)
        or not isinstance(results, list)
        or type(observed) is not int
        or observed != len(results)
        or (declared is not None and (type(declared) is not int or declared < 0))
        or (complete and (declared is None or declared != observed))
    ):
        return "malformed libtest result accounting"
    if issues:
        return "ambiguous libtest result accounting: " + "; ".join(issues)
    identities: set[str] = set()
    for row in results:
        if (
            not isinstance(row, dict)
            or not isinstance(row.get("identity"), str)
            or not row["identity"].strip()
            or not isinstance(row.get("status"), str)
            or row.get("status") not in {"pass", "fail", "ignored"}
        ):
            return "malformed libtest result row"
        if row["identity"] in identities:
            return "duplicate libtest result identity: " + row["identity"]
        identities.add(row["identity"])
    if not complete:
        return "binary lacks complete libtest result accounting"
    if receipt.get("status") == "success" and receipt.get("failure_identities"):
        return "successful binary contains confirmed failure identities"
    if receipt.get("status") == "success" and any(
        not isinstance(row, dict) or row.get("status") not in {"pass", "ignored"}
        for row in results
    ):
        return "successful binary contains a non-success result"
    return None
