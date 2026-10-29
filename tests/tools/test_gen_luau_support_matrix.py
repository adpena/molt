from __future__ import annotations

from pathlib import Path
import pytest

from tools import gen_luau_support_matrix as mod


REPO_ROOT = Path(__file__).resolve().parents[2]


def test_classifies_luau_op_arms_from_fixture() -> None:
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind {
            molt_ir::OpKind::Add | molt_ir::OpKind::InplaceAdd => {
                self.emit_line("local out = a + b");
            }
            molt_ir::OpKind::ConstEllipsis => {
                self.emit_line("local out = nil -- [unsupported op: const_ellipsis]");
            }
            molt_ir::OpKind::CallAsync => {
                self.emit_line("local out = poll_target(payload)");
            }
            molt_ir::OpKind::StateTransition => {
                self.emit_line("local out = nil -- [async: state_transition]");
            }
            molt_ir::OpKind::BrIf => {
                self.emit_line("if cond then goto label_1 end");
                self.emit_line("error(\"[unsupported op: br_if cond missing target label]\")");
            }
            molt_ir::OpKind::ExceptionNew => {
                self.emit_line("local out: any = error({__type=\"RuntimeError\", __msg=\"fixture runtime error: \" .. tostring(msg)})");
            }
            molt_ir::OpKind::ClassSetDefinition => {
                self.emit_line("if type(cls) == \"table\" then cls.__molt_class_definition = 1 end");
            }
            molt_ir::OpKind::AllocClass => {
                self.emit_line("local out = setmetatable({}, cls)");
            }
            molt_ir::OpKind::ObjectNewBound => {
                self.emit_line(&format!("-- [class op: {}]", op.kind));
            }
            molt_ir::OpKind::ClassmethodNew => {
                self.emit_line("local out = {__molt_descriptor_kind=\"classmethod\", __func=f}");
            }
            molt_ir::OpKind::StaticmethodNew => {
                self.emit_line("local out = {__molt_descriptor_kind=\"staticmethod\", __func=f}");
            }
            molt_ir::OpKind::PropertyNew => {
                self.emit_line("local out = {__molt_descriptor_kind=\"property\", __get=g}");
            }
            molt_ir::OpKind::CallInternal => {
                let mapped = match name {
                    "molt_abs_builtin" => "function(a) return math.abs(a[1]) end",
                    _ => "nil",
                };
                self.emit_line(mapped);
            }
            molt_ir::OpKind::CallMethod => {
                self.emit_line("local out; do local __method = molt_get_attr(obj, \"name\"); out = __method() end");
            }
            molt_ir::OpKind::GetAttrGenericObj | molt_ir::OpKind::SetAttrGenericObj | molt_ir::OpKind::DelAttrGenericObj => {
                self.emit_line("molt_get_attr(obj, \"name\")");
            }
            molt_ir::OpKind::HasAttrName => {
                self.emit_line("local out = molt_has_attr(obj, name)");
            }
            molt_ir::OpKind::Isinstance => {
                self.emit_line("local out = molt_isinstance(obj, cls)");
            }
            molt_ir::OpKind::Issubclass => {
                self.emit_line("local out = molt_issubclass(sub, cls)");
            }
            molt_ir::OpKind::VecSum | molt_ir::OpKind::VecProd =>
            {
                self.emit_line("local out = {acc, false} -- [vectorized: kind]");
            }
            molt_ir::OpKind::Is => {
                // Python non-None identity maps to equality in Luau.
                self.emit_line("local out = (a == b)");
            }
        }
    }
    """

    rows = {row.op: row for row in mod.collect_rows_from_text(source)}

    assert rows["add"].status == "implemented-target-limited"
    assert rows["inplace_add"].status == "implemented-target-limited"
    assert rows["const_ellipsis"].status == "compile-error"
    assert rows["call_async"].status == "not-admitted"
    assert rows["state_transition"].status == "not-admitted"
    assert rows["br_if"].status == "compile-error"
    assert "Checked Luau emission rejects" in rows["br_if"].note
    assert rows["exception_new"].status == "implemented-exact"
    assert rows["class_set_definition"].status == "implemented-exact"
    assert rows["object_new_bound"].status == "not-admitted"
    assert "semantic stub markers" in rows["object_new_bound"].note
    assert rows["alloc_class"].status == "implemented-exact"
    assert rows["classmethod_new"].status == "not-admitted"
    assert rows["staticmethod_new"].status == "not-admitted"
    assert rows["property_new"].status == "not-admitted"
    assert rows["call_method"].status == "implemented-exact"
    assert rows["get_attr_generic_obj"].status == "implemented-exact"
    assert rows["set_attr_generic_obj"].status == "implemented-exact"
    assert rows["del_attr_generic_obj"].status == "implemented-exact"
    assert rows["has_attr_name"].status == "not-admitted"
    assert rows["call_internal"].status == "implemented-exact"
    assert "molt_abs_builtin" not in rows
    assert rows["isinstance"].status == "not-admitted"
    assert rows["issubclass"].status == "not-admitted"
    assert rows["vec_sum"].status == "not-admitted"
    assert rows["vec_prod"].status == "not-admitted"
    assert rows["is"].status == "implemented-exact"
    assert "getargv" not in rows


def test_check_mode_detects_stale_generated_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    source = tmp_path / "luau.rs"
    output = tmp_path / "luau_support_matrix.generated.md"
    source.write_text(
        """
        fn emit_op(&mut self, op: &OpIR) {
            match op.kind {
                molt_ir::OpKind::Add => { self.emit_line("local out = a + b"); }
            }
        }
        """,
        encoding="utf-8",
    )
    output.write_text("stale\n", encoding="utf-8")
    monkeypatch.setattr(mod, "SOURCE", source)
    monkeypatch.setattr(mod, "OUTPUT", output)

    assert mod.generated_outputs() == {output: mod.build_output(source)}
    assert mod.main(["--check"]) == 1
    assert f"stale: {output}" in capsys.readouterr().err
    assert output.read_text(encoding="utf-8") == "stale\n"
    assert mod.main(["--write"]) == 0
    assert mod.main(["--check"]) == 0
    assert "| `add` | `implemented-target-limited` |" in output.read_text(
        encoding="utf-8"
    )


def test_build_output_aggregates_decomposed_emitter_directory(tmp_path: Path) -> None:
    source_dir = tmp_path / "luau"
    source_dir.mkdir()
    (source_dir / "op_alpha.rs").write_text(
        """
        impl LuauBackend {
            pub(super) fn emit_alpha_op(&mut self, op: &OpIR) -> bool {
                match op.kind {
                    molt_ir::OpKind::ConstNone => { self.emit_line("local out = nil"); }
                    _ => return false,
                }
                true
            }
        }
        """,
        encoding="utf-8",
    )
    (source_dir / "op_beta.rs").write_text(
        """
        impl LuauBackend {
            pub(super) fn emit_beta_op(&mut self, op: &OpIR) -> bool {
                match op.kind {
                    molt_ir::OpKind::ConstBool => {
                        self.emit_line("local out = nil -- [unsupported op: const_bool]");
                    }
                    molt_ir::OpKind::VecSum => {
                        self.emit_line("local out = {acc, false} -- [vectorized: kind]");
                    }
                    _ => return false,
                }
                true
            }
        }
        """,
        encoding="utf-8",
    )
    tests_dir = source_dir / "tests"
    tests_dir.mkdir()
    (tests_dir / "ignored.rs").write_text(
        """
        impl LuauBackend {
            pub(super) fn emit_ignored_op(&mut self, op: &OpIR) -> bool {
                match op.kind {
                    molt_ir::OpKind::ConstEllipsis => { self.emit_line("local out = 1"); }
                    _ => return false,
                }
                true
            }
        }
        """,
        encoding="utf-8",
    )

    output = mod.build_output(source_dir)

    assert "**Source:**" in output
    assert "`const_none` | `implemented-exact`" in output
    assert "`const_bool` | `compile-error`" in output
    assert "`vec_sum` | `not-admitted`" in output
    assert "const_ellipsis" not in output


def test_execution_frames_are_implemented_but_introspection_is_not_admitted() -> None:
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind {
            molt_ir::OpKind::TraceEnterSlot => { self.emit_line("molt_frame_enter(code)"); }
            molt_ir::OpKind::TraceExit => { self.emit_line("molt_frame_exit(cookie)"); }
            molt_ir::OpKind::Line => { self.emit_line("molt_frame_set_line(7)"); }
            molt_ir::OpKind::FrameLocalsSet => { self.emit_line("molt_frame_locals_set(locals)"); }
            molt_ir::OpKind::Getframe => {
                self.emit_line("local frame = molt_getframe(depth)");
            }
        }
    }
    """

    rows = {row.op: row for row in mod.collect_rows_from_text(source)}

    for kind in ("frame_locals_set", "line", "trace_enter_slot", "trace_exit"):
        assert rows[kind].status == "implemented-exact"
    assert rows["getframe"].status == "not-admitted"


