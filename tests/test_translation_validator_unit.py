"""Unit tests for each TranslationValidator check and per-pass validator.

Tests operate on hand-crafted op lists (dicts) so they are fast and
independent of the Molt compiler.
"""

from __future__ import annotations

from pathlib import Path

import pytest

# Adjust sys.path so we can import from tools/
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))

from translation_validator import (
    TranslationValidator,
)

TV_DUMPS = Path(__file__).resolve().parent / "fixtures" / "tv_dumps"

# ---------------------------------------------------------------------------
# Helpers to build op dicts concisely
# ---------------------------------------------------------------------------


def _op(
    kind: str, args=None, name: str = "none", type_hint: str = "Unknown", metadata=None
):
    result = {"name": name, "type_hint": type_hint}
    return {"kind": kind, "args": args or [], "result": result, "metadata": metadata}


def _val(name: str, type_hint: str = "int"):
    return {"name": name, "type_hint": type_hint}


# ---------------------------------------------------------------------------
# check_op_count_monotonic
# ---------------------------------------------------------------------------


class TestCheckOpCountMonotonic:
    def test_equal_counts_passes(self):
        before = [_op("CONST", [1], "v0")]
        after = [_op("CONST", [1], "v0")]
        r = TranslationValidator.check_op_count_monotonic(before, after)
        assert r.passed

    def test_decreasing_counts_passes(self):
        before = [_op("CONST", [1], "v0"), _op("CONST", [2], "v1")]
        after = [_op("CONST", [1], "v0")]
        r = TranslationValidator.check_op_count_monotonic(before, after)
        assert r.passed

    def test_increasing_counts_fails(self):
        before = [_op("CONST", [1], "v0")]
        after = [_op("CONST", [1], "v0"), _op("CONST", [2], "v1")]
        r = TranslationValidator.check_op_count_monotonic(before, after)
        assert not r.passed
        assert "+1" in r.detail


# ---------------------------------------------------------------------------
# check_no_new_variables
# ---------------------------------------------------------------------------


class TestCheckNoNewVariables:
    def test_no_new_vars_passes(self):
        before = [_op("CONST", [1], "v0"), _op("CONST", [2], "v1")]
        after = [_op("CONST", [1], "v0")]
        r = TranslationValidator.check_no_new_variables(before, after)
        assert r.passed

    def test_new_var_fails(self):
        before = [_op("CONST", [1], "v0")]
        after = [_op("CONST", [1], "v0"), _op("CONST", [2], "v_new")]
        r = TranslationValidator.check_no_new_variables(before, after)
        assert not r.passed
        assert "v_new" in r.detail

    def test_phi_new_var_allowed(self):
        """PHI nodes are allowed to introduce new variable names."""
        before = [_op("CONST", [1], "v0")]
        after = [_op("CONST", [1], "v0"), _op("PHI", [], "phi_0")]
        r = TranslationValidator.check_no_new_variables(before, after)
        assert r.passed


# ---------------------------------------------------------------------------
# check_control_flow_structure
# ---------------------------------------------------------------------------


class TestCheckControlFlowStructure:
    def test_balanced_nesting_passes(self):
        before = [_op("IF"), _op("CONST", [1], "v0"), _op("END_IF")]
        after = [_op("IF"), _op("CONST", [1], "v0"), _op("END_IF")]
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert r.passed

    def test_unmatched_close_fails(self):
        before = []
        after = [_op("END_IF")]
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert not r.passed
        assert "unmatched" in r.detail

    def test_unclosed_open_fails(self):
        before = []
        after = [_op("IF"), _op("CONST", [1], "v0")]
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert not r.passed
        assert "unclosed" in r.detail

    def test_loop_balanced_passes(self):
        before = []
        after = [_op("LOOP_START"), _op("CONST", [1], "v0"), _op("LOOP_END")]
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert r.passed

    def test_mismatched_nesting_fails(self):
        before = []
        after = [_op("IF"), _op("LOOP_END")]
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert not r.passed
        assert "mismatched" in r.detail

    def test_empty_after_passes(self):
        before = [_op("IF"), _op("END_IF")]
        after = []
        r = TranslationValidator.check_control_flow_structure(before, after)
        assert r.passed


# ---------------------------------------------------------------------------
# check_pure_op_preservation
# ---------------------------------------------------------------------------


class TestCheckPureOpPreservation:
    def test_no_removals_passes(self):
        ops = [_op("CONST", [1], "v0"), _op("ADD", [_val("v0")], "v1")]
        r = TranslationValidator.check_pure_op_preservation(ops, ops)
        assert r.passed

    def test_removal_of_unused_pure_passes(self):
        before = [
            _op("CONST", [1], "v0"),
            _op("CONST", [2], "v1"),
            _op("RETURN", [_val("v0")]),
        ]
        after = [
            _op("CONST", [1], "v0"),
            _op("RETURN", [_val("v0")]),
        ]
        r = TranslationValidator.check_pure_op_preservation(before, after)
        assert r.passed

    def test_dangling_use_after_removal_fails(self):
        before = [
            _op("CONST", [1], "v0"),
            _op("ADD", [_val("v0")], "v1"),
            _op("RETURN", [_val("v1")]),
        ]
        # v0 removed but v0 is still used in ADD's args in after
        after = [
            _op("ADD", [_val("v0")], "v1"),
            _op("RETURN", [_val("v1")]),
        ]
        r = TranslationValidator.check_pure_op_preservation(before, after)
        assert not r.passed
        assert "v0" in r.detail


