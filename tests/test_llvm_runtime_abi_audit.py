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
    from molt._wasm_abi_generated import runtime_execution_abi

    exports = AUDIT.runtime_exports(AUDIT.runtime_src_roots(ROOT))
    for logical in ("molt_statistics_mean", "molt_statistics_stdev"):
        execution = runtime_execution_abi(logical)
        assert execution is not None
        source = exports[execution.physical_symbol].source.replace("\\", "/")
        assert source.endswith(
            "runtime/molt-runtime-math/src/attached_exports_generated/math/statistics_tail.rs"
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


def test_physical_context_and_boundary_contracts_are_distinct() -> None:
    from molt._wasm_abi_generated import runtime_execution_abi

    facts = {
        ("molt_add", 2): AUDIT.AbiFact("molt_add", 2, "I64", ("I64", "I64")),
        ("molt_runtime_init", 0): AUDIT.AbiFact("molt_runtime_init", 0, "Void", ()),
    }
    physical = AUDIT.physical_runtime_import_facts(facts)
    attached = runtime_execution_abi("molt_add")
    assert attached is not None
    assert physical[(attached.physical_symbol, 3)].param_abis == ("I64", "I64", "I64")
    assert physical[("molt_runtime_init", 0)].param_abis == ()


def test_export_observation_keeps_physical_context_shape(tmp_path: Path) -> None:
    source = tmp_path / "runtime.rs"
    source.write_text(
        "\n".join(
            [
                '#[unsafe(export_name = "attached_probe")]',
                'pub extern "C" fn local_name(context: *mut u8, value: u64) -> u64 { value }',
                'pub extern "C" fn private_callback(value: u64) -> u64 { value }',
                "#[cfg(test)] mod tests {",
                '#[unsafe(export_name = "attached_probe")]',
                'pub extern "C" fn fixture(value: u64) -> u64 { value }',
                "}",
            ]
        ),
        encoding="utf-8",
    )
    exports = AUDIT.runtime_exports(tmp_path)
    assert set(exports) == {"attached_probe"}
    facts = {
        ("attached_probe", 2): AUDIT.AbiFact("attached_probe", 2, "I64", ("I64", "I64"))
    }
    issues = AUDIT.validate_classified_facts(exports, facts)
    assert [issue.problem for issue in issues] == ["param-mismatch"]
    assert issues[0].actual == "Ptr,I64"


def test_stale_logical_export_does_not_satisfy_attached_declaration(
    tmp_path: Path,
) -> None:
    source = tmp_path / "runtime.rs"
    source.write_text(
        '#[unsafe(no_mangle)] pub extern "C" fn molt_add(a: u64, b: u64) -> u64 { a + b }',
        encoding="utf-8",
    )
    facts = {("molt_add", 2): AUDIT.AbiFact("molt_add", 2, "I64", ("I64", "I64"))}
    issues = AUDIT.validate_classified_facts(
        AUDIT.runtime_exports(tmp_path), AUDIT.physical_runtime_import_facts(facts)
    )
    assert [issue.problem for issue in issues] == ["missing-runtime-export"]
