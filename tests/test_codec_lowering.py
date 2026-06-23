from molt.frontend import compile_to_tir


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


def test_default_codec_is_msgpack():
    src = """
import molt_json
x = molt_json.parse(42)
"""
    ir = compile_to_tir(src)
    assert "msgpack_parse" in _op_kinds(ir)
    assert "json_parse" not in _op_kinds(ir)


def test_json_codec_flag():
    src = """
import molt_json
x = molt_json.parse(42)
"""
    ir = compile_to_tir(src, parse_codec="json")
    assert "json_parse" in _op_kinds(ir)
    assert "msgpack_parse" not in _op_kinds(ir)


def test_explicit_msgpack_parse():
    src = """
import molt_msgpack
x = molt_msgpack.parse(42)
"""
    ir = compile_to_tir(src, parse_codec="json")
    assert "msgpack_parse" in _op_kinds(ir)


def test_explicit_cbor_parse():
    src = """
import molt_cbor
x = molt_cbor.parse(42)
"""
    ir = compile_to_tir(src)
    assert "cbor_parse" in _op_kinds(ir)


def test_const_bytes_lowering():
    src = "x = b'hi'"
    ir = compile_to_tir(src)
    assert "const_bytes" in _op_kinds(ir)


def test_len_lowering():
    src = "x = len(b'hello')"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "call_func" in kinds
    assert 5 in _const_values(ir)


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


def test_list_dict_method_lowering():
    src = """
lst = [1]
lst.append(2)
lst.extend([3])
lst.insert(0, 4)
lst.remove(2)
lst.count(2)
lst.index(2)
lst.pop()
d = {"a": 1}
d.get("a")
d.get("b", 2)
d.pop("a", 3)
d.keys()
d.values()
d.items()
t = (1, 2, 3)
t.count(1)
t.index(2)
s = "hello"
s.find("ell")
s.split("e")
s.replace("e", "x")
s.startswith("he")
s.endswith("lo")
s.count("l")
s.join(["a", "b"])
b = b"hello"
b.find(b"lo")
b.split(b"e")
b.replace(b"e", b"x")
ba = bytearray(b"hello")
ba.find(b"lo")
ba.split(b"e")
ba.replace(b"e", b"x")
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "list_append" in kinds
    assert "list_extend" in kinds
    assert "list_insert" in kinds
    assert "list_remove" in kinds
    assert "list_count" in kinds
    assert "list_index" in kinds
    assert "list_pop" in kinds
    assert "dict_get" in kinds
    assert "dict_pop" in kinds
    assert "dict_keys" in kinds
    assert "dict_values" in kinds
    assert "dict_items" in kinds
    assert "tuple_count" in kinds
    assert "tuple_index" in kinds
    assert "string_find" in kinds
    assert "string_startswith" in kinds
    assert "string_endswith" in kinds
    assert "string_count" in kinds
    assert "string_join" in kinds
    assert "string_split" in kinds
    assert "string_replace" in kinds
    assert "bytes_find" in kinds
    assert "bytes_split" in kinds
    assert "bytes_replace" in kinds
    assert "bytearray_from_obj" in kinds
    assert "bytearray_find" in kinds
    assert "bytearray_split" in kinds
    assert "bytearray_replace" in kinds


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


def test_async_sleep_call_async_uses_poll_table_target():
    src = """
from molt.concurrency import molt_async_sleep

async def main():
    await molt_async_sleep(0, None)
"""
    ir = compile_to_tir(src)
    call_async_ops = [op for op in _all_ops(ir) if op["kind"] == "call_async"]

    assert call_async_ops
    assert {op["s_value"] for op in call_async_ops} == {"molt_async_sleep_poll"}


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


def test_string_split_fixed_indexes_scalarize_non_escaping_local():
    src = """
def main() -> None:
    line = "1|NA|2"
    parts = line.split("|")
    first = parts[0]
    second = parts[1]
    print(first, second)