def test_ordered_mapping_requirement_cannot_be_reported_as_exact() -> None:
    row = mod._classify(
        "callargs_expand_kwstar", "molt_callargs_expand_kwstar(builder, value)"
    )
    assert row.status == "implemented-target-limited"
    assert "keys/getitem" in row.note
    assert "callargs_expand_kwstar" in mod._kind_set(
        "simpleir_luau_ordered_mapping_kinds"
    )


def test_pending_call_poll_requirement_cannot_be_reported_as_exact() -> None:
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind {
            molt_ir::OpKind::StateYield => {
                self.emit_line("return yielded");
            }
            molt_ir::OpKind::ExceptionFinallyPendingObserver => {
                self.emit_line("local pending = molt_exception_last_pending()");
            }
        }
    }
    """

    rows = {row.op: row for row in mod.collect_rows_from_text(source)}

    assert rows["async_work_poll"].status == "not-admitted"
    assert rows["state_yield"].status == "not-admitted"
    assert (
        rows["exception_finally_pending_observer"].status
        == "implemented-target-limited"
    )
    assert "marked variant" in rows["exception_finally_pending_observer"].note
    assert "target contract rejects" in rows["async_work_poll"].note


@pytest.mark.parametrize(
    "helper", ["emit_unsupported_op", "emit_unsupported_op_with_reason"]
)
@pytest.mark.parametrize("kind", ["const_ellipsis", "const_float", "callargs_new"])
def test_rejection_only_shared_helper_is_never_reported_implemented(helper, kind):
    body = f'{{ self.{helper}(op, "reason"); }}'
    assert mod._classify(kind, body).status == "compile-error"


@pytest.mark.parametrize("kind", ["func_new_closure", "getframe", "state_yield"])
def test_pre_source_rejection_remains_primary_over_source_helper(kind):
    row = mod._classify(kind, "{ self.emit_unsupported_op(op); }")
    assert row.status == "not-admitted"
    assert "before source generation" in row.note


@pytest.mark.parametrize(
    "kind", ["const_int", "const_bigint", "add", "callargs_expand_kwstar"]
)
def test_declared_limits_preserve_their_specific_conditional_contract(kind):
    body = '{ if admitted { self.emit_line("value"); } else { self.emit_unsupported_op(op); } }'
    row = mod._classify(kind, body)
    assert row.status == "implemented-target-limited"
    assert "branch-admitted" not in row.note


@pytest.mark.parametrize("kind", ["const_float", "builtin_func", "callargs_new", "box"])
def test_conditional_rejection_is_not_universal_rejection_or_exact_support(kind):
    body = '{ if admitted { self.emit_line("value"); } else { self.emit_unsupported_op_with_reason(op, "bad operand"); } }'
    assert mod._unsupported_emission_kind(body) == "conditional"
    row = mod._classify(kind, body)
    if kind in mod._PRE_SOURCE_NOT_ADMITTED:
        assert row.status == "not-admitted"
    else:
        assert row.status == (
            "implemented-target-limited"
            if kind == "builtin_func"
            else "implemented-exact"
        )
        if kind == "builtin_func":
            assert "builtin-name whitelist" in row.note


@pytest.mark.parametrize(
    "noncode",
    [
        "// self.emit_unsupported_op(op);\n",
        "/* self.emit_unsupported_op(op); /* nested */ */",
        'let text = r###"self.emit_unsupported_op(op); { }"###;',
        'let text = "self.emit_unsupported_op_with_reason(op, reason)";',
    ],
)
def test_shared_helper_spelling_in_noncode_does_not_lower_support(noncode):
    body = f'molt_ir::OpKind::ConstNone => {{ {noncode} self.emit_line("nil"); }}'
    assert mod._unsupported_emission_kind(body) is None
    assert mod._classify("const_none", body).status == "implemented-exact"


def test_supported_branch_before_late_rejection_is_not_universal_rejection():
    body = '{ if valid { self.emit_line("value"); return true; } self.emit_unsupported_op(op); }'
    assert mod._unsupported_emission_kind(body) == "conditional"
    assert mod._classify("const_float", body).status == "implemented-exact"


def test_every_actual_shared_rejection_arm_has_honest_support_status():
    source = "\n".join(
        path.read_text(encoding="utf-8") for path in mod._source_files(mod.SOURCE)
    )
    seen = set()
    for match in mod._extract_emit_op_matches(source):
        for kinds, body in mod._iter_arms(match):
            rejection = mod._unsupported_emission_kind(body)
            if rejection is None:
                continue
            for kind in kinds:
                seen.add(kind)
                if rejection == "only" or kind == "builtin_func":
                    assert mod._classify(kind, body).status != "implemented-exact", kind
    assert {
        "func_new_closure",
        "const_float",
        "builtin_func",
        "callargs_new",
        "box",
    } <= seen
    ellipsis_arms = [
        body
        for match in mod._extract_emit_op_matches(source)
        if match.function != "emit_op"
        for kinds, body in mod._iter_arms(match)
        if "const_ellipsis" in kinds
    ]
    assert len(ellipsis_arms) == 1
    assert "molt_ellipsis" in ellipsis_arms[0]
    assert mod._unsupported_emission_kind(ellipsis_arms[0]) is None
    assert (
        mod._classify("const_ellipsis", ellipsis_arms[0]).status == "implemented-exact"
    )


def test_luau_classifier_source_family_and_regressions_are_mandatory():
    import tomllib

    plan = tomllib.loads(
        (REPO_ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    commands = {command["id"]: command for command in plan["command"]}
    assert (
        "tests/tools/test_gen_luau_support_matrix.py"
        in commands["repository.docs-tests"]["pytest"]
    )
    assert {
        "tools/gen_luau_support_matrix.py",
        "tests/tools/test_gen_luau_support_matrix.py",
        "src/molt/rust_source_scan.py",
        "runtime/molt-ir/src/tir/op_kinds.toml",
    } <= set(plan["authority_inputs"])
    assert {
        path.relative_to(REPO_ROOT).as_posix() for path in mod._source_files(mod.SOURCE)
    } <= set(plan["authority_inputs"])


def test_actual_raw_fallback_report_explicitly_excludes_structured_cfg_acceptance():
    output = mod.build_output(mod.SOURCE)
    assert "**Scope:** raw OpIR emitter-arm classification" in output
    assert "whole-function acceptance or execution" in output
    assert "`function_body.rs` and `flow_dispatch.rs`" in output
    assert "exception-check edges can bypass raw `emit_op`" in output
    assert "that route requires its own validation" in output
    for kind in ("jump", "goto", "br_if", "branch_false", "check_exception"):
        assert f"| `{kind}` | `compile-error` |" in output
    flow = (mod.SOURCE / "flow_dispatch.rs").read_text(encoding="utf-8")
    function = (mod.SOURCE / "function_body.rs").read_text(encoding="utf-8")
    assert ".is_exception_check()" in flow
    assert ".is_structural()" in flow
    assert "emit_logical_flow" in function


def test_source_reader_excludes_test_code_even_with_emitter_shaped_text(tmp_path):
    root = tmp_path / "luau"
    root.mkdir()
    production = root / "op_values.rs"
    production.write_text("production", encoding="utf-8")
    (root / "tests.rs").write_text(
        "fn emit_fake_op() { match op.kind { molt_ir::OpKind::ConstNone => self.emit_unsupported_op(op), } }",
        encoding="utf-8",
    )
    (root / "tests").mkdir()
    (root / "tests" / "fake.rs").write_text("test code", encoding="utf-8")
    assert mod._source_files(root) == [production]


@pytest.mark.parametrize("fallback_first", [False, True])
def test_exhaustive_refusal_does_not_override_delegated_lowering(fallback_first):
    fallback = """
    fn emit_op(&mut self, op: &OpIR) {
        if self.emit_value_op(op) { return; }
        match op.kind {
            OpKind::ConstNone | OpKind::ConstBool => self.emit_unsupported_op(op),
        }
    }
    """
    family = """
    fn emit_value_op(&mut self, op: &OpIR) -> bool {
        match op.kind {
            OpKind::ConstNone => { self.emit_line("nil"); }
            _ => return false,
        }
        true
    }
    """
    source = fallback + family if fallback_first else family + fallback
    rows = {row.op: row for row in mod.collect_rows_from_text(source)}
    assert rows["const_none"].status == "implemented-exact"
    assert rows["const_bool"].status == "compile-error"
    assert "*" not in rows


def test_generated_return_shape_dispatch_precedes_exhaustive_refusal():
    source = """
    fn emit_op(&mut self, op: &OpIR) {
        if self.emit_return_op(op) { return; }
        match op.kind {
            OpKind::Ret | OpKind::RetVoid | OpKind::Load => self.emit_unsupported_op(op),
        }
    }
    fn emit_return_op(&mut self, op: &OpIR) -> bool {
        match molt_ir::tir::op_kinds_generated::simpleir_return_shape(op.kind) {
            SimpleIrReturnShape::Value => { self.emit_line("return value"); }
            SimpleIrReturnShape::Void => { self.emit_line("return"); }
            SimpleIrReturnShape::NotReturn => return false,
        }
        true
    }
    """
    rows = {row.op: row for row in mod.collect_rows_from_text(source)}
    assert rows["ret"].status == "implemented-exact"
    assert rows["ret_void"].status == "implemented-exact"
    assert rows["load"].status == "compile-error"


@pytest.mark.parametrize(
    "pattern",
    [
        '"const_none"',
        "OpKind::UnknownKind",
        "OpKind::ConstNone if admitted",
        "_",
    ],
)
def test_unclassified_typed_patterns_fail_closed(pattern):
    source = f"""
    fn emit_op(&mut self, op: &OpIR) {{
        match op.kind {{ {pattern} => {{ self.emit_line("nil"); }} }}
    }}
    """
    with pytest.raises(ValueError, match="unclassified"):
        mod.collect_rows_from_text(source)


def test_wire_names_are_read_from_canonical_enum_serde_names(tmp_path, monkeypatch):
    generated = tmp_path / "op_kinds_generated.rs"
    generated.write_text(
        'pub enum OpKind { #[serde(rename = "const_none")] ArbitraryVariant, }',
        encoding="utf-8",
    )
    monkeypatch.setattr(mod, "WIRE_SOURCE", generated)
    source = """
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind { OpKind::ArbitraryVariant => { self.emit_line("nil"); } }
    }
    """
    rows = {row.op: row for row in mod.collect_rows_from_text(source)}
    assert rows["const_none"].status == "implemented-exact"
    assert "arbitrary_variant" not in rows


def test_legacy_string_dispatch_is_not_a_typed_emitter():
    source = 'fn emit_op(&mut self, op: &OpIR) { match op.kind.as_str() { "const_none" => {} } }'
    with pytest.raises(ValueError, match="could not find"):
        mod.collect_rows_from_text(source)
