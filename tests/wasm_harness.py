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
let nextPtr = 1n << 40n;
let memory = null;
let table = null;
let heapPtr = 1024;
const align = (size, align) => (size + (align - 1)) & ~(align - 1);
const isNone = (val) => isTag(val, TAG_NONE);
const boxPtrAddr = (addr) => QNAN | TAG_PTR | (BigInt(addr) & POINTER_MASK);
const ptrAddr = (val) => Number(val & POINTER_MASK);
const boxPtr = (obj) => {
  const id = nextPtr++;
  heap.set(id, obj);
  return QNAN | TAG_PTR | id;
};
const getObj = (val) => heap.get(val & POINTER_MASK);
const isGenerator = (val) => isPtr(val) && !heap.has(val & POINTER_MASK);
const getList = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'list') return null;
  return obj;
};
const getTuple = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'tuple') return null;
  return obj;
};
const getIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'iter') return null;
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
const tupleFromArray = (items) => boxPtr({ type: 'tuple', items });
const getStr = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'str') return obj.value;
  return '';
};
const getException = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'exception') return obj;
  return null;
};
let lastException = boxNone();
const exceptionStack = [];
const exceptionDepth = () => exceptionStack.length;
const exceptionSetDepth = (depth) => {
  const target = Math.max(0, depth);
  while (exceptionStack.length > target) {
    exceptionStack.pop();
  }
  while (exceptionStack.length < target) {
    exceptionStack.push(1);
  }
};
const exceptionNew = (kindBits, msgBits) => {
  return boxPtr({
    type: 'exception',
    kindBits,
    msgBits,
    causeBits: boxNone(),
    contextBits: boxNone(),
    suppressBits: boxBool(false),
  });
};
const exceptionSetCause = (excBits, causeBits) => {
  const exc = getException(excBits);
  if (!exc) return boxNone();
  if (!isNone(causeBits) && !getException(causeBits)) {
    throw new Error('TypeError: exception cause must be an exception or None');
  }
  exc.causeBits = causeBits;
  exc.suppressBits = boxBool(true);
  return boxNone();
};
const exceptionKind = (excBits) => {
  const exc = getException(excBits);
  if (!exc) return boxNone();
  return exc.kindBits;
};
const exceptionMessage = (excBits) => {
  const exc = getException(excBits);
  if (!exc) return boxNone();
  return exc.msgBits;
};
const exceptionLast = () => lastException;
const exceptionClear = () => {
  lastException = boxNone();
  return boxNone();
};
const exceptionPending = () => (isNone(lastException) ? 0n : 1n);
const exceptionPush = () => {
  exceptionStack.push(1);
  return boxNone();
};
const exceptionPop = () => {
  if (!exceptionStack.length) {
    throw new Error('RuntimeError: exception handler stack underflow');
  }
  exceptionStack.pop();
  return boxNone();
};
const raiseException = (excBits) => {
  const exc = getException(excBits);
  if (exc && !isNone(lastException) && isNone(exc.contextBits)) {
    exc.contextBits = lastException;
  }
  lastException = excBits;
  if (!exceptionStack.length) {
    const kind = exc ? getStr(exc.kindBits) : 'Exception';
    const msg = exc ? getStr(exc.msgBits) : '';
    throw new Error(`${kind}: ${msg}`);
  }
  return boxNone();
};
const generatorSend = (gen, sendVal) => {
  if (!isGenerator(gen) || !memory || !table) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const addr = ptrAddr(gen);
  const view = new DataView(memory.buffer);
  const closedBits = view.getBigInt64(addr + 16, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const callerDepth = exceptionDepth();
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  view.setBigInt64(addr + 0, sendVal, true);
  view.setBigInt64(addr + 8, boxNone(), true);
  const pollIdx = view.getUint32(addr - 24, true);
  const poll = table.get(pollIdx);
  const res = poll
    ? poll(gen)
    : tupleFromArray([boxNone(), boxBool(true)]);
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
  exceptionSetDepth(callerDepth);
  return res;
};
const generatorThrow = (gen, exc) => {
  if (!isGenerator(gen) || !memory || !table) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const addr = ptrAddr(gen);
  const view = new DataView(memory.buffer);
  const closedBits = view.getBigInt64(addr + 16, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const callerDepth = exceptionDepth();
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  view.setBigInt64(addr + 8, exc, true);
  view.setBigInt64(addr + 0, boxNone(), true);
  const pollIdx = view.getUint32(addr - 24, true);
  const poll = table.get(pollIdx);
  const res = poll
    ? poll(gen)
    : tupleFromArray([boxNone(), boxBool(true)]);
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
  exceptionSetDepth(callerDepth);
  return res;
};
const generatorClose = (gen) => {
  if (!isGenerator(gen) || !memory || !table) {
    return boxNone();
  }
  const addr = ptrAddr(gen);
  const view = new DataView(memory.buffer);
  const closedBits = view.getBigInt64(addr + 16, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) return boxNone();
  const callerDepth = exceptionDepth();
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  const exc = exceptionNew(
    boxPtr({ type: 'str', value: 'GeneratorExit' }),
    boxPtr({ type: 'str', value: '' }),
  );
  view.setBigInt64(addr + 8, exc, true);
  view.setBigInt64(addr + 0, boxNone(), true);
  const pollIdx = view.getUint32(addr - 24, true);
  const poll = table.get(pollIdx);
  const res = poll ? poll(gen) : null;
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
  exceptionSetDepth(callerDepth);
  if (res) {
    const pair = getTuple(res);
    if (pair) {
      const doneBits = pair.items[1];
      const done = isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n;
      if (!done) {
        const errExc = exceptionNew(
          boxPtr({ type: 'str', value: 'RuntimeError' }),
          boxPtr({ type: 'str', value: 'generator ignored GeneratorExit' }),
        );
        raiseException(errExc);
      }
    }
  }
  view.setBigInt64(addr + 16, boxBool(true), true);
  return boxNone();
};
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
  alloc: (size) => {
    if (!memory) return boxNone();
    const bytes = align(Number(size), 8);
    const addr = heapPtr;
    heapPtr += bytes;
    const needed = heapPtr - memory.buffer.byteLength;
    if (needed > 0) {
      const pageSize = 65536;
      const pages = Math.ceil(needed / pageSize);
      memory.grow(pages);
    }
    return boxPtrAddr(addr);
  },
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
  eq: (a, b) => {
    if (isPtr(a) && isPtr(b)) {
      const left = getObj(a);
      const right = getObj(b);
      if (left && right && left.type === 'str' && right.type === 'str') {
        return boxBool(left.value === right.value);
      }
    }
    return boxBool(a === b);
  },
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
    if (isTag(val, TAG_NONE)) {
      return 0n;
    }
    if (isPtr(val)) {
      const obj = getObj(val);
      if (obj && obj.type === 'str') return obj.value.length ? 1n : 0n;
      if (obj && obj.type === 'bytes') return obj.data.length ? 1n : 0n;
      if (obj && obj.type === 'bytearray') return obj.data.length ? 1n : 0n;
      if (obj && obj.type === 'list') return obj.items.length ? 1n : 0n;
      if (obj && obj.type === 'tuple') return obj.items.length ? 1n : 0n;
      if (obj && obj.type === 'iter') return 1n;
    }
    return 0n;
  },
  json_parse_scalar: () => 0,
  msgpack_parse_scalar: () => 0,
  cbor_parse_scalar: () => 0,
  string_from_bytes: (ptr, len, out) => {
    if (!memory) return 0;
    const view = new DataView(memory.buffer);
    const addr = Number(ptr);
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const value = Buffer.from(bytes).toString('utf8');
    const boxed = boxPtr({ type: 'str', value });
    const outAddr = ptrAddr(out);
    view.setBigInt64(outAddr, boxed, true);
    return 0;
  },
  bytes_from_bytes: (ptr, len, out) => {
    if (!memory) return 0;
    const view = new DataView(memory.buffer);
    const addr = Number(ptr);
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const boxed = boxPtr({ type: 'bytes', data: Uint8Array.from(bytes) });
    const outAddr = ptrAddr(out);
    view.setBigInt64(outAddr, boxed, true);
    return 0;
  },
  memoryview_new: () => boxNone(),
  memoryview_tobytes: () => boxNone(),
  str_from_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      return boxPtr({ type: 'str', value: unboxInt(val).toString() });
    }
    if (isTag(val, TAG_BOOL)) {
      return boxPtr({ type: 'str', value: (val & 1n) === 1n ? 'True' : 'False' });
    }
    if (isTag(val, TAG_NONE)) {
      return boxPtr({ type: 'str', value: 'None' });
    }
    const obj = getObj(val);
    if (obj && obj.type === 'str') {
      return val;
    }
    return boxPtr({ type: 'str', value: '<obj>' });
  },
  len: (val) => {
    const list = getList(val);
    if (list) return boxInt(list.items.length);
    const tup = getTuple(val);
    if (tup) return boxInt(tup.items.length);
    const bytes = getBytes(val);
    if (bytes) return boxInt(bytes.data.length);
    return boxInt(0);
  },
  slice: () => boxNone(),
  slice_new: () => boxNone(),
  range_new: () => boxNone(),
  list_builder_new: () => boxPtr({ type: 'list_builder', items: [] }),
  list_builder_append: (builder, val) => {
    const obj = getObj(builder);
    if (obj && obj.type === 'list_builder') {
      obj.items.push(val);
    }
  },
  list_builder_finish: (builder) => {
    const obj = getObj(builder);
    if (obj && obj.type === 'list_builder') {
      return listFromArray(obj.items);
    }
    return boxNone();
  },
  tuple_builder_finish: (builder) => {
    const obj = getObj(builder);
    if (obj && obj.type === 'list_builder') {
      return tupleFromArray(obj.items);
    }
    return boxNone();
  },
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
  iter: (val) => {
    if (isGenerator(val)) {
      return val;
    }
    const list = getList(val);
    if (list) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    const tup = getTuple(val);
    if (tup) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    return boxNone();
  },
  iter_next: (val) => {
    if (isGenerator(val)) {
      return generatorSend(val, boxNone());
    }
    const iter = getIter(val);
    if (!iter) return boxNone();
    const target = iter.target;
    const list = getList(target);
    const tup = getTuple(target);
    const items = list ? list.items : tup ? tup.items : null;
    if (!items) return boxNone();
    if (iter.idx >= items.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = items[iter.idx];
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  },
  generator_send: (gen, sendVal) => generatorSend(gen, sendVal),
  generator_throw: (gen, exc) => generatorThrow(gen, exc),
  generator_close: (gen) => generatorClose(gen),
  is_generator: (val) => boxBool(isGenerator(val)),
  index: (seq, idxBits) => {
    const idx = Number(unboxInt(idxBits));
    const list = getList(seq);
    const tup = getTuple(seq);
    const items = list ? list.items : tup ? tup.items : null;
    if (!items) return boxNone();
    let pos = idx;
    if (pos < 0) pos += items.length;
    if (pos < 0 || pos >= items.length) return boxNone();
    return items[pos];
  },
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
  exception_push: () => exceptionPush(),
  exception_pop: () => exceptionPop(),
  exception_last: () => exceptionLast(),
  exception_new: (kind, msg) => exceptionNew(kind, msg),
  exception_clear: () => exceptionClear(),
  exception_pending: () => exceptionPending(),
  exception_kind: (exc) => exceptionKind(exc),
  exception_message: (exc) => exceptionMessage(exc),
  exception_set_cause: (exc, cause) => exceptionSetCause(exc, cause),
  raise: (exc) => raiseException(exc),
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
        "    table = mod.instance.exports.molt_table;\n"
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
