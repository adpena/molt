import ast
from pathlib import Path

import pytest

from molt.frontend import (
    BoundMethodHint,
    BuiltinClass,
    MoltValue,
    SimpleTIRGenerator,
    compile_to_tir,
)


def _op_kinds(ir: dict, func_name: str = "molt_main") -> list[str]:
    for func in ir["functions"]:
        if func["name"] == func_name:
            return [op["kind"] for op in func["ops"]]
    raise AssertionError(f"Missing function {func_name}")


def _const_values(ir: dict, func_name: str = "molt_main") -> list[int]:
    for func in ir["functions"]:
        if func["name"] == func_name:
            values: list[int] = []
            for op in func["ops"]:
                if op["kind"] != "const":
                    continue
                if "value" in op:
                    values.append(op["value"])
                elif "args" in op:
                    values.append(op["args"][0])
            return values
    raise AssertionError(f"Missing function {func_name}")


def _ops_by_kind(ir: dict, kind: str, func_name: str = "molt_main") -> list[dict]:
    for func in ir["functions"]:
        if func["name"] == func_name:
            return [op for op in func["ops"] if op["kind"] == kind]
    raise AssertionError(f"Missing function {func_name}")


def _ops_by_func_suffix(ir: dict, suffix: str) -> list[dict]:
    for func in ir["functions"]:
        if func["name"].endswith(suffix):
            return func["ops"]
    raise AssertionError(f"Missing function with suffix {suffix}")


def _all_ops(ir: dict) -> list[dict]:
    return [op for func in ir["functions"] for op in func["ops"]]


def _assert_control_values_are_ints(ir: dict) -> None:
    control_kinds = {"label", "state_label", "jump", "check_exception"}
    for func in ir["functions"]:
        for op in func["ops"]:
            if op["kind"] not in control_kinds:
                continue
            assert isinstance(op.get("value"), int), (
                f"{func['name']} emitted non-int control value: {op}"
            )


