from __future__ import annotations

from pathlib import Path

from tools import llvm_runtime_abi_audit as AUDIT

ROOT = Path(__file__).resolve().parents[1]


def test_llvm_runtime_abi_audit_passes_current_repo() -> None:
    result = AUDIT.run_audit(ROOT)

    assert result.ok is True, AUDIT.format_report(result)
    assert result.duplicate_facts == ()
    assert result.classified_fact_issues == ()
    assert result.boxed_fact_issues == ()


def test_boxed_projection_does_not_infer_semantics_from_machine_facts() -> None:
    boxed = AUDIT.runtime_boxed_abi_facts()
    machine, duplicates = AUDIT.runtime_import_abi_facts()
    assert not duplicates
    for symbol in (
        "molt_int_from_i64",
        "molt_int_as_i64",
        "molt_is_truthy",
        "molt_obj_get_state",
    ):
        assert (symbol, 1) in machine
        assert (symbol, 1) not in boxed
    assert ("molt_object_field_get_ptr", 2) in machine
    assert ("molt_object_field_get_ptr", 2) not in boxed
    for symbol, arity in (
        ("molt_cell_new", 1),
        ("molt_math_sin", 1),
        ("molt_statistics_mean", 1),
    ):
        assert boxed[(symbol, arity)] == AUDIT.AbiFact(
            symbol, arity, "I64", ("I64",) * arity
        )
    assert boxed[("molt_spawn", 1)].return_abi == "I64"
    assert boxed[("molt_print_newline", 0)].return_abi == "Void"


def test_dedicated_machine_facts_do_not_mirror_generated_boxed_contracts() -> None:
    conservative, duplicates = AUDIT.runtime_import_abi_facts(include_fixed=False)
    assert not duplicates
    assert not conservative.keys() & AUDIT.runtime_boxed_abi_facts().keys()
    assert ("molt_asyncgen_new", 1) in conservative
    assert ("molt_asyncgen_new", 1) not in AUDIT.runtime_boxed_abi_facts()


def test_runtime_export_scan_includes_runtime_leaf_crates() -> None:
    exports = AUDIT.runtime_exports(AUDIT.runtime_src_roots(ROOT))
    mean_source = exports["molt_statistics_mean"].source.replace("\\", "/")
    stdev_source = exports["molt_statistics_stdev"].source.replace("\\", "/")

    assert mean_source.endswith("runtime/molt-runtime-math/src/math/statistics_tail.rs")
    assert stdev_source.endswith(
        "runtime/molt-runtime-math/src/math/statistics_tail.rs"
    )


def test_runtime_import_abi_facts_reports_duplicate_keys(tmp_path: Path) -> None:
    conservative_imports = tmp_path / "abi_facts.rs"
    fixed_imports = tmp_path / "fixed.rs"
    constants = tmp_path / "runtime_import_abi.rs"
    conservative_imports.write_text(
        "\n".join(
            [
                "pub(crate) const CONSERVATIVE_RUNTIME_IMPORTS: &[RuntimeImportSignature] = &[",
                'runtime_sig("molt_alpha", 1, RuntimeReturnAbi::I64),',
                'runtime_sig("molt_alpha", 1, RuntimeReturnAbi::Void),',
                "];",
            ]
        ),
        encoding="utf-8",
    )
    fixed_imports.write_text(
        "pub(super) const FIXED_RUNTIME_IMPORTS: &[FixedRuntimeImportSpec] = &[];",
        encoding="utf-8",
    )
    constants.write_text("", encoding="utf-8")

    facts, duplicates = AUDIT.runtime_import_abi_facts(
        conservative_imports, fixed_imports, constants
    )

    assert facts[("molt_alpha", 1)] == AUDIT.AbiFact(
        "molt_alpha",
        1,
        "I64",
        ("I64",),
    )
    assert duplicates == (AUDIT.DuplicateAbiFact("molt_alpha", 1, "I64", "Void"),)


