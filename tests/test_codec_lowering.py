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


def _assert_control_values_are_ints(ir: dict) -> None:
    control_kinds = {"label", "state_label", "jump", "check_exception"}
    for func in ir["functions"]:
        for op in func["ops"]:
            if op["kind"] not in control_kinds:
                continue
            assert isinstance(
                op.get("value"), int
            ), f"{func['name']} emitted non-int control value: {op}"


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
    assert "len" in _op_kinds(ir)


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
    assert "vec_sum_int" in kinds
    has_iter_loop = "loop_start" in kinds and "loop_break_if_true" in kinds
    has_index_loop = (
        "loop_index_start" in kinds
        and "loop_index_next" in kinds
        and "loop_break_if_false" in kinds
    )
    assert has_iter_loop or has_index_loop
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
    assert "vec_sum_float" in kinds


def test_for_loop_float_range_reduction_lowering():
    src = """
total = 0.5
for i in range(10):
    total += i
"""
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "vec_sum_float_range_iter" in kinds


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


def test_simple_range_listcomp_lowering():
    src = "x = [i for i in range(5)]"
    ir = compile_to_tir(src)
    kinds = _op_kinds(ir)
    assert "list_from_range" in kinds


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
    assert "loop_index_start" not in kinds
    assert "loop_start" not in kinds
    assert 37 in _const_values(ir)


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
    assert "guard_tag" in kinds


def test_type_hint_fast_int_for_comparison_bitwise_and_shift():
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
        assert all(op.get("fast_int") is True for op in ops)


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