def test_stored_bound_method_call_without_method_info_compiles():
    src = """
def show(label: str, value: int) -> None:
    print(label, value)

s = "banana"
s_find = s.find
show("str_find", s_find("na"))
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_main")
    assert any(op["kind"] == "call_func" for op in ops)
    assert any(op.get("s_value") == "__main____show" for op in ops)


@pytest.mark.parametrize(
    "module", ["molt_json", "molt_msgpack", "molt_cbor", "molt_buffer"]
)
@pytest.mark.parametrize("from_import", [False, True])
def test_runtime_packages_publish_real_import_bindings(module, from_import):
    member = "new" if module == "molt_buffer" else "parse"
    statement = (
        f"from {module} import {member} as bound"
        if from_import
        else f"import {module} as bound"
    )
    generator = SimpleTIRGenerator()
    generator.visit(ast.parse(statement))
    ops = [op for fn in generator.funcs_map.values() for op in fn["ops"]]
    names = {
        op.result.name for op in ops if op.kind == "CONST_STR" and op.args == ["bound"]
    }
    publications = [
        op
        for op in ops
        if op.kind == "MODULE_SET_ATTR"
        and isinstance(op.args[1], MoltValue)
        and op.args[1].name in names
    ]
    assert len(publications) == 1
    origins = generator.imported_names if from_import else generator.imported_modules
    assert origins["bound"] == module


@pytest.mark.parametrize(
    "module", ["molt_json", "molt_msgpack", "molt_cbor", "molt_buffer"]
)
@pytest.mark.parametrize(
    "binding", ["direct", "alias", "from", "captured", "rebound", "shadowed", "mutated"]
)
def test_runtime_package_calls_use_live_callable(module, binding):
    member = "new" if module == "molt_buffer" else "parse"
    args = "2, 2" if module == "molt_buffer" else "'1'"
    statements = {
        "direct": f"import {module}\nx = {module}.{member}({args})",
        "alias": f"import {module} as api\nx = api.{member}({args})",
        "from": f"from {module} import {member} as invoke\nx = invoke({args})",
        "captured": f"import {module}\ninvoke = {module}.{member}\nx = invoke({args})",
        "rebound": f"import {module}\n{module} = replacement\nx = {module}.{member}({args})",
        "shadowed": f"def run({module}):\n    return {module}.{member}({args})",
        "mutated": f"import {module}\n{module}.{member} = replacement\nx = {module}.{member}({args})",
    }
    ir = compile_to_tir(statements[binding])
    ops = _all_ops(ir)
    assert not any(
        op["kind"] in {"json_parse", "msgpack_parse", "cbor_parse"}
        or op["kind"].startswith("buffer2d_")
        for op in ops
    )
    # Package intrinsics belong inside the package function. The caller must
    # retain the actual callable, including alias/capture/rebinding semantics.
    call_line = len(statements[binding].splitlines())
    captured_binding = binding in {"from", "captured"}
    expected_call = "call_func" if captured_binding else "call_loaded_method"
    calls = [
        op
        for op in ops
        if op["kind"] == expected_call and op.get("source_line") == call_line
    ]
    assert len(calls) == 1
    call = calls[0]
    function_ops = next(fn["ops"] for fn in ir["functions"] if call in fn["ops"])
    if captured_binding:
        assert len(call["args"]) == (3 if module == "molt_buffer" else 2)
        callee = next(op for op in function_ops if op.get("out") == call["args"][0])
        assert callee["kind"] == "module_get_global"
    else:
        # Method-form attribute lookup produces the live callable and its
        # self-or-null word. The source call must consume that exact pair.
        assert len(call["args"]) == (4 if module == "molt_buffer" else 3)
        (load,) = (
            op
            for op in function_ops
            if op["kind"] == "load_method" and op.get("var") == call["args"][0]
        )
        assert load["s_value"] == member
        assert call["args"][1] == load["out"]
        assert function_ops.index(load) < function_ops.index(call)
    assert any(
        op["kind"] in {"module_set_attr", "ret"} and call["out"] in op.get("args", [])
        for op in function_ops
    )


@pytest.mark.parametrize("parse_codec", ["msgpack", "cbor", "json"])
def test_json_package_intrinsic_is_independent_of_transport_codec(parse_codec):
    source = Path(__file__).resolve().parents[1] / "src/molt_json/__init__.py"
    ir = compile_to_tir(source.read_text(encoding="utf-8"), parse_codec=parse_codec)
    symbols = {op.get("s_value") for op in _all_ops(ir)}
    assert "molt_json_parse_scalar_obj" in symbols
    assert "molt_msgpack_parse_scalar_obj" not in symbols
    assert "molt_cbor_parse_scalar_obj" not in symbols


def test_const_bytes_lowering():
    src = "x = b'hi'"
    ir = compile_to_tir(src)
    assert "const_bytes" in _op_kinds(ir)


def test_len_lowering():
    src = "x = len(b'hello')"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert 5 in _const_values(ir)
    assert "len" not in kinds
    assert "const_bytes" not in kinds


@pytest.mark.parametrize(
    "hint", ["Any", BoundMethodHint(BuiltinClass("list"), "append")]
)
def test_positional_dynamic_call_uses_live_callable_without_argument_builder(hint):
    generator = SimpleTIRGenerator()
    generator._populate_sema_state(ast.Module(body=[], type_ignores=[]))
    callee = MoltValue("live_callable", type_hint=hint)
    result = generator._emit_dynamic_call(
        ast.parse("callee(1)", mode="eval").body, callee
    )
    calls = [op for op in generator.current_ops if op.kind == "CALL_FUNC"]
    assert len(calls) == 1
    assert calls[0].args[0] is callee
    assert len(calls[0].args) == 2
    assert calls[0].result is result
    assert result.type_hint == "Any"
    assert not any(
        op.kind.startswith("CALLARGS_") or op.kind == "CALL_INDIRECT"
        for op in generator.current_ops
    )


def test_dynamic_keyword_call_keeps_runtime_argument_binding():
    generator = SimpleTIRGenerator()
    # The argument builder chooses CALL or CALL_FUNCTION_EX from module facts.
    generator._populate_sema_state(ast.Module(body=[], type_ignores=[]))
    callee = MoltValue("live_callable", type_hint="Any")
    result = generator._emit_dynamic_call(
        ast.parse("callee(value=1)", mode="eval").body, callee
    )
    calls = [op for op in generator.current_ops if op.kind == "CALL_INDIRECT"]
    assert len(calls) == 1
    assert calls[0].args[0] is callee
    assert calls[0].result is result
    assert any(op.kind == "CALLARGS_PUSH_KW" for op in generator.current_ops)


def test_slice_lowering():
    src = "x = b'hello'[1:4]\ny = b'world'[:2]\nz = b'world'[2:]"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "slice" in kinds
    assert "const_none" in kinds


def test_slice_object_lowering():
    src = "x = slice(1, 4, 2)\ny = b'hello'[1:4:2]"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "slice_new" in kinds


def test_list_dict_lowering():
    src = "x = [1, 2]\ny = {'a': 3}\nz = x[0]\ny['b'] = 4"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "list_new" in kinds
    assert "dict_new" in kinds
    assert "index" in kinds
    assert "store_index" in kinds


@pytest.mark.parametrize(
    ("receiver", "method", "expected_kinds"),
    [
        ("[1]", "append(2)", ("list_append",)),
        ("[1]", "extend([3])", ("list_extend",)),
        ("[1]", "insert(0, 4)", ("list_insert",)),
        ("[1, 2]", "remove(2)", ("list_remove",)),
        ("[1, 2]", "count(2)", ("list_count",)),
        ("[1, 2]", "index(2)", ("list_index",)),
        ("[1]", "pop()", ("list_pop",)),
        ('{"a": 1}', 'get("a")', ("dict_get",)),
        ('{"a": 1}', 'get("b", 2)', ("dict_get",)),
        ('{"a": 1}', 'pop("a", 3)', ("dict_pop",)),
        ('{"a": 1}', "keys()", ("dict_keys",)),
        ('{"a": 1}', "values()", ("dict_values",)),
        ('{"a": 1}', "items()", ("dict_items",)),
        ("(1, 2, 3)", "count(1)", ("tuple_count",)),
        ("(1, 2, 3)", "index(2)", ("tuple_index",)),
        ('"hello"', 'find("ell")', ("string_find",)),
        ('"hello"', 'split("e")', ("string_split",)),
        ('"hello"', 'replace("e", "x")', ("string_replace",)),
        ('"hello"', 'startswith("he")', ("string_startswith",)),
        ('"hello"', 'endswith("lo")', ("string_endswith",)),
        ('"hello"', 'count("l")', ("string_count",)),
        ('"hello"', 'join(["a", "b"])', ("string_join",)),
        ('b"hello"', 'find(b"lo")', ("bytes_find",)),
        ('b"hello"', 'split(b"e")', ("bytes_split",)),
        ('b"hello"', 'replace(b"e", b"x")', ("bytes_replace",)),
        (
            'bytearray(b"hello")',
            'find(b"lo")',
            ("bytearray_from_obj", "bytearray_find"),
        ),
        (
            'bytearray(b"hello")',
            'split(b"e")',
            ("bytearray_from_obj", "bytearray_split"),
        ),
        (
            'bytearray(b"hello")',
            'replace(b"e", b"x")',
            ("bytearray_from_obj", "bytearray_replace"),
        ),
    ],
)
def test_list_dict_method_lowering(receiver, method, expected_kinds):
    # Each case establishes an exact receiver at its source point. Earlier
    # Python callbacks may replace module bindings and invalidate those facts.
    ir = compile_to_tir(f"value = {receiver}\nresult = value.{method}\n")
    for kind in expected_kinds:
        assert len(_ops_by_kind(ir, kind)) == 1, kind


def test_for_loop_lowering():
    src = """
