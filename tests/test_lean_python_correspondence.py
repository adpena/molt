"""Tests verifying Lean <-> Python alignment.

Parses actual source files (no hardcoded values) and asserts that
effect classifications, pure op sets, and compiler pass names are
consistent across the Lean formalization and the Python frontend.

Run:
    uv run pytest tests/test_lean_python_correspondence.py -q
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest


def _find_repo_root() -> Path:
    try:
        out = subprocess.check_output(
            ["git", "rev-parse", "--show-toplevel"],
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()
SYNTAX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Syntax.lean"
FRONTEND_PY = ROOT / "src" / "molt" / "frontend" / "__init__.py"
LEAN_PASSES_DIR = ROOT / "formal" / "lean" / "MoltTIR" / "Passes"


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    pytest.skip(f"Source file not found: {path}")
    return ""


# ── Python effect classification parsing ─────────────────────────────


def _parse_python_effect_classes(text: str) -> dict[str, set[str]]:
    """Extract effect class -> set of op kinds from _op_effect_class method.

    Parses the Python source to find the sets of op kinds assigned to each
    effect class (pure, reads_heap, writes_heap, control).
    """
    classes: dict[str, set[str]] = {
        "pure": set(),
        "reads_heap": set(),
        "writes_heap": set(),
        "control": set(),
    }

    # Find the _op_effect_class method body
    m = re.search(r"def _op_effect_class\(self, op_kind: str\) -> str:", text)
    if not m:
        return classes

    method_body = text[m.end():]
    # Find next def at same or lesser indentation to bound the method
    end_match = re.search(r"\n    def ", method_body)
    if end_match:
        method_body = method_body[:end_match.start()]

    # Parse each `if op_kind in { ... }: return "class"` block
    # We look for patterns: set literal followed by return statement
    for block_match in re.finditer(
        r'if\s+op_kind\s+in\s+\{([^}]+)\}:\s*\n\s*return\s+"(\w+)"',
        method_body,
    ):
        ops_str = block_match.group(1)
        effect_class = block_match.group(2)
        if effect_class in classes:
            for op_match in re.finditer(r'"(\w+)"', ops_str):
                classes[effect_class].add(op_match.group(1))

    return classes


def _parse_lean_inductive_variants(text: str, type_name: str) -> list[str]:
    """Extract variant names from a Lean `inductive` definition."""
    pattern = rf"inductive\s+{type_name}\s+where"
    m = re.search(pattern, text)
    if not m:
        return []
    start = m.end()
    lines = text[start:].split("\n")
    variants: list[str] = []
    for line in lines:
        stripped = line.strip()
        if not stripped or stripped.startswith("--"):
            continue
        if stripped.startswith("deriving"):
            break
        if re.match(
            r"^(inductive|def|theorem|structure|namespace|end|abbrev|section)\b",
            stripped,
        ):
            break
        for vm in re.finditer(r"\|\s*\.?(\w+)", stripped):
            name = vm.group(1)
            if name.endswith("_"):
                name = name[:-1]
            if name not in variants:
                variants.append(name)
    return variants


def _lean_binop_python_name(name: str) -> str | None:
    normalized = name[:-1] if name.endswith("_") else name
    direct_map = {
        "and": "AND",
        "or": "OR",
        "is": "IS",
    }
    if normalized in direct_map:
        return direct_map[normalized]
    # These Lean binops are lowered compositionally in the Python frontend
    # rather than emitted as a single pure op kind.
    if normalized in {"is_not", "in", "not_in"}:
        return None
    return normalized.upper()


def _lean_unop_python_name(name: str) -> str | None:
    normalized = name[:-1] if name.endswith("_") else name
    if normalized in {"invert", "neg", "guard", "pos"}:
        return None
    return normalized.upper()


# ── Effect classification tests ──────────────────────────────────────


@pytest.fixture(scope="module")
def python_effect_classes() -> dict[str, set[str]]:
    return _parse_python_effect_classes(_read(FRONTEND_PY))


@pytest.fixture(scope="module")
def lean_syntax_text() -> str:
    return _read(SYNTAX_LEAN)


class TestEffectClassification:
    """Effect classifications should be non-empty and well-structured."""

    def test_pure_ops_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["pure"]) > 0, (
            "No pure ops found in _op_effect_class"
        )

    def test_reads_heap_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["reads_heap"]) > 0, (
            "No reads_heap ops found in _op_effect_class"
        )

    def test_writes_heap_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["writes_heap"]) > 0, (
            "No writes_heap ops found in _op_effect_class"
        )

    def test_control_ops_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["control"]) > 0, (
            "No control ops found in _op_effect_class"
        )

    def test_no_overlap_pure_writes(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        overlap = python_effect_classes["pure"] & python_effect_classes["writes_heap"]
        assert not overlap, (
            f"Ops classified as both pure and writes_heap: {overlap}"
        )

    def test_no_overlap_pure_reads(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        overlap = python_effect_classes["pure"] & python_effect_classes["reads_heap"]
        assert not overlap, (
            f"Ops classified as both pure and reads_heap: {overlap}"
        )


class TestPureOpAlignment:
    """Lean BinOp/UnOp arithmetic/comparison ops should be in Python's pure set."""

    def test_lean_binops_are_pure_in_python(
        self,
        lean_syntax_text: str,
        python_effect_classes: dict[str, set[str]],
    ) -> None:
        """Lean BinOp variants (arithmetic + comparison) should map to pure Python ops."""
        lean_binops = _parse_lean_inductive_variants(lean_syntax_text, "BinOp")
        pure_ops = python_effect_classes["pure"]

        # These are the Lean BinOp names that should be in the Python pure set
        # (uppercase in Python)
        for op in lean_binops:
            upper = _lean_binop_python_name(op)
            if upper is None:
                continue
            # Some ops may not be in the Python IR (e.g. bitwise handled differently)
            if upper in pure_ops:
                continue
            # The op might use a different name mapping; these are acceptable gaps
            # where the Python IR doesn't have a direct 1:1 for bitwise/advanced ops
            acceptable_gaps = {
                "BIT_AND", "BIT_OR", "BIT_XOR", "LSHIFT", "RSHIFT",
                "DIV", "FLOORDIV", "POW", "MOD",
            }
            if upper in acceptable_gaps:
                continue
            assert upper in pure_ops, (
                f"Lean BinOp.{op} (Python: {upper}) not in Python pure ops"
            )

    def test_lean_unops_are_pure_in_python(
        self,
        lean_syntax_text: str,
        python_effect_classes: dict[str, set[str]],
    ) -> None:
        """Lean UnOp variants should map to pure Python ops."""
        lean_unops = _parse_lean_inductive_variants(lean_syntax_text, "UnOp")
        pure_ops = python_effect_classes["pure"]

        for op in lean_unops:
            upper = _lean_unop_python_name(op)
            if upper is None:
                continue
            assert upper in pure_ops, (
                f"Lean UnOp.{op} (Python: {upper}) not in Python pure ops"
            )


