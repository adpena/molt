from __future__ import annotations

import importlib.util
import sys
import uuid
from pathlib import Path
from types import ModuleType


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
            "spawn" => {
                self.emit_line("local out = nil -- [async: spawn]");
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
    assert rows["spawn"].status == "not-admitted"
    assert rows["br_if"].status == "not-admitted"
    assert "target contract rejects" in rows["br_if"].note
    assert rows["bridge_unavailable"].status == "not-admitted"
    assert rows["object_set_class"].status == "not-admitted"
    assert rows["class_set_layout_version"].status == "not-admitted"
    assert rows["class_apply_set_name"].status == "not-admitted"
    assert rows["class_layout_version"].status == "not-admitted"
    assert rows["class_merge_layout"].status == "not-admitted"
    assert rows["classmethod_new"].status == "not-admitted"
    assert rows["staticmethod_new"].status == "not-admitted"
    assert rows["property_new"].status == "not-admitted"
    assert rows["call_method"].status == "not-admitted"
    assert rows["get_attr_generic_obj"].status == "not-admitted"
    assert rows["set_attr_generic_obj"].status == "not-admitted"
    assert rows["del_attr_generic_obj"].status == "not-admitted"
    assert rows["has_attr_name"].status == "not-admitted"
    assert rows["call_internal"].status == "not-admitted"
    assert "molt_abs_builtin" not in rows
    assert rows["isinstance"].status == "not-admitted"
    assert rows["issubclass"].status == "not-admitted"
    assert rows["vec_sum_*"].status == "not-admitted"
    assert rows["vec_prod_*"].status == "not-admitted"
    assert rows["is"].status == "not-admitted"
    assert rows["getargv"].status == "not-admitted"


def test_check_mode_detects_stale_generated_output(tmp_path: Path) -> None:
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

    rc = mod.main(["--source", str(source), "--output", str(output), "--check"])

    assert rc == 1


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
    assert "`const_none` | `not-admitted`" in output
    assert "`const_bool` | `compile-error`" in output
    assert "`vec_fixture_*` | `not-admitted`" in output
    assert "ignored_test_only" not in output


def test_observable_frame_state_siblings_are_all_rejected_before_source() -> None:
    mod = _load_module()
    source = r'''
    fn emit_op(&mut self, op: &OpIR) {
        match op.kind.as_str() {
            "frame_locals_set" | "line" | "trace_enter_slot" | "trace_exit" => {
                self.emit_line("-- legacy silent drop");
            }
        }
    }
    '''

    rows = {row.op: row for row in mod.collect_rows_from_text(source)}

    for kind in ("frame_locals_set", "line", "trace_enter_slot", "trace_exit"):
        assert rows[kind].status == "not-admitted"
        assert "target contract rejects" in rows[kind].note
