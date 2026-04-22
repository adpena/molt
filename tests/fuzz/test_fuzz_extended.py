"""Extended fuzzing tests for CI Tier 3.

Generates programs at various complexity levels, validates syntax, runs them
under CPython, and checks determinism.  Reports aggregate statistics at the
end of each parametrized batch.
"""

from __future__ import annotations

import ast
import subprocess
import sys
from pathlib import Path
from random import Random

import pytest

# Allow importing tools/fuzz_compiler.py from the repo root.
_REPO_ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(_REPO_ROOT / "tools"))

from fuzz_compiler import SafeProgramGenerator  # noqa: E402

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
        runtime_errors: list[tuple[int, int]] = []
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth, max_stmts)
            try:
                result = subprocess.run(
                    [sys.executable, "-c", source],
                    capture_output=True,
                    text=True,
                    timeout=15,
                )
            except subprocess.TimeoutExpired:
                hangs.append(seed)
                continue
            if result.returncode != 0:
                runtime_errors.append((seed, result.returncode))
        assert not hangs, f"[{level}] Programs hung for seeds: {hangs}"
        # Runtime errors are tracked but not fatal -- the generator tries to
        # produce clean programs but some edge cases may trigger runtime
        # exceptions.  We allow up to 10% failure rate.
        max_allowed = max(1, SEED_BATCH_SIZE // 10)
        assert len(runtime_errors) <= max_allowed, (
            f"[{level}] Too many runtime errors ({len(runtime_errors)}/{SEED_BATCH_SIZE}): "
            f"seeds {[s for s, _ in runtime_errors]}"
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


# ---------------------------------------------------------------------------
# Molt compilation (best-effort)
# ---------------------------------------------------------------------------


class TestFuzzMoltCompile:
    """Try compiling generated programs with Molt if available.

    These tests are skipped when Molt cannot be imported, so they are safe
    to run in environments where only CPython is present.
    """

    @staticmethod
    def _molt_available() -> bool:
        try:
            result = subprocess.run(
                [sys.executable, "-m", "molt.cli", "--help"],
                capture_output=True,
                text=True,
                timeout=10,
                cwd=str(_REPO_ROOT),
            )
            return result.returncode == 0
        except Exception:
            return False

    @pytest.fixture(autouse=True)
    def _skip_unless_molt(self) -> None:
        if not self._molt_available():
            pytest.skip("molt CLI not available")

    def test_compile_ten_programs(self, tmp_path: Path) -> None:
        """Compile 10 generated programs with molt build."""
        compile_failures: list[tuple[int, str]] = []
        for seed in range(10):
            source = _generate(seed, max_depth=3, max_stmts=15)
            src_file = tmp_path / f"fuzz_{seed}.py"
            src_file.write_text(source)
            try:
                result = subprocess.run(
                    [
                        sys.executable,
                        "-m",
                        "molt.cli",
                        "build",
                        "--build-profile",
                        "dev",
                        "--deterministic",
                        str(src_file),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=120,
                    cwd=str(_REPO_ROOT),
                )
            except subprocess.TimeoutExpired:
                compile_failures.append((seed, "(timed out)"))
                continue
            if result.returncode != 0:
                compile_failures.append(
                    (seed, result.stderr[:300] if result.stderr else "(no stderr)")
                )
        # We expect most programs to compile; allow a 30% failure rate since
        # Molt does not yet support every Python construct.
        max_allowed = max(1, 10 * 30 // 100)
        assert len(compile_failures) <= max_allowed, (
            f"{len(compile_failures)}/10 programs failed Molt compilation: "
            f"{compile_failures}"
        )


# ---------------------------------------------------------------------------
# Statistics report (always runs last via test ordering)
# ---------------------------------------------------------------------------


class TestFuzzExtendedReport:
    """Aggregate statistics across a medium-complexity batch."""

    def test_report(self, capsys: pytest.CaptureFixture[str]) -> None:
        stats = {
            "generated": 0,
            "syntax_valid": 0,
            "runtime_success": 0,
            "runtime_error": 0,
            "timeout": 0,
            "deterministic": 0,
        }
        max_depth, max_stmts = COMPLEXITY_PRESETS["medium"]
        for seed in range(SEED_BATCH_SIZE):
            source = _generate(seed, max_depth, max_stmts)
            stats["generated"] += 1

            try:
                ast.parse(source)
                stats["syntax_valid"] += 1
            except SyntaxError:
                continue

            # Determinism
            if _generate(seed, max_depth, max_stmts) == source:
                stats["deterministic"] += 1

            # Runtime
            try:
                result = subprocess.run(
                    [sys.executable, "-c", source],
                    capture_output=True,
                    text=True,
                    timeout=15,
                )
                if result.returncode == 0:
                    stats["runtime_success"] += 1
                else:
                    stats["runtime_error"] += 1
            except subprocess.TimeoutExpired:
                stats["timeout"] += 1

        # Print report (visible with pytest -s or captured by capsys)
        lines = [
            "",
            "=== Fuzz Extended Report (medium) ===",
            f"  Generated:       {stats['generated']}",
            f"  Syntax valid:    {stats['syntax_valid']}",
            f"  Deterministic:   {stats['deterministic']}",
            f"  Runtime success: {stats['runtime_success']}",
            f"  Runtime error:   {stats['runtime_error']}",
            f"  Timeout:         {stats['timeout']}",
            "=====================================",
        ]
        report = "\n".join(lines)
        print(report)

        # Basic sanity: all should be syntax-valid and deterministic
        assert stats["syntax_valid"] == stats["generated"]
        assert stats["deterministic"] == stats["generated"]
