from __future__ import annotations

from pathlib import Path

BASE_PREAMBLE = """\
const fs = require('fs');
const wasmPath = process.argv[2];
const wasmBuffer = fs.readFileSync(wasmPath);
const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_BOOL = 0x0002000000000000n;
const TAG_NONE = 0x0003000000000000n;
const TAG_PTR = 0x0004000000000000n;
const TAG_MASK = 0x0007000000000000n;
const POINTER_MASK = 0x0000ffffffffffffn;
const INT_SIGN_BIT = 1n << 46n;
const INT_WIDTH = 47n;
const INT_MASK = (1n << INT_WIDTH) - 1n;
const isTag = (val, tag) => (val & (QNAN | TAG_MASK)) === (QNAN | tag);
const isPtr = (val) => isTag(val, TAG_PTR);
const unboxInt = (val) => {
  let v = val & INT_MASK;
  if ((v & INT_SIGN_BIT) !== 0n) {
    v = v - (1n << INT_WIDTH);
  }
  return v;
};
const boxInt = (n) => {
  const v = BigInt(n) & INT_MASK;
  return QNAN | TAG_INT | v;
};
const boxBool = (b) => QNAN | TAG_BOOL | (b ? 1n : 0n);
const boxNone = () => QNAN | TAG_NONE;
const heap = new Map();
let nextPtr = 1n;
let memory = null;
const boxPtr = (obj) => {
  const id = nextPtr++;
  heap.set(id, obj);
  return QNAN | TAG_PTR | id;
};
const getObj = (val) => heap.get(val & POINTER_MASK);
const getList = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'list') return null;
  return obj;
};
const getDict = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'dict') return null;
  return obj;
};
const getBytes = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'bytes') return null;
  return obj;
};
const getBytearray = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'bytearray') return null;
  return obj;
};
const listFromArray = (items) => boxPtr({ type: 'list', items });
"""