total = 0
for x in [1, 2]:
    total = total + x
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "loop_start" in kinds
    assert "add" in kinds
    assert "loop_continue" in kinds
    assert "loop_end" in kinds


def test_for_loop_float_reduction_lowering():
    src = """
total = 1.5
values = [1.0, 2.5, 3]
for x in values:
    total += x
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "loop_start" in kinds
    assert "inplace_add" in kinds


def test_for_loop_float_range_reduction_lowering():
    src = """
total = 0.5
for i in range(10):
    total += i
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "loop_index_start" in kinds
    assert "loop_index_next" in kinds
    assert "inplace_add" in kinds


def test_async_control_flow_values_are_ints():
    src = """
async def step():
    return 1

async def work():
    total = 0
    i = 0
    while i < 3:
        total += await step()
        i += 1
    return total
"""
    ir = compile_to_tir(src)
    _assert_control_values_are_ints(ir)


def test_imported_async_sleep_uses_live_callable_and_awaitable_result():
    src = """
from moltlib._concurrency_runtime import molt_async_sleep

async def main():
    await molt_async_sleep(0, None)
"""
    ir = compile_to_tir(src)
    ops = _all_ops(ir)
    assert not any(op["kind"] == "call_async" for op in ops)
    names = {
        op["out"]
        for op in ops
        if op["kind"] == "const_str" and op.get("s_value") == "molt_async_sleep"
    }
    callees = {
        op["out"]
        for op in ops
        if op["kind"] == "module_get_global" and op["args"][1] in names
    }
    calls = [op for op in ops if op["kind"] == "call_func" and op["args"][0] in callees]
    assert len(calls) == 1
    call = calls[0]
    assert len(call["args"]) == 3
    function_ops = next(fn["ops"] for fn in ir["functions"] if call in fn["ops"])
    argument_values = call["args"][1:]
    assert len(argument_values) == 2
    definitions = {op["out"]: op for op in function_ops if "out" in op}
    assert definitions[argument_values[0]]["kind"] == "const"
    assert definitions[argument_values[0]]["value"] == 0
    assert definitions[argument_values[1]]["kind"] == "const_none"
    helpers = {
        op["out"]
        for op in function_ops
        if op["kind"] == "builtin_func" and op.get("s_value") == "molt_get_awaitable"
    }
    acquisitions = [
        op
        for op in function_ops
        if op["kind"] == "call_func"
        and op["args"][0] in helpers
        and op["args"][1:] == [call["out"]]
    ]
    assert len(acquisitions) == 1
    stores = [
        op
        for op in function_ops
        if op["kind"] == "closure_store" and op["args"][-1] == acquisitions[0]["out"]
    ]
    assert len(stores) == 1
    store = stores[0]
    resumes = {
        op["out"]
        for op in function_ops
        if op["kind"] == "closure_load"
        and op["args"] == store["args"][:1]
        and op["value"] == store["value"]
    }
    assert any(
        op["kind"] == "state_transition" and op["args"][0] in resumes
        for op in function_ops
    )


