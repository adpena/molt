"""Pytest's JUnit boundary: each test's collected node ID and its outcome.

``molt.pytest_capacity_plugin`` records every collected item's node ID as the
``molt_nodeid`` property of its JUnit testcase. That ID, not the testcase's
``file`` and ``classname``, names the test: a definition's file can lie
outside the test tree (Hypothesis's inherited ``runTest``), and a generated
class has no module path. A landing reruns a failed test by this ID
(``tools/molt_dev_gates.py``), and postsubmit names failing tests by it.
A report that carries the property also proves the checkout's plugin
accepts the node-selection options (``--molt-run-nodes``,
``--molt-skip-nodes``), which ship with it.
"""

from __future__ import annotations

from enum import StrEnum
from pathlib import Path
from xml.etree import ElementTree

NODE_ID_PROPERTY = "molt_nodeid"


class PytestOutcome(StrEnum):
    PASSED = "passed"
    FAILED = "failed"
    SKIPPED = "skipped"


def collected_nodeid(case: ElementTree.Element) -> str | None:
    """The node ID the plugin recorded on one testcase, if it recorded one."""
    values = [
        prop.get("value", "")
        for prop in case.findall("./properties/property")
        if prop.get("name") == NODE_ID_PROPERTY
    ]
    if not values:
        return None
    if len(set(values)) != 1 or not values[0]:
        raise ValueError(
            f"testcase records conflicting node IDs: {sorted(set(values))}"
        )
    return values[0]


def junit_outcomes(path: Path) -> dict[str, PytestOutcome]:
    """Every executed test of one report, by collected node ID.

    A testcase without the property is a collection error or a module-level
    skip, which names no executed test: a skip is dropped, and a failure
    raises, since no rerun can select it. Setup, call and teardown may each
    report; a later phase never erases an earlier failure.
    """
    try:
        tree = ElementTree.parse(path)
    except (OSError, ElementTree.ParseError) as exc:
        raise ValueError(f"cannot read pytest JUnit {path}: {exc}") from exc
    outcomes: dict[str, PytestOutcome] = {}
    for case in tree.iter("testcase"):
        failed = case.find("failure") is not None or case.find("error") is not None
        node = collected_nodeid(case)
        if node is None:
            if failed:
                raise ValueError(
                    f"{path}: a failure names no collected test "
                    f"({case.get('classname', '')}::{case.get('name', '')})"
                )
            if case.find("skipped") is not None:
                continue
            raise ValueError(f"{path}: a test has no {NODE_ID_PROPERTY} property")
        outcome = (
            PytestOutcome.FAILED
            if failed
            else PytestOutcome.SKIPPED
            if case.find("skipped") is not None
            else PytestOutcome.PASSED
        )
        if outcomes.get(node) is not PytestOutcome.FAILED:
            outcomes[node] = outcome
    return outcomes
