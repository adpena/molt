"""Contract tests for the safe-mode program generator at three complexity levels.

``tools/fuzz_compiler_safe.SafeProgramGenerator`` feeds the compiler fuzzer,
which compiles, runs and compares its programs against CPython in
``nightly.verification-t3.fuzz-compiler`` and counts every CPython error as a
failure. These tests hold the generator to that contract without building:
each program parses, is reproducible from its seed, prints, and runs to a
clean exit under CPython; and the batch covers the documented AST node types.
"""

from __future__ import annotations

import ast
import subprocess
import sys
from random import Random

import pytest

from tests.native_process_guard import run_native_test_process
from tools.fuzz_compiler_safe import SafeProgramGenerator

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Complexity presets: (max_depth, max_stmts)
COMPLEXITY_PRESETS: dict[str, tuple[int, int]] = {
    "simple": (2, 8),
    "medium": (3, 15),
    "complex": (4, 25),
}

SEED_BATCH_SIZE = 50


def _generate(seed: int, max_depth: int = 3, max_stmts: int = 15) -> str:
    rng = Random(seed)
    gen = SafeProgramGenerator(rng, max_depth=max_depth, max_stmts=max_stmts)
    return gen.generate()


def _collect_ast_node_types(source: str) -> set[str]:
    """Return the set of distinct AST node type names in *source*."""
    tree = ast.parse(source)
    return {type(node).__name__ for node in ast.walk(tree)}


# ---------------------------------------------------------------------------
# Tests parametrized by complexity level
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("level", ["simple", "medium", "complex"])
class TestFuzzExtendedBatch:
    """Generate 50 programs per complexity level and validate them."""

    def test_syntax_valid(self, level: str) -> None:
        max_depth, max_stmts = COMPLEXITY_PRESETS[level]
        failures: list[tuple[int, str]] = []
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth, max_stmts)
            try:
                ast.parse(source)
            except SyntaxError as exc:
                failures.append((seed, str(exc)))
        assert not failures, (
            f"[{level}] {len(failures)}/{SEED_BATCH_SIZE} programs had syntax "
            f"errors: seeds {[s for s, _ in failures]}"
        )

    def test_determinism(self, level: str) -> None:
        max_depth, max_stmts = COMPLEXITY_PRESETS[level]
        mismatches: list[int] = []
        for seed in range(SEED_BATCH_SIZE):
            a = _generate(seed, max_depth, max_stmts)
            b = _generate(seed, max_depth, max_stmts)
            if a != b:
                mismatches.append(seed)
        assert not mismatches, f"[{level}] Non-deterministic for seeds: {mismatches}"

    def test_cpython_terminates(self, level: str) -> None:
        max_depth, max_stmts = COMPLEXITY_PRESETS[level]
        hangs: list[int] = []
        runtime_errors: list[tuple[int, int, str]] = []
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth, max_stmts)
            try:
                result = run_native_test_process(
                    [sys.executable, "-c", source],
                    capture_output=True,
                    text=True,
                    timeout=15,
                )
            except subprocess.TimeoutExpired:
                hangs.append(seed)
                continue
            if result.returncode != 0:
                last_line = (result.stderr.strip().splitlines() or [""])[-1]
                runtime_errors.append((seed, result.returncode, last_line))
        assert not hangs, f"[{level}] Programs hung for seeds: {hangs}"
        # The fuzzer counts a CPython error as a failed proof, so every safe
        # program must run to a clean exit.
        assert not runtime_errors, (
            f"[{level}] {len(runtime_errors)}/{SEED_BATCH_SIZE} programs failed "
            f"under CPython: {runtime_errors}"
        )

    def test_all_programs_have_print(self, level: str) -> None:
        max_depth, max_stmts = COMPLEXITY_PRESETS[level]
        missing: list[int] = []
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth, max_stmts)
            if "print(" not in source:
                missing.append(seed)
        assert not missing, f"[{level}] Programs without print() for seeds: {missing}"


# ---------------------------------------------------------------------------
# AST node-type coverage
# ---------------------------------------------------------------------------


class TestFuzzASTCoverage:
    """Check that the generator produces a good variety of AST node types."""

    # Minimum distinct node types we expect across all generated programs.
    MIN_NODE_TYPES = 20

    def test_node_type_diversity(self) -> None:
        all_types: set[str] = set()
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth=4, max_stmts=25)
            all_types |= _collect_ast_node_types(source)
        assert len(all_types) >= self.MIN_NODE_TYPES, (
            f"Only {len(all_types)} distinct AST node types across "
            f"{SEED_BATCH_SIZE} programs (expected >= {self.MIN_NODE_TYPES}): "
            f"{sorted(all_types)}"
        )

    def test_expected_node_types_present(self) -> None:
        """Specific node types the generator is documented to produce."""
        expected = {
            "FunctionDef",
            "If",
            "For",
            "While",
            "Return",
            "Assign",
            "BinOp",
            "Compare",
            "Call",
            "Name",
            "Constant",
            "List",
            "Dict",
            "Tuple",
        }
        all_types: set[str] = set()
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth=4, max_stmts=25)
            all_types |= _collect_ast_node_types(source)
        missing = expected - all_types
        assert not missing, f"Expected AST node types not generated: {sorted(missing)}"