def test_task_target_serialization_preserves_exact_symbols_without_suffix_inference():
    import pytest

    from molt.frontend.lowering.serialization import SerializationMixin

    for kind in ("ALLOC_TASK", "CALL_ASYNC"):
        for target in ("opaque_task_body", "ordinary_poll"):
            assert SerializationMixin._require_async_poll_target(kind, target) == target
        for target in (None, "", 1):
            with pytest.raises(
                ValueError, match="nonempty table-addressable task target"
            ):
                SerializationMixin._require_async_poll_target(kind, target)


def test_for_file_text_loop_item_hint_enables_string_split():
    src = """
with open("sample.txt") as f:
    for line in f:
        parts = line.split("|")
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "string_split" in kinds


def test_for_file_bytes_loop_item_hint_enables_bytes_split():
    src = """
with open("sample.bin", "rb") as f:
    for chunk in f:
        parts = chunk.split(b"|")
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "bytes_split" in kinds


@pytest.mark.parametrize(
    "src",
    [
        r"""
def main() -> None:
    line = "1|NA|2"
    parts = line.split("|")
    first = parts[0]
    second = parts[1]
    print(first, second)
main()
""",
        r"""
def main() -> None:
    data = "header,value\nalpha,beta"
    lines = data.split("\n")
    line = lines[1]
    fields = line.split(",")
    print(fields[0], fields[1])
main()
""",
        r"""
def main() -> None:
    parts = "a-b|c-d".split("|")
    print(parts[0] is parts[0])
main()
""",
        r"""
def main() -> None:
    line = "1|NA|2"
    parts: list[str] = line.split("|")
    first = parts[0]
    second = parts[1]
    print(first, second)
main()
""",
        r"""
def main(flag: bool) -> None:
    line = "1|NA|2|tail"
    parts = line.split("|")
    first = parts[0]
    if flag:
        print(parts[1])
    else:
        print(parts[2])
    print(first, parts[3])
main(True)
""",
        r"""
def main() -> None:
    fields = "alpha|22|eu|tail".split("|")
    total = len(fields[0])
    if fields[2] == "eu":
        total += len(fields[3])
    print(total)
main()
""",
    ],
    ids=[
        "fixed-indexes",
        "nested-split",
        "repeated-index",
        "annotated-list",
        "nested-control",
        "field-consumers",
    ],
)
def test_string_split_frame_binding_keeps_materialized_list(src):
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    splits = [op for op in ops if op["kind"] == "string_split"]
    assert splits
    # A synchronous frame owns this source binding. Scalarizing through its
    # home would replace the list observable through f_locals with a sentinel.
    for split in splits:
        assert any(
            op["kind"] == "frame_home_store" and op["args"] == [split["out"]]
            for op in ops
        )
    assert not any(op["kind"].startswith("string_split_field") for op in ops)
    assert not any(op["kind"] == "string_split_validate" for op in ops)


@pytest.mark.parametrize("index", [0, 1, 3])
def test_string_split_ephemeral_fixed_index_scalarizes(index):
    ir = compile_to_tir(f'x = "alpha|22|eu|tail".split("|")[{index}]')
    ops = _all_ops(ir)
    kinds = [op["kind"] for op in ops]
    assert "string_split" not in kinds
    assert kinds.count("string_split_validate") == 1
    fields = [op for op in ops if op["kind"] == "string_split_field"]
    assert len(fields) == 1
    definitions = {op["out"]: op for op in ops if "out" in op}
    assert definitions[fields[0]["args"][2]]["value"] == index
    assert any(
        op["kind"] == "module_set_attr" and fields[0]["out"] in op.get("args", [])
        for op in ops
    )


def test_string_split_scalarization_keeps_escaping_and_dynamic_uses_on_list_path():
    cases = [
        """
def main(i: int) -> None:
    parts = "1|NA|2".split("|")
    print(parts[i])
""",
        """
def main() -> None:
    parts = "1|NA|2".split("|")
    print(parts[-1])
""",
        """
def main() -> None:
    parts = "1|NA|2".split("|")
    print(len(parts))
""",
        """
def sink(x: list[str]) -> None:
    print(x)
def main() -> None:
    parts = "1|NA|2".split("|")
    sink(parts)
""",
        """
def main() -> list[str]:
    parts = "1|NA|2".split("|")
    return parts
""",
        """
def main() -> None:
    parts = "1|NA|2".split("|", 1)
    print(parts[0])
""",
        """
def main() -> None:
    parts = "1 NA 2".split()
    print(parts[0])
""",
        """
def main() -> None:
    sep = None
    parts = "1 NA 2".split(sep)
    print(parts[0])
""",
        """
def main(sep: str | None) -> None:
    parts = "1 NA 2".split(sep)
    print(parts[0])
""",
        """
def main(flag: bool) -> None:
    parts = ["fallback"]
    if flag:
        parts = "1|NA|2".split("|")
    print(parts[0])
""",
        """
def main() -> None:
    parts = "1|NA|2".split("|")
    while False:
        print("never")
    print(parts[0])
""",
        """
def main() -> None:
    parts = "1|NA|2".split("|")
    print(parts[0])
    if parts[1] == "NA":
        print("hit")
    print(len(parts))
""",
    ]
    for src in cases:
        ir = compile_to_tir(src)
        ops = _ops_by_func_suffix(ir, "molt_user_main")
        kinds = [op["kind"] for op in ops]
        assert "string_split_field" not in kinds
        assert "string_split_validate" not in kinds
        assert "string_split" in kinds or "string_split_max" in kinds


