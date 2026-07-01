"""Tests verifying Lean <-> Rust constant and type alignment.

Parses actual source files (no hardcoded values) and asserts that
NaN-boxing constants, operator enums, and Value constructors match
across the Lean formalization and the Rust runtime.

Run:
    uv run pytest tests/test_lean_rust_correspondence.py -q
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest
from tools.correspondence_sources import (
    parse_lean_hex_constants,
    parse_lean_inductive_variants,
    parse_rust_unsigned_constants,
)
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
NANBOX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Runtime" / "NanBox.lean"
SYNTAX_LEAN = ROOT / "formal" / "lean" / "MoltTIR" / "Syntax.lean"
CODEGEN_ABI_RS = ROOT / "runtime" / "molt-codegen-abi" / "src" / "lib.rs"
OBJ_MODEL_RS = ROOT / "runtime" / "molt-obj-model" / "src" / "lib.rs"
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.frontend.lowering.op_kinds_generated import FRONTEND_EFFECT_CLASS  # noqa: E402


# Parsing helpers


def _read(path: Path) -> str:
    if path.exists():
        return path.read_text(errors="replace")
    pytest.skip(f"Source file not found: {path}")
    return ""


# NaN-boxing constant tests


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
    return parse_lean_hex_constants(_read(NANBOX_LEAN))


@pytest.fixture(scope="module")
def rust_nanbox_consts() -> dict[str, int]:
    return parse_rust_unsigned_constants(_read(CODEGEN_ABI_RS))


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
            f"Rust constant {rust_name} not found in {CODEGEN_ABI_RS}"
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


# Operator enum tests


@pytest.fixture(scope="module")
def lean_syntax_text() -> str:
    return _read(SYNTAX_LEAN)


@pytest.fixture(scope="module")
def python_effect_ops() -> set[str]:
    return set(FRONTEND_EFFECT_CLASS)


def _lean_binop_python_name(name: str) -> str | None:
    normalized = name.rstrip("_")
    if normalized in {"and", "or"}:
        return normalized.upper()
    return normalized.upper()


class TestBinOpAlignment:
    """BinOp variants in Lean must have corresponding Python op kinds."""

    def test_binop_nonempty(self, lean_syntax_text: str) -> None:
        variants = parse_lean_inductive_variants(lean_syntax_text, "BinOp")
        assert len(variants) > 0, "No BinOp variants found in Lean Syntax"

    def test_binop_variants_in_python(
        self, lean_syntax_text: str, python_effect_ops: set[str]
    ) -> None:
        lean_binops = parse_lean_inductive_variants(lean_syntax_text, "BinOp")

        missing = []
        for v in lean_binops:
            upper = _lean_binop_python_name(v)
            if upper is None:
                continue
            if upper not in python_effect_ops:
                missing.append(v)
        assert not missing, f"Lean BinOp variants not found in Python: {missing}"


class TestUnOpAlignment:
    """UnOp variants in Lean must have corresponding Python op kinds."""

    def test_unop_nonempty(self, lean_syntax_text: str) -> None:
        variants = parse_lean_inductive_variants(lean_syntax_text, "UnOp")
        assert len(variants) > 0, "No UnOp variants found in Lean Syntax"

    def test_unop_variants_in_python(
        self, lean_syntax_text: str, python_effect_ops: set[str]
    ) -> None:
        lean_unops = parse_lean_inductive_variants(lean_syntax_text, "UnOp")

        # GUARD is a Lean-side type-narrowing primitive not yet surfaced in Python
        acceptable_gaps = {"invert", "guard"}
        missing = []
        for v in lean_unops:
            if v in acceptable_gaps:
                continue
            if v.upper() not in python_effect_ops:
                missing.append(v)
        assert not missing, f"Lean UnOp variants not found in Python: {missing}"


class TestValueConstructors:
    """Value constructors in Lean must correspond to Python/Rust representations."""

    def test_value_nonempty(self, lean_syntax_text: str) -> None:
        variants = parse_lean_inductive_variants(lean_syntax_text, "Value")
        assert len(variants) > 0, "No Value variants found in Lean Syntax"

    def test_value_constructors_in_rust(self, lean_syntax_text: str) -> None:
        """Each Lean Value constructor should have a Rust MoltObject method."""
        rust_text = _read(OBJ_MODEL_RS)
        lean_values = parse_lean_inductive_variants(lean_syntax_text, "Value")

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
