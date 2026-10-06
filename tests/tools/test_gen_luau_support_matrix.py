from __future__ import annotations

import importlib.util
import sys
import uuid
from pathlib import Path
from types import ModuleType
import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "tools" / "gen_luau_support_matrix.py"


def _load_module() -> ModuleType:
    name = f"gen_luau_support_matrix_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(name, MODULE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_classifies_luau_op_arms_from_fixture() -> None:
    mod = _load_module()
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            "add" | "inplace_add" => {
                self.emit_line("local out = a + b");
            }
            "unsupported_fixture_op" => {
                self.emit_line("local out = nil -- [unsupported op: unsupported_fixture_op]");
            }
            "call_async" => {
                self.emit_line("local out = poll_target(payload)");
            }
            "state_transition" => {
                self.emit_line("local out = nil -- [async: state_transition]");
            }
            "br_if" => {
                self.emit_line("if cond then goto label_1 end");
                self.emit_line("error(\"[unsupported op: br_if cond missing target label]\")");
            }
            "bridge_unavailable" => {
                self.emit_line("local out: any = error({__type=\"RuntimeError\", __msg=\"Molt bridge unavailable: \" .. tostring(msg)})");
            }
            "object_set_class" => {
                self.emit_line("setmetatable(obj, class)");
            }
            "class_layout_version" => {
                self.emit_line("local out = if type(cls.__molt_layout_version) == \"number\" then cls.__molt_layout_version else 0");
            }
            "class_set_layout_version" => {
                self.emit_line("cls.__molt_layout_version = version");
            }
            "class_merge_layout" => {
                self.emit_line("cls.__molt_layout_size__ = size");
            }
            "class_apply_set_name" => {
                self.emit_line(&format!("-- [class op: {}]", op.kind));
            }
            "classmethod_new" => {
                self.emit_line("local out = {__molt_descriptor_kind=\"classmethod\", __func=f}");
            }
            "staticmethod_new" => {
                self.emit_line("local out = {__molt_descriptor_kind=\"staticmethod\", __func=f}");
            }
            "property_new" => {
                self.emit_line("local out = {__molt_descriptor_kind=\"property\", __get=g}");
            }
            "call_internal" => {
                let mapped = match name {
                    "molt_abs_builtin" => "function(a) return math.abs(a[1]) end",
                    _ => "nil",
                };
                self.emit_line(mapped);
            }
            "call_method" => {
                self.emit_line("local out; do local __method = molt_get_attr(obj, \"name\"); out = __method() end");
            }
            "get_attr_generic_obj" | "set_attr_generic_obj" | "del_attr_generic_obj" => {
                self.emit_line("molt_get_attr(obj, \"name\")");
            }
            "has_attr_name" => {
                self.emit_line("local out = molt_has_attr(obj, name)");
            }
            "isinstance" => {
                self.emit_line("local out = molt_isinstance(obj, cls)");
            }
            "issubclass" => {
                self.emit_line("local out = molt_issubclass(sub, cls)");
            }
            kind if kind.starts_with("vec_sum_")
                || kind.starts_with("vec_prod_") =>
            {
                self.emit_line("local out = {acc, false} -- [vectorized: kind]");
            }
            "is" => {
                // Python non-None identity maps to equality in Luau.
                self.emit_line("local out = (a == b)");
            }
            "getargv" => {
                self.emit_line("local out = {}");
            }
        }
    }
    """

    rows = {row.op: row for row in mod.collect_rows_from_text(source)}

    assert rows["add"].status == "implemented-target-limited"
    assert rows["inplace_add"].status == "implemented-target-limited"
    assert rows["unsupported_fixture_op"].status == "not-admitted"
    assert rows["call_async"].status == "not-admitted"
    assert rows["state_transition"].status == "not-admitted"
    assert rows["br_if"].status == "compile-error"
    assert "Checked Luau emission rejects" in rows["br_if"].note
    assert rows["bridge_unavailable"].status == "not-admitted"
    assert rows["object_set_class"].status == "implemented-exact"
    assert rows["class_set_layout_version"].status == "implemented-exact"
    assert rows["class_apply_set_name"].status == "not-admitted"
    assert rows["class_layout_version"].status == "implemented-exact"
    assert rows["class_merge_layout"].status == "implemented-exact"
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
    assert rows["vec_sum_*"].status == "not-admitted"
    assert rows["vec_prod_*"].status == "not-admitted"
    assert rows["is"].status == "implemented-exact"
    assert rows["getargv"].status == "not-admitted"


def test_check_mode_detects_stale_generated_output(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture
) -> None:
    mod = _load_module()
    source = tmp_path / "luau.rs"
    output = tmp_path / "luau_support_matrix.generated.md"
    source.write_text(
        """
        fn emit_op(&mut self, op: &OpIR) {
            match op.kind.as_str() {
                "add" => { self.emit_line("local out = a + b"); }
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
    mod = _load_module()
    source_dir = tmp_path / "luau"
    source_dir.mkdir()
    (source_dir / "op_alpha.rs").write_text(
        """
        impl LuauBackend {
            pub(super) fn emit_alpha_op(&mut self, op: &OpIR) -> bool {
                match op.kind.as_str() {
                    "const_none" => { self.emit_line("local out = nil"); }
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
                match op.kind.as_str() {
                    "const_bool" => {
                        self.emit_line("local out = nil -- [unsupported op: const_bool]");
                    }
                    kind if kind.starts_with("vec_fixture_") => {
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
                match op.kind.as_str() {
                    "ignored_test_only" => { self.emit_line("local out = 1"); }
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
    assert "`vec_fixture_*` | `not-admitted`" in output
    assert "ignored_test_only" not in output


def test_execution_frames_are_implemented_but_introspection_is_not_admitted() -> None:
    mod = _load_module()
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            "trace_enter_slot" => { self.emit_line("molt_frame_enter(code)"); }
            "trace_exit" => { self.emit_line("molt_frame_exit(cookie)"); }
            "line" => { self.emit_line("molt_frame_set_line(7)"); }
            "frame_locals_set" => { self.emit_line("molt_frame_locals_set(locals)"); }
            "getframe" => {
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
    mod = _load_module()
    row = mod._classify(
        "callargs_expand_kwstar", "molt_callargs_expand_kwstar(builder, value)"
    )
    assert row.status == "implemented-target-limited"
    assert "keys/getitem" in row.note
    assert "callargs_expand_kwstar" in mod._kind_set(
        "simpleir_luau_ordered_mapping_kinds"
    )


def test_pending_call_poll_requirement_cannot_be_reported_as_exact() -> None:
    mod = _load_module()
    source = r"""
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            "state_yield" => {
                self.emit_line("return yielded");
            }
            "exception_finally_pending_observer" => {
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
    mod = _load_module()
    body = f'"{kind}" => {{ self.{helper}(op, "reason"); }}'
    assert mod._classify(kind, body).status == "compile-error"


@pytest.mark.parametrize("kind", ["func_new_closure", "getframe", "state_yield"])
def test_pre_source_rejection_remains_primary_over_source_helper(kind):
    mod = _load_module()
    row = mod._classify(kind, f'"{kind}" => {{ self.emit_unsupported_op(op); }}')
    assert row.status == "not-admitted"
    assert "before source generation" in row.note


@pytest.mark.parametrize(
    "kind", ["const_int", "const_bigint", "add", "callargs_expand_kwstar"]
)
def test_declared_limits_preserve_their_specific_conditional_contract(kind):
    mod = _load_module()
    body = f'"{kind}" => {{ if admitted {{ self.emit_line("value"); }} else {{ self.emit_unsupported_op(op); }} }}'
    row = mod._classify(kind, body)
    assert row.status == "implemented-target-limited"
    assert "branch-admitted" not in row.note


@pytest.mark.parametrize("kind", ["const_float", "builtin_func", "callargs_new", "box"])
def test_conditional_rejection_is_not_universal_rejection_or_exact_support(kind):
    mod = _load_module()
    body = f'"{kind}" => {{ if admitted {{ self.emit_line("value"); }} else {{ self.emit_unsupported_op_with_reason(op, "bad operand"); }} }}'
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
    mod = _load_module()
    body = f'"const_none" => {{ {noncode} self.emit_line("nil"); }}'
    assert mod._unsupported_emission_kind(body) is None
    assert mod._classify("const_none", body).status == "implemented-exact"


def test_supported_branch_before_late_rejection_is_not_universal_rejection():
    mod = _load_module()
    body = '"const_float" => { if valid { self.emit_line("value"); return true; } self.emit_unsupported_op(op); }'
    assert mod._unsupported_emission_kind(body) == "conditional"
    assert mod._classify("const_float", body).status == "implemented-exact"


def test_every_actual_shared_rejection_arm_has_honest_support_status():
    mod = _load_module()
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

    mod = _load_module()
    plan = tomllib.loads(
        (REPO_ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    commands = {command["id"]: command for command in plan["command"]}
    assert (
        "tests/tools/test_gen_luau_support_matrix.py"
        in commands["repository.docs-tests"]["argv"]
    )
    assert commands["repository.docs-tests"]["timeout_seconds"] == 600
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
    mod = _load_module()
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
    assert "simpleir_kind_is_exception_check" in flow
    assert "simpleir_kind_is_structural" in flow
    assert "emit_logical_flow" in function


def test_source_reader_excludes_test_code_even_with_emitter_shaped_text(tmp_path):
    mod = _load_module()
    root = tmp_path / "luau"
    root.mkdir()
    production = root / "op_values.rs"
    production.write_text("production", encoding="utf-8")
    (root / "tests.rs").write_text(
        'fn emit_fake_op() { match op.kind { "const_none" => self.emit_unsupported_op(op), } }',
        encoding="utf-8",
    )
    (root / "tests").mkdir()
    (root / "tests" / "fake.rs").write_text("test code", encoding="utf-8")
    assert mod._source_files(root) == [production]
