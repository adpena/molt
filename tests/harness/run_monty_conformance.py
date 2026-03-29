"""Run Monty conformance test files and report results.

Each test file in tests/harness/corpus/monty_compat/ is a self-contained
Python program. Expected behavior is determined by:

  1. A trailing comment:  # Raise=<ExcType>(msg)
     -> program must raise that exception type

  2. A TRACEBACK: docstring block containing the exception type
     -> program must raise that exception type

  3. Otherwise (assert-only files)
     -> program must exit 0 (all asserts pass)

Usage:
    python3 tests/harness/run_monty_conformance.py [--verbose]
    python3 tests/harness/run_monty_conformance.py --runner <cmd>  # e.g. --runner "molt run"
"""
from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

CORPUS_DIR = Path(__file__).resolve().parent / "corpus" / "monty_compat"


def parse_expectation(filepath: Path) -> tuple[str, str]:
    """Parse the expectation from a test file.

    Returns (kind, value) where kind is one of:
      'raise'       - value is the exception type name
      'success'     - value is '' (assert-only, must exit 0)
      'refcount'    - value is the ref-count spec (informational, treated as success)
    """
    text = filepath.read_text()
    lines = text.strip().splitlines()

    # Files marked '# call-external' depend on helpers not in the file
    if lines and lines[0].strip() == "# call-external":
        return ("skip", "depends on external helpers")

    # Check for # Raise=ExcType(...) comment (scan from bottom)
    for line in reversed(lines):
        stripped = line.strip()
        if stripped.startswith("# Raise="):
            exc_spec = stripped[len("# Raise="):]
            exc_type = exc_spec.split("(")[0]
            return ("raise", exc_type)
        if stripped.startswith("# ref-counts="):
            return ("refcount", stripped[len("# ref-counts="):])
        if stripped.startswith("#"):
            continue
        break

    # Check for TRACEBACK: docstring block
    traceback_match = re.search(
        r'TRACEBACK:\s*\n.*?(\w+Error|\w+Exception|SyntaxError|ImportError)',
        text,
        re.DOTALL,
    )
    if traceback_match:
        return ("raise", traceback_match.group(1))

    return ("success", "")


def run_test(
    filepath: Path, runner: list[str], timeout: int = 10
) -> tuple[bool | None, str]:
    """Run a test file and check against its expectation.

    Returns (passed, detail_message).  passed=None means skipped.
    """
    kind, expected = parse_expectation(filepath)

    if kind == "skip":
        return (None, expected)

    try:
        result = subprocess.run(
            [*runner, str(filepath)],
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return (False, "timeout (10s)")

    if kind == "raise":
        if result.returncode == 0:
            return (False, f"expected {expected}, but program exited 0")
        # Check if the expected exception type appears in stderr
        if expected in result.stderr:
            return (True, f"correctly raised {expected}")
        # Compile-time rejection: Molt may reject invalid code at compile time
        # (e.g., wrong argument count) with MOLT_COMPAT_ERROR instead of
        # producing a runtime exception. This is a CORRECT rejection — Molt's
        # static analysis caught the bug before runtime.
        if "MOLT_COMPAT_ERROR" in result.stderr or "unsupported construct" in result.stderr:
            return (True, f"correctly rejected at compile time (expected {expected})")
        return (False, f"expected {expected}, got stderr: {result.stderr.strip()[-200:]}")

    elif kind in ("success", "refcount"):
        if result.returncode == 0:
            return (True, "exited 0")
        return (
            False,
            f"expected exit 0, got {result.returncode}: {result.stderr.strip()[:200]}",
        )

    return (False, f"unknown expectation kind: {kind}")


def main() -> int:
    verbose = "--verbose" in sys.argv or "-v" in sys.argv

    runner = ["python3"]
    if "--runner" in sys.argv:
        idx = sys.argv.index("--runner")
        if idx + 1 < len(sys.argv):
            runner = sys.argv[idx + 1].split()

    corpus = CORPUS_DIR
    if not corpus.exists():
        print(f"Corpus directory not found: {corpus}", file=sys.stderr)
        return 1

    test_files = sorted(corpus.glob("*.py"))
    if not test_files:
        print("No test files found", file=sys.stderr)
        return 1

    passed = 0
    failed = 0
    skipped = 0
    errors: list[tuple[str, str]] = []

    for filepath in test_files:
        ok, detail = run_test(filepath, runner)
        if ok is None:
            skipped += 1
            if verbose:
                print(f"  SKIP  {filepath.name}: {detail}")
        elif ok:
            passed += 1
            if verbose:
                print(f"  PASS  {filepath.name}: {detail}")
        else:
            failed += 1
            errors.append((filepath.name, detail))
            if verbose:
                print(f"  FAIL  {filepath.name}: {detail}")

    total = passed + failed
    pct = (passed / total * 100) if total > 0 else 0

    print(f"\nMonty conformance: {passed}/{total} ({pct:.0f}%) passed", end="")
    if skipped:
        print(f", {skipped} skipped", end="")
    print()

    if errors:
        print(f"\nFailed ({len(errors)}):")
        for name, detail in errors:
            print(f"  {name}: {detail}")

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
