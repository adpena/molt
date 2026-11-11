"""Validate representation records from the Rust pass-delta boundary."""

from __future__ import annotations

from tools.translation_validator import TranslationValidator


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