def test_simple_range_listcomp_lowering():
    src = "x = [i for i in range(5)]"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "list_from_range" in kinds


def test_const_int_range_listcomp_lowers_to_flat_list_int():
    src = """
n = 5
x = [1 for _ in range(n)]
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "range_new" in kinds
    assert "len" in kinds
    assert "list_int_new" in kinds
    assert "list_append" not in kinds


def test_bool_range_listcomp_does_not_lower_to_flat_int_list():
    src = "x = [True for _ in range(5)]"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "list_int_new" not in kinds
    assert "list_fill_new" in kinds


def test_const_str_range_listcomp_lowers_to_fill_list():
    src = """
n = 5
x = ["a" for _ in range(n)]
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "range_new" in kinds
    assert "len" in kinds
    assert "list_fill_new" in kinds
    assert "list_append" not in kinds


def test_dataclass_constructor_after_callback_preserves_live_class_dispatch():
    src = """
from dataclasses import dataclass

@dataclass
class Order:
    order_id: int
    region: str
    qty: int
    price: int
    status: str

order = Order(1, "NA", 2, 3, "paid")
print(order.qty)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "dataclass_new_values" not in kinds
    assert "dataclass_new" not in kinds
    assert "dataclass_get" not in kinds
    # The decorator callback invalidates the binding proof. The constructor
    # must call the published class, preserving its current construction policy.
    calls = [
        op
        for op in _all_ops(ir)
        if op["kind"] == "call_func" and op.get("source_line") == 12
    ]
    assert len(calls) == 1
    constructor = calls[0]
    assert len(constructor["args"]) == 6
    definitions = {op["out"]: op for op in _all_ops(ir) if "out" in op}
    callee = definitions[constructor["args"][0]]
    assert callee["kind"] == "module_get_global"
    assert definitions[callee["args"][1]]["s_value"] == "Order"
    assert any(
        op["kind"] == "get_attr_generic_obj" and op.get("s_value") == "qty"
        for op in _all_ops(ir)
    )


def test_annotated_dataclass_list_iteration_preserves_dynamic_fields():
    src = """
from dataclasses import dataclass

@dataclass
class Order:
    qty: int
    status: str

def main() -> None:
    orders: list[Order] = []
    orders.append(Order(2, "paid"))
    total = 0
    for order in orders:
        if order.status == "paid":
            total += order.qty
    print(total)

if __name__ == "__main__":
    main()
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir, "__main____molt_user_main")
    assert "dataclass_get" not in kinds
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    fields = [op.get("s_value") for op in ops if op["kind"] == "get_attr_generic_obj"]
    assert fields.count("status") == 1
    assert fields.count("qty") == 1


def test_decorated_dataclass_dict_update_preserves_attribute_reads():
    src = """
from dataclasses import dataclass

@dataclass
class Order:
    region: str

def main() -> None:
    totals: dict[str, int] = {}
    order = Order("NA")
    revenue = 5
    totals[order.region] = totals.get(order.region, 0) + revenue
    print(totals["NA"])

if __name__ == "__main__":
    main()
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir, "__main____molt_user_main")
    # A decorator's result gives no exact-layout or attribute-purity proof.
    # Reading region twice must retain both possible descriptor callbacks.
    assert "dict_str_int_inc" not in kinds
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    fields = [
        op
        for op in ops
        if op["kind"] == "get_attr_generic_obj" and op.get("s_value") == "region"
    ]
    assert len(fields) == 2
    assert kinds.count("dict_get") == 1
    assert "store_index" in kinds


def test_generic_attribute_dict_increment_keeps_normal_update_path():
    src = """
class Box:
    @property
    def key(self):
        return "NA"

def main() -> None:
    totals: dict[str, int] = {}
    box = Box()
    totals[box.key] = totals.get(box.key, 0) + 1
    print(totals["NA"])

if __name__ == "__main__":
    main()
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir, "__main____molt_user_main")
    assert "dict_str_int_inc" not in kinds
    assert "dict_get" in kinds


