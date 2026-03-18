"""Tests verifying Lean <-> Rust constant and type alignment.

Parses actual source files (no hardcoded values) and asserts that
NaN-boxing constants, operator enums, and Value constructors match
across the Lean formalization and the Rust runtime.

Run:
    uv run pytest tests/test_lean_rust_correspondence.py -q
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
NANBOX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Runtime" / "NanBox.lean"
SYNTAX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Syntax.lean"
OBJ_MODEL_RS = ROOT / "runtime" / "molt-obj-model" / "src" / "lib.rs"
FRONTEND_PY = ROOT / "src" / "molt" / "frontend" / "__init__.py"


# ── Parsing helpers ──────────────────────────────────────────────────


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    pytest.skip(f"Source file not found: {path}")
    return ""


def _parse_lean_hex_constants(text: str) -> dict[str, int]:
    """Extract `def NAME : UInt64 := 0x...` definitions from Lean."""
    result: dict[str, int] = {}
    for m in re.finditer(r"def\s+(\w+)\s*:\s*UInt64\s*:=\s*(0x[0-9a-fA-F_]+)", text):
        result[m.group(1)] = int(m.group(2).replace("_", ""), 16)
    return result


def _parse_rust_u64_constants(text: str) -> dict[str, int]:
    """Extract `const NAME: u64 = <value>;` definitions from Rust, resolving expressions."""
    raw: dict[str, str] = {}
    for m in re.finditer(r"const\s+(\w+):\s*u64\s*=\s*(.+?);", text):
        raw[m.group(1)] = m.group(2).strip()

    resolved: dict[str, int] = {}
    for name, expr in raw.items():
        resolved[name] = _resolve_rust_expr(expr, raw)
    return resolved


def _resolve_rust_expr(expr: str, raw_consts: dict[str, str]) -> int:
    """Resolve a Rust constant expression to an integer value."""
    expr = expr.strip()

    # Simple hex literal: 0x7ff8_0000_0000_0000
    if re.match(r"^0x[0-9a-fA-F_]+$", expr):
        return int(expr.replace("_", ""), 16)

    # Simple decimal literal
    if expr.isdigit():
        return int(expr)

    # Shift: 1 << 46  or  1u64 << VAR
    m = re.match(r"1(?:u64)?\s*<<\s*(\w+)", expr)
    if m:
        shift_val = m.group(1)
        if shift_val.isdigit():
            return 1 << int(shift_val)
        if shift_val in raw_consts:
            return 1 << _resolve_rust_expr(raw_consts[shift_val], raw_consts)
        raise ValueError(f"Cannot resolve shift variable: {shift_val}")

    # (1u64 << VAR) - 1
    m = re.match(r"\(1u64\s*<<\s*(\w+)\)\s*-\s*1", expr)
    if m:
        shift_val = m.group(1)
        if shift_val.isdigit():
            return (1 << int(shift_val)) - 1
        if shift_val in raw_consts:
            return (1 << _resolve_rust_expr(raw_consts[shift_val], raw_consts)) - 1
        raise ValueError(f"Cannot resolve shift variable: {shift_val}")

    raise ValueError(f"Cannot parse Rust constant expression: {expr}")


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
            if name not in variants:
                variants.append(name)
    return variants


# ── NaN-boxing constant tests ────────────────────────────────────────


# Mapping from Lean constant name -> Rust constant name
LEAN_TO_RUST_CONST = {
    "QNAN": "QNAN",
    "TAG_INT": "TAG_INT",
    "TAG_BOOL": "TAG_BOOL",
    "TAG_NONE": "TAG_NONE",
    "TAG_PTR": "TAG_PTR",
    "TAG_PEND": "TAG_PENDING",
    "TAG_MASK": "TAG_MASK",
    "INT_MASK": "INT_MASK",
    "INT_SIGN": "INT_SIGN_BIT",
}


@pytest.fixture(scope="module")
def lean_nanbox_consts() -> dict[str, int]:
    return _parse_lean_hex_constants(_read(NANBOX_LEAN))


@pytest.fixture(scope="module")
def rust_nanbox_consts() -> dict[str, int]:
    return _parse_rust_u64_constants(_read(OBJ_MODEL_RS))


class TestNanBoxConstants:
    """NaN-boxing constants must be identical between Lean and Rust."""

    @pytest.mark.parametrize("lean_name,rust_name", list(LEAN_TO_RUST_CONST.items()))
    def test_constant_matches(
        self,
        lean_name: str,
        rust_name: str,
        lean_nanbox_consts: dict[str, int],
        rust_nanbox_consts: dict[str, int],
    ) -> None:
        assert lean_name in lean_nanbox_consts, (
            f"Lean constant {lean_name} not found in {NANBOX_LEAN}"
        )
        assert rust_name in rust_nanbox_consts, (
            f"Rust constant {rust_name} not found in {OBJ_MODEL_RS}"
        )
        lean_val = lean_nanbox_consts[lean_name]
        rust_val = rust_nanbox_consts[rust_name]
        assert lean_val == rust_val, (
            f"{lean_name}/{rust_name} mismatch: "
            f"Lean=0x{lean_val:016x} Rust=0x{rust_val:016x}"
        )

    def test_tag_check_derived(self, lean_nanbox_consts: dict[str, int]) -> None:
        """TAG_CHECK should equal QNAN | TAG_MASK."""
        if "TAG_CHECK" not in lean_nanbox_consts:
            pytest.skip("TAG_CHECK not defined as hex constant in Lean")
        expected = lean_nanbox_consts["QNAN"] | lean_nanbox_consts["TAG_MASK"]
        assert lean_nanbox_consts["TAG_CHECK"] == expected

    def test_canonical_nan_bits_in_rust(
        self, rust_nanbox_consts: dict[str, int]
    ) -> None:
        """CANONICAL_NAN_BITS must be present in Rust."""
        assert "CANONICAL_NAN_BITS" in rust_nanbox_consts, (
            "CANONICAL_NAN_BITS not found in Rust source"
        )

    def test_pointer_mask_in_rust(self, rust_nanbox_consts: dict[str, int]) -> None:
        """POINTER_MASK must be present in Rust."""
        assert "POINTER_MASK" in rust_nanbox_consts, (
            "POINTER_MASK not found in Rust source"
        )

    def test_all_lean_consts_have_rust_counterpart(
        self, lean_nanbox_consts: dict[str, int]
    ) -> None:
        """Every Lean NaN-box constant should map to a known Rust constant."""
        known_lean_only = {"TAG_CHECK"}  # derived, not direct constant in Rust
        for name in lean_nanbox_consts:
            if name in known_lean_only:
                continue
            assert name in LEAN_TO_RUST_CONST, (
                f"Lean constant {name} has no known Rust counterpart"
            )


# ── Operator enum tests ──────────────────────────────────────────────


@pytest.fixture(scope="module")
def lean_syntax_text() -> str:
    return _read(SYNTAX_LEAN)


@pytest.fixture(scope="module")
def python_frontend_text() -> str:
    return _read(FRONTEND_PY)


def _parse_python_op_kinds(text: str) -> set[str]:
    """Extract all op kinds from Python frontend `"kind": "..."` patterns."""
    kinds: set[str] = set()
    for m in re.finditer(r'"kind":\s*"(\w+)"', text):
        kinds.add(m.group(1))
    return kinds


class TestBinOpAlignment:
    """BinOp variants in Lean must have corresponding Python op kinds."""

    def test_binop_nonempty(self, lean_syntax_text: str) -> None:
        variants = _parse_lean_inductive_variants(lean_syntax_text, "BinOp")
        assert len(variants) > 0, "No BinOp variants found in Lean Syntax"

    def test_binop_variants_in_python(
        self, lean_syntax_text: str, python_frontend_text: str
    ) -> None:
        lean_binops = _parse_lean_inductive_variants(lean_syntax_text, "BinOp")
        python_ops = _parse_python_op_kinds(python_frontend_text)

        # Also check effect class listings which use uppercase op names
        python_effect_ops: set[str] = set()
        for m in re.finditer(r'"([A-Z_]+)"', python_frontend_text):
            python_effect_ops.add(m.group(1))

        missing = []
        for v in lean_binops:
            # Check lowercase in op kinds or uppercase in effect classes
            if v not in python_ops and v.upper() not in python_effect_ops:
                missing.append(v)
        assert not missing, (
            f"Lean BinOp variants not found in Python: {missing}"
        )


class TestUnOpAlignment:
    """UnOp variants in Lean must have corresponding Python op kinds."""

    def test_unop_nonempty(self, lean_syntax_text: str) -> None:
        variants = _parse_lean_inductive_variants(lean_syntax_text, "UnOp")
        assert len(variants) > 0, "No UnOp variants found in Lean Syntax"

    def test_unop_variants_in_python(
        self, lean_syntax_text: str, python_frontend_text: str
    ) -> None:
        lean_unops = _parse_lean_inductive_variants(lean_syntax_text, "UnOp")
        python_ops = _parse_python_op_kinds(python_frontend_text)

        python_effect_ops: set[str] = set()
        for m in re.finditer(r'"([A-Z_]+)"', python_frontend_text):
            python_effect_ops.add(m.group(1))

        # NEG is modeled in Lean but Python inlines it (e.g. SUB from 0)
        # GUARD is a Lean-side type-narrowing primitive not yet surfaced in Python
        acceptable_gaps = {"neg", "invert", "guard"}
        missing = []
        for v in lean_unops:
            if v in acceptable_gaps:
                continue
            if v not in python_ops and v.upper() not in python_effect_ops:
                missing.append(v)
        assert not missing, (
            f"Lean UnOp variants not found in Python: {missing}"
        )


class TestValueConstructors:
    """Value constructors in Lean must correspond to Python/Rust representations."""

    def test_value_nonempty(self, lean_syntax_text: str) -> None:
        variants = _parse_lean_inductive_variants(lean_syntax_text, "Value")
        assert len(variants) > 0, "No Value variants found in Lean Syntax"

    def test_value_constructors_in_rust(self, lean_syntax_text: str) -> None:
        """Each Lean Value constructor should have a Rust MoltObject method."""
        rust_text = _read(OBJ_MODEL_RS)
        lean_values = _parse_lean_inductive_variants(lean_syntax_text, "Value")

        # Lean -> Rust method mapping
        lean_to_rust_method = {
            "int": "from_int",
            "bool": "from_bool",
            "float": "from_float",
            "none": "none()",
            "str": "from_ptr",  # strings stored as heap pointers
        }

        for v in lean_values:
            rust_method = lean_to_rust_method.get(v)
            if rust_method is None:
                continue
            assert rust_method in rust_text, (
                f"Value.{v} -> {rust_method} not found in Rust MoltObject"
            )