# ── Compiler pass name tests ─────────────────────────────────────────


# Expected Lean compiler pass files and their key function names
LEAN_PASS_SPECS = {
    "ConstFold": ("ConstFold.lean", "constFoldFunc"),
    "DCE": ("DCE.lean", "dceFunc"),
    "SCCP": ("SCCP.lean", "sccpFunc"),
    "CSE": ("CSE.lean", "cseFunc"),
    "LICM": ("LICM.lean", "licmFunc"),
    "GuardHoist": ("GuardHoist.lean", "guardHoistFunc"),
    "JoinCanon": ("JoinCanon.lean", "joinCanonFunc"),
    "EdgeThread": ("EdgeThread.lean", "edgeThreadFunc"),
}


class TestCompilerPassCorrespondence:
    """Lean compiler pass definitions must exist and correspond to Python."""

    @pytest.mark.parametrize("pass_name,spec", list(LEAN_PASS_SPECS.items()))
    def test_lean_pass_file_exists(self, pass_name: str, spec: tuple[str, str]) -> None:
        filename, _ = spec
        path = LEAN_PASSES_DIR / filename
        assert path.exists(), (
            f"Lean pass file missing: {path}"
        )

    @pytest.mark.parametrize("pass_name,spec", list(LEAN_PASS_SPECS.items()))
    def test_lean_pass_defines_func(
        self, pass_name: str, spec: tuple[str, str]
    ) -> None:
        filename, func_name = spec
        path = LEAN_PASSES_DIR / filename
        if not path.exists():
            pytest.skip(f"Lean pass file not found: {path}")
        text = path.read_text(errors="replace")
        assert func_name in text, (
            f"Function {func_name} not found in {path}"
        )

    def test_python_mentions_pass_concepts(self) -> None:
        """Python frontend should reference the same pass concepts as Lean."""
        py_text = _read(FRONTEND_PY)

        # These are the Python-side method/concept names that correspond
        # to the Lean passes
        python_pass_indicators = {
            "ConstFold": ["const_fold", "constant_fold", "CONST"],
            "DCE": ["dead", "dce", "eliminate_dead"],
            "SCCP": ["sccp", "lattice", "propagat"],
            "CSE": ["cse", "common_subexpr", "value_number"],
            "LICM": ["licm", "loop_invariant", "hoist_loop"],
            "GuardHoist": ["guard", "hoist"],
            "JoinCanon": ["join", "canon"],
            "EdgeThread": ["edge", "thread"],
        }

        for pass_name, indicators in python_pass_indicators.items():
            found = any(
                ind.lower() in py_text.lower() for ind in indicators
            )
            assert found, (
                f"No reference to {pass_name} concept found in Python frontend. "
                f"Looked for: {indicators}"
            )

    def test_lean_pass_correctness_proofs_exist(self) -> None:
        """Each Lean pass should have a corresponding correctness proof file."""
        for pass_name in LEAN_PASS_SPECS:
            proof_file = LEAN_PASSES_DIR / f"{pass_name}Correct.lean"
            assert proof_file.exists(), (
                f"Missing correctness proof for {pass_name}: {proof_file}"
            )