main()
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "string_split" not in kinds
    assert kinds.count("string_split_validate") == 1
    assert kinds.count("string_split_field") == 2
    field_ops = [op for op in ops if op["kind"] == "string_split_field"]
    assert field_ops[0]["args"][:2] == field_ops[1]["args"][:2]


def test_string_split_result_element_hint_enables_nested_split_scalarization():
    src = """
def main() -> None:
    data = "header,value\\nalpha,beta"
    lines = data.split("\\n")
    line = lines[1]
    fields = line.split(",")
    print(fields[0], fields[1])
main()
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "call_indirect" not in kinds
    assert "string_split" not in kinds
    assert kinds.count("string_split_validate") == 2
    assert kinds.count("string_split_field") == 3


def test_string_split_duplicate_fixed_index_reuses_materialized_field():
    src = """
def main() -> None:
    parts = "a-b|c-d".split("|")
    print(parts[0] is parts[0])
main()
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert kinds.count("string_split_validate") == 1
    assert kinds.count("string_split_field") == 1
    assert "copy_var" in kinds


def test_string_split_scalarization_ignores_compiler_list_guard():
    src = """
def main() -> None:
    line = "1|NA|2"
    parts: list[str] = line.split("|")
    first = parts[0]
    second = parts[1]
    print(first, second)
main()
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "string_split" not in kinds
    assert "guard_tag" not in kinds
    assert kinds.count("string_split_validate") == 1
    assert kinds.count("string_split_field") == 2


def test_string_split_scalarization_keeps_local_across_nested_control():
    src = """
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
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "string_split" not in kinds
    assert kinds.count("string_split_validate") == 1
    assert kinds.count("string_split_field") == 4


def test_string_split_field_len_and_eq_fuse_without_field_allocation():
    src = """
def main() -> None:
    fields = "alpha|22|eu|tail".split("|")
    total = len(fields[0])
    if fields[2] == "eu":
        total += len(fields[3])
    print(total)
main()
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "string_split" not in kinds
    assert kinds.count("string_split_validate") == 1
    assert kinds.count("string_split_field_len") == 2
    assert kinds.count("string_split_field_eq") == 1
    assert kinds.count("string_split_field") == 0


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
    assert "intarray_from_seq" not in kinds


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


def test_fully_positional_dataclass_constructor_skips_init_dispatch():
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
    assert "dataclass_new_values" in kinds
    assert "dataclass_new" not in kinds
    dataclass_ops = _ops_by_kind(ir, "dataclass_new_values")
    assert len(dataclass_ops) == 1
    dataclass_args = dataclass_ops[0]["args"]
    assert len(dataclass_args) == 8
    tuple_outs = {op["out"] for op in _ops_by_kind(ir, "tuple_new")}
    assert dataclass_args[1] in tuple_outs
    assert not any(arg in tuple_outs for arg in dataclass_args[3:])
    assert "dataclass_get" in kinds
    assert "bound_method_new" not in kinds
    assert "dataclass_set" not in kinds


def test_annotated_dataclass_list_iteration_preserves_field_lowering():
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
    assert kinds.count("dataclass_get") == 2
    assert "get_attr_generic_obj" not in kinds


def test_dataclass_field_dict_increment_uses_single_fused_update():
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
    assert "dict_str_int_inc" in kinds
    assert "dict_get" not in kinds
    assert "guard_dict_shape" not in kinds


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


def test_ord_subscript_lowers_to_fused_ord_at():
    src = """
def parse_digit(text: str, i: int) -> int:
    return ord(text[i]) - 48
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir, "__main____parse_digit")
    assert "ord_at" in kinds
    assert "ord" not in kinds
    assert kinds.count("index") == 0


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


def test_prod_reduction_over_flat_listcomp_skips_intarray_conversion():
    src = """
def main():
    n = 5
    nums = [1 for _ in range(n)]
    acc = 1
    for x in nums:
        acc = acc * x
    print(acc)

main()
"""
    ir = compile_to_tir(src)
    kinds = [op["kind"] for op in _ops_by_func_suffix(ir, "molt_user_main")]
    assert "list_int_new" in kinds
    assert "vec_prod_int" in kinds
    assert "intarray_from_seq" not in kinds


