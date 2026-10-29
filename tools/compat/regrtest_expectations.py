"""Classify a regrtest run against the expectations that name open findings.

`tools/suite_honesty/differential_expectations.json` holds, in its `regrtest`
block, the CPython test cases Molt is known to fail; each `fail` dimension
(`<backend>@<version>`) names the open finding that owns the failure
(`tools/check_suite_honesty.py` lints the block). This module compares one
run's per-test outcomes (`tools/compat/regrtest_results.py`) with those
entries, with strict semantics in both directions:

* a failure with an entry is expected, owned by its finding, only when the
  run reports exactly the entry's `failure` reason; any other failure of an
  owned test is a new defect;
* a failure without one is a new defect and fails the run;
* a pass with an entry is an unexpected pass and fails the run until the entry
  and its ledger row close together;
* an entry the run cannot confirm (its test skipped, not run, or keyed by a
  case the run did not report) fails the run, as an unconfirmed debt does in
  the suite-honesty guard;
* a timeout or worker failure is never expected.

A module-level key (`test.<module>`) owns the outcome of the whole regrtest
test; a case key (`test.<module>::<Class>::<method>`) needs per-case results.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field

from tests.findings import finding_problem
from tools.compat.item_outcome import ItemOutcome

CASE_SEPARATOR = "::"


def expectation_key(test_name: str) -> str:
    """The expectation key of one regrtest test name (`test_x` or `test_x.sub`)."""
    return test_name if test_name.startswith("test.") else f"test.{test_name}"


@dataclass(frozen=True, slots=True)
class ExpectationReport:
    dimension: str
    expected: dict[str, str] = field(default_factory=dict)
    unexpected_failures: dict[str, str] = field(default_factory=dict)
    unexpected_passes: dict[str, str] = field(default_factory=dict)
    unconfirmed: dict[str, str] = field(default_factory=dict)
    errors: dict[str, str] = field(default_factory=dict)
    problems: tuple[str, ...] = ()

    @property
    def returncode(self) -> int:
        failing = (
            self.unexpected_failures,
            self.unexpected_passes,
            self.unconfirmed,
            self.errors,
            self.problems,
        )
        return 1 if any(failing) else 0

    def lines(self) -> list[str]:
        out = [
            f"expectations {self.dimension}: {len(self.expected)} expected, "
            f"{len(self.unexpected_failures)} unexpected failures, "
            f"{len(self.unexpected_passes)} unexpected passes, "
            f"{len(self.unconfirmed)} unconfirmed, {len(self.errors)} errors"
        ]
        for title, entries in (
            ("expected failure", self.expected),
            ("UNEXPECTED FAILURE (no owning finding)", self.unexpected_failures),
            ("UNEXPECTED PASS (close the entry and its row)", self.unexpected_passes),
            ("UNCONFIRMED expectation", self.unconfirmed),
            ("ERROR (never expected)", self.errors),
        ):
            for key, detail in sorted(entries.items()):
                out.append(f"  {title}: {key} -- {detail}")
        out.extend(f"  problem: {problem}" for problem in self.problems)
        return out


def dimension_entries(
    manifest: Mapping[str, object], dimension: str
) -> dict[str, Mapping[str, object]]:
    """The `fail` entries of the regrtest block for one dimension, by key."""
    block = manifest.get("regrtest", {})
    if not isinstance(block, Mapping):
        return {}
    entries: dict[str, Mapping[str, object]] = {}
    for key, entry in block.items():
        dims = entry.get("dimensions", {}) if isinstance(entry, Mapping) else {}
        status = dims.get(dimension) if isinstance(dims, Mapping) else None
        if isinstance(status, Mapping) and status.get("status") == "fail":
            entries[key] = status
    return entries


def classify(
    outcomes: Mapping[str, ItemOutcome],
    manifest: Mapping[str, object],
    *,
    dimension: str,
    selected: Iterable[str],
) -> ExpectationReport:
    """Compare per-test outcomes, keyed by regrtest test name, with the manifest."""
    report = ExpectationReport(dimension=dimension)
    selected_keys = {expectation_key(name) for name in selected}
    entries = dimension_entries(manifest, dimension)
    problems: list[str] = []
    for key, status in sorted(entries.items()):
        if problem := finding_problem(status.get("finding")):
            problems.append(f"{key}: {problem}")
    observed = {expectation_key(name): outcome for name, outcome in outcomes.items()}
    for key, outcome in sorted(observed.items()):
        entry = entries.get(key)
        detail = outcome.reason or outcome.status
        if outcome.status == "errors":
            report.errors[key] = detail
        elif outcome.status == "failed":
            if entry is None:
                report.unexpected_failures[key] = detail
            elif detail != entry.get("failure"):
                report.unexpected_failures[key] = (
                    f"{detail} ({entry.get('finding')} owns: {entry.get('failure')})"
                )
            else:
                report.expected[key] = str(entry.get("finding"))
        elif outcome.status == "passed" and entry is not None:
            report.unexpected_passes[key] = str(entry.get("finding"))
    for key, entry in sorted(entries.items()):
        module = key.split(CASE_SEPARATOR, 1)[0]
        if module not in selected_keys:
            continue
        outcome = observed.get(key)
        if CASE_SEPARATOR in key:
            report.unconfirmed[key] = (
                f"{entry.get('finding')}: this run reports no per-case results"
            )
        elif outcome is None:
            report.unconfirmed[key] = f"{entry.get('finding')}: no result"
        elif outcome.status in {"skipped", "not_run"}:
            report.unconfirmed[key] = (
                f"{entry.get('finding')}: {outcome.status} ({outcome.reason})"
            )
    return ExpectationReport(
        dimension=report.dimension,
        expected=report.expected,
        unexpected_failures=report.unexpected_failures,
        unexpected_passes=report.unexpected_passes,
        unconfirmed=report.unconfirmed,
        errors=report.errors,
        problems=tuple(problems),
    )