def _builtin_guard_arms(ops: list[dict]) -> tuple[list[str], list[str]]:
    """The kinds in the fast and slow arms of the first builtin binding guard."""
    (guard,) = [
        op
        for op in ops
        if op["kind"] == "call" and op.get("s_value") == "molt_guard_builtin_bindings"
    ][:1]
    start = next(
        index
        for index, op in enumerate(ops)
        if op["kind"] == "if" and op.get("args") == [guard["out"]]
    )
    arms: list[list[str]] = [[], []]
    depth = 0
    arm = 0
    for op in ops[start + 1 :]:
        kind = op["kind"]
        if kind == "if":
            depth += 1
        elif kind == "else" and depth == 0:
            arm = 1
            continue
        elif kind == "end_if":
            if depth == 0:
                break
            depth -= 1
        arms[arm].append(kind)
    return arms[0], arms[1]


def test_ord_subscript_lowers_to_fused_ord_at():
    src = """
def parse_digit(text: str, i: int) -> int:
    return ord(text[i]) - 48
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "__main____parse_digit")
    fast, slow = _builtin_guard_arms(ops)
    # A function's `ord` is a live lookup (HF-79). Where the binding guard
    # proves the runtime's builtin, ord(text[i]) fuses to ord_at; otherwise the
    # call is the dynamic lookup and call of whatever `ord` names.
    assert "ord_at" in fast
    assert "ord" not in fast
    assert "index" not in fast
    assert "module_get_global" not in fast
    assert "module_get_global" in slow
    assert "index" in slow
    assert "ord_at" not in slow


def test_ord_slice_keeps_normal_index_then_ord_path():
    src = """
def parse_digit(text: str) -> int:
    return ord(text[0:1])
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir, "__main____parse_digit")
    assert "ord_at" not in kinds
    assert "slice" in kinds
    assert "ord" in kinds


def test_product_loop_fuses_as_a_chunked_prefix_of_the_ordinary_loop():
    src = """
def main():
    nums = [2, 3, 4]
    acc = 1
    for x in nums:
        acc = acc * x
    print(acc, x)

main()
"""
    ir = compile_to_tir(src)
    kinds = [op["kind"] for op in _ops_by_func_suffix(ir, "molt_user_main")]
    assert kinds.count("vec_prod") == 1
    # The iterator is acquired once; the kernel runs chunks inside a loop of its
    # own, whose back edge observes pending work, and the ordinary loop then
    # continues on the same iterator.
    assert kinds.count("iter") == 1
    chunk_start = kinds.index("loop_start")
    chunk_end = kinds.index("loop_end", chunk_start)
    assert kinds.index("iter") < chunk_start < kinds.index("vec_prod") < chunk_end
    assert "loop_start" in kinds[chunk_end:]
    assert "vec_prod" not in kinds[chunk_end:]


def _dict_increment_kinds(body: str) -> list[str]:
    src = "def update(counts, key, step, flag):\n" + body
    return [op["kind"] for op in _ops_by_func_suffix(compile_to_tir(src), "update")]


def test_dict_increment_statement_lowering():
    kinds = _dict_increment_kinds("    counts[key] = counts.get(key, 0) + step\n")
    assert kinds.count("dict_str_int_inc") == 1
    assert "dict_get" not in kinds[: kinds.index("dict_str_int_inc")]


@pytest.mark.parametrize(
    "body",
    [
        # delta + d.get(k, 0) calls delta's __add__ first: another statement.
        "    counts[key] = step + counts.get(key, 0)\n",
        # A read the fused op moves ahead of d.get must not raise.
        "    if flag:\n        total = {}\n    total[key] = total.get(key, 0) + 1\n",
    ],
    ids=["reversed-operands", "maybe-unbound-dict"],
)
def test_dict_increment_statement_declines_unsound_shapes(body):
    assert "dict_str_int_inc" not in _dict_increment_kinds(body)


def _split_count_kinds(src: str, func: str) -> list[str]:
    ir = compile_to_tir(src)
    if func == "molt_main":
        return _op_kinds(ir)
    return [op["kind"] for op in _ops_by_func_suffix(ir, func)]


def test_for_split_whitespace_dict_increment_fused():
    src = """
def count(line):
    counts = {}
    for word in line.split():
        counts[word] = counts.get(word, 0) + 1
    return counts, word
"""
    kinds = _split_count_kinds(src, "count")
    assert kinds.count("string_split_ws_dict_inc") == 1
    assert kinds.index("string_split_ws_dict_inc") < kinds.index("loop_start")


def test_for_split_separator_dict_increment_fused():
    src = """
def count(line):
    counts = {}
    step = 2
    for word in line.split("|"):
        counts[word] = counts.get(word, 0) + step
    return counts
"""
    kinds = _split_count_kinds(src, "count")
    assert kinds.count("string_split_sep_dict_inc") == 1
    assert kinds.index("string_split_sep_dict_inc") < kinds.index("loop_start")