def test_dict_increment_lowering():
    src = """
counts = {}
key = "molt"
counts[key] = counts.get(key, 0) + 1
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "dict_inc" in kinds or "dict_str_int_inc" in kinds


def test_dict_str_int_increment_lowering():
    src = """
counts: dict[str, int] = {}
key = "molt"
counts[key] = counts.get(key, 0) + 1
"""
    ir = compile_to_tir(src, type_hint_policy="check")
    kinds = _op_kinds(ir)
    assert "dict_str_int_inc" in kinds


def test_for_split_whitespace_dict_increment_fused():
    src = """
counts = {}
line = "a b a"
for word in line.split():
    counts[word] = counts.get(word, 0) + 1
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "string_split_ws_dict_inc" in kinds


def test_for_split_separator_dict_increment_fused():
    src = """
counts = {}
line = "a|b|a"
for word in line.split("|"):
    counts[word] = counts.get(word, 0) + 2
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "string_split_sep_dict_inc" in kinds


def test_taq_ingest_line_fused():
    src = """
BUCKET_SIZE = 1_000_000_000
data = {}
header = True
for line in ["header", "100|X|AAPL|X|200"]:
    if header:
        header = False
        continue
    x = line.split("|")
    if x[0] == "END" or x[4] == "ENDP":
        continue
    timestamp = int(x[0])
    symbol = x[2]
    volume = int(x[4])
    series = data.setdefault(symbol, [])
    series.append((timestamp // BUCKET_SIZE, volume))
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "taq_ingest_line" in kinds


def test_statistics_slice_lowering():
    src = """
from statistics import mean, stdev
values = [1.0, 2.0, 3.0, 4.0]
m = mean(values[1:3])
s = stdev(values[1:4])
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "statistics_mean_slice" in kinds
    assert "statistics_stdev_slice" in kinds


def test_dict_setdefault_empty_list_lowering():
    src = """
data = {}
series = data.setdefault("k", [])
series.append(1)
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "dict_setdefault_empty_list" in kinds


def test_bytearray_counted_fill_lowers_to_range_primitive():
    src = """
def main():
    size = 16
    data = bytearray(size)
    i = 0
    while i < size:
        data[i] = 97
        i += 1
    return bytes(data).find(b"a")
"""
    ir = compile_to_tir(src)
    ops = _ops_by_func_suffix(ir, "molt_user_main")
    kinds = [op["kind"] for op in ops]
    assert "bytearray_fill_range" in kinds
    assert "store_index" not in kinds


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


def test_format_spec_lowering():
    src = 'x = "hi {name:>4}".format(name="a")'
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "string_format" in kinds


def test_type_hint_check_lowering():
    src = "x: int = 1"
    ir = compile_to_tir(src, type_hint_policy="check")
    kinds = _op_kinds(ir)
    # Type hint check lowering emits type verification via builtin_type + dict
    # based checking rather than a dedicated guard_tag op.
    assert "builtin_type" in kinds or "guard_tag" in kinds


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


def test_direct_call_result_hints_do_not_poison_fast_int() -> None:
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
    assert add_ops[1].get("fast_int") is not True


def test_tuple_lowering():
    src = "t = (1, 2, 3)"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "tuple_new" in kinds


def test_buffer2d_lowering():
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
    assert "buffer2d_new" in kinds
    assert "buffer2d_set" in kinds
    assert "buffer2d_get" in kinds
    assert "buffer2d_matmul" in kinds


def test_buffer2d_matmul_loop_lowering():
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
    assert "buffer2d_matmul" in kinds


def test_range_lowering():
    src = "r = range(5)\nitems = list(range(1, 4, 2))"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "range_new" in kinds
    assert "list_from_range" in kinds