# ---------------------------------------------------------------------------
# validate_dce
# ---------------------------------------------------------------------------


class TestValidateDCE:
    def setup_method(self):
        self.tv = TranslationValidator()

    def test_valid_dce(self):
        before = [
            _op("CONST", [10], "v0"),
            _op("CONST", [20], "v1"),
            _op("ADD", [_val("v0"), _val("v1")], "v2"),
            _op("CONST", [99], "v3"),  # dead -- unused
            _op("MUL", [_val("v3"), _val("v0")], "v4"),  # dead -- unused
            _op("RETURN", [_val("v2")]),
        ]
        after = [
            _op("CONST", [10], "v0"),
            _op("CONST", [20], "v1"),
            _op("ADD", [_val("v0"), _val("v1")], "v2"),
            _op("RETURN", [_val("v2")]),
        ]
        report = self.tv.validate_dce(before, after)
        assert report.passed

    def test_invalid_dce_removes_used_impure_op(self):
        before = [
            _op("CONST", [1], "v0"),
            _op("CALL", [_val("v0")], "v1"),  # impure, result used
            _op("RETURN", [_val("v1")]),
        ]
        after = [
            _op("CONST", [1], "v0"),
            _op("RETURN", [_val("v1")]),  # v1 used but its def was removed
        ]
        report = self.tv.validate_dce(before, after)
        assert not report.passed


# ---------------------------------------------------------------------------
# check_repr_lattice_monotonic
# ---------------------------------------------------------------------------


class TestCheckReprLatticeMonotonic:
    def test_raw_promotion_passes(self):
        before = {"value_reprs": {"0": "MaybeBigInt", "1": "DynBox"}}
        after = {"value_reprs": {"0": "RawI64Safe", "1": "RawI64FullDeopt"}}

        r = TranslationValidator.check_repr_lattice_monotonic(before, after)

        assert r.passed

    def test_synthetic_repr_downgrade_fails(self):
        before = {"value_reprs": {"7": "RawI64Safe"}}
        after = {"value_reprs": {"7": "MaybeBigInt"}}

        r = TranslationValidator.check_repr_lattice_monotonic(before, after)

        assert not r.passed
        assert r.check == "repr_lattice_monotonic"
        assert "7: RawI64Safe->MaybeBigInt" in r.detail

    def test_missing_value_reprs_fails_closed(self):
        before = {"repr_counts": {"RawI64Safe": 1}}
        after = {"value_reprs": {"7": "RawI64Safe"}}

        r = TranslationValidator.check_repr_lattice_monotonic(before, after)

        assert not r.passed
        assert "before.value_reprs" in r.detail

    def test_pass_delta_record_runs_repr_gate(self):
        report = self.tv.validate_pass_delta_record(
            {
                "function": "hot_loop",
                "pass": "bce",
                "before": {"value_reprs": {"3": "RawI64FullDeopt"}},
                "after": {"value_reprs": {"3": "RawI64Safe"}},
            }
        )

        assert not report.passed
        assert report.function == "hot_loop"
        assert report.pass_name == "bce"
        assert report.checks[0].check == "repr_lattice_monotonic"

    def test_pass_delta_record_consumes_rust_translation_verdict(self):
        report = self.tv.validate_pass_delta_record(
            {
                "function": "hot_loop",
                "pass": "copy_prop",
                "before": {"value_reprs": {"3": "RawI64Safe"}},
                "after": {"value_reprs": {"3": "RawI64Safe"}},
                "translation_validation": {
                    "check": "repr_lattice_monotonic",
                    "passed": False,
                    "violations": [
                        {
                            "value_id": "3",
                            "before": "RawI64Safe",
                            "after": "MaybeBigInt",
                            "reason": "moves downward in repr proof order",
                        }
                    ],
                },
            }
        )

        assert not report.passed
        assert report.checks[0].check == "repr_lattice_monotonic"
        assert "3: RawI64Safe->MaybeBigInt" in report.checks[0].detail

    def setup_method(self):
        self.tv = TranslationValidator()


# ---------------------------------------------------------------------------
# validate_dump_directory (integration with fixtures)
# ---------------------------------------------------------------------------


class TestValidateDumpDirectory:
    def setup_method(self):
        self.tv = TranslationValidator()

    def test_fixture_directory_all_pass(self):
        """All hand-crafted fixtures should validate cleanly."""
        reports = self.tv.validate_dump_directory(str(TV_DUMPS))
        assert len(reports) > 0, "Expected at least one function report"
        for fr in reports:
            for pr in fr.passes:
                if not pr.passed:
                    details = "; ".join(
                        f"{c.check}: {c.detail}" for c in pr.checks if not c.passed
                    )
                    pytest.fail(f"{fr.function}/{pr.pass_name} failed: {details}")

    def test_nonexistent_directory_returns_empty(self):
        reports = self.tv.validate_dump_directory("/nonexistent/path")
        assert reports == []
