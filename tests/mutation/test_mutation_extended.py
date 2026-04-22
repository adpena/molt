"""Extended mutation tests for the new operators (MOL-283).

These tests run mutations from each new operator against sample programs
and report kill rates. This is the "extended" test referenced in CI Tier 3.
"""

from __future__ import annotations

import ast
import os
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

import pytest

# Ensure tools/ is importable.
REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO_ROOT / "tools"))

from mutation_test import (  # noqa: E402
    MutationSite,
    apply_single_mutation,
    discover_mutations,
)

# ---------------------------------------------------------------------------
# Sample programs exercising different language features
# ---------------------------------------------------------------------------

SAMPLE_PROGRAMS: dict[str, str] = {
    "loops": textwrap.dedent("""\
        results = []
        for i in range(5):
            results.append(i * 2)
        for j in range(3):
            results.append(j + 10)
        print(results)
    """),
    "strings": textwrap.dedent("""\
        text = "  Hello, World!  "
        print(text.strip())
        print(text.upper())
        print(text.lower())
        print("prefix".startswith("pre"))
        print("suffix".endswith("fix"))
        words = text.strip().title()
        print(words)
    """),
    "exceptions": textwrap.dedent("""\
        def safe_op(a, b):
            try:
                return a / b
            except ZeroDivisionError:
                return -1
            except TypeError:
                return -2

        print(safe_op(10, 2))
        print(safe_op(10, 0))
        print(safe_op("a", 2))
    """),
    "slices": textwrap.dedent("""\
        data = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        print(data[1:])
        print(data[:5])
        print(data[2:8])
        print(data[1:7])
        s = "abcdefghij"
        print(s[3:])
        print(s[:4])
    """),
    "containers": textwrap.dedent("""\
        items = []
        items.append(1)
        items.append(2)
        items.append(3)
        print(items)

        s = set()
        s.add(10)
        s.add(20)
        s.add(10)
        print(sorted(s))

        items2 = []
        items2.extend([4, 5])
        items2.append(6)
        print(items2)
    """),
    "mixed": textwrap.dedent("""\
        def process(data):
            result = []
            for i in range(len(data)):
                val = data[i]
                try:
                    cleaned = str(val).strip().upper()
                    result.append(cleaned)
                except Exception:
                    result.append("ERROR")
            return result

        data = [" hello ", 42, " world ", None]
        out = process(data)
        print(out)
        print(out[1:])
        print(out[:2])
    """),
}

NEW_OPERATORS = [
    "loop_bound",
    "exception_swallow",
    "slice_modify",
    "string_method_swap",
    "container_method_swap",
]


def _run_code(code: str) -> tuple[str, int]:
    """Execute code in a subprocess, return (stdout, returncode)."""
    fd, path = tempfile.mkstemp(suffix=".py")
    try:
        os.write(fd, code.encode())
        os.close(fd)
        result = subprocess.run(
            [sys.executable, path],
            capture_output=True,
            text=True,
            timeout=10,
        )
        return result.stdout, result.returncode
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Test: every new operator produces valid Python on all sample programs
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("operator", NEW_OPERATORS)
def test_operator_produces_valid_python(operator: str) -> None:
    """All mutations from each new operator must produce parseable Python."""
    total = 0
    for name, source in SAMPLE_PROGRAMS.items():
        sites = discover_mutations(f"<{name}>", source, {operator})
        for site in sites:
            mutated = apply_single_mutation(source, site)
            if mutated is None:
                continue
            total += 1
            try:
                ast.parse(mutated)
            except SyntaxError as exc:
                pytest.fail(
                    f"Operator {operator} on {name} ({site.description}) "
                    f"produced invalid Python: {exc}"
                )
    # Each operator should find at least some sites across all programs
    assert total > 0, f"Operator {operator} found no applicable mutation sites"


# ---------------------------------------------------------------------------
# Test: run 20 mutations per operator, report kill rate
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("operator", NEW_OPERATORS)
def test_operator_kill_rate(operator: str) -> None:
    """Run up to 20 mutations per operator and verify kill rate > 0.

    This is the extended mutation test for CI Tier 3.
    """
    import random

    rng = random.Random(42)  # deterministic

    # Collect all mutation sites for this operator across sample programs.
    all_sites: list[tuple[str, str, MutationSite]] = []
    for name, source in SAMPLE_PROGRAMS.items():
        sites = discover_mutations(f"<{name}>", source, {operator})
        for site in sites:
            all_sites.append((name, source, site))

    assert len(all_sites) > 0, f"No mutation sites for operator {operator}"

    # Sample up to 20.
    if len(all_sites) > 20:
        all_sites = rng.sample(all_sites, 20)

    killed = 0
    survived = 0
    skipped = 0

    for name, source, site in all_sites:
        mutated = apply_single_mutation(source, site)
        if mutated is None:
            skipped += 1
            continue

        orig_out, orig_rc = _run_code(source)
        mut_out, mut_rc = _run_code(mutated)

        if orig_out != mut_out or orig_rc != mut_rc:
            killed += 1
        else:
            survived += 1

    total = killed + survived
    kill_rate = killed / total if total > 0 else 0.0

    # Report
    print(
        f"\n  [{operator}] killed={killed} survived={survived} "
        f"skipped={skipped} kill_rate={kill_rate:.0%}"
    )

    # At least one mutation must be killed to prove the operator is useful.
    assert killed > 0, (
        f"Operator {operator}: zero kills out of {total} mutations. "
        f"The operator may not be producing semantically meaningful mutations."
    )


# ---------------------------------------------------------------------------
# Test: each operator discovers mutations in relevant programs
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "operator,expected_program",
    [
        ("loop_bound", "loops"),
        ("exception_swallow", "exceptions"),
        ("slice_modify", "slices"),
        ("string_method_swap", "strings"),
        ("container_method_swap", "containers"),
    ],
)
def test_operator_discovers_in_target_program(
    operator: str, expected_program: str
) -> None:
    """Each operator should discover sites in its corresponding sample."""
    source = SAMPLE_PROGRAMS[expected_program]
    sites = discover_mutations(f"<{expected_program}>", source, {operator})
    assert len(sites) >= 1, f"Operator {operator} found no sites in {expected_program}"


# ---------------------------------------------------------------------------
# Test: mixed program exercises multiple new operators
# ---------------------------------------------------------------------------


def test_mixed_program_multi_operator() -> None:
    """The mixed program should have sites for multiple new operators."""
    source = SAMPLE_PROGRAMS["mixed"]
    found_ops: set[str] = set()
    for op in NEW_OPERATORS:
        sites = discover_mutations("<mixed>", source, {op})
        if sites:
            found_ops.add(op)

    # The mixed program should exercise at least 3 of the 5 new operators.
    assert len(found_ops) >= 3, (
        f"Mixed program only exercises {found_ops} of the new operators"
    )


# ---------------------------------------------------------------------------
# Standalone runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    pytest.main([__file__, "-v", "--tb=short"])