@pytest.mark.parametrize(
    ("src", "func"),
    [
        # A module's loop target is a module global: one final store cannot
        # stand for a store per word.
        (
            """
counts = {}
line = "a b a"
for word in line.split():
    counts[word] = counts.get(word, 0) + 1
""",
            "molt_main",
        ),
        (
            """
def count(line):
    counts = {}
    for word in line.split():
        counts[word] = 1 + counts.get(word, 0)
    return counts
""",
            "count",
        ),
        (
            """
def count(line, flag):
    if flag:
        counts = {}
    for word in line.split():
        counts[word] = counts.get(word, 0) + 1
    return counts
""",
            "count",
        ),
    ],
    ids=["module-target", "reversed-operands", "maybe-unbound-dict"],
)
def test_for_split_dict_increment_declines_unsound_shapes(src, func):
    kinds = _split_count_kinds(src, func)
    assert "string_split_ws_dict_inc" not in kinds
    assert "string_split_sep_dict_inc" not in kinds


def test_statistics_slice_preserves_live_imported_callable():
    src = """
from statistics import mean, stdev
values = [1.0, 2.0, 3.0, 4.0]
m = mean(values[1:3])
s = stdev(values[1:4])
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "statistics_mean_slice" not in kinds
    assert "statistics_stdev_slice" not in kinds
    ops = _all_ops(ir)
    definitions = {op["out"]: op for op in ops if "out" in op}
    for line, name in [(4, "mean"), (5, "stdev")]:
        calls = [
            op
            for op in ops
            if op["kind"] == "call_func" and op.get("source_line") == line
        ]
        assert len(calls) == 1
        callee, argument = (definitions[value] for value in calls[0]["args"])
        assert callee["kind"] == "module_get_global"
        assert definitions[callee["args"][1]]["s_value"] == name
        assert argument["kind"] == "slice"


def test_dict_setdefault_empty_list_lowering():
    src = """
data = {}
series = data.setdefault("k", [])
series.append(1)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "dict_setdefault_empty_list" in kinds


def test_bytearray_counted_fill_never_fuses_a_shadowed_bytearray():
    src = """
def main(bytearray):
    size = 16
    data = bytearray(size)
    i = 0
    while i < size:
        data[i] = 97
        i += 1
    return data
"""
    ir = compile_to_tir(src)
    kinds = [op["kind"] for op in _ops_by_func_suffix(ir, "molt_user_main")]
    assert "bytearray_fill_range" not in kinds
    assert "store_index" in kinds


def test_nested_function_locals_cache_does_not_leak_into_outer_function():
    src = """
def outer():
    def inner():
        return locals()
    x = 1
    return x
"""
    ir = compile_to_tir(src)

    for fn in ir["functions"]:
        defined: set[str] = set()
        for op in fn["ops"]:
            out = op.get("out")
            if isinstance(out, str):
                defined.add(out)
            for arg in op.get("args", []):
                if isinstance(arg, str) and arg.startswith("v"):
                    assert arg in defined, (
                        f"{fn['name']} references undefined temp {arg} in {op}"
                    )


def test_abs_builtin_lowering():
    src = """
x = abs(-7)
y = abs(-3.5)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "abs" in kinds


def test_nested_constant_while_strength_reduction():
    src = """
total = 7
i = 0
while i < 3:
    j = 0
    while j < 5:
        total += 2
        j = j + 1
    i = i + 1
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert kinds.count("loop_start") >= 2
    assert kinds.count("lt") >= 2
    assert 7 in _const_values(ir)
    assert 2 in _const_values(ir)


def test_fstring_lowering():
    src = 'x = f"hi {1}"'
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "string_format" in kinds
    assert "string_join" in kinds


def test_format_keyword_call_preserves_method_pair_and_keyword_binding():
    src = 'x = "hi {name:>4}".format(name="a")'
    ir = compile_to_tir(src)
    ops = _all_ops(ir)
    (method,) = [op for op in ops if op["kind"] == "load_method"]
    assert method["s_value"] == "format"
    (call,) = [op for op in ops if op["kind"] == "call_bind"]
    assert call["args"][0] == method["var"]
    (receiver,) = [op for op in ops if op["kind"] == "callargs_push_method_receiver"]
    assert receiver["args"] == [call["args"][1], method["out"]]
    (keyword,) = [op for op in ops if op["kind"] == "callargs_push_kw"]
    assert keyword["args"][0] == call["args"][1]
    definitions = {op["out"]: op for op in ops if "out" in op}
    assert definitions[keyword["args"][1]]["s_value"] == "name"
    assert definitions[keyword["args"][2]]["s_value"] == "a"


def test_type_hint_check_lowering():
    src = "def verify(value):\n    checked: int = value\n    return checked\n"
    ir = compile_to_tir(src, type_hint_policy="check")
    ops = _ops_by_func_suffix(ir, "verify")
    assert any(op["kind"] == "guard_tag" for op in ops)