def test_runtime_import_abi_facts_reads_custom_fixed_pointer_params(
    tmp_path: Path,
) -> None:
    conservative_imports = tmp_path / "abi_facts.rs"
    fixed_imports = tmp_path / "fixed.rs"
    constants = tmp_path / "runtime_import_abi.rs"
    conservative_imports.write_text(
        "pub(crate) const CONSERVATIVE_RUNTIME_IMPORTS: &[RuntimeImportSignature] = &[];",
        encoding="utf-8",
    )
    fixed_imports.write_text(
        "\n".join(
            [
                "const PTR_PTR: &[FixedRuntimeParamAbi] = &[",
                "FixedRuntimeParamAbi::Ptr,",
                "FixedRuntimeParamAbi::Ptr,",
                "];",
                "pub(super) const FIXED_RUNTIME_IMPORTS: &[FixedRuntimeImportSpec] = &[",
                'custom("molt_ptr_pair", PTR_PTR, FixedRuntimeReturnAbi::I64, ATTR_NONE),',
                'custom("molt_ptr_pair_status", PTR_PTR, FixedRuntimeReturnAbi::I32, ATTR_NONE),',
                "];",
            ]
        ),
        encoding="utf-8",
    )
    constants.write_text("", encoding="utf-8")

    facts, duplicates = AUDIT.runtime_import_abi_facts(
        conservative_imports, fixed_imports, constants
    )

    assert facts[("molt_ptr_pair", 2)] == AUDIT.AbiFact(
        "molt_ptr_pair",
        2,
        "I64",
        ("Ptr", "Ptr"),
    )
    assert facts[("molt_ptr_pair_status", 2)] == AUDIT.AbiFact(
        "molt_ptr_pair_status",
        2,
        "I32",
        ("Ptr", "Ptr"),
    )
    assert duplicates == ()


def test_classified_fact_validation_rejects_export_drift() -> None:
    exports = {
        "molt_alpha": AUDIT.RuntimeSignature(
            "molt_alpha", 2, "()", "runtime.rs", "", ("u64", "u64")
        ),
        "molt_beta": AUDIT.RuntimeSignature(
            "molt_beta", 1, "i32", "runtime.rs", "", ("u64",)
        ),
        "molt_gamma": AUDIT.RuntimeSignature(
            "molt_gamma", 1, "u64", "runtime.rs", "", ("u64",)
        ),
        "molt_delta": AUDIT.RuntimeSignature(
            "molt_delta", 1, "u64", "runtime.rs", "", ("*mut u8",)
        ),
        "molt_epsilon": AUDIT.RuntimeSignature(
            "molt_epsilon", 2, "i32", "runtime.rs", "", ("*mut u8", "*const u8")
        ),
        "molt_zeta": AUDIT.RuntimeSignature(
            "molt_zeta", 1, "f64", "runtime.rs", "", ("u64",)
        ),
    }
    facts = {
        ("molt_alpha", 1): AUDIT.AbiFact("molt_alpha", 1, "I64", ("I64",)),
        ("molt_beta", 1): AUDIT.AbiFact("molt_beta", 1, "I64", ("I64",)),
        ("molt_gamma", 1): AUDIT.AbiFact("molt_gamma", 1, "Void", ("I64",)),
        ("molt_delta", 1): AUDIT.AbiFact("molt_delta", 1, "I64", ("I64",)),
        ("molt_epsilon", 2): AUDIT.AbiFact("molt_epsilon", 2, "I32", ("Ptr", "Ptr")),
        ("molt_zeta", 1): AUDIT.AbiFact("molt_zeta", 1, "I64", ("I64",)),
        ("molt_missing", 1): AUDIT.AbiFact("molt_missing", 1, "I64", ("I64",)),
    }

    assert AUDIT.validate_classified_facts(exports, facts) == (
        AUDIT.ClassifiedFactIssue(
            "arity-mismatch",
            "molt_alpha",
            1,
            "2",
            "()",
            "2",
            "1",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "missing-runtime-export",
            "molt_missing",
            1,
            "<missing>",
            "<missing>",
            "<runtime-export>",
            "I64",
            "<missing>",
        ),
        AUDIT.ClassifiedFactIssue(
            "param-mismatch",
            "molt_alpha",
            1,
            "2",
            "()",
            "I64",
            "I64,I64",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "param-mismatch",
            "molt_delta",
            1,
            "1",
            "u64",
            "I64",
            "Ptr",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "return-mismatch",
            "molt_alpha",
            1,
            "2",
            "()",
            "Void",
            "I64",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "return-mismatch",
            "molt_beta",
            1,
            "1",
            "i32",
            "I32",
            "I64",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "return-mismatch",
            "molt_gamma",
            1,
            "1",
            "u64",
            "I64",
            "Void",
            "runtime.rs",
        ),
        AUDIT.ClassifiedFactIssue(
            "unsupported-return",
            "molt_zeta",
            1,
            "1",
            "f64",
            "<I64-I32-or-Void>",
            "I64",
            "runtime.rs",
        ),
    )


def test_classified_fact_validation_normalizes_return_aliases() -> None:
    exports = {
        "molt_chan_new": AUDIT.RuntimeSignature(
            "molt_chan_new", 1, "ChanHandle", "runtime.rs", "", ("u64",)
        )
    }
    facts = {("molt_chan_new", 1): AUDIT.AbiFact("molt_chan_new", 1, "I64", ("I64",))}

    assert (
        AUDIT.validate_classified_facts(exports, facts, aliases={"ChanHandle": "u64"})
        == ()
    )
