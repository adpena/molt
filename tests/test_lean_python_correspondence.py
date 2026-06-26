"""Tests verifying Lean <-> Python alignment.

Parses actual source files (no hardcoded values) and asserts that generated
frontend effect classifications and compiler pass names stay consistent with
the Lean formalization and the Python frontend.

Run:
    uv run pytest tests/test_lean_python_correspondence.py -q
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

import pytest
from tests.process_guard_common import run_guarded_test_process


def _find_repo_root() -> Path:
    try:
        proc = run_guarded_test_process(
            ["git", "rev-parse", "--show-toplevel"],
            prefix="MOLT_FORMAL",
            text=True,
            check=True,
        )
        out = (proc.stdout or "").strip()
        return Path(out)
    except (subprocess.CalledProcessError, FileNotFoundError):
        return Path(__file__).resolve().parents[1]


ROOT = _find_repo_root()
SYNTAX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Syntax.lean"
FRONTEND_MIDEND_PY_FILES = (
    ROOT / "src" / "molt" / "frontend" / "lowering" / "midend_canonicalization.py",
    ROOT / "src" / "molt" / "frontend" / "lowering" / "midend_cfg.py",
    ROOT / "src" / "molt" / "frontend" / "lowering" / "midend_dataflow.py",
    ROOT / "src" / "molt" / "frontend" / "lowering" / "midend_pipeline.py",
    ROOT / "src" / "molt" / "frontend" / "lowering" / "midend_policy.py",
)
LEAN_PASSES_DIR = ROOT / "formal" / "lean" / "MoltTIR" / "Passes"


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    pytest.skip(f"Source file not found: {path}")
    return ""


def _read_all(paths: tuple[Path, ...]) -> str:
    return "\n".join(_read(path) for path in paths)


# ── Python effect classification parsing ─────────────────────────────


def _generated_python_effect_classes() -> dict[str, set[str]]:
    from molt.frontend.lowering.op_kinds_generated import FRONTEND_EFFECT_CLASS

    classes: dict[str, set[str]] = {
        "pure": set(),
        "reads_heap": set(),
        "writes_heap": set(),
        "control": set(),
    }
    for kind, effect in FRONTEND_EFFECT_CLASS.items():
        classes[effect].add(kind)
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
    return normalized.upper()


def _lean_unop_python_name(name: str) -> str | None:
    normalized = name[:-1] if name.endswith("_") else name
    if normalized in {"invert", "guard"}:
        return None
    return normalized.upper()


# ── Effect classification tests ──────────────────────────────────────


@pytest.fixture(scope="module")
def python_effect_classes() -> dict[str, set[str]]:
    return _generated_python_effect_classes()


@pytest.fixture(scope="module")
def lean_syntax_text() -> str:
    return _read(SYNTAX_LEAN)


class TestEffectClassification:
    """Effect classifications should be non-empty and well-structured."""

    def test_pure_ops_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["pure"]) > 0, (
            "No pure ops found in FRONTEND_EFFECT_CLASS"
        )

    def test_reads_heap_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["reads_heap"]) > 0, (
            "No reads_heap ops found in FRONTEND_EFFECT_CLASS"
        )

    def test_writes_heap_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["writes_heap"]) > 0, (
            "No writes_heap ops found in FRONTEND_EFFECT_CLASS"
        )

    def test_control_ops_nonempty(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        assert len(python_effect_classes["control"]) > 0, (
            "No control ops found in FRONTEND_EFFECT_CLASS"
        )

    def test_no_overlap_pure_writes(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        overlap = python_effect_classes["pure"] & python_effect_classes["writes_heap"]
        assert not overlap, f"Ops classified as both pure and writes_heap: {overlap}"

    def test_no_overlap_pure_reads(
        self, python_effect_classes: dict[str, set[str]]
    ) -> None:
        overlap = python_effect_classes["pure"] & python_effect_classes["reads_heap"]
        assert not overlap, f"Ops classified as both pure and reads_heap: {overlap}"


class TestFrontendEffectAlignment:
    """Lean BinOp/UnOp names should have frontend pre-specialization effects."""

    def test_lean_binops_have_frontend_effects(
        self,
        lean_syntax_text: str,
        python_effect_classes: dict[str, set[str]],
    ) -> None:
        """Lean BinOp variants map to generated frontend effect classes."""
        lean_binops = _parse_lean_inductive_variants(lean_syntax_text, "BinOp")
        effect_ops = set().union(*python_effect_classes.values())

        for op in lean_binops:
            upper = _lean_binop_python_name(op)
            if upper is None:
                continue
            assert upper in effect_ops, (
                f"Lean BinOp.{op} (Python: {upper}) has no frontend effect class"
            )

    def test_pre_specialization_raising_binops_are_barriers(
        self,
        lean_syntax_text: str,
        python_effect_classes: dict[str, set[str]],
    ) -> None:
        """Typed Lean arithmetic/comparisons are pure, but frontend ops may raise."""
        lean_binops = set(_parse_lean_inductive_variants(lean_syntax_text, "BinOp"))
        barrier_ops = {
            "add",
            "sub",
            "mul",
            "div",
            "floordiv",
            "mod",
            "pow",
            "eq",
            "ne",
            "lt",
            "le",
            "gt",
            "ge",
            "bit_and",
            "bit_or",
            "bit_xor",
            "lshift",
            "rshift",
            "in",
            "not_in",
        }
        for op in barrier_ops & lean_binops:
            upper = _lean_binop_python_name(op)
            assert upper is not None
            assert upper in python_effect_classes["writes_heap"], (
                f"Lean BinOp.{op} frontend op {upper} must be a may-raise barrier"
            )

    def test_frontend_boolean_and_unops_stay_pure(
        self,
        lean_syntax_text: str,
        python_effect_classes: dict[str, set[str]],
    ) -> None:
        lean_binops = set(_parse_lean_inductive_variants(lean_syntax_text, "BinOp"))
        lean_unops = _parse_lean_inductive_variants(lean_syntax_text, "UnOp")
        pure_ops = python_effect_classes["pure"]

        for op in {"and", "or", "is", "is_not"} & lean_binops:
            upper = _lean_binop_python_name(op)
            assert upper in pure_ops, (
                f"Lean BinOp.{op} (Python: {upper}) not in frontend pure ops"
            )
        for op in lean_unops:
            upper = _lean_unop_python_name(op)
            if upper is None:
                continue
            assert upper in pure_ops, (
                f"Lean UnOp.{op} (Python: {upper}) not in frontend pure ops"
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
        assert path.exists(), f"Lean pass file missing: {path}"

    @pytest.mark.parametrize("pass_name,spec", list(LEAN_PASS_SPECS.items()))
    def test_lean_pass_defines_func(
        self, pass_name: str, spec: tuple[str, str]
    ) -> None:
        filename, func_name = spec
        path = LEAN_PASSES_DIR / filename
        if not path.exists():
            pytest.skip(f"Lean pass file not found: {path}")
        text = path.read_text(errors="replace")
        assert func_name in text, f"Function {func_name} not found in {path}"

    def test_python_mentions_pass_concepts(self) -> None:
        """Python frontend should reference the same pass concepts as Lean."""
        py_text = _read_all(FRONTEND_MIDEND_PY_FILES)

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
            found = any(ind.lower() in py_text.lower() for ind in indicators)
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