BASE_IMPORTS = """\
  print_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      console.log(unboxInt(val).toString());
      return;
    }
    if (isTag(val, TAG_BOOL)) {
      console.log((val & 1n) === 1n ? 'True' : 'False');
      return;
    }
    if (isTag(val, TAG_NONE)) {
      console.log('None');
      return;
    }
    console.log(val.toString());
  },
  print_newline: () => console.log(''),
  alloc: () => 0n,
  async_sleep: () => 0n,
  block_on: () => 0n,
  add: (a, b) => boxInt(unboxInt(a) + unboxInt(b)),
  vec_sum_int: () => boxNone(),
  vec_sum_int_trusted: () => boxNone(),
  vec_sum_int_range: () => boxNone(),
  vec_sum_int_range_trusted: () => boxNone(),
  vec_prod_int: () => boxNone(),
  vec_prod_int_trusted: () => boxNone(),
  vec_prod_int_range: () => boxNone(),
  vec_prod_int_range_trusted: () => boxNone(),
  vec_min_int: () => boxNone(),
  vec_min_int_trusted: () => boxNone(),
  vec_min_int_range: () => boxNone(),
  vec_min_int_range_trusted: () => boxNone(),
  vec_max_int: () => boxNone(),
  vec_max_int_trusted: () => boxNone(),
  vec_max_int_range: () => boxNone(),
  vec_max_int_range_trusted: () => boxNone(),
  sub: (a, b) => boxInt(unboxInt(a) - unboxInt(b)),
  mul: (a, b) => boxInt(unboxInt(a) * unboxInt(b)),
  lt: (a, b) => boxBool(unboxInt(a) < unboxInt(b)),
  eq: (a, b) => boxBool(a === b),
  is: (a, b) => boxBool(a === b),
  not: (val) => {
    if (isTag(val, TAG_BOOL)) {
      return boxBool((val & 1n) !== 1n);
    }
    if (isTag(val, TAG_INT)) {
      return boxBool(unboxInt(val) === 0n);
    }
    return boxBool(true);
  },
  contains: () => boxBool(false),
  guard_type: (val, expected) => val,
  get_attr_generic: () => boxNone(),
  get_attr_object: () => boxNone(),
  set_attr_generic: () => boxNone(),
  set_attr_object: () => boxNone(),
  get_attr_name: () => boxNone(),
  get_attr_name_default: () => boxNone(),
  has_attr_name: () => boxBool(false),
  set_attr_name: () => boxNone(),
  is_truthy: (val) => {
    if (isTag(val, TAG_BOOL)) {
      return (val & 1n) === 1n ? 1n : 0n;
    }
    if (isTag(val, TAG_INT)) {
      return unboxInt(val) !== 0n ? 1n : 0n;
    }
    return 0n;
  },
  json_parse_scalar: () => 0,
  msgpack_parse_scalar: () => 0,
  cbor_parse_scalar: () => 0,
  string_from_bytes: () => 0,
  bytes_from_bytes: () => 0,
  memoryview_new: () => boxNone(),
  memoryview_tobytes: () => boxNone(),
  str_from_obj: () => boxNone(),
  len: () => boxInt(0),
  slice: () => boxNone(),
  slice_new: () => boxNone(),
  range_new: () => boxNone(),
  list_builder_new: () => boxNone(),
  list_builder_append: () => {},
  list_builder_finish: () => boxNone(),
  tuple_builder_finish: () => boxNone(),
  list_append: () => boxNone(),
  list_pop: () => boxNone(),
  list_extend: () => boxNone(),
  list_insert: () => boxNone(),
  list_remove: () => boxNone(),
  list_count: () => boxNone(),
  list_index: () => boxNone(),
  dict_new: () => boxNone(),
  dict_set: () => boxNone(),
  dict_get: () => boxNone(),
  dict_pop: () => boxNone(),
  dict_keys: () => boxNone(),
  dict_values: () => boxNone(),
  dict_items: () => boxNone(),
  tuple_count: () => boxNone(),
  tuple_index: () => boxNone(),
  iter: () => boxNone(),
  iter_next: () => boxNone(),
  generator_send: () => boxNone(),
  generator_throw: () => boxNone(),
  generator_close: () => boxNone(),
  is_generator: () => boxBool(false),
  index: () => boxNone(),
  store_index: () => boxNone(),
  bytes_find: () => boxNone(),
  bytearray_find: () => boxNone(),
  string_find: () => boxNone(),
  string_format: () => boxNone(),
  string_startswith: () => boxBool(false),
  string_endswith: () => boxBool(false),
  string_count: () => boxInt(0),
  string_join: () => boxNone(),
  string_split: () => boxNone(),
  bytes_split: () => boxNone(),
  bytearray_split: () => boxNone(),
  string_replace: () => boxNone(),
  bytes_replace: () => boxNone(),
  bytearray_replace: () => boxNone(),
  bytearray_from_obj: () => boxNone(),
  intarray_from_seq: () => boxNone(),
  buffer2d_new: () => boxNone(),
  buffer2d_get: () => boxNone(),
  buffer2d_set: () => boxNone(),
  buffer2d_matmul: () => boxNone(),
  dataclass_new: () => boxNone(),
  dataclass_get: () => boxNone(),
  dataclass_set: () => boxNone(),
  dataclass_set_class: () => boxNone(),
  class_new: () => boxNone(),
  classmethod_new: () => boxNone(),
  staticmethod_new: () => boxNone(),
  property_new: () => boxNone(),
  object_set_class: () => boxNone(),
  context_null: (val) => val,
  context_enter: (val) => val,
  context_exit: () => boxBool(false),
  context_unwind: () => boxBool(false),
  context_depth: () => boxInt(0),
  context_unwind_to: () => boxNone(),
  env_get: () => boxNone(),
  exception_push: () => boxNone(),
  exception_pop: () => boxNone(),
  exception_last: () => boxNone(),
  exception_new: () => boxNone(),
  exception_clear: () => boxNone(),
  exception_pending: () => 0n,
  exception_kind: () => boxNone(),
  exception_message: () => boxNone(),
  exception_set_cause: () => boxNone(),
  raise: () => boxNone(),
  context_closing: (val) => val,
  bridge_unavailable: () => boxNone(),
  file_open: () => boxNone(),
  file_read: () => boxNone(),
  file_write: () => boxNone(),
  file_close: () => boxNone(),
  stream_new: () => 0n,
  stream_send: () => 0n,
  stream_recv: () => 0n,
  stream_close: () => {},
  ws_connect: () => 0,
  ws_pair: () => 0,
  ws_send: () => 0n,
  ws_recv: () => 0n,
  ws_close: () => {},
"""


def wasm_runner_source(*, extra_js: str = "", import_overrides: str = "") -> str:
    parts = [BASE_PREAMBLE]
    if extra_js:
        parts.append(extra_js.rstrip() + "\n")
    parts.append("const baseImports = {\n")
    parts.append(BASE_IMPORTS.rstrip() + "\n")
    parts.append("};\n")
    parts.append("const overrideImports = {\n")
    if import_overrides:
        parts.append(import_overrides.rstrip() + "\n")
    parts.append("};\n")
    parts.append(
        "const imports = { molt_runtime: { ...baseImports, ...overrideImports } };\n"
    )
    parts.append(
        "WebAssembly.instantiate(wasmBuffer, imports)\n"
        "  .then((mod) => {\n"
        "    memory = mod.instance.exports.molt_memory;\n"
        "    return mod.instance.exports.molt_main();\n"
        "  })\n"
        "  .catch((err) => {\n"
        "    console.error(err);\n"
        "    process.exit(1);\n"
        "  });\n"
    )
    return "".join(parts)


def write_wasm_runner(
    tmp_path: Path,
    name: str,
    *,
    extra_js: str = "",
    import_overrides: str = "",
) -> Path:
    runner = tmp_path / name
    runner.write_text(
        wasm_runner_source(extra_js=extra_js, import_overrides=import_overrides)
    )
    return runner
