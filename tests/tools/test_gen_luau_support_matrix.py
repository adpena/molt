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
            "matmul" => {
                self.emit_line("local out = nil -- [unsupported op: matmul]");
            }
            "call_async" | "spawn" => {
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
            "call_internal" => {
                let mapped = match name {
                    "molt_abs_builtin" => "function(a) return math.abs(a[1]) end",
                    _ => "nil",
                };
                self.emit_line(mapped);
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

    assert rows["add"].status == "implemented-exact"
    assert rows["inplace_add"].status == "implemented-exact"
    assert rows["matmul"].status == "compile-error"
    assert rows["call_async"].status == "not-admitted"
    assert rows["spawn"].status == "not-admitted"
    assert rows["br_if"].status == "implemented-exact"
    assert "missing target labels fail closed" in rows["br_if"].note
    assert rows["bridge_unavailable"].status == "implemented-exact"
    assert rows["object_set_class"].status == "implemented-exact"
    assert rows["class_set_layout_version"].status == "implemented-target-limited"
    assert rows["class_apply_set_name"].status == "not-admitted"
    assert rows["class_layout_version"].status == "implemented-target-limited"
    assert rows["class_merge_layout"].status == "implemented-target-limited"
    assert rows["call_internal"].status == "implemented-exact"
    assert "molt_abs_builtin" not in rows
    assert rows["vec_sum_*"].status == "implemented-exact"
    assert rows["vec_prod_*"].status == "implemented-exact"
    assert rows["is"].status == "implemented-target-limited"
    assert rows["getargv"].status == "implemented-target-limited"


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