def test_type_hint_check_eliminates_proven_constant_guard():
    ir = compile_to_tir("x: int = 1", type_hint_policy="check")
    assert "guard_tag" not in _op_kinds(ir)


def test_type_hint_integer_lowering_preserves_semantic_op_shapes():
    src = """
a: int = 7
b: int = 3
div = a / b
fdiv = a // b
mod = a % b
lt = a < b
le = a <= b
gt = a > b
ge = a >= b
eq = a == b
ne = a != b
bor = a | b
band = a & b
bxor = a ^ b
lsh = a << b
rsh = a >> b
c: int = 10
c |= b
d: int = 11
d &= b
e: int = 12
e ^= b
"""
    ir = compile_to_tir(src, type_hint_policy="check")
    for kind in (
        "lt",
        "le",
        "gt",
        "ge",
        "eq",
        "ne",
    ):
        ops = _ops_by_kind(ir, kind)
        assert ops, f"expected at least one {kind} op"

    for kind in (
        "bit_or",
        "bit_and",
        "bit_xor",
        "lshift",
        "rshift",
        "div",
        "floordiv",
        "mod",
        "inplace_bit_or",
        "inplace_bit_and",
        "inplace_bit_xor",
    ):
        ops = _ops_by_kind(ir, kind)
        assert ops, f"expected at least one {kind} op"


def test_matmul_and_inplace_matmul_lower_to_distinct_wire_ops() -> None:
    src = """
a = 1
b = 2
c = a @ b
a @= b
"""
    ir = compile_to_tir(src)
    assert _ops_by_kind(ir, "matmul")
    assert _ops_by_kind(ir, "inplace_matmul")


def test_direct_call_result_hints_do_not_poison_operand_types() -> None:
    src = """
def f(x):
    return x

a = f(1)
b = f(1.0)
c = a + 1
d = b + 1
"""
    ir = compile_to_tir(src, type_hint_policy="check")
    add_ops = _ops_by_kind(ir, "add")
    assert len(add_ops) == 2
    main_ops = next(fn for fn in ir["functions"] if fn["name"] == "molt_main")["ops"]
    float_operand = add_ops[1]["args"][0]
    producer = next(op for op in main_ops if op.get("out") == float_operand)
    # f(1) and f(1.0) share one callee; neither result may claim int.
    assert producer.get("type_hint") not in {"int", "bool"}


def test_tuple_lowering():
    src = "t = (1, 2, 3)"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "tuple_new" in kinds


def test_buffer2d_calls_do_not_substitute_raw_storage_for_public_objects():
    src = """
import molt_buffer
a = molt_buffer.new(2, 2, 0)
molt_buffer.set(a, 0, 1, 3)
x = molt_buffer.get(a, 0, 1)
b = molt_buffer.new(2, 2, 0)
c = molt_buffer.matmul(a, b)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "call_func" in kinds
    assert not any(kind.startswith("buffer2d_") for kind in kinds)


def test_buffer2d_loop_preserves_live_methods_and_existing_destination():
    src = """
import molt_buffer
a = molt_buffer.new(2, 2, 0)
b = molt_buffer.new(2, 2, 0)
out = molt_buffer.new(2, 2, 0)
for i in range(2):
    for j in range(2):
        acc = 0
        for k in range(2):
            acc = acc + molt_buffer.get(a, i, k) * molt_buffer.get(b, k, j)
        molt_buffer.set(out, i, j, acc)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "buffer2d_matmul" not in kinds
    assert "call_func" in kinds


def test_range_lowering():
    src = "r = range(5)\nitems = list(range(1, 4, 2))"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "range_new" in kinds
    assert "list_from_range" in kinds


@pytest.mark.parametrize("public_fixture", [False, True])
def test_text_predicate_producers_cover_shared_call_family_and_arity(public_fixture):
    source = ["s = 'abcabc'", "b = b'abcabc'", "ba = bytearray(b'abcabc')"]
    expected = {}
    for receiver, variable, needle in (
        ("string", "s", "'bc'"),
        ("bytes", "b", "b'bc'"),
        ("bytearray", "ba", "b'bc'"),
    ):
        for method in ("find", "startswith", "endswith", "count"):
            for suffix, bounds, arity in (("", "", 2), ("_slice", ", -5, 9", 6)):
                source.append(f"{variable}.{method}({needle}{bounds})")
                expected[f"{receiver}_{method}{suffix}"] = arity
    if public_fixture:
        path = (
            Path(__file__).parent / "differential/basic/startswith_endswith_slices.py"
        )
        text = path.read_text(encoding="utf-8")
    else:
        text = "\n".join(source)
    ir = compile_to_tir(text)
    for kind, arity in expected.items():
        calls = _ops_by_kind(ir, kind)
        if public_fixture:
            # The executable oracle must reach each specialized family with
            # both result-using and resultless source calls, before callbacks.
            assert len(calls) >= 2, kind
        else:
            assert len(calls) == 1, kind
        assert all(len(call["args"]) == arity for call in calls), kind
