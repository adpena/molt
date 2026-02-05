from __future__ import annotations

from pathlib import Path
import re


def _load_intrinsic_specs() -> list[tuple[str, str, int]]:
    manifest = (
        Path(__file__).resolve().parents[1]
        / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
    )
    specs: list[tuple[str, str, int]] = []
    text = manifest.read_text()
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("def "):
            continue
        match = re.match(r"def\s+([A-Za-z0-9_]+)\(([^)]*)\)", line)
        if not match:
            continue
        name = match.group(1)
        raw_args = match.group(2).strip()
        if not raw_args:
            arity = 0
        else:
            parts = []
            for part in raw_args.split(","):
                part = part.strip()
                if not part or part in {"*", "/"}:
                    continue
                parts.append(part)
            arity = len(parts)
        import_name = name
        if name.startswith("molt_"):
            import_name = name[len("molt_") :]
        specs.append((name, import_name, arity))
    return specs


INTRINSIC_SPECS = _load_intrinsic_specs()


def _intrinsic_registry_js() -> str:
    lines = ["const intrinsicSpecs = ["]
    for name, import_name, arity in INTRINSIC_SPECS:
        lines.append(f"  ['{name}', '{import_name}', {arity}],")
    lines.append("];")
    lines.append("let intrinsicsInstalled = false;")
    lines.append("const installIntrinsics = (builtinsBits) => {")
    lines.append("  if (intrinsicsInstalled) return;")
    lines.append("  intrinsicsInstalled = true;")
    lines.append("  const builtins = getModule(builtinsBits);")
    lines.append("  if (!builtins) return;")
    lines.append("  const dict = getDict(builtins.dictBits);")
    lines.append("  if (!dict) return;")
    lines.append("  const registryBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });")
    lines.append("  const registry = getDict(registryBits);")
    lines.append("  if (!registry) return;")
    lines.append("  dictSetValue(dict, boxPtr({ type: 'str', value: '_molt_intrinsics' }), registryBits);")
    lines.append("  dictSetValue(dict, boxPtr({ type: 'str', value: '_molt_intrinsics_strict' }), boxBool(true));")
    lines.append("  dictSetValue(dict, boxPtr({ type: 'str', value: '_molt_runtime' }), boxBool(true));")
    lines.append("  for (const [name, importName, arity] of intrinsicSpecs) {")
    lines.append("    let fn = baseImports[importName];")
    lines.append("    if (!fn) {")
    lines.append("      fn = (..._args) => baseImports.unsupported_import(importName);")
    lines.append("    }")
    lines.append("    const idx = getOrAddTableFunc(fn, arity);")
    lines.append("    if (idx === null) continue;")
    lines.append("    const fnBits = baseImports.func_new(BigInt(idx), 0n, BigInt(arity));")
    lines.append("    const nameBits = boxPtr({ type: 'str', value: name });")
    lines.append("    dictSetValue(dict, nameBits, fnBits);")
    lines.append("    dictSetValue(registry, nameBits, fnBits);")
    lines.append("    if (name.startsWith('molt_')) {")
    lines.append("      const alias = `_molt_${name.slice(5)}`;")
    lines.append("      const aliasBits = boxPtr({ type: 'str', value: alias });")
    lines.append("      dictSetValue(dict, aliasBits, fnBits);")
    lines.append("      dictSetValue(registry, aliasBits, fnBits);")
    lines.append("    }")
    lines.append("  }")
    lines.append("};")
    lines.append("if (!moduleCache.get('builtins')) {")
    lines.append("  const builtinsName = boxPtr({ type: 'str', value: 'builtins' });")
    lines.append("  const builtinsBits = baseImports.module_new(builtinsName);")
    lines.append("  baseImports.module_cache_set(builtinsName, builtinsBits);")
    lines.append("}")
    return "\n".join(lines)


INTRINSIC_REGISTRY_JS = _intrinsic_registry_js()

BASE_PREAMBLE = """\
const fs = require('fs');
const os = require('os');
const crypto = require('crypto');
const wasmPath = process.argv[2];
const runtimeArgv = [wasmPath, ...process.argv.slice(3)];
const wasmBuffer = fs.readFileSync(wasmPath);
const MONO_START =
  typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
    ? process.hrtime.bigint()
    : BigInt(Date.now()) * 1000000n;
const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_BOOL = 0x0002000000000000n;
const TAG_NONE = 0x0003000000000000n;
const TAG_PTR = 0x0004000000000000n;
const TAG_PENDING = 0x0005000000000000n;
const TAG_MASK = 0x0007000000000000n;
const POINTER_MASK = 0x0000ffffffffffffn;
const INT_SIGN_BIT = 1n << 46n;
const INT_WIDTH = 47n;
const INT_MASK = (1n << INT_WIDTH) - 1n;
const isTag = (val, tag) => (val & (QNAN | TAG_MASK)) === (QNAN | tag);
const isPending = (val) => isTag(val, TAG_PENDING);
const isPtr = (val) => isTag(val, TAG_PTR);
const unboxInt = (val) => {
  let v = val & INT_MASK;
  if ((v & INT_SIGN_BIT) !== 0n) {
    v = v - (1n << INT_WIDTH);
  }
  return v;
};
const formatIntBase = (val, base, prefix) => {
  let num = null;
  if (isTag(val, TAG_INT)) {
    num = unboxInt(val);
  } else if (isTag(val, TAG_BOOL)) {
    num = (val & 1n) === 1n ? 1n : 0n;
  } else {
    const obj = getObj(val);
    if (obj && obj.type === 'bigint') {
      num = obj.value;
    }
  }
  if (num === null) return boxNone();
  const negative = num < 0n;
  const absVal = negative ? -num : num;
  const digits = absVal.toString(base);
  const value = `${negative ? '-' : ''}${prefix}${digits}`;
  return boxPtr({ type: 'str', value });
};
const boxInt = (n) => {
  const v = BigInt(n) & INT_MASK;
  return QNAN | TAG_INT | v;
};
const FLOAT_BUF = new ArrayBuffer(8);
const FLOAT_VIEW = new DataView(FLOAT_BUF);
const CANONICAL_NAN = 0x7ff0000000000001n;
const isFloat = (val) => (val & QNAN) !== QNAN;
const boxFloat = (val) => floatToBits(val);
const bitsToFloat = (bits) => {
  FLOAT_VIEW.setBigUint64(0, bits, true);
  return FLOAT_VIEW.getFloat64(0, true);
};
const floatToBits = (val) => {
  if (Number.isNaN(val)) return CANONICAL_NAN;
  FLOAT_VIEW.setFloat64(0, val, true);
  return FLOAT_VIEW.getBigUint64(0, true);
};
const boxBool = (b) => QNAN | TAG_BOOL | (b ? 1n : 0n);
const boxNone = () => QNAN | TAG_NONE;
const boxPending = () => QNAN | TAG_PENDING;
const isIntLike = (val) => isTag(val, TAG_INT) || isTag(val, TAG_BOOL);
const unboxIntLike = (val) => {
  if (isTag(val, TAG_INT)) return unboxInt(val);
  return (val & 1n) === 1n ? 1n : 0n;
};
const boxComplex = (re, im) => boxPtr({ type: 'complex', re, im });
const getBigIntValue = (val) => {
  if (isIntLike(val)) return unboxIntLike(val);
  const obj = getObj(val);
  if (obj && obj.type === 'bigint') return obj.value;
  return null;
};
const INLINE_INT_MIN = -(1n << (INT_WIDTH - 1n));
const INLINE_INT_MAX = (1n << (INT_WIDTH - 1n)) - 1n;
const fitsInlineInt = (value) => value >= INLINE_INT_MIN && value <= INLINE_INT_MAX;
const boxIntOrBigint = (value) =>
  fitsInlineInt(value) ? boxInt(value) : boxPtr({ type: 'bigint', value });
const indexBigIntFromBits = (bits, errMsg) => {
  let value = getBigIntValue(bits);
  if (value === null) {
    const indexAttr = lookupAttr(bits, '__index__');
    if (indexAttr !== undefined) {
      const res = callCallable0(indexAttr);
      if (exceptionPending() !== 0n) return null;
      value = getBigIntValue(res);
      if (value === null) {
        throw new Error(`TypeError: __index__ returned non-int (type ${typeName(res)})`);
      }
    }
  }
  if (value === null) {
    throw new Error(`TypeError: ${errMsg}`);
  }
  return value;
};
const indexFromBitsWithOverflow = (bits, errMsg, overflowErr) => {
  const value = indexBigIntFromBits(bits, errMsg);
  if (value === null) return null;
  const limit = BigInt(Number.MAX_SAFE_INTEGER);
  if (value > limit || value < -limit) {
    if (overflowErr) {
      throw new Error(`IndexError: ${overflowErr}`);
    }
    throw new Error(
      `IndexError: cannot fit '${typeName(bits)}' into an index-sized integer`,
    );
  }
  return Number(value);
};
const BUILTIN_TYPE_TAGS = new Map([
  [1, 'int'],
  [2, 'float'],
  [19, 'complex'],
  [3, 'bool'],
  [4, 'NoneType'],
  [5, 'str'],
  [6, 'bytes'],
  [7, 'bytearray'],
  [8, 'list'],
  [9, 'tuple'],
  [10, 'dict'],
  [17, 'set'],
  [18, 'frozenset'],
  [11, 'range'],
  [12, 'slice'],
  [15, 'memoryview'],
  [100, 'object'],
  [101, 'type'],
  [102, 'BaseException'],
  [103, 'Exception'],
]);
const builtinTypes = new Map();
const builtinBaseTag = (tag) => {
  if (tag === 3) return 1;
  if (tag === 103) return 102;
  if (tag === 102) return 100;
  if (tag === 101) return 100;
  if (tag === 100) return null;
  if (tag === 4) return 100;
  return 100;
};
const getBuiltinType = (tag) => {
  if (builtinTypes.has(tag)) return builtinTypes.get(tag);
  if (tag === 102) {
    const clsBits = getBaseExceptionClass();
    builtinTypes.set(tag, clsBits);
    return clsBits;
  }
  if (tag === 103) {
    const clsBits = getExceptionClass();
    builtinTypes.set(tag, clsBits);
    return clsBits;
  }
  const name = BUILTIN_TYPE_TAGS.get(tag);
  if (!name) return boxNone();
  const baseTag = builtinBaseTag(tag);
  const baseBits = baseTag ? getBuiltinType(baseTag) : boxNone();
  const clsBits = boxPtr({
    type: 'class',
    name,
    attrs: new Map(),
    baseBits: boxNone(),
    basesBits: null,
    mroBits: null,
  });
  classLayoutVersions.set(clsBits, 0n);
  builtinTypes.set(tag, clsBits);
  setClassBases(clsBits, baseBits);
  return clsBits;
};
let generatorTypeBits = null;
const getGeneratorType = () => {
  if (generatorTypeBits !== null) return generatorTypeBits;
  const clsBits = boxPtr({
    type: 'class',
    name: 'generator',
    attrs: new Map(),
    baseBits: boxNone(),
    basesBits: null,
    mroBits: null,
  });
  classLayoutVersions.set(clsBits, 0n);
  setClassBases(clsBits, getBuiltinType(100));
  generatorTypeBits = clsBits;
  return clsBits;
};
let asyncGeneratorTypeBits = null;
const getAsyncGeneratorType = () => {
  if (asyncGeneratorTypeBits !== null) return asyncGeneratorTypeBits;
  const clsBits = boxPtr({
    type: 'class',
    name: 'async_generator',
    attrs: new Map(),
    baseBits: boxNone(),
    basesBits: null,
    mroBits: null,
  });
  classLayoutVersions.set(clsBits, 0n);
  setClassBases(clsBits, getBuiltinType(100));
  asyncGeneratorTypeBits = clsBits;
  return clsBits;
};
const heap = new Map();
const instanceClasses = new Map();
const classLayoutVersions = new Map();
const classFieldOffsets = new Map();
const instanceAttrs = new Map();
let nextPtr = 1n << 40n;
let memory = null;
let table = null;
let memViewCache = null;
let memViewBuffer = null;
const memView = () => {
  if (!memory) throw new Error('memory not initialized');
  const buf = memory.buffer;
  if (memViewCache === null || memViewBuffer !== buf) {
    memViewBuffer = buf;
    memViewCache = new DataView(buf);
  }
  return memViewCache;
};
const readMemoryBytes = (ptr, len) => {
  if (!memory) return null;
  const addr = expectPtrAddr(ptr, 'read_memory_bytes');
  const size = Number(len);
  if (!Number.isFinite(addr) || addr === 0 || !Number.isFinite(size) || size < 0) {
    return null;
  }
  return new Uint8Array(memory.buffer.slice(addr, addr + size));
};
const boxBytes = (data) => {
  const bytes = data instanceof Uint8Array ? data : Uint8Array.from(data);
  return boxPtr({ type: 'bytes', data: Uint8Array.from(bytes) });
};
const streamGet = (handle) => {
  const obj = getObj(handle);
  if (obj && obj.type === 'stream') return obj;
  return null;
};
const streamCreate = (_capacity = 0n) => {
  const handle = boxPtr({ type: 'stream', queue: [], closed: false });
  return { handle, obj: streamGet(handle) };
};
const streamRelease = (handle) => {
  const id = handle & POINTER_MASK;
  const obj = heap.get(id);
  if (obj && obj.type === 'stream') heap.delete(id);
};
const HOST_TABLE_FLAG = 0x80000000;
const HOST_TABLE_MASK = 0x7fffffff;
const hostTable = [];
const chanQueues = new Map();
const chanCaps = new Map();
const moduleCache = new Map();
const asyncgenRegistry = new Set();
const taskWaitingOn = new Map();
const runnableTasks = new Set();
const runnableQueue = [];
let codeSlots = null;
const frameStack = [];
let lastAttrName = null;
let lastAttrObjType = null;
const cancelPending = new Set();
const cancelTokens = new Map();
const taskTokens = new Map();
let nextCancelTokenId = 2n;
let currentTokenId = 1n;
let currentTaskPtr = 0;
let nextChanId = 1;
let nextLockId = 1;
let nextRLockId = 1;
const lockStates = new Map();
const rlockStates = new Map();
let sysVersionInfo = null;
let sysVersionStr = null;
let heapPtr = 1 << 20;
let lastBuiltinName = null;
let recursionLimit = 1000;
let recursionDepth = 0;
const HEADER_SIZE = 40;
const HEADER_POLL_FN_OFFSET = HEADER_SIZE - 8;
const HEADER_STATE_OFFSET = HEADER_SIZE - 16;
const HEADER_FLAGS_OFFSET = HEADER_SIZE - 32;
const GEN_SEND_OFFSET = 0;
const GEN_THROW_OFFSET = 8;
const GEN_CLOSED_OFFSET = 16;
const GEN_EXC_DEPTH_OFFSET = 24;
const GEN_FRAME_OFFSET = 32;
const GEN_YIELD_FROM_OFFSET = 40;
const GEN_CONTROL_SIZE = 48;
const TASK_KIND_FUTURE = 0n;
const TASK_KIND_GENERATOR = 1n;
const ASYNCGEN_OP_ANEXT = 0n;
const ASYNCGEN_OP_ASEND = 1n;
const ASYNCGEN_OP_ATHROW = 2n;
const ASYNCGEN_OP_ACLOSE = 3n;
const GEN_FLAG_STARTED = 1n << 2n;
const GEN_FLAG_RUNNING = 1n << 3n;
const align = (size, align) => (size + (align - 1)) & ~(align - 1);
const allocRaw = (payload) => {
  if (!memory) return 0;
  const bytes = align(Number(payload) + HEADER_SIZE, 8);
  const addr = heapPtr;
  heapPtr += bytes;
  const needed = heapPtr - memory.buffer.byteLength;
  if (needed > 0) {
    const pageSize = 65536;
    const pages = Math.ceil(needed / pageSize);
    memory.grow(pages);
  }
  new Uint8Array(memory.buffer, addr, bytes).fill(0);
  return addr + HEADER_SIZE;
};
const isNone = (val) => isTag(val, TAG_NONE);
const SLICE_INDEX_ERR =
  'slice indices must be integers or None or have an __index__ method';
const LIST_INDEX_ERR = 'slice indices must be integers or have an __index__ method';
const MAX_SLICE_STEP = BigInt(Number.MAX_SAFE_INTEGER);
const decodeSliceBound = (bits, len, defaultVal) => {
  if (isNone(bits)) return defaultVal;
  const idx = indexBigIntFromBits(bits, SLICE_INDEX_ERR);
  if (idx === null) return null;
  let value = idx;
  const lenBig = BigInt(len);
  if (value < 0n) value += lenBig;
  if (value < 0n) return 0;
  if (value > lenBig) return len;
  return Number(value);
};
const sliceBoundsFromArgs = (startBits, endBits, hasStartBits, hasEndBits, len) => {
  let start = 0;
  let end = len;
  let startGtLen = false;
  const lenBig = BigInt(len);
  if (isTruthyBits(hasStartBits)) {
    if (!isNone(startBits)) {
      let value = indexBigIntFromBits(startBits, SLICE_INDEX_ERR);
      if (value === null) return null;
      if (value < 0n) value += lenBig;
      startGtLen = value > lenBig;
      if (value < 0n) value = 0n;
      if (value > lenBig) value = lenBig;
      start = Number(value);
    }
  }
  if (isTruthyBits(hasEndBits)) {
    if (!isNone(endBits)) {
      let value = indexBigIntFromBits(endBits, SLICE_INDEX_ERR);
      if (value === null) return null;
      if (value < 0n) value += lenBig;
      if (value < 0n) value = 0n;
      if (value > lenBig) value = lenBig;
      end = Number(value);
    }
  }
  return { start, end, startGtLen };
};
const decodeSliceBoundNeg = (bits, len, defaultVal) => {
  if (isNone(bits)) return defaultVal;
  const idx = indexBigIntFromBits(bits, SLICE_INDEX_ERR);
  if (idx === null) return null;
  let value = idx;
  const lenBig = BigInt(len);
  if (value < 0n) value += lenBig;
  if (value < -1n) return -1;
  if (value >= lenBig) return len - 1;
  return Number(value);
};
const decodeSliceStep = (bits) => {
  if (isNone(bits)) return 1;
  const step = indexBigIntFromBits(bits, SLICE_INDEX_ERR);
  if (step === null) return null;
  if (step === 0n) {
    throw new Error('ValueError: slice step cannot be zero');
  }
  if (step > MAX_SLICE_STEP) return Number(MAX_SLICE_STEP);
  if (step < -MAX_SLICE_STEP) return Number(-MAX_SLICE_STEP);
  return Number(step);
};
const normalizeBytesRange = (
  total,
  startBits,
  endBits,
  hasStartBits,
  hasEndBits,
  startErr,
  endErr,
) => {
  let start = 0;
  let end = total;
  if (isTruthyBits(hasStartBits)) {
    const idx = indexFromBitsWithOverflow(startBits, startErr, null);
    if (idx === null) return null;
    start = idx;
  }
  if (isTruthyBits(hasEndBits)) {
    const idx = indexFromBitsWithOverflow(endBits, endErr, null);
    if (idx === null) return null;
    end = idx;
  }
  if (start < 0) start += total;
  if (end < 0) end += total;
  if (start < 0) start = 0;
  if (end < 0) end = 0;
  if (start > total) start = total;
  if (end > total) end = total;
  if (end < start) end = start;
  return { start, end };
};
const bytesFindInRange = (hay, needle, start, end) => {
  if (needle.length === 0) return start;
  const limit = end - needle.length;
  for (let i = start; i <= limit; i += 1) {
    let ok = true;
    for (let j = 0; j < needle.length; j += 1) {
      if (hay[i + j] !== needle[j]) {
        ok = false;
        break;
      }
    }
    if (ok) return i;
  }
  return -1;
};
const bytesStartsWithInRange = (hay, needle, start, end) => {
  const len = end - start;
  if (needle.length > len) return false;
  for (let i = 0; i < needle.length; i += 1) {
    if (hay[start + i] !== needle[i]) return false;
  }
  return true;
};
const bytesEndsWithInRange = (hay, needle, start, end) => {
  const len = end - start;
  if (needle.length > len) return false;
  const offset = end - needle.length;
  for (let i = 0; i < needle.length; i += 1) {
    if (hay[offset + i] !== needle[i]) return false;
  }
  return true;
};
const bytesReplaceLimited = (hay, needle, repl, count) => {
  if (count === 0) return hay.slice();
  if (needle.length === 0) {
    const out = [];
    let remaining = count;
    if (remaining < 0) remaining = hay.length + 1;
    if (remaining > 0) {
      for (const b of repl) out.push(b);
      remaining -= 1;
    }
    for (const b of hay) {
      out.push(b);
      if (remaining > 0) {
        for (const rb of repl) out.push(rb);
        remaining -= 1;
      }
    }
    return Uint8Array.from(out);
  }
  let remaining = count;
  if (remaining < 0) remaining = Number.MAX_SAFE_INTEGER;
  const out = [];
  let i = 0;
  while (i + needle.length <= hay.length) {
    let match = true;
    for (let j = 0; j < needle.length; j += 1) {
      if (hay[i + j] !== needle[j]) {
        match = false;
        break;
      }
    }
    if (match && remaining > 0) {
      for (const b of repl) out.push(b);
      remaining -= 1;
      i += needle.length;
    } else {
      out.push(hay[i]);
      i += 1;
    }
  }
  for (; i < hay.length; i += 1) out.push(hay[i]);
  return Uint8Array.from(out);
};
const normalizeSliceIndices = (len, startBits, stopBits, stepBits) => {
  const step = decodeSliceStep(stepBits);
  if (step === null) return null;
  if (step > 0) {
    const start = decodeSliceBound(startBits, len, 0);
    if (start === null) return null;
    const stop = decodeSliceBound(stopBits, len, len);
    if (stop === null) return null;
    return { start, stop, step };
  }
  const startDefault = len === 0 ? -1 : len - 1;
  const stopDefault = -1;
  const start = decodeSliceBoundNeg(startBits, len, startDefault);
  if (start === null) return null;
  const stop = decodeSliceBoundNeg(stopBits, len, stopDefault);
  if (stop === null) return null;
  return { start, stop, step };
};
const collectSliceIndices = (start, stop, step) => {
  const out = [];
  if (step > 0) {
    for (let i = start; i < stop; i += step) {
      out.push(i);
    }
  } else {
    for (let i = start; i > stop; i += step) {
      out.push(i);
    }
  }
  return out;
};
const boxPtrAddr = (addr) => QNAN | TAG_PTR | (BigInt(addr) & POINTER_MASK);
const ptrAddr = (val) => (typeof val === 'number' ? val : Number(val & POINTER_MASK));
const expectPtrAddr = (val, context) => {
  if (typeof val === 'number') {
    if (!Number.isInteger(val) || val < 0) {
      throw new TypeError(`TypeError: ${context} expected raw pointer address`);
    }
    return val;
  }
  if (val === 0n) return 0;
  if ((val & QNAN) === QNAN) {
    throw new TypeError(`TypeError: ${context} expected raw pointer address`);
  }
  return Number(val);
};
const boxPtr = (obj) => {
  const id = nextPtr++;
  heap.set(id, obj);
  return QNAN | TAG_PTR | id;
};
let missingBits = null;
const missingSentinel = () => {
  if (missingBits === null) {
    missingBits = boxPtr({ type: 'missing' });
  }
  return missingBits;
};
let ellipsisBits = null;
const ellipsisObj = () => {
  if (ellipsisBits === null) {
    ellipsisBits = boxPtr({ type: 'ellipsis' });
  }
  return ellipsisBits;
};
let notImplementedBits = null;
const notImplementedSentinel = () => {
  if (notImplementedBits === null) {
    notImplementedBits = boxPtr({ type: 'not_implemented' });
  }
  return notImplementedBits;
};
let anextDefaultPollIdx = null;
let generatorSendMethodIdx = null;
let generatorThrowMethodIdx = null;
let generatorCloseMethodIdx = null;
let generatorIterMethodIdx = null;
let generatorNextMethodIdx = null;
const generatorMethodBits = new Map();
let asyncgenAiterMethodIdx = null;
let asyncgenAnextMethodIdx = null;
let asyncgenAsendMethodIdx = null;
let asyncgenAthrowMethodIdx = null;
let asyncgenAcloseMethodIdx = null;
const asyncgenMethodBits = new Map();
let asyncgenPollIdx = null;
let promisePollIdx = null;
const tableFuncCache = new Map();
const getObj = (val) => heap.get(val & POINTER_MASK);
const getComplex = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'complex') return obj;
  return null;
};
const getAsyncGenerator = (val) => {
  const obj = getObj(val);
  return obj && obj.type === 'asyncgen' ? obj : null;
};
const isAsyncGenerator = (val) => getAsyncGenerator(val) !== null;
const ensureRootToken = () => {
  if (!cancelTokens.has(1n)) {
    cancelTokens.set(1n, { parent: 0n, cancelled: false, refs: 1n });
  }
};
const tokenIdFromBits = (bits) => {
  if (isTag(bits, TAG_NONE)) return 0n;
  if (!isTag(bits, TAG_INT)) {
    throw new Error('TypeError: cancel token id must be int');
  }
  const id = unboxInt(bits);
  if (id < 0n) {
    throw new Error('TypeError: cancel token id must be non-negative');
  }
  return id;
};
const retainToken = (id) => {
  if (id <= 1n) return;
  const entry = cancelTokens.get(id);
  if (entry) entry.refs += 1n;
};
const releaseToken = (id) => {
  if (id <= 1n) return;
  const entry = cancelTokens.get(id);
  if (!entry) return;
  entry.refs -= 1n;
  if (entry.refs <= 0n) {
    cancelTokens.delete(id);
  }
};
const registerTaskToken = (taskPtr, tokenId) => {
  const key = taskPtr.toString();
  const prev = taskTokens.get(key);
  if (prev !== undefined) {
    releaseToken(prev);
  }
  taskTokens.set(key, tokenId);
  retainToken(tokenId);
};
const ensureTaskToken = (taskPtr) => {
  const key = taskPtr.toString();
  const existing = taskTokens.get(key);
  if (existing !== undefined) return existing;
  registerTaskToken(taskPtr, currentTokenId);
  return currentTokenId;
};
const clearTaskToken = (taskPtr) => {
  const key = taskPtr.toString();
  const existing = taskTokens.get(key);
  if (existing !== undefined) {
    releaseToken(existing);
    taskTokens.delete(key);
  }
};
const tokenIsCancelled = (id) => {
  ensureRootToken();
  let current = id;
  let depth = 0;
  while (current !== 0n && depth < 64) {
    const entry = cancelTokens.get(current);
    if (!entry) return false;
    if (entry.cancelled) return true;
    current = entry.parent;
    depth += 1;
  }
  return false;
};
const setCurrentTokenId = (id) => {
  ensureRootToken();
  const next = id === 0n ? 1n : id;
  retainToken(next);
  const prev = currentTokenId;
  currentTokenId = next;
  releaseToken(prev);
  return prev;
};
const getFunction = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'function') return obj;
  return null;
};
const getCode = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'code') return obj;
  return null;
};
const getBoundMethod = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'bound_method') return obj;
  return null;
};
const isBoundMethod = (val) => getBoundMethod(val) !== null;
const getClass = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'class') return obj;
  return null;
};
const classLayoutSize = (classBits) => {
  if (!getClass(classBits)) return 8;
  const sizeBits = lookupClassAttr(classBits, '__molt_layout_size__');
  if (sizeBits !== undefined && isIntLike(sizeBits)) {
    return Number(unboxIntLike(sizeBits));
  }
  return 8;
};
const allocInstanceForClass = (classBits) => {
  const size = classLayoutSize(classBits);
  const addr = allocRaw(size);
  if (!addr) return boxNone();
  const instBits = boxPtrAddr(addr);
  instanceClasses.set(ptrAddr(instBits), classBits);
  return instBits;
};
const classLayoutVersion = (classBits) => {
  if (!getClass(classBits)) return null;
  const current = classLayoutVersions.get(classBits);
  if (current !== undefined) return current;
  classLayoutVersions.set(classBits, 0n);
  return 0n;
};
const bumpClassLayoutVersion = (classBits) => {
  const current = classLayoutVersion(classBits);
  if (current === null) return;
  classLayoutVersions.set(classBits, current + 1n);
};
const getModule = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'module') return obj;
  return null;
};
const getInstanceAttrMap = (objBits) => {
  if (!isPtr(objBits)) return null;
  if (heap.has(objBits & POINTER_MASK)) return null;
  const key = ptrAddr(objBits);
  let attrs = instanceAttrs.get(key);
  if (!attrs) {
    attrs = new Map();
    instanceAttrs.set(key, attrs);
  }
  return attrs;
};
const callAsyncFunction = (funcBits, func, args) => {
  const payload = [];
  if (func.closure && func.closure !== 0n && isPtr(func.closure)) {
    payload.push(func.closure);
  }
  payload.push(...args);
  let payloadBytes = payload.length * 8;
  const sizeBits = lookupAttr(funcBits, '__molt_closure_size__');
  if (sizeBits !== undefined && isIntLike(sizeBits)) {
    const size = Number(unboxIntLike(sizeBits));
    if (size > payloadBytes) {
      payloadBytes = size;
    }
  }
  const res = baseImports.alloc(payloadBytes);
  if (isNone(res)) return res;
  if (!memory) return boxNone();
  const addr = ptrAddr(res);
  const view = new DataView(memory.buffer);
  for (let i = 0; i < payload.length; i += 1) {
    view.setBigInt64(addr + i * 8, payload[i], true);
  }
  view.setUint32(addr - HEADER_POLL_FN_OFFSET, func.idx, true);
  view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
  return res;
};
const getTableFunc = (idx) => {
  if ((idx >>> 31) === 1) {
    return hostTable[idx & HOST_TABLE_MASK] ?? null;
  }
  if (!table) return null;
  return table.get(idx);
};
const DIRECT_CALL_MAX = 12;
const functionNeedsTaskTrampoline = (funcBits) => {
  const genBits = lookupAttr(funcBits, '__molt_is_generator__');
  if (genBits !== undefined && isTruthyBits(genBits)) return true;
  const coroBits = lookupAttr(funcBits, '__molt_is_coroutine__');
  if (coroBits !== undefined && isTruthyBits(coroBits)) return true;
  const asyncgenBits = lookupAttr(funcBits, '__molt_is_async_generator__');
  if (asyncgenBits !== undefined && isTruthyBits(asyncgenBits)) return true;
  return false;
};
const callFunctionTrampoline = (func, args) => {
  if (!memory) {
    throw new Error('RuntimeError: wasm memory unavailable for trampoline');
  }
  if (!func || !func.trampoline) {
    throw new Error('RuntimeError: function trampoline missing');
  }
  const trampFn = getTableFunc(func.trampoline);
  if (!trampFn) {
    throw new Error('RuntimeError: function trampoline not found');
  }
  let addr = 0;
  if (args.length) {
    addr = allocRaw(args.length * 8);
    if (!addr) {
      throw new Error('RuntimeError: trampoline arg allocation failed');
    }
    const view = new DataView(memory.buffer);
    for (let i = 0; i < args.length; i += 1) {
      view.setBigInt64(addr + i * 8, args[i], true);
    }
  }
  const closureBits = func.closure !== undefined ? BigInt(func.closure) : 0n;
  return trampFn(closureBits, BigInt(addr), BigInt(args.length));
};
const callFunctionBits = (funcBits, args) => {
  const func = getFunction(funcBits);
  if (!func) {
    throw new Error('TypeError: call expects function object');
  }
  const fn = getTableFunc(func.idx);
  if (!fn) {
    throw new Error('TypeError: call expects function object');
  }
  if (functionNeedsTaskTrampoline(funcBits)) {
    return callFunctionTrampoline(func, args);
  }
  if (func.trampoline && args.length > DIRECT_CALL_MAX && memory) {
    return callFunctionTrampoline(func, args);
  }
  if (func.closure && func.closure !== 0n && isPtr(func.closure)) {
    return fn(func.closure, ...args);
  }
  return fn(...args);
};
const callCallable0 = (callableBits) => {
  const bound = getBoundMethod(callableBits);
  if (bound) {
    return callFunctionBits(bound.func, [bound.self]);
  }
  const func = getFunction(callableBits);
  if (func) {
    return callFunctionBits(callableBits, []);
  }
  const cls = getClass(callableBits);
  if (cls) {
    const instBits = allocInstanceForClass(callableBits);
    const initBits = lookupClassAttr(callableBits, '__init__', instBits);
    if (initBits !== undefined) {
      callCallable0(initBits);
    }
    return instBits;
  }
  throw new Error('TypeError: object is not callable');
};
const callCallable1 = (callableBits, arg0) => {
  const bound = getBoundMethod(callableBits);
  if (bound) {
    return callFunctionBits(bound.func, [bound.self, arg0]);
  }
  const func = getFunction(callableBits);
  if (func) {
    return callFunctionBits(callableBits, [arg0]);
  }
  const cls = getClass(callableBits);
  if (cls) {
    const instBits = allocInstanceForClass(callableBits);
    const initBits = lookupClassAttr(callableBits, '__init__', instBits);
    if (initBits !== undefined) {
      callCallable1(initBits, arg0);
    }
    return instBits;
  }
  throw new Error('TypeError: object is not callable');
};
const callCallable2 = (callableBits, arg0, arg1) => {
  const bound = getBoundMethod(callableBits);
  if (bound) {
    return callFunctionBits(bound.func, [bound.self, arg0, arg1]);
  }
  const func = getFunction(callableBits);
  if (func) {
    return callFunctionBits(callableBits, [arg0, arg1]);
  }
  const cls = getClass(callableBits);
  if (cls) {
    const instBits = allocInstanceForClass(callableBits);
    const initBits = lookupClassAttr(callableBits, '__init__', instBits);
    if (initBits !== undefined) {
      callCallable2(initBits, arg0, arg1);
    }
    return instBits;
  }
  throw new Error('TypeError: object is not callable');
};
const callableArity = (callableBits) => {
  const bound = getBoundMethod(callableBits);
  if (bound) {
    const func = getFunction(bound.func);
    if (func) {
      const fn = getTableFunc(func.idx);
      if (fn) return fn.length;
    }
  }
  const func = getFunction(callableBits);
  if (func) {
    const fn = getTableFunc(func.idx);
    if (fn) return fn.length;
  }
  return 0;
};
const numberFromVal = (val) => {
  if (isTag(val, TAG_INT)) return Number(unboxInt(val));
  if (isTag(val, TAG_BOOL)) return (val & 1n) === 1n ? 1 : 0;
  if (isFloat(val)) return bitsToFloat(val);
  return null;
};
const complexFromValStrict = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'complex') return { re: obj.re, im: obj.im };
  if (isFloat(val)) return { re: bitsToFloat(val), im: 0 };
  if (isIntLike(val)) return { re: Number(unboxIntLike(val)), im: 0 };
  if (obj && obj.type === 'bigint') {
    const num = Number(obj.value);
    if (!Number.isFinite(num)) return { overflow: true };
    return { re: num, im: 0 };
  }
  return null;
};
const complexFromValLossy = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'complex') return { re: obj.re, im: obj.im };
  if (isFloat(val)) return { re: bitsToFloat(val), im: 0 };
  if (isIntLike(val)) return { re: Number(unboxIntLike(val)), im: 0 };
  if (obj && obj.type === 'bigint') {
    const num = Number(obj.value);
    if (!Number.isFinite(num)) return null;
    return { re: num, im: 0 };
  }
  return null;
};
const complexPow = (base, exp) => {
  if (base.re === 0 && base.im === 0) {
    if (exp.re === 0 && exp.im === 0) {
      return { re: 1, im: 0 };
    }
    if (exp.im !== 0 || exp.re < 0) {
      return null;
    }
    return { re: 0, im: 0 };
  }
  const r = Math.hypot(base.re, base.im);
  const theta = Math.atan2(base.im, base.re);
  const logR = Math.log(r);
  const u = exp.re * logR - exp.im * theta;
  const v = exp.im * logR + exp.re * theta;
  const expU = Math.exp(u);
  return { re: expU * Math.cos(v), im: expU * Math.sin(v) };
};
const typeName = (val) => {
  if (isTag(val, TAG_NONE)) return 'NoneType';
  if (isTag(val, TAG_BOOL)) return 'bool';
  if (isTag(val, TAG_INT)) return 'int';
  if (isFloat(val)) return 'float';
  const obj = getObj(val);
  if (obj) {
    if (obj.type === 'class') return obj.name ?? 'type';
    if (obj.type === 'exception') {
      const classBits = obj.classBits;
      if (classBits && !isNone(classBits)) {
        const cls = getClass(classBits);
        if (cls && cls.name) return cls.name;
      }
      return getStr(obj.kindBits) || 'Exception';
    }
    if (obj.type === 'str') return 'str';
    if (obj.type === 'bytes') return 'bytes';
    if (obj.type === 'bytearray') return 'bytearray';
    if (obj.type === 'list') return 'list';
    if (obj.type === 'tuple') return 'tuple';
    if (obj.type === 'slice') return 'slice';
    if (obj.type === 'memoryview') return 'memoryview';
    if (obj.type === 'ellipsis') return 'ellipsis';
    if (obj.type === 'set') return 'set';
    if (obj.type === 'frozenset') return 'frozenset';
    if (obj.type === 'dict') return 'dict';
    if (obj.type === 'dict_keys') return 'dict_keys';
    if (obj.type === 'dict_values') return 'dict_values';
    if (obj.type === 'dict_items') return 'dict_items';
    if (obj.type === 'complex') return 'complex';
    if (obj.type === 'module') return 'module';
    if (obj.type === 'function') return 'function';
    if (obj.type === 'map') return 'map';
    if (obj.type === 'filter') return 'filter';
    if (obj.type === 'zip') return 'zip';
    if (obj.type === 'reversed') return 'reversed';
    if (obj.type === 'asyncgen') return 'async_generator';
    if (obj.type === 'call_iter') return 'callable_iterator';
  }
  if (isPtr(val) && !heap.has(val & POINTER_MASK)) {
    const clsBits = instanceClasses.get(ptrAddr(val));
    if (clsBits !== undefined) {
      const cls = getClass(clsBits);
      if (cls && cls.name) return cls.name;
    }
  }
  return 'object';
};
const byteFromBits = (valBits) => {
  let value = getBigIntValue(valBits);
  if (value === null) {
    const indexAttr = lookupAttr(valBits, '__index__');
    if (indexAttr !== undefined) {
      const res = callCallable0(indexAttr);
      if (exceptionPending() !== 0n) return null;
      value = getBigIntValue(res);
      if (value === null) {
        throw new Error(`TypeError: __index__ returned non-int (type ${typeName(res)})`);
      }
    }
  }
  if (value === null) {
    throw new Error(
      `TypeError: '${typeName(valBits)}' object cannot be interpreted as an integer`,
    );
  }
  if (value < 0n || value > 255n) {
    throw new Error('ValueError: byte must be in range(0, 256)');
  }
  return Number(value);
};
const collectIterableValues = (bits, errMsg) => {
  const iterBits = baseImports.iter(bits);
  if (isNone(iterBits)) {
    if (exceptionPending() !== 0n) return null;
    throw new Error(`TypeError: ${errMsg}`);
  }
  const out = [];
  while (true) {
    const pairBits = baseImports.iter_next(iterBits);
    if (exceptionPending() !== 0n) return null;
    const tuple = getTuple(pairBits);
    if (!tuple || tuple.items.length < 2) return null;
    if (isTruthyBits(tuple.items[1])) break;
    out.push(tuple.items[0]);
  }
  return out;
};
const MEMORYVIEW_LONG_SIZE = 4;
const MEMORYVIEW_SIZE_T_SIZE = 4;
const MEMORYVIEW_PTR_SIZE = 4;
const memoryviewFormatFromStr = (formatStr) => {
  if (!formatStr) return null;
  let code = null;
  if (formatStr.length === 1) {
    code = formatStr[0];
  } else if (formatStr.length === 2 && formatStr[0] === '@') {
    code = formatStr[1];
  } else {
    return null;
  }
  switch (code) {
    case 'b':
      return { code, itemsize: 1, kind: 'signed' };
    case 'B':
      return { code, itemsize: 1, kind: 'unsigned' };
    case 'h':
      return { code, itemsize: 2, kind: 'signed' };
    case 'H':
      return { code, itemsize: 2, kind: 'unsigned' };
    case 'i':
      return { code, itemsize: 4, kind: 'signed' };
    case 'I':
      return { code, itemsize: 4, kind: 'unsigned' };
    case 'l':
      return { code, itemsize: MEMORYVIEW_LONG_SIZE, kind: 'signed' };
    case 'L':
      return { code, itemsize: MEMORYVIEW_LONG_SIZE, kind: 'unsigned' };
    case 'q':
      return { code, itemsize: 8, kind: 'signed' };
    case 'Q':
      return { code, itemsize: 8, kind: 'unsigned' };
    case 'n':
      return { code, itemsize: MEMORYVIEW_SIZE_T_SIZE, kind: 'signed' };
    case 'N':
      return { code, itemsize: MEMORYVIEW_SIZE_T_SIZE, kind: 'unsigned' };
    case 'P':
      return { code, itemsize: MEMORYVIEW_PTR_SIZE, kind: 'unsigned' };
    case 'f':
      return { code, itemsize: 4, kind: 'float' };
    case 'd':
      return { code, itemsize: 8, kind: 'float' };
    case '?':
      return { code, itemsize: 1, kind: 'bool' };
    case 'c':
      return { code, itemsize: 1, kind: 'char' };
    default:
      return null;
  }
};
const memoryviewFormatFromBits = (bits) => {
  const format = getStrObj(bits);
  if (format === null) return null;
  return memoryviewFormatFromStr(format);
};
const memoryviewShape = (view) => {
  if (Array.isArray(view.shape)) return view.shape;
  const ndim = view.ndim ?? 1;
  if (ndim === 0) return [];
  return [view.len ?? 0];
};
const memoryviewStrides = (view) => {
  if (Array.isArray(view.strides)) return view.strides;
  const ndim = view.ndim ?? 1;
  if (ndim === 0) return [];
  return [view.stride ?? view.itemsize];
};
const memoryviewShapeProduct = (shape) => {
  let total = 1n;
  for (const dim of shape) {
    if (dim < 0) return null;
    total *= BigInt(dim);
  }
  return total;
};
const stringCountFromChars = (hayChars, needleChars) => {
  if (needleChars.length === 0) return hayChars.length + 1;
  let count = 0;
  let idx = 0;
  while (idx + needleChars.length <= hayChars.length) {
    let match = true;
    for (let j = 0; j < needleChars.length; j += 1) {
      if (hayChars[idx + j] !== needleChars[j]) {
        match = false;
        break;
      }
    }
    if (match) {
      count += 1;
      idx += needleChars.length;
    } else {
      idx += 1;
    }
  }
  return count;
};
const stringStripChars = (hay, chars) => {
  if (chars === '') return hay;
  const hayChars = Array.from(hay);
  const stripSet = new Set(Array.from(chars));
  let start = 0;
  while (start < hayChars.length && stripSet.has(hayChars[start])) {
    start += 1;
  }
  let end = hayChars.length;
  while (end > start && stripSet.has(hayChars[end - 1])) {
    end -= 1;
  }
  return hayChars.slice(start, end).join('');
};
const stringLStripChars = (hay, chars) => {
  if (chars === '') return hay;
  const hayChars = Array.from(hay);
  const stripSet = new Set(Array.from(chars));
  let start = 0;
  while (start < hayChars.length && stripSet.has(hayChars[start])) {
    start += 1;
  }
  return hayChars.slice(start).join('');
};
const stringRStripChars = (hay, chars) => {
  if (chars === '') return hay;
  const hayChars = Array.from(hay);
  const stripSet = new Set(Array.from(chars));
  let end = hayChars.length;
  while (end > 0 && stripSet.has(hayChars[end - 1])) {
    end -= 1;
  }
  return hayChars.slice(0, end).join('');
};
const splitMaxsplitFromBits = (bits) => {
  const errMsg = `'${typeName(bits)}' object cannot be interpreted as an integer`;
  const value = indexBigIntFromBits(bits, errMsg);
  if (value === null) return null;
  if (value < 0n) return -1;
  const maxSafe = BigInt(Number.MAX_SAFE_INTEGER);
  if (value > maxSafe) return Number.MAX_SAFE_INTEGER;
  return Number(value);
};
const isWhitespaceChar = (ch) => /\\s/u.test(ch);
const stringSplitWhitespaceMax = (hay, maxsplit) => {
  const chars = Array.from(hay);
  const out = [];
  let idx = 0;
  while (idx < chars.length && isWhitespaceChar(chars[idx])) {
    idx += 1;
  }
  if (idx >= chars.length) return out;
  if (maxsplit === 0) {
    out.push(chars.slice(idx).join(''));
    return out;
  }
  const limit = maxsplit < 0 ? Number.MAX_SAFE_INTEGER : maxsplit;
  let splits = 0;
  while (idx < chars.length && splits < limit) {
    const start = idx;
    while (idx < chars.length && !isWhitespaceChar(chars[idx])) {
      idx += 1;
    }
    out.push(chars.slice(start, idx).join(''));
    while (idx < chars.length && isWhitespaceChar(chars[idx])) {
      idx += 1;
    }
    splits += 1;
  }
  if (idx < chars.length) {
    out.push(chars.slice(idx).join(''));
  }
  return out;
};
const stringSplitSepMax = (hay, needle, maxsplit) => {
  const hayChars = Array.from(hay);
  const needleChars = Array.from(needle);
  if (needleChars.length === 0) {
    throw new Error('ValueError: empty separator');
  }
  if (maxsplit === 0) return [hay];
  const limit = maxsplit < 0 ? Number.MAX_SAFE_INTEGER : maxsplit;
  const out = [];
  let start = 0;
  let idx = 0;
  let splits = 0;
  while (idx + needleChars.length <= hayChars.length && splits < limit) {
    let match = true;
    for (let j = 0; j < needleChars.length; j += 1) {
      if (hayChars[idx + j] !== needleChars[j]) {
        match = false;
        break;
      }
    }
    if (match) {
      out.push(hayChars.slice(start, idx).join(''));
      idx += needleChars.length;
      start = idx;
      splits += 1;
    } else {
      idx += 1;
    }
  }
  out.push(hayChars.slice(start).join(''));
  return out;
};
const isAsciiWhitespace = (byte) =>
  byte === 9 || byte === 10 || byte === 11 || byte === 12 || byte === 13 || byte === 32;
const bytesSplitWhitespaceMax = (hay, maxsplit) => {
  const out = [];
  let idx = 0;
  while (idx < hay.length && isAsciiWhitespace(hay[idx])) {
    idx += 1;
  }
  if (idx >= hay.length) return out;
  if (maxsplit === 0) {
    out.push(hay.slice(idx));
    return out;
  }
  const limit = maxsplit < 0 ? Number.MAX_SAFE_INTEGER : maxsplit;
  let splits = 0;
  while (idx < hay.length && splits < limit) {
    const start = idx;
    while (idx < hay.length && !isAsciiWhitespace(hay[idx])) {
      idx += 1;
    }
    out.push(hay.slice(start, idx));
    while (idx < hay.length && isAsciiWhitespace(hay[idx])) {
      idx += 1;
    }
    splits += 1;
  }
  if (idx < hay.length) {
    out.push(hay.slice(idx));
  }
  return out;
};
const bytesSplitSepMax = (hay, needle, maxsplit) => {
  if (needle.length === 0) {
    throw new Error('ValueError: empty separator');
  }
  if (maxsplit === 0) return [hay.slice()];
  const limit = maxsplit < 0 ? Number.MAX_SAFE_INTEGER : maxsplit;
  const out = [];
  let start = 0;
  let idx = 0;
  let splits = 0;
  while (idx + needle.length <= hay.length && splits < limit) {
    let match = true;
    for (let j = 0; j < needle.length; j += 1) {
      if (hay[idx + j] !== needle[j]) {
        match = false;
        break;
      }
    }
    if (match) {
      out.push(hay.slice(start, idx));
      idx += needle.length;
      start = idx;
      splits += 1;
    } else {
      idx += 1;
    }
  }
  out.push(hay.slice(start));
  return out;
};
const memoryviewNbytesBig = (view) => {
  const shape = memoryviewShape(view);
  const product = memoryviewShapeProduct(shape);
  if (product === null) return null;
  return product * BigInt(view.itemsize);
};
const memoryviewNbytes = (view) => {
  const total = memoryviewNbytesBig(view);
  if (total === null) return null;
  if (total > BigInt(Number.MAX_SAFE_INTEGER)) return null;
  return Number(total);
};
const memoryviewIsCContiguous = (shape, strides, itemsize) => {
  if (shape.length !== strides.length) return false;
  let expected = itemsize;
  for (let idx = shape.length - 1; idx >= 0; idx -= 1) {
    const dim = shape[idx];
    const stride = strides[idx];
    if (dim > 1 && stride !== expected) return false;
    expected *= Math.max(1, dim);
  }
  return true;
};
const memoryviewIsCContiguousView = (view) =>
  memoryviewIsCContiguous(memoryviewShape(view), memoryviewStrides(view), view.itemsize);
const memoryviewCollectBytes = (view) => {
  const owner = getBytes(view.ownerBits) || getBytearray(view.ownerBits);
  if (!owner) return null;
  const data = owner.data;
  const shape = memoryviewShape(view);
  const strides = memoryviewStrides(view);
  if (shape.length !== strides.length) return null;
  const nbytes = memoryviewNbytes(view);
  if (nbytes === null) return null;
  const offset = view.offset;
  const out = [];
  if (memoryviewIsCContiguous(shape, strides, view.itemsize)) {
    const start = offset;
    const end = offset + nbytes;
    if (start < 0 || end < 0 || end > data.length) return null;
    for (let i = start; i < end; i += 1) {
      out.push(data[i]);
    }
    return out;
  }
  const total = memoryviewShapeProduct(shape);
  if (total === null) return null;
  const totalCount = Number(total);
  const indices = new Array(shape.length).fill(0);
  for (let count = 0; count < totalCount; count += 1) {
    let pos = offset;
    for (let axis = 0; axis < shape.length; axis += 1) {
      pos += indices[axis] * strides[axis];
    }
    if (pos < 0 || pos + view.itemsize > data.length) return null;
    for (let i = 0; i < view.itemsize; i += 1) {
      out.push(data[pos + i]);
    }
    for (let axis = shape.length - 1; axis >= 0; axis -= 1) {
      indices[axis] += 1;
      if (indices[axis] < shape[axis]) break;
      indices[axis] = 0;
    }
  }
  return out;
};
const memoryviewBytes = (view) => memoryviewCollectBytes(view);
const memoryviewReadScalar = (data, offset, fmt) => {
  if (offset < 0 || offset + fmt.itemsize > data.length) return null;
  if (fmt.kind === 'char') {
    return boxPtr({ type: 'bytes', data: Uint8Array.from([data[offset]]) });
  }
  if (fmt.kind === 'bool') {
    return boxBool(data[offset] !== 0);
  }
  const buf = new ArrayBuffer(fmt.itemsize);
  const view = new DataView(buf);
  for (let i = 0; i < fmt.itemsize; i += 1) {
    view.setUint8(i, data[offset + i]);
  }
  if (fmt.kind === 'float') {
    const num = fmt.itemsize === 4 ? view.getFloat32(0, true) : view.getFloat64(0, true);
    return boxFloat(num);
  }
  if (fmt.kind === 'signed') {
    let value = 0n;
    if (fmt.itemsize === 1) value = BigInt(view.getInt8(0));
    else if (fmt.itemsize === 2) value = BigInt(view.getInt16(0, true));
    else if (fmt.itemsize === 4) value = BigInt(view.getInt32(0, true));
    else if (fmt.itemsize === 8) value = view.getBigInt64(0, true);
    else return null;
    return boxIntOrBigint(value);
  }
  if (fmt.kind === 'unsigned') {
    let value = 0n;
    if (fmt.itemsize === 1) value = BigInt(view.getUint8(0));
    else if (fmt.itemsize === 2) value = BigInt(view.getUint16(0, true));
    else if (fmt.itemsize === 4) value = BigInt(view.getUint32(0, true));
    else if (fmt.itemsize === 8) value = view.getBigUint64(0, true);
    else return null;
    return boxIntOrBigint(value);
  }
  return null;
};
const memoryviewWriteScalar = (data, offset, fmt, valBits) => {
  if (offset < 0 || offset + fmt.itemsize > data.length) return null;
  if (fmt.kind === 'char') {
    const bytes = getBytes(valBits);
    if (!bytes) {
      throw new Error(`TypeError: memoryview: invalid type for format '${fmt.code}'`);
    }
    if (bytes.data.length !== 1) {
      throw new Error(`ValueError: memoryview: invalid value for format '${fmt.code}'`);
    }
    data[offset] = bytes.data[0];
    return true;
  }
  if (fmt.kind === 'bool') {
    data[offset] = isTruthyBits(valBits) ? 1 : 0;
    return true;
  }
  if (fmt.kind === 'float') {
    let num = numberFromVal(valBits);
    if (num === null) {
      const obj = getObj(valBits);
      if (obj && obj.type === 'bigint') {
        num = Number(obj.value);
      }
    }
    if (num === null) {
      throw new Error(`TypeError: memoryview: invalid type for format '${fmt.code}'`);
    }
    const buf = new ArrayBuffer(fmt.itemsize);
    const view = new DataView(buf);
    if (fmt.itemsize === 4) view.setFloat32(0, num, true);
    else if (fmt.itemsize === 8) view.setFloat64(0, num, true);
    else return null;
    for (let i = 0; i < fmt.itemsize; i += 1) {
      data[offset + i] = view.getUint8(i);
    }
    return true;
  }
  const errMsg = `memoryview: invalid type for format '${fmt.code}'`;
  const value = indexBigIntFromBits(valBits, errMsg);
  if (value === null) return null;
  const bits = BigInt(fmt.itemsize * 8);
  let min = 0n;
  let max = 0n;
  if (fmt.kind === 'signed') {
    const limit = 1n << (bits - 1n);
    min = -limit;
    max = limit - 1n;
  } else if (fmt.kind === 'unsigned') {
    min = 0n;
    max = (1n << bits) - 1n;
  } else {
    return null;
  }
  if (value < min || value > max) {
    throw new Error(`ValueError: memoryview: invalid value for format '${fmt.code}'`);
  }
  const buf = new ArrayBuffer(fmt.itemsize);
  const view = new DataView(buf);
  if (fmt.kind === 'signed') {
    if (fmt.itemsize === 1) view.setInt8(0, Number(value));
    else if (fmt.itemsize === 2) view.setInt16(0, Number(value), true);
    else if (fmt.itemsize === 4) view.setInt32(0, Number(value), true);
    else if (fmt.itemsize === 8) view.setBigInt64(0, value, true);
    else return null;
  } else if (fmt.itemsize === 1) {
    view.setUint8(0, Number(value));
  } else if (fmt.itemsize === 2) {
    view.setUint16(0, Number(value), true);
  } else if (fmt.itemsize === 4) {
    view.setUint32(0, Number(value), true);
  } else if (fmt.itemsize === 8) {
    view.setBigUint64(0, value, true);
  } else {
    return null;
  }
  for (let i = 0; i < fmt.itemsize; i += 1) {
    data[offset + i] = view.getUint8(i);
  }
  return true;
};
const collectBytearrayAssignBytes = (bits) => {
  const bytes = getBytes(bits);
  if (bytes) return [...bytes.data];
  const bytearray = getBytearray(bits);
  if (bytearray) return [...bytearray.data];
  const view = getMemoryview(bits);
  if (view) return memoryviewBytes(view);
  const strVal = getStrObj(bits);
  if (strVal !== null) {
    throw new Error(
      'TypeError: can assign only bytes, buffers, or iterables of ints in range(0, 256)',
    );
  }
  const iterBits = baseImports.iter(bits);
  if (isNone(iterBits)) {
    if (exceptionPending() !== 0n) return null;
    throw new Error(
      'TypeError: can assign only bytes, buffers, or iterables of ints in range(0, 256)',
    );
  }
  const out = [];
  while (true) {
    const pairBits = baseImports.iter_next(iterBits);
    if (exceptionPending() !== 0n) return null;
    const tuple = getTuple(pairBits);
    if (!tuple || tuple.items.length < 2) return null;
    if (isTruthyBits(tuple.items[1])) break;
    const byte = byteFromBits(tuple.items[0]);
    if (byte === null) return null;
    out.push(byte);
  }
  return out;
};
const collectMemoryviewAssignBytes = (bits) => {
  const bytes = getBytes(bits);
  if (bytes) return [...bytes.data];
  const bytearray = getBytearray(bits);
  if (bytearray) return [...bytearray.data];
  const view = getMemoryview(bits);
  if (view) return memoryviewBytes(view);
  throw new Error(`TypeError: a bytes-like object is required, not '${typeName(bits)}'`);
};
const compareTypeError = (op, left, right) => {
  throw new Error(
    `TypeError: '${op}' not supported between instances of '${typeName(
      left,
    )}' and '${typeName(right)}'`
  );
};
const compareBigIntFloat = (big, num) => {
  if (Number.isNaN(num)) return { kind: 'unordered' };
  if (!Number.isFinite(num)) {
    return { kind: 'ordered', ordering: num < 0 ? 1 : -1 };
  }
  const bigNum = Number(big);
  if (!Number.isFinite(bigNum)) {
    return { kind: 'ordered', ordering: big < 0n ? -1 : 1 };
  }
  if (bigNum === num && Number.isInteger(num) && big === BigInt(num)) {
    return { kind: 'ordered', ordering: 0 };
  }
  return { kind: 'ordered', ordering: bigNum < num ? -1 : 1 };
};
const compareNumbersOutcome = (left, right) => {
  const leftBig = getBigIntValue(left);
  const rightBig = getBigIntValue(right);
  if (leftBig !== null && rightBig !== null) {
    if (leftBig === rightBig) return { kind: 'ordered', ordering: 0 };
    return { kind: 'ordered', ordering: leftBig < rightBig ? -1 : 1 };
  }
  const leftNum = numberFromVal(left);
  const rightNum = numberFromVal(right);
  if (leftNum !== null && rightNum !== null) {
    if (Number.isNaN(leftNum) || Number.isNaN(rightNum)) {
      return { kind: 'unordered' };
    }
    if (leftNum === rightNum) return { kind: 'ordered', ordering: 0 };
    return { kind: 'ordered', ordering: leftNum < rightNum ? -1 : 1 };
  }
  if (leftBig !== null && rightNum !== null) {
    return compareBigIntFloat(leftBig, rightNum);
  }
  if (rightBig !== null && leftNum !== null) {
    const outcome = compareBigIntFloat(rightBig, leftNum);
    if (outcome.kind === 'ordered') {
      return { kind: 'ordered', ordering: -outcome.ordering };
    }
    return outcome;
  }
  if ((leftBig !== null || leftNum !== null) && (rightBig !== null || rightNum !== null)) {
    return { kind: 'unordered' };
  }
  return { kind: 'notComparable' };
};
const compareObjectsBuiltin = (left, right) => {
  const numOutcome = compareNumbersOutcome(left, right);
  if (numOutcome.kind !== 'notComparable') return numOutcome;
  const leftObj = getObj(left);
  const rightObj = getObj(right);
  if (!leftObj || !rightObj) return { kind: 'notComparable' };
  if (leftObj.type === 'str' && rightObj.type === 'str') {
    if (leftObj.value === rightObj.value) return { kind: 'ordered', ordering: 0 };
    return { kind: 'ordered', ordering: leftObj.value < rightObj.value ? -1 : 1 };
  }
  const leftBytes =
    leftObj.type === 'bytes' || leftObj.type === 'bytearray' ? leftObj.data : null;
  const rightBytes =
    rightObj.type === 'bytes' || rightObj.type === 'bytearray' ? rightObj.data : null;
  if (leftBytes && rightBytes) {
    const cmp = Buffer.from(leftBytes).compare(Buffer.from(rightBytes));
    return { kind: 'ordered', ordering: cmp === 0 ? 0 : cmp < 0 ? -1 : 1 };
  }
  if (leftObj.type === 'list' && rightObj.type === 'list') {
    const common = Math.min(leftObj.items.length, rightObj.items.length);
    for (let i = 0; i < common; i += 1) {
      const lBits = leftObj.items[i];
      const rBits = rightObj.items[i];
      if (isTruthyBits(baseImports.eq(lBits, rBits))) {
        continue;
      }
      return compareObjects(lBits, rBits);
    }
    if (leftObj.items.length === rightObj.items.length) {
      return { kind: 'ordered', ordering: 0 };
    }
    return {
      kind: 'ordered',
      ordering: leftObj.items.length < rightObj.items.length ? -1 : 1,
    };
  }
  if (leftObj.type === 'tuple' && rightObj.type === 'tuple') {
    const common = Math.min(leftObj.items.length, rightObj.items.length);
    for (let i = 0; i < common; i += 1) {
      const lBits = leftObj.items[i];
      const rBits = rightObj.items[i];
      if (isTruthyBits(baseImports.eq(lBits, rBits))) {
        continue;
      }
      return compareObjects(lBits, rBits);
    }
    if (leftObj.items.length === rightObj.items.length) {
      return { kind: 'ordered', ordering: 0 };
    }
    return {
      kind: 'ordered',
      ordering: leftObj.items.length < rightObj.items.length ? -1 : 1,
    };
  }
  return { kind: 'notComparable' };
};
const richCompareBool = (left, right, opName, reverseName) => {
  const leftAttr = lookupAttr(left, opName);
  if (leftAttr !== undefined) {
    const res = callCallable1(leftAttr, right);
    if (exceptionPending() !== 0n) return { kind: 'error' };
    return { kind: 'bool', value: isTruthyBits(res) };
  }
  if (exceptionPending() !== 0n) return { kind: 'error' };
  const rightAttr = lookupAttr(right, reverseName);
  if (rightAttr !== undefined) {
    const res = callCallable1(rightAttr, left);
    if (exceptionPending() !== 0n) return { kind: 'error' };
    return { kind: 'bool', value: isTruthyBits(res) };
  }
  if (exceptionPending() !== 0n) return { kind: 'error' };
  return { kind: 'notComparable' };
};
const richCompareOrder = (left, right) => {
  const lt = richCompareBool(left, right, '__lt__', '__gt__');
  if (lt.kind === 'error') return { kind: 'error' };
  if (lt.kind === 'notComparable') return { kind: 'notComparable' };
  if (lt.value) return { kind: 'ordered', ordering: -1 };
  const gt = richCompareBool(right, left, '__lt__', '__gt__');
  if (gt.kind === 'error') return { kind: 'error' };
  if (gt.kind === 'notComparable') return { kind: 'notComparable' };
  if (gt.value) return { kind: 'ordered', ordering: 1 };
  return { kind: 'ordered', ordering: 0 };
};
const compareObjects = (left, right) => {
  const outcome = compareObjectsBuiltin(left, right);
  if (outcome.kind !== 'notComparable') return outcome;
  return richCompareOrder(left, right);
};
const compareBuiltinBool = (left, right, op) => {
  const outcome = compareObjectsBuiltin(left, right);
  if (outcome.kind === 'ordered') {
    if (op === '<') return { kind: 'bool', value: outcome.ordering < 0 };
    if (op === '<=') return { kind: 'bool', value: outcome.ordering <= 0 };
    if (op === '>') return { kind: 'bool', value: outcome.ordering > 0 };
    if (op === '>=') return { kind: 'bool', value: outcome.ordering >= 0 };
  }
  if (outcome.kind === 'unordered') return { kind: 'bool', value: false };
  if (outcome.kind === 'error') return { kind: 'error' };
  return { kind: 'notComparable' };
};
const compareKeys = (left, right, op) => {
  const outcome = compareObjects(left, right);
  if (outcome.kind === 'ordered') return outcome.ordering;
  if (outcome.kind === 'unordered') return 0;
  if (outcome.kind === 'error') return null;
  compareTypeError(op, left, right);
  return 0;
};
const heapLt = (left, right) => {
  const ordering = compareKeys(left, right, '<');
  if (ordering === null) return null;
  return ordering < 0;
};
const heapSiftDown = (heap, startpos, pos) => {
  const newitem = heap[pos];
  while (pos > startpos) {
    const parentpos = (pos - 1) >> 1;
    const parent = heap[parentpos];
    const lt = heapLt(newitem, parent);
    if (lt === null) return null;
    if (lt) {
      heap[pos] = parent;
      pos = parentpos;
      continue;
    }
    break;
  }
  heap[pos] = newitem;
  return true;
};
const heapSiftUp = (heap, pos) => {
  const endpos = heap.length;
  const startpos = pos;
  const newitem = heap[pos];
  let childpos = 2 * pos + 1;
  while (childpos < endpos) {
    const rightpos = childpos + 1;
    if (rightpos < endpos) {
      const lt = heapLt(heap[childpos], heap[rightpos]);
      if (lt === null) return null;
      if (!lt) {
        childpos = rightpos;
      }
    }
    heap[pos] = heap[childpos];
    pos = childpos;
    childpos = 2 * pos + 1;
  }
  heap[pos] = newitem;
  return heapSiftDown(heap, startpos, pos);
};
const formatFloat = (val) => {
  if (Number.isNaN(val)) return 'nan';
  if (!Number.isFinite(val)) return val < 0 ? '-inf' : 'inf';
  if (Number.isInteger(val)) return val.toFixed(1);
  return val.toString();
};
const formatComplexFloat = (val) => {
  const text = formatFloat(val);
  return text.endsWith('.0') ? text.slice(0, -2) : text;
};
const formatComplex = (re, im) => {
  const reZero = re === 0 && !Object.is(re, -0);
  const reText = formatComplexFloat(re);
  if (reZero) {
    return `${formatComplexFloat(im)}j`;
  }
  const sign = im < 0 || Object.is(im, -0) ? '-' : '+';
  const imText = formatComplexFloat(Math.abs(im));
  return `(${reText}${sign}${imText}j)`;
};
const stringChars = (text) => Array.from(text);
const stringLength = (text) => stringChars(text).length;
const truncateString = (text, count) => stringChars(text).slice(0, count).join('');
const applyGrouping = (text, group, sep) => {
  let out = '';
  let count = 0;
  for (let i = text.length - 1; i >= 0; i -= 1) {
    out = text[i] + out;
    count += 1;
    if (count % group === 0 && i !== 0) {
      out = sep + out;
    }
  }
  return out;
};
const applyAlignment = (prefix, body, spec, defaultAlign) => {
  const text = `${prefix}${body}`;
  if (spec.width === null || spec.width === undefined) {
    return text;
  }
  const len = stringLength(text);
  if (len >= spec.width) {
    return text;
  }
  const padLen = spec.width - len;
  const align = spec.align ?? defaultAlign;
  const fill = spec.fill ?? ' ';
  if (align === '=') {
    const padding = fill.repeat(padLen);
    return `${prefix}${padding}${body}`;
  }
  const padding = fill.repeat(padLen);
  if (align === '<') return `${text}${padding}`;
  if (align === '>') return `${padding}${text}`;
  if (align === '^') {
    const left = Math.floor(padLen / 2);
    const right = padLen - left;
    return `${fill.repeat(left)}${text}${fill.repeat(right)}`;
  }
  return text;
};
const parseFormatSpec = (spec) => {
  if (spec.length === 0) {
    return {
      fill: ' ',
      align: null,
      sign: null,
      alternate: false,
      width: null,
      grouping: null,
      precision: null,
      type: null,
    };
  }
  let fill = ' ';
  let align = null;
  let sign = null;
  let alternate = false;
  let width = null;
  let grouping = null;
  let precision = null;
  let type = null;
  let idx = 0;
  const isAlign = (ch) => ch === '<' || ch === '>' || ch === '^' || ch === '=';
  const c1 = spec[idx];
  const c2 = spec[idx + 1];
  if (c1 && c2 && isAlign(c2)) {
    fill = c1;
    align = c2;
    idx += 2;
  } else if (c1 && isAlign(c1)) {
    align = c1;
    idx += 1;
  }
  const signChar = spec[idx];
  if (signChar === '+' || signChar === '-' || signChar === ' ') {
    sign = signChar;
    idx += 1;
  }
  if (spec[idx] === '#') {
    alternate = true;
    idx += 1;
  }
  if (align === null && spec[idx] === '0') {
    fill = '0';
    align = '=';
    idx += 1;
  }
  let widthText = '';
  while (idx < spec.length && spec[idx] >= '0' && spec[idx] <= '9') {
    widthText += spec[idx];
    idx += 1;
  }
  if (widthText.length) {
    width = Number(widthText);
    if (!Number.isFinite(width)) {
      throw new Error('ValueError: Invalid format width');
    }
  }
  if (spec[idx] === ',' || spec[idx] === '_') {
    grouping = spec[idx];
    idx += 1;
  }
  if (spec[idx] === '.') {
    idx += 1;
    let precText = '';
    while (idx < spec.length && spec[idx] >= '0' && spec[idx] <= '9') {
      precText += spec[idx];
      idx += 1;
    }
    if (!precText.length) {
      throw new Error('ValueError: Invalid format precision');
    }
    precision = Number(precText);
    if (!Number.isFinite(precision)) {
      throw new Error('ValueError: Invalid format precision');
    }
  }
  const remaining = spec.slice(idx);
  if (remaining.length > 1) {
    throw new Error('ValueError: Invalid format spec');
  }
  if (remaining.length === 1) {
    type = remaining;
  }
  return {
    fill,
    align,
    sign,
    alternate,
    width,
    grouping,
    precision,
    type,
  };
};
const formatStringWithSpec = (text, spec) => {
  let out = text;
  if (spec.precision !== null && spec.precision !== undefined) {
    out = truncateString(out, spec.precision);
  }
  return applyAlignment('', out, spec, '<');
};
const trimFloatTrailing = (text, alternate) => {
  if (alternate) return text;
  const expPos = text.search(/[eE]/);
  let mantissa = text;
  let exp = '';
  if (expPos >= 0) {
    mantissa = text.slice(0, expPos);
    exp = text.slice(expPos);
  }
  let end = mantissa.length;
  const dot = mantissa.indexOf('.');
  if (dot >= 0) {
    while (end > dot + 1 && mantissa[end - 1] === '0') {
      end -= 1;
    }
    if (end === dot + 1) {
      end = dot;
    }
  }
  return `${mantissa.slice(0, end)}${exp}`;
};
const normalizeExponent = (text, upper) => {
  const expPos = text.indexOf('e') >= 0 ? text.indexOf('e') : text.indexOf('E');
  if (expPos < 0) return text;
  const expChar = text[expPos];
  const mantissa = text.slice(0, expPos);
  const expText = text.slice(expPos + 1);
  const expVal = Number.parseInt(expText, 10);
  const expNum = Number.isNaN(expVal) ? 0 : expVal;
  const sign = expNum < 0 ? '-' : '+';
  const expAbs = Math.abs(expNum);
  const expOut = upper ? 'E' : expChar;
  return `${mantissa}${expOut}${sign}${expAbs.toString().padStart(2, '0')}`;
};
const normalizeScientific = (formatted) => {
  const normalized = formatted.toLowerCase();
  const expPos = normalized.indexOf('e');
  if (expPos < 0) return normalized;
  let mantissa = normalized.slice(0, expPos);
  if (mantissa.includes('.')) {
    while (mantissa.endsWith('0')) {
      mantissa = mantissa.slice(0, -1);
    }
    if (mantissa.endsWith('.')) {
      mantissa = mantissa.slice(0, -1);
    }
  }
  const expVal = Number.parseInt(normalized.slice(expPos + 1), 10);
  const expNum = Number.isNaN(expVal) ? 0 : expVal;
  const sign = expNum < 0 ? '-' : '+';
  const expAbs = Math.abs(expNum);
  return `${mantissa}e${sign}${expAbs.toString().padStart(2, '0')}`;
};
const formatFloatScientific = (val) => {
  const raw = val.toString();
  if (raw.includes('e') || raw.includes('E')) {
    return normalizeScientific(raw);
  }
  let digits = raw.startsWith('-') ? raw.slice(1) : raw;
  const digitsOnly = digits.replace('.', '');
  let sigDigits = digitsOnly.replace(/^0+/, '').length;
  if (sigDigits < 1) sigDigits = 1;
  const precision = Math.max(sigDigits - 1, 0);
  const formatted = val.toExponential(precision);
  return normalizeScientific(formatted);
};
const formatFloatDefault = (val) => {
  if (Number.isNaN(val)) return 'nan';
  if (!Number.isFinite(val)) return val < 0 ? '-inf' : 'inf';
  const abs = Math.abs(val);
  if (abs !== 0 && (abs < 1e-4 || abs >= 1e16)) {
    return formatFloatScientific(val);
  }
  if (Number.isInteger(val)) {
    if (Object.is(val, -0)) return '-0.0';
    return val.toFixed(1);
  }
  return val.toString();
};
const formatComplexFloatDefault = (val) => {
  const text = formatFloatDefault(val);
  return text.endsWith('.0') ? text.slice(0, -2) : text;
};
const formatFloatWithSpec = (valBits, spec) => {
  let val = null;
  if (isFloat(valBits)) {
    val = bitsToFloat(valBits);
  } else if (isTag(valBits, TAG_INT) || isTag(valBits, TAG_BOOL)) {
    val = Number(unboxIntLike(valBits));
  }
  if (val === null) {
    throw new Error('TypeError: format requires float');
  }
  const useDefault = spec.type === null && spec.precision === null;
  const ty = spec.type ?? 'g';
  const upper = ty === 'F' || ty === 'E' || ty === 'G';
  if (Number.isNaN(val)) {
    const text = upper ? 'NAN' : 'nan';
    const prefix = val < 0 ? '-' : '';
    return applyAlignment(prefix, text, spec, '>');
  }
  if (!Number.isFinite(val)) {
    const text = upper ? 'INF' : 'inf';
    const prefix = val < 0 ? '-' : '';
    return applyAlignment(prefix, text, spec, '>');
  }
  const negative = val < 0 || Object.is(val, -0);
  let prefix = '';
  if (negative) {
    prefix = '-';
  } else if (spec.sign === '+' || spec.sign === ' ') {
    prefix = spec.sign;
  }
  const absVal = Math.abs(val);
  const prec = spec.precision ?? 6;
  let body = '';
  if (useDefault) {
    body = formatFloatDefault(absVal);
  } else {
    if (ty === 'f' || ty === 'F') {
      body = absVal.toFixed(prec);
    } else if (ty === 'e' || ty === 'E') {
      body = absVal.toExponential(prec);
    } else if (ty === 'g' || ty === 'G') {
      const digits = prec === 0 ? 1 : prec;
      if (absVal === 0) {
        body = '0';
      } else {
        const exp = Math.floor(Math.log10(absVal));
        if (exp < -4 || exp >= digits) {
          const text = absVal.toExponential(digits - 1);
          body = trimFloatTrailing(text, spec.alternate);
        } else {
          const frac = Math.max(digits - 1 - exp, 0);
          const text = absVal.toFixed(frac);
          body = trimFloatTrailing(text, spec.alternate);
        }
      }
    } else if (ty === '%') {
      body = (absVal * 100).toFixed(prec);
    } else {
      throw new Error('ValueError: unsupported float format type');
    }
  }
  body = normalizeExponent(body, upper);
  if (upper) {
    body = body.replace('e', 'E');
  }
  if (
    spec.alternate &&
    !body.includes('.') &&
    !body.includes('e') &&
    !body.includes('E')
  ) {
    body += '.';
  }
  if (spec.grouping) {
    if (!body.includes('e') && !body.includes('E')) {
      const parts = body.split('.', 2);
      const grouped = applyGrouping(parts[0], 3, spec.grouping);
      body = parts.length > 1 ? `${grouped}.${parts[1]}` : grouped;
    }
  }
  if (ty === '%') {
    body += '%';
  }
  return applyAlignment(prefix, body, spec, '>');
};
const applyGroupingToFloatText = (text, sep) => {
  if (text.includes('e') || text.includes('E')) {
    return text;
  }
  const parts = text.split('.', 2);
  const grouped = applyGrouping(parts[0], 3, sep);
  return parts.length > 1 ? `${grouped}.${parts[1]}` : grouped;
};
const formatComplexWithSpec = (value, spec) => {
  let ty = spec.type;
  let grouping = spec.grouping;
  if (ty === 'n') {
    if (grouping) {
      const msg =
        grouping === ','
          ? "Cannot specify ',' with 'n'."
          : "Cannot specify '_' with 'n'.";
      throw new Error(`ValueError: ${msg}`);
    }
    ty = 'g';
    grouping = null;
  }
  if (ty && !['e', 'E', 'f', 'F', 'g', 'G'].includes(ty)) {
    throw new Error(
      `ValueError: Unknown format code '${ty}' for object of type 'complex'`,
    );
  }
  if (spec.fill === '0') {
    throw new Error(
      'ValueError: Zero padding is not allowed in complex format specifier',
    );
  }
  if (spec.align === '=') {
    throw new Error(
      "ValueError: '=' alignment flag is not allowed in complex format specifier",
    );
  }
  const re = value.re;
  const im = value.im;
  const reIsZero = re === 0 && !Object.is(re, -0);
  const imIsNegative = im < 0 || Object.is(im, -0);
  const imSign = imIsNegative ? '-' : '+';
  const useDefault = spec.type === null && spec.precision === null;
  let realText = '';
  let imagText = '';
  if (useDefault) {
    realText = formatComplexFloatDefault(Math.abs(re));
    imagText = formatComplexFloatDefault(Math.abs(im));
    if (grouping) {
      realText = applyGroupingToFloatText(realText, grouping);
      imagText = applyGroupingToFloatText(imagText, grouping);
    }
  } else {
    const realSpec = {
      fill: spec.fill,
      align: null,
      sign: spec.sign,
      alternate: spec.alternate,
      width: null,
      grouping,
      precision: spec.precision,
      type: ty,
    };
    const imagSpec = {
      fill: spec.fill,
      align: null,
      sign: null,
      alternate: spec.alternate,
      width: null,
      grouping,
      precision: spec.precision,
      type: ty,
    };
    realText = formatFloatWithSpec(boxFloat(re), realSpec);
    imagText = formatFloatWithSpec(boxFloat(Math.abs(im)), imagSpec);
  }
  const includeReal = ty !== null || !reIsZero;
  let body = '';
  if (includeReal) {
    let reTextOut = realText;
    if (useDefault) {
      let prefix = '';
      if (re < 0 || Object.is(re, -0)) {
        prefix = '-';
      } else if (spec.sign === '+' || spec.sign === ' ') {
        prefix = spec.sign;
      }
      reTextOut = `${prefix}${realText}`;
    }
    const combined = `${reTextOut}${imSign}${imagText}j`;
    body = ty === null ? `(${combined})` : combined;
  } else {
    let prefix = '';
    if (imIsNegative) {
      prefix = '-';
    } else if (spec.sign === '+' || spec.sign === ' ') {
      prefix = spec.sign;
    }
    body = `${prefix}${imagText}j`;
  }
  return applyAlignment('', body, spec, '>');
};
const formatObjStr = (val) => {
  const strBits = baseImports.str_from_obj(val);
  const text = getStrObj(strBits);
  return text === null ? '' : text;
};
const formatIntWithSpec = (valBits, spec) => {
  if (spec.precision !== null && spec.precision !== undefined) {
    throw new Error('ValueError: precision not allowed in integer format');
  }
  const ty = spec.type ?? 'd';
  let value = getBigIntValue(valBits);
  if (value === null) {
    throw new Error('TypeError: format requires int');
  }
  if (ty === 'c') {
    if (value < 0n) {
      throw new Error('ValueError: format c requires non-negative int');
    }
    if (value > 0x10ffffn) {
      throw new Error('ValueError: format c out of range');
    }
    const ch = String.fromCodePoint(Number(value));
    return formatStringWithSpec(ch, spec);
  }
  let base = 10;
  if (ty === 'b') base = 2;
  else if (ty === 'o') base = 8;
  else if (ty === 'x' || ty === 'X') base = 16;
  else if (ty === 'd' || ty === 'n') base = 10;
  else {
    throw new Error('ValueError: unsupported int format type');
  }
  let negative = value < 0n;
  if (negative) value = -value;
  let digits = value.toString(base);
  if (ty === 'X') {
    digits = digits.toUpperCase();
  }
  if (spec.grouping) {
    const group = base === 2 || base === 16 ? 4 : base === 8 ? 3 : 3;
    digits = applyGrouping(digits, group, spec.grouping);
  }
  let prefix = '';
  if (negative) {
    prefix = '-';
  } else if (spec.sign === '+' || spec.sign === ' ') {
    prefix = spec.sign;
  }
  if (spec.alternate) {
    if (ty === 'b') prefix += '0b';
    else if (ty === 'o') prefix += '0o';
    else if (ty === 'x') prefix += '0x';
    else if (ty === 'X') prefix += '0X';
  }
  return applyAlignment(prefix, digits, spec, '>');
};
const formatWithSpec = (valBits, spec) => {
  const obj = getObj(valBits);
  if (obj && obj.type === 'complex') {
    return formatComplexWithSpec(obj, spec);
  }
  if (spec.type === 'n') {
    if (spec.grouping) {
      const msg =
        spec.grouping === ','
          ? "Cannot specify ',' with 'n'."
          : "Cannot specify '_' with 'n'.";
      throw new Error(`ValueError: ${msg}`);
    }
    const normalized = {
      fill: spec.fill,
      align: spec.align,
      sign: spec.sign,
      alternate: spec.alternate,
      width: spec.width,
      grouping: null,
      precision: spec.precision,
      type: null,
    };
    if (isFloat(valBits)) {
      normalized.type = 'g';
      return formatFloatWithSpec(valBits, normalized);
    }
    normalized.type = 'd';
    return formatIntWithSpec(valBits, normalized);
  }
  if (spec.type === 's') {
    return formatStringWithSpec(formatObjStr(valBits), spec);
  }
  if (spec.type && ['d', 'b', 'o', 'x', 'X', 'c'].includes(spec.type)) {
    return formatIntWithSpec(valBits, spec);
  }
  if (spec.type && ['f', 'F', 'e', 'E', 'g', 'G', '%'].includes(spec.type)) {
    return formatFloatWithSpec(valBits, spec);
  }
  if (spec.type) {
    throw new Error('ValueError: unsupported format type');
  }
  if (isFloat(valBits)) {
    return formatFloatWithSpec(valBits, spec);
  }
  if (isTag(valBits, TAG_BOOL)) {
    return formatStringWithSpec(formatObjStr(valBits), spec);
  }
  if (isTag(valBits, TAG_INT) || (obj && obj.type === 'bigint')) {
    return formatIntWithSpec(valBits, spec);
  }
  return formatStringWithSpec(formatObjStr(valBits), spec);
};
const parseFloatLiteral = (text) => {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const lowered = trimmed.toLowerCase();
  if (lowered === 'nan' || lowered === '+nan' || lowered === '-nan') {
    return NaN;
  }
  if (
    lowered === 'inf' ||
    lowered === '+inf' ||
    lowered === 'infinity' ||
    lowered === '+infinity'
  ) {
    return Infinity;
  }
  if (lowered === '-inf' || lowered === '-infinity') {
    return -Infinity;
  }
  const parsed = Number(trimmed);
  if (Number.isNaN(parsed)) return null;
  return parsed;
};
const parseComplexFromString = (text) => {
  let trimmed = text.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith('(') && trimmed.endsWith(')') && trimmed.length >= 2) {
    trimmed = trimmed.slice(1, -1).trim();
    if (!trimmed) return null;
  }
  if (/\s/.test(trimmed)) return null;
  const endsWithJ = trimmed.endsWith('j') || trimmed.endsWith('J');
  if (endsWithJ) {
    const core = trimmed.slice(0, -1);
    if (!core || core === '+') return { re: 0, im: 1 };
    if (core === '-') return { re: 0, im: -1 };
    let sepIdx = -1;
    for (let i = 1; i < core.length; i += 1) {
      const ch = core[i];
      if (ch === '+' || ch === '-') {
        const prev = core[i - 1];
        if (prev === 'e' || prev === 'E') continue;
        sepIdx = i;
      }
    }
    if (sepIdx >= 0) {
      const realPart = core.slice(0, sepIdx);
      const imagPart = core.slice(sepIdx);
      const real = parseFloatLiteral(realPart);
      if (real === null) return null;
      let imag = null;
      if (imagPart === '+') {
        imag = 1;
      } else if (imagPart === '-') {
        imag = -1;
      } else {
        imag = parseFloatLiteral(imagPart);
      }
      if (imag === null) return null;
      return { re: real, im: imag };
    }
    const imag = parseFloatLiteral(core);
    if (imag === null) return null;
    return { re: 0, im: imag };
  }
  const real = parseFloatLiteral(trimmed);
  if (real === null) return null;
  return { re: real, im: 0 };
};
const formatStringRepr = (text) => {
  const useDouble = text.includes("'") && !text.includes('"');
  const quote = useDouble ? '"' : "'";
  let out = quote;
  for (const ch of text) {
    if (ch === '\\\\') {
      out += '\\\\\\\\';
      continue;
    }
    if (ch === '\\n') {
      out += '\\\\n';
      continue;
    }
    if (ch === '\\r') {
      out += '\\\\r';
      continue;
    }
    if (ch === '\\t') {
      out += '\\\\t';
      continue;
    }
    if (ch === quote) {
      out += `\\\\${quote}`;
      continue;
    }
    const code = ch.codePointAt(0);
    if (code !== undefined && (code < 0x20 || code === 0x7f)) {
      const bytes = Buffer.from(ch, 'utf8');
      for (const b of bytes) {
        out += `\\\\x${b.toString(16).padStart(2, '0')}`;
      }
      continue;
    }
    out += ch;
  }
  out += quote;
  return out;
};
const isGenerator = (val) =>
  isPtr(val) &&
  !heap.has(val & POINTER_MASK) &&
  !instanceClasses.has(ptrAddr(val));
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
const getSlice = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'slice') return null;
  return obj;
};
const getIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'iter') return null;
  return obj;
};
const getEnumerate = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'enumerate') return null;
  return obj;
};
const getCallIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'call_iter') return null;
  return obj;
};
const getReversed = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'reversed') return null;
  return obj;
};
const getZipIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'zip') return null;
  return obj;
};
const getMapIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'map') return null;
  return obj;
};
const getFilterIter = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'filter') return null;
  return obj;
};
const getDict = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'dict') return null;
  return obj;
};
const getDictKeysView = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'dict_keys') return null;
  return obj;
};
const getDictValuesView = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'dict_values') return null;
  return obj;
};
const getDictItemsView = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'dict_items') return null;
  return obj;
};
const getSet = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'set') return null;
  return obj;
};
const getFrozenSet = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'frozenset') return null;
  return obj;
};
const getSetLike = (val) => {
  const obj = getObj(val);
  if (!obj || (obj.type !== 'set' && obj.type !== 'frozenset')) return null;
  return obj;
};
const getSetInplaceRhs = (val) =>
  getSetLike(val) || getDictKeysView(val) || getDictItemsView(val);
const dictViewItems = (view) => {
  const dict = getDict(view.dictBits);
  if (!dict) return null;
  const items = new Set();
  for (const [keyBits, valBits] of dict.entries) {
    if (view.type === 'dict_keys') {
      items.add(keyBits);
    } else if (view.type === 'dict_items') {
      items.add(tupleFromArray([keyBits, valBits]));
    } else {
      return null;
    }
  }
  return items;
};
const getSetOpItems = (val) => {
  const setLike = getSetLike(val);
  if (setLike) {
    return { items: setLike.items, type: setLike.type, isView: false };
  }
  const dictKeys = getDictKeysView(val);
  const dictItems = getDictItemsView(val);
  const view = dictKeys || dictItems;
  if (view) {
    const items = dictViewItems(view);
    if (!items) return null;
    return { items, type: 'set', isView: true };
  }
  return null;
};
const setItemsFromIterable = (otherBits) => {
  const other = getSetLike(otherBits);
  if (other) {
    return new Set(other.items);
  }
  const iterBits = baseImports.iter(otherBits);
  if (isNone(iterBits)) {
    throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
  }
  const items = new Set();
  while (true) {
    const pairBits = baseImports.iter_next(iterBits);
    const tuple = getTuple(pairBits);
    if (!tuple || tuple.items.length < 2) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    const doneBits = tuple.items[1];
    if (isTruthyBits(doneBits)) break;
    items.add(tuple.items[0]);
  }
  return items;
};
const setCopyBits = (selfBits) => {
  const setLike = getSetLike(selfBits);
  if (!setLike) return boxNone();
  if (setLike.type === 'frozenset') {
    return selfBits;
  }
  return boxPtr({ type: 'set', items: new Set(setLike.items) });
};
const setClearBits = (selfBits) => {
  const set = getSet(selfBits);
  if (!set) return boxNone();
  set.items.clear();
  return boxNone();
};
const setUnionItems = (leftItems, rightItems) => {
  const out = new Set(leftItems);
  for (const item of rightItems) {
    out.add(item);
  }
  return out;
};
const setIntersectionItems = (leftItems, rightItems) => {
  const out = new Set();
  for (const item of leftItems) {
    if (rightItems.has(item)) out.add(item);
  }
  return out;
};
const setDifferenceItems = (leftItems, rightItems) => {
  const out = new Set();
  for (const item of leftItems) {
    if (!rightItems.has(item)) out.add(item);
  }
  return out;
};
const setSymdiffItems = (leftItems, rightItems) => {
  const out = new Set();
  for (const item of leftItems) {
    if (!rightItems.has(item)) out.add(item);
  }
  for (const item of rightItems) {
    if (!leftItems.has(item)) out.add(item);
  }
  return out;
};
const setUnionMulti = (selfBits, othersBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  let resultItems = new Set(self.items);
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    resultItems = setUnionItems(resultItems, items);
  }
  return boxPtr({ type: self.type, items: resultItems });
};
const setIntersectionMulti = (selfBits, othersBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  let resultItems = new Set(self.items);
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    resultItems = setIntersectionItems(resultItems, items);
  }
  return boxPtr({ type: self.type, items: resultItems });
};
const setDifferenceMulti = (selfBits, othersBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  let resultItems = new Set(self.items);
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    resultItems = setDifferenceItems(resultItems, items);
  }
  return boxPtr({ type: self.type, items: resultItems });
};
const setSymdiffBits = (selfBits, otherBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const otherItems = setItemsFromIterable(otherBits);
  const resultItems = setSymdiffItems(self.items, otherItems);
  return boxPtr({ type: self.type, items: resultItems });
};
const setUpdateMulti = (selfBits, othersBits) => {
  const set = getSet(selfBits);
  if (!set) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    for (const item of items) {
      set.items.add(item);
    }
  }
  return boxNone();
};
const setIntersectionUpdateMulti = (selfBits, othersBits) => {
  const set = getSet(selfBits);
  if (!set) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    for (const item of [...set.items]) {
      if (!items.has(item)) {
        set.items.delete(item);
      }
    }
  }
  return boxNone();
};
const setDifferenceUpdateMulti = (selfBits, othersBits) => {
  const set = getSet(selfBits);
  if (!set) return boxNone();
  const tuple = getTuple(othersBits);
  const others = tuple ? tuple.items : [othersBits];
  for (const otherBits of others) {
    const items = setItemsFromIterable(otherBits);
    for (const item of items) {
      set.items.delete(item);
    }
  }
  return boxNone();
};
const setSymdiffUpdateBits = (selfBits, otherBits) => {
  const set = getSet(selfBits);
  if (!set) return boxNone();
  const items = setItemsFromIterable(otherBits);
  set.items = setSymdiffItems(set.items, items);
  return boxNone();
};
const setIsDisjointBits = (selfBits, otherBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const otherItems = setItemsFromIterable(otherBits);
  let left = self.items;
  let right = otherItems;
  if (left.size > right.size) {
    left = otherItems;
    right = self.items;
  }
  for (const item of left) {
    if (right.has(item)) return boxBool(false);
  }
  return boxBool(true);
};
const setIsSubsetBits = (selfBits, otherBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const otherItems = setItemsFromIterable(otherBits);
  for (const item of self.items) {
    if (!otherItems.has(item)) return boxBool(false);
  }
  return boxBool(true);
};
const setIsSupersetBits = (selfBits, otherBits) => {
  const self = getSetLike(selfBits);
  if (!self) return boxNone();
  const otherItems = setItemsFromIterable(otherBits);
  for (const item of otherItems) {
    if (!self.items.has(item)) return boxBool(false);
  }
  return boxBool(true);
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
const getMemoryview = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'memoryview') return null;
  return obj;
};
const getCallArgs = (val) => {
  const obj = getObj(val);
  if (!obj || obj.type !== 'callargs') return null;
  return obj;
};
const listFromArray = (items) => boxPtr({ type: 'list', items });
const tupleFromArray = (items) => boxPtr({ type: 'tuple', items });
const iterNextInternal = (val) => {
  if (isGenerator(val)) {
    return generatorSend(val, boxNone());
  }
  const callIter = getCallIter(val);
  if (callIter) {
    const value = callCallable0(callIter.callable);
    if (exceptionPending() !== 0n) return boxNone();
    if (isTruthyBits(baseImports.eq(value, callIter.sentinel))) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    return tupleFromArray([value, boxBool(false)]);
  }
  const mapIter = getMapIter(val);
  if (mapIter) {
    const vals = [];
    for (const iterBits of mapIter.iters) {
      const pair = iterNextInternal(iterBits);
      const tuple = getTuple(pair);
      if (!tuple || tuple.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      vals.push(tuple.items[0]);
    }
    let res = boxNone();
    if (vals.length === 1) {
      res = callCallable1(mapIter.func, vals[0]);
    } else {
      const builder = baseImports.callargs_new(boxInt(vals.length), boxInt(0));
      for (const valBits of vals) {
        baseImports.callargs_push_pos(builder, valBits);
      }
      res = baseImports.call_bind(mapIter.func, builder);
    }
    if (exceptionPending() !== 0n) return boxNone();
    return tupleFromArray([res, boxBool(false)]);
  }
  const filterIter = getFilterIter(val);
  if (filterIter) {
    while (true) {
      const pair = iterNextInternal(filterIter.iterBits);
      const tuple = getTuple(pair);
      if (!tuple || tuple.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      const valBits = tuple.items[0];
      let keep = false;
      if (isNone(filterIter.func)) {
        keep = isTruthyBits(valBits);
      } else {
        const pred = callCallable1(filterIter.func, valBits);
        if (exceptionPending() !== 0n) return boxNone();
        keep = isTruthyBits(pred);
      }
      if (keep) {
        return tupleFromArray([valBits, boxBool(false)]);
      }
    }
  }
  const zipIter = getZipIter(val);
  if (zipIter) {
    if (zipIter.iters.length === 0) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const vals = [];
    for (const iterBits of zipIter.iters) {
      const pair = iterNextInternal(iterBits);
      const tuple = getTuple(pair);
      if (!tuple || tuple.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      vals.push(tuple.items[0]);
    }
    const tupleBits = tupleFromArray(vals);
    return tupleFromArray([tupleBits, boxBool(false)]);
  }
  const revIter = getReversed(val);
  if (revIter) {
    const target = revIter.target;
    const list = getList(target);
    const tup = getTuple(target);
    const dict = getDict(target);
    const bytes = getBytes(target);
    const bytearray = getBytearray(target);
    const strVal = getStrObj(target);
    if (list || tup) {
      const items = list ? list.items : tup.items;
      const idx = Math.min(revIter.idx, items.length);
      if (idx === 0) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      const value = items[idx - 1];
      revIter.idx = idx - 1;
      return tupleFromArray([value, boxBool(false)]);
    }
    if (bytes || bytearray) {
      const data = bytes ? bytes.data : bytearray.data;
      const idx = Math.min(revIter.idx, data.length);
      if (idx === 0) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      const value = boxInt(data[idx - 1]);
      revIter.idx = idx - 1;
      return tupleFromArray([value, boxBool(false)]);
    }
    if (dict) {
      const idx = Math.min(revIter.idx, dict.entries.length);
      if (idx === 0) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      const value = dict.entries[idx - 1][0];
      revIter.idx = idx - 1;
      return tupleFromArray([value, boxBool(false)]);
    }
    if (strVal !== null) {
      const chars = Array.from(strVal);
      const idx = Math.min(revIter.idx, chars.length);
      if (idx === 0) {
        return tupleFromArray([boxNone(), boxBool(true)]);
      }
      const value = boxPtr({ type: 'str', value: chars[idx - 1] });
      revIter.idx = idx - 1;
      return tupleFromArray([value, boxBool(false)]);
    }
    return boxNone();
  }
  const iter = getIter(val);
  if (!iter) return boxNone();
  const target = iter.target;
  const list = getList(target);
  const tup = getTuple(target);
  const setLike = getSetLike(target);
  const dict = getDict(target);
  const bytes = getBytes(target);
  const bytearray = getBytearray(target);
  const strVal = getStrObj(target);
  if (list || tup || setLike) {
    const items = list ? list.items : tup ? tup.items : [...setLike.items];
    if (iter.idx >= items.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = items[iter.idx];
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  }
  if (dict) {
    if (iter.idx >= dict.entries.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = dict.entries[iter.idx][0];
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  }
  const dictKeys = getDictKeysView(target);
  const dictValues = getDictValuesView(target);
  const dictItems = getDictItemsView(target);
  if (dictKeys || dictValues || dictItems) {
    const view = dictKeys || dictValues || dictItems;
    const dictView = getDict(view.dictBits);
    if (!dictView || iter.idx >= dictView.entries.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const [keyBits, valBits] = dictView.entries[iter.idx];
    let value = keyBits;
    if (dictValues) {
      value = valBits;
    } else if (dictItems) {
      value = tupleFromArray([keyBits, valBits]);
    }
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  }
  if (bytes || bytearray) {
    const data = bytes ? bytes.data : bytearray.data;
    if (iter.idx >= data.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = boxInt(data[iter.idx]);
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  }
  if (strVal !== null) {
    const chars = Array.from(strVal);
    if (iter.idx >= chars.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = boxPtr({ type: 'str', value: chars[iter.idx] });
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  }
  return boxNone();
};
const dictKey = (bits) => {
  if (isTag(bits, TAG_NONE)) return 'n:None';
  if (isTag(bits, TAG_INT) || isTag(bits, TAG_BOOL)) {
    return `i:${unboxIntLike(bits)}`;
  }
  if (isFloat(bits)) return `f:${bits.toString()}`;
  const str = getStrObj(bits);
  if (str !== null) return `s:${str}`;
  return `p:${bits.toString()}`;
};
const dictGetIndex = (dict, keyBits) => dict.lookup.get(dictKey(keyBits));
const dictGetValue = (dict, keyBits) => {
  const idx = dictGetIndex(dict, keyBits);
  if (idx === undefined) return null;
  return dict.entries[idx][1];
};
const dictSetValue = (dict, keyBits, valBits) => {
  const key = dictKey(keyBits);
  const idx = dict.lookup.get(key);
  if (idx === undefined) {
    dict.lookup.set(key, dict.entries.length);
    dict.entries.push([keyBits, valBits]);
  } else {
    dict.entries[idx][1] = valBits;
  }
};
const dictDelete = (dict, keyBits) => {
  const key = dictKey(keyBits);
  const idx = dict.lookup.get(key);
  if (idx === undefined) return false;
  dict.entries.splice(idx, 1);
  dict.lookup = new Map();
  for (let i = 0; i < dict.entries.length; i++) {
    dict.lookup.set(dictKey(dict.entries[i][0]), i);
  }
  return true;
};
const setFromArray = (items) => {
  const set = new Set();
  for (const item of items) {
    set.add(item);
  }
  return boxPtr({ type: 'set', items: set });
};
const frozensetFromArray = (items) => {
  const set = new Set();
  for (const item of items) {
    set.add(item);
  }
  return boxPtr({ type: 'frozenset', items: set });
};
const classBasesList = (classBits) => {
  const cls = getClass(classBits);
  if (!cls) return [];
  if (cls.basesBits) {
    const bases = getTuple(cls.basesBits);
    if (bases) return [...bases.items];
  }
  if (cls.baseBits && !isNone(cls.baseBits)) return [cls.baseBits];
  return [];
};
const c3Merge = (seqs) => {
  const result = [];
  while (true) {
    seqs = seqs.filter((seq) => seq.length);
    if (!seqs.length) return result;
    let candidate = null;
    for (const seq of seqs) {
      const head = seq[0];
      let inTail = false;
      for (const other of seqs) {
        if (other.slice(1).includes(head)) {
          inTail = true;
          break;
        }
      }
      if (!inTail) {
        candidate = head;
        break;
      }
    }
    if (candidate === null) return null;
    result.push(candidate);
    for (const seq of seqs) {
      if (seq.length && seq[0] === candidate) {
        seq.shift();
      }
    }
  }
};
const classMroList = (classBits) => {
  const cls = getClass(classBits);
  if (!cls) return [classBits];
  if (cls.mroBits) {
    const mroTuple = getTuple(cls.mroBits);
    if (mroTuple) return [...mroTuple.items];
  }
  const bases = classBasesList(classBits);
  if (!bases.length) return [classBits];
  const merged = c3Merge([...bases.map((base) => [...classMroList(base)]), [...bases]]);
  if (!merged) return [classBits, ...bases];
  return [classBits, ...merged];
};
const setClassBases = (classBits, baseBits) => {
  const cls = getClass(classBits);
  if (!cls) return;
  let basesBits = baseBits;
  let bases = [];
  if (basesBits && !isNone(basesBits)) {
    const tuple = getTuple(basesBits);
    if (tuple) {
      bases = [...tuple.items];
    } else if (getClass(basesBits)) {
      bases = [basesBits];
      basesBits = tupleFromArray(bases);
    } else {
      throw new Error('TypeError: base must be a type object or tuple of types');
    }
  } else {
    basesBits = tupleFromArray([]);
  }
  const seen = new Set();
  for (const base of bases) {
    if (seen.has(base)) {
      throw new Error('TypeError: duplicate base class');
    }
    seen.add(base);
    if (!getClass(base)) {
      throw new Error('TypeError: base must be a type object');
    }
    if (base === classBits) {
      throw new Error('TypeError: class cannot inherit from itself');
    }
  }
  const mro = c3Merge([...bases.map((base) => [...classMroList(base)]), [...bases]]);
  if (!mro) {
    throw new Error(
      'TypeError: Cannot create a consistent method resolution order (MRO) for bases'
    );
  }
  const mroBits = tupleFromArray([classBits, ...mro]);
  cls.baseBits = bases.length ? bases[0] : boxNone();
  cls.basesBits = basesBits;
  cls.mroBits = mroBits;
  cls.attrs.set('__bases__', basesBits);
  cls.attrs.set('__mro__', mroBits);
  bumpClassLayoutVersion(classBits);
};
let superTypeBits = null;
const getSuperType = () => {
  if (superTypeBits) return superTypeBits;
  superTypeBits = boxPtr({
    type: 'class',
    name: 'super',
    attrs: new Map(),
    baseBits: boxNone(),
    basesBits: null,
    mroBits: null,
  });
  classLayoutVersions.set(superTypeBits, 0n);
  setClassBases(superTypeBits, getBuiltinType(100));
  return superTypeBits;
};
let baseExceptionBits = null;
let exceptionBits = null;
let frameTypeBits = null;
let tracebackTypeBits = null;
const exceptionClasses = new Map();
const exceptionAliases = new Map([
  ['EnvironmentError', 'OSError'],
  ['IOError', 'OSError'],
  ['WindowsError', 'OSError'],
]);
const exceptionBaseSpecs = new Map([
  ['BaseExceptionGroup', ['BaseException']],
  ['ExceptionGroup', ['BaseExceptionGroup', 'Exception']],
  ['GeneratorExit', ['BaseException']],
  ['KeyboardInterrupt', ['BaseException']],
  ['SystemExit', ['BaseException']],
  [
    'ArithmeticError',
    ['Exception'],
  ],
  ['AssertionError', ['Exception']],
  ['AttributeError', ['Exception']],
  ['BufferError', ['Exception']],
  ['EOFError', ['Exception']],
  ['ImportError', ['Exception']],
  ['LookupError', ['Exception']],
  ['MemoryError', ['Exception']],
  ['NameError', ['Exception']],
  ['OSError', ['Exception']],
  ['ReferenceError', ['Exception']],
  ['RuntimeError', ['Exception']],
  ['StopIteration', ['Exception']],
  ['StopAsyncIteration', ['Exception']],
  ['SyntaxError', ['Exception']],
  ['SystemError', ['Exception']],
  ['TypeError', ['Exception']],
  ['ValueError', ['Exception']],
  ['Warning', ['Exception']],
  ['FloatingPointError', ['ArithmeticError']],
  ['OverflowError', ['ArithmeticError']],
  ['ZeroDivisionError', ['ArithmeticError']],
  ['ModuleNotFoundError', ['ImportError']],
  ['IndexError', ['LookupError']],
  ['KeyError', ['LookupError']],
  ['UnboundLocalError', ['NameError']],
  ['ConnectionError', ['OSError']],
  ['BrokenPipeError', ['ConnectionError']],
  ['ConnectionAbortedError', ['ConnectionError']],
  ['ConnectionRefusedError', ['ConnectionError']],
  ['ConnectionResetError', ['ConnectionError']],
  ['BlockingIOError', ['OSError']],
  ['ChildProcessError', ['OSError']],
  ['FileExistsError', ['OSError']],
  ['FileNotFoundError', ['OSError']],
  ['InterruptedError', ['OSError']],
  ['IsADirectoryError', ['OSError']],
  ['NotADirectoryError', ['OSError']],
  ['PermissionError', ['OSError']],
  ['ProcessLookupError', ['OSError']],
  ['TimeoutError', ['OSError']],
  ['NotImplementedError', ['RuntimeError']],
  ['RecursionError', ['RuntimeError']],
  ['IndentationError', ['SyntaxError']],
  ['TabError', ['IndentationError']],
  ['UnicodeError', ['ValueError']],
  ['UnicodeDecodeError', ['UnicodeError']],
  ['UnicodeEncodeError', ['UnicodeError']],
  ['UnicodeTranslateError', ['UnicodeError']],
  ['DeprecationWarning', ['Warning']],
  ['PendingDeprecationWarning', ['Warning']],
  ['RuntimeWarning', ['Warning']],
  ['SyntaxWarning', ['Warning']],
  ['UserWarning', ['Warning']],
  ['FutureWarning', ['Warning']],
  ['ImportWarning', ['Warning']],
  ['UnicodeWarning', ['Warning']],
  ['BytesWarning', ['Warning']],
  ['ResourceWarning', ['Warning']],
  ['EncodingWarning', ['Warning']],
]);
const makeExceptionClass = (name, baseBits) => {
  const clsBits = boxPtr({
    type: 'class',
    name,
    attrs: new Map(),
    baseBits: boxNone(),
    basesBits: null,
    mroBits: null,
  });
  classLayoutVersions.set(clsBits, 0n);
  setClassBases(clsBits, baseBits);
  const nameBits = boxPtr({ type: 'str', value: name });
  const cls = getClass(clsBits);
  if (cls) {
    cls.attrs.set('__name__', nameBits);
    cls.attrs.set('__qualname__', nameBits);
    cls.attrs.set('__module__', boxPtr({ type: 'str', value: 'builtins' }));
  }
  return clsBits;
};
const getBaseExceptionClass = () => {
  if (baseExceptionBits) return baseExceptionBits;
  baseExceptionBits = makeExceptionClass('BaseException', getBuiltinType(100));
  return baseExceptionBits;
};
const getExceptionClass = () => {
  if (exceptionBits) return exceptionBits;
  exceptionBits = makeExceptionClass('Exception', getBaseExceptionClass());
  return exceptionBits;
};
const getFrameType = () => {
  if (frameTypeBits) return frameTypeBits;
  frameTypeBits = makeExceptionClass('frame', getBuiltinType(100));
  return frameTypeBits;
};
const getTracebackType = () => {
  if (tracebackTypeBits) return tracebackTypeBits;
  tracebackTypeBits = makeExceptionClass('traceback', getBuiltinType(100));
  return tracebackTypeBits;
};
const getExceptionClassForName = (name) => {
  if (name === 'BaseException') return getBaseExceptionClass();
  if (name === 'Exception') return getExceptionClass();
  if (exceptionClasses.has(name)) return exceptionClasses.get(name);
  const alias = exceptionAliases.get(name);
  if (alias) {
    const aliasBits = getExceptionClassForName(alias);
    exceptionClasses.set(name, aliasBits);
    return aliasBits;
  }
  const baseNames = exceptionBaseSpecs.get(name);
  let baseBits = getExceptionClass();
  if (baseNames && baseNames.length) {
    const baseBitsList = baseNames.map((base) => getExceptionClassForName(base));
    baseBits = baseBitsList.length === 1 ? baseBitsList[0] : tupleFromArray(baseBitsList);
  }
  const clsBits = makeExceptionClass(name, baseBits);
  exceptionClasses.set(name, clsBits);
  return clsBits;
};
const getStr = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'str') return obj.value;
  return '';
};
const getStrObj = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'str') return obj.value;
  return null;
};
const getException = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'exception') return obj;
  return null;
};
const readUtf8 = (ptr, len) => {
  if (!memory) return '';
  const addr = Number(ptr);
  const size = Number(len);
  if (!size) return '';
  const bytes = new Uint8Array(memory.buffer, addr, size);
  return Buffer.from(bytes).toString('utf8');
};
const isTruthyBits = (val) => {
  if (isTag(val, TAG_BOOL)) {
    return (val & 1n) === 1n;
  }
  if (isTag(val, TAG_INT)) {
    return unboxInt(val) !== 0n;
  }
  if (isTag(val, TAG_NONE)) {
    return false;
  }
  if (isPtr(val)) {
    const obj = getObj(val);
    if (obj && obj.type === 'str') return obj.value.length !== 0;
    if (obj && obj.type === 'bytes') return obj.data.length !== 0;
    if (obj && obj.type === 'bytearray') return obj.data.length !== 0;
    if (obj && obj.type === 'list') return obj.items.length !== 0;
    if (obj && obj.type === 'tuple') return obj.items.length !== 0;
    if (obj && obj.type === 'complex') return obj.re !== 0 || obj.im !== 0;
    if (
      obj &&
      (obj.type === 'iter' ||
        obj.type === 'enumerate' ||
        obj.type === 'call_iter' ||
        obj.type === 'reversed' ||
        obj.type === 'zip' ||
        obj.type === 'map' ||
        obj.type === 'filter')
    ) {
      return true;
    }
  }
  return false;
};
const lookupExceptionAttr = (excBits, exc, name) => {
  switch (name) {
    case '__cause__':
      return exc.causeBits;
    case '__context__':
      return exc.contextBits;
    case '__suppress_context__':
      return exc.suppressBits;
    case '__traceback__':
      return exc.traceBits ?? boxNone();
    case '__class__':
      return exc.classBits || exceptionClass(exc.kindBits);
    case '__dict__':
      return exceptionEnsureDict(exc);
    case 'args':
      return exc.argsBits || tupleFromArray([]);
    case 'value':
      if (getStr(exc.kindBits) === 'StopIteration') {
        return exc.valueBits ?? boxNone();
      }
      break;
  }
  const dictBits = exc.dictBits;
  if (dictBits && !isNone(dictBits)) {
    const dict = getDict(dictBits);
    if (dict) {
      const nameBits = boxPtr({ type: 'str', value: name });
      const value = dictGetValue(dict, nameBits);
      if (value !== null) return value;
    }
  }
  const classBits = exc.classBits || exceptionClass(exc.kindBits);
  const val = lookupClassAttr(classBits, name, excBits);
  if (val !== undefined) return val;
  return undefined;
};
const makeBoundMethod = (funcBits, selfBits) => {
  const addr = allocRaw(16);
  if (addr && memory) {
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, funcBits, true);
    view.setBigInt64(addr + 8, selfBits, true);
  }
  return boxPtr({
    type: 'bound_method',
    func: funcBits,
    self: selfBits,
    memAddr: addr || null,
  });
};
const lookupClassAttr = (classBits, name, instanceBits = null, startAfter = null) => {
  const mro = classMroList(classBits);
  let foundStart = startAfter === null;
  for (const currentBits of mro) {
    if (!foundStart) {
      if (currentBits === startAfter) {
        foundStart = true;
      }
      continue;
    }
    const cls = getClass(currentBits);
    if (!cls) continue;
    if (cls.attrs.has(name)) {
      const attrVal = cls.attrs.get(name);
      if (instanceBits && getFunction(attrVal)) {
        return makeBoundMethod(attrVal, instanceBits);
      }
      return attrVal;
    }
    const builtinBits = getBuiltinMethodBits(currentBits, name);
    if (!isNone(builtinBits)) {
      if (instanceBits && getFunction(builtinBits)) {
        return makeBoundMethod(builtinBits, instanceBits);
      }
      return builtinBits;
    }
  }
  return undefined;
};
const lookupAttr = (objBits, name) => {
  const exc = getException(objBits);
  if (exc) {
    return lookupExceptionAttr(objBits, exc, name);
  }
  const superObj = getObj(objBits);
  if (superObj && superObj.type === 'super') {
    const startBits = superObj.startBits;
    const targetBits = superObj.objBits;
    const targetClass = getClass(targetBits);
    const objTypeBits = targetClass ? targetBits : typeOfBits(targetBits);
    const instanceBits = targetClass ? null : targetBits;
    const val = lookupClassAttr(objTypeBits, name, instanceBits, startBits);
    if (val !== undefined) return val;
    return undefined;
  }
  const cls = getClass(objBits);
  if (cls) {
    const val = lookupClassAttr(objBits, name);
    if (val !== undefined) return val;
  }
  const func = getFunction(objBits);
  if (func) {
    if (name === '__closure__') {
      if (func.closure && func.closure !== 0n) return func.closure;
      return boxNone();
    }
    if (func.attrs && func.attrs.has(name)) {
      return func.attrs.get(name);
    }
  }
  const obj = getObj(objBits);
  if (obj && obj.type === 'module') {
    if (name === '__dict__') return obj.dictBits ?? boxNone();
    const dict = getDict(obj.dictBits);
    if (!dict) return undefined;
    const nameBits = boxPtr({ type: 'str', value: name });
    const val = dictGetValue(dict, nameBits);
    return val === null ? undefined : val;
  }
  if (obj && obj.type === 'memoryview') {
    if (name === 'format') return obj.formatBits ?? boxNone();
    if (name === 'itemsize') return boxInt(obj.itemsize);
    if (name === 'ndim') return boxInt(obj.ndim ?? memoryviewShape(obj).length);
    if (name === 'readonly') return boxBool(!!obj.readonly);
    if (name === 'shape') {
      const shape = memoryviewShape(obj);
      return tupleFromArray(shape.map((dim) => boxIntOrBigint(BigInt(dim))));
    }
    if (name === 'strides') {
      const strides = memoryviewStrides(obj);
      return tupleFromArray(strides.map((stride) => boxIntOrBigint(BigInt(stride))));
    }
    if (name === 'nbytes') {
      const nbytes = memoryviewNbytesBig(obj);
      if (nbytes === null) return boxNone();
      return boxIntOrBigint(nbytes);
    }
  }
  if (obj && obj.type === 'complex') {
    if (name === 'real') return boxFloat(obj.re);
    if (name === 'imag') return boxFloat(obj.im);
  }
  if (isGenerator(objBits) && memory) {
    const addr = ptrAddr(objBits);
    const view = new DataView(memory.buffer);
    const closedBits = view.getBigInt64(addr + GEN_CLOSED_OFFSET, true);
    const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
    if (name === 'gi_running') {
      return boxBool(generatorIsRunning(addr));
    }
    if (name === 'gi_frame') {
      if (closed) return boxNone();
      const frameBits = generatorFrameBits(addr);
      return isNone(frameBits) ? boxNone() : frameBits;
    }
    if (name === 'gi_yieldfrom') {
      if (closed) return boxNone();
      const yieldBits = generatorYieldFromBits(addr);
      return isNone(yieldBits) ? boxNone() : yieldBits;
    }
    if (name === 'gi_code') {
      return boxNone();
    }
    const funcBits = getGeneratorMethodBits(name);
    if (!isNone(funcBits)) {
      return makeBoundMethod(funcBits, objBits);
    }
  }
  const asyncgenObj = getAsyncGenerator(objBits);
  if (asyncgenObj) {
    if (name === 'ag_running') {
      return boxBool(asyncgenRunning(asyncgenObj));
    }
    if (name === 'ag_await') {
      return asyncgenAwaitBits(asyncgenObj);
    }
    if (name === 'ag_code') {
      return asyncgenCodeBits(asyncgenObj);
    }
    const funcBits = getAsyncGeneratorMethodBits(name);
    if (!isNone(funcBits)) {
      return makeBoundMethod(funcBits, objBits);
    }
  }
  if (
    obj &&
    (obj.type === 'list' ||
      obj.type === 'tuple' ||
      obj.type === 'dict' ||
      obj.type === 'str' ||
      obj.type === 'bytes' ||
      obj.type === 'bytearray' ||
      obj.type === 'memoryview' ||
      obj.type === 'set' ||
      obj.type === 'frozenset' ||
      obj.type === 'complex')
  ) {
    const typeBits = typeOfBits(objBits);
    const val = lookupClassAttr(typeBits, name, objBits);
    if (val !== undefined) return val;
  }
  if (isPtr(objBits) && !heap.has(objBits & POINTER_MASK)) {
    const key = ptrAddr(objBits);
    const attrs = instanceAttrs.get(key);
    if (attrs && attrs.has(name)) {
      return attrs.get(name);
    }
    const clsBits = instanceClasses.get(key);
    if (clsBits !== undefined) {
      const offsetsBits = lookupClassAttr(clsBits, '__molt_field_offsets__');
      const offsets = offsetsBits !== undefined ? getDict(offsetsBits) : null;
      if (offsets && memory) {
        const nameBits = boxPtr({ type: 'str', value: name });
        const offsetBits = dictGetValue(offsets, nameBits);
        if (offsetBits !== null && isIntLike(offsetBits)) {
          const addr = ptrAddr(objBits) + Number(unboxIntLike(offsetBits));
          const view = new DataView(memory.buffer);
          return view.getBigInt64(addr, true);
        }
      }
      const val = lookupClassAttr(clsBits, name, objBits);
      if (val !== undefined) return val;
    }
  }
  return undefined;
};
const lookupSpecialAttr = (objBits, name) => {
  const exc = getException(objBits);
  if (exc) {
    return lookupExceptionAttr(objBits, exc, name);
  }
  const superObj = getObj(objBits);
  if (superObj && superObj.type === 'super') {
    const startBits = superObj.startBits;
    const targetBits = superObj.objBits;
    const targetClass = getClass(targetBits);
    const objTypeBits = targetClass ? targetBits : typeOfBits(targetBits);
    const instanceBits = targetClass ? null : targetBits;
    const val = lookupClassAttr(objTypeBits, name, instanceBits, startBits);
    if (val !== undefined) return val;
    return undefined;
  }
  const cls = getClass(objBits);
  if (cls) {
    const val = lookupClassAttr(objBits, name);
    if (val !== undefined) return val;
  }
  if (isPtr(objBits) && !heap.has(objBits & POINTER_MASK)) {
    const key = ptrAddr(objBits);
    const clsBits = instanceClasses.get(key);
    if (clsBits !== undefined) {
      const val = lookupClassAttr(clsBits, name, objBits);
      if (val !== undefined) return val;
    }
  }
  return undefined;
};
const isSubclass = (subBits, classBits) => {
  const mro = classMroList(subBits);
  return mro.includes(classBits);
};
const typeOfBits = (objBits) => {
  if (isTag(objBits, TAG_NONE)) return getBuiltinType(4);
  if (isTag(objBits, TAG_BOOL)) return getBuiltinType(3);
  if (isTag(objBits, TAG_INT)) return getBuiltinType(1);
  if (isFloat(objBits)) return getBuiltinType(2);
  const obj = getObj(objBits);
  if (obj) {
    if (obj.type === 'class') return getBuiltinType(101);
    if (obj.type === 'super') return getSuperType();
    if (obj.type === 'exception') {
      if (obj.classBits && !isNone(obj.classBits)) return obj.classBits;
      const name = getStr(obj.kindBits) || 'Exception';
      const clsBits = getExceptionClassForName(name);
      obj.classBits = clsBits;
      return clsBits;
    }
    if (obj.type === 'str') return getBuiltinType(5);
    if (obj.type === 'bytes') return getBuiltinType(6);
    if (obj.type === 'bytearray') return getBuiltinType(7);
    if (obj.type === 'list') return getBuiltinType(8);
    if (obj.type === 'tuple') return getBuiltinType(9);
    if (obj.type === 'dict') return getBuiltinType(10);
    if (obj.type === 'set') return getBuiltinType(17);
    if (obj.type === 'frozenset') return getBuiltinType(18);
    if (obj.type === 'memoryview') return getBuiltinType(15);
    if (obj.type === 'complex') return getBuiltinType(19);
  }
  if (isAsyncGenerator(objBits)) return getAsyncGeneratorType();
  if (isGenerator(objBits)) return getGeneratorType();
  if (isPtr(objBits) && !heap.has(objBits & POINTER_MASK)) {
    const clsBits = instanceClasses.get(ptrAddr(objBits));
    if (clsBits !== undefined) return clsBits;
  }
  return getBuiltinType(100);
};
const getAttrValue = (objBits, name) => {
  const val = lookupAttr(objBits, name);
  if (val === undefined) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'AttributeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: `'${typeName(objBits)}' object has no attribute '${name}'`,
        }),
      ),
    );
    return raiseException(exc);
  }
  return val;
};
const getAttrSpecialValue = (objBits, name) => {
  const val = lookupSpecialAttr(objBits, name);
  if (val === undefined) return boxNone();
  return val;
};
const setExceptionAttr = (exc, name, valBits) => {
  if (name === '__cause__' || name === '__context__') {
    if (!isNone(valBits) && !getException(valBits)) {
      const msg =
        name === '__cause__'
          ? 'TypeError: exception cause must be an exception or None'
          : 'TypeError: exception context must be an exception or None';
      throw new Error(msg);
    }
    if (name === '__cause__') {
      exc.causeBits = valBits;
      exc.suppressBits = boxBool(true);
    } else {
      exc.contextBits = valBits;
    }
    return true;
  }
  if (name === '__suppress_context__') {
    exc.suppressBits = boxBool(isTruthyBits(valBits));
    return true;
  }
  if (name === '__traceback__') {
    exc.traceBits = valBits;
    return true;
  }
  if (name === 'args') {
    const argsBits = exceptionArgsFromIterable(valBits);
    if (argsBits === null) return true;
    exc.argsBits = argsBits;
    exc.msgBits = exceptionMessageFromArgs(argsBits);
    exceptionSetValueFromArgs(exc, argsBits);
    return true;
  }
  if (name === '__dict__') {
    const dict = getDict(valBits);
    if (!dict) {
      throw new Error(
        `TypeError: __dict__ must be set to a dictionary, not a '${typeName(valBits)}'`,
      );
    }
    exc.dictBits = valBits;
    return true;
  }
  if (name === 'value' && getStr(exc.kindBits) === 'StopIteration') {
    exc.valueBits = valBits;
    return true;
  }
  const dictBits = exceptionEnsureDict(exc);
  const dict = getDict(dictBits);
  if (dict) {
    const nameBits = boxPtr({ type: 'str', value: name });
    dictSetValue(dict, nameBits, valBits);
    return true;
  }
  return false;
};
const setAttrValue = (objBits, name, valBits) => {
  const exc = getException(objBits);
  if (exc && setExceptionAttr(exc, name, valBits)) {
    return boxNone();
  }
  const cls = getClass(objBits);
  if (cls) {
    cls.attrs.set(name, valBits);
    if (name === '__molt_field_offsets__') {
      const offsets = getDict(valBits);
      if (offsets) {
        const map = new Map();
        for (const [keyBits, offsetBits] of offsets.entries) {
          const key = getStrObj(keyBits);
          if (key === null || !isIntLike(offsetBits)) continue;
          map.set(key, Number(unboxIntLike(offsetBits)));
        }
        classFieldOffsets.set(objBits, map);
      }
    }
    bumpClassLayoutVersion(objBits);
    return boxNone();
  }
  const func = getFunction(objBits);
  if (func) {
    if (!func.attrs) {
      func.attrs = new Map();
    }
    func.attrs.set(name, valBits);
    return boxNone();
  }
  const moduleObj = getModule(objBits);
  if (moduleObj) {
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    const nameBits = boxPtr({ type: 'str', value: name });
    dictSetValue(dict, nameBits, valBits);
    return boxNone();
  }
  const instanceAttrsMap = getInstanceAttrMap(objBits);
  if (instanceAttrsMap) {
    const clsBits = instanceClasses.get(ptrAddr(objBits));
    if (clsBits !== undefined) {
      const offsetsBits = lookupClassAttr(clsBits, '__molt_field_offsets__');
      const offsets = offsetsBits !== undefined ? getDict(offsetsBits) : null;
      if (offsets && memory) {
        const nameBits = boxPtr({ type: 'str', value: name });
        const offsetBits = dictGetValue(offsets, nameBits);
        if (offsetBits !== null && isIntLike(offsetBits)) {
          const addr = ptrAddr(objBits) + Number(unboxIntLike(offsetBits));
          const view = new DataView(memory.buffer);
          view.setBigInt64(addr, valBits, true);
        }
      }
    }
    instanceAttrsMap.set(name, valBits);
    return boxNone();
  }
  return boxNone();
};
const delAttrValue = (objBits, name) => {
  const exc = getException(objBits);
  if (exc) {
    if (name === '__cause__') {
      exc.causeBits = boxNone();
      exc.suppressBits = boxBool(false);
      return boxNone();
    }
    if (name === '__context__') {
      exc.contextBits = boxNone();
      return boxNone();
    }
    if (name === '__suppress_context__') {
      exc.suppressBits = boxBool(false);
      return boxNone();
    }
    if (name === '__traceback__') {
      exc.traceBits = boxNone();
      return boxNone();
    }
    if (name === 'value' && getStr(exc.kindBits) === 'StopIteration') {
      exc.valueBits = boxNone();
      return boxNone();
    }
  }
  const cls = getClass(objBits);
  if (cls) {
    cls.attrs.delete(name);
    bumpClassLayoutVersion(objBits);
    return boxNone();
  }
  const func = getFunction(objBits);
  if (func && func.attrs) {
    func.attrs.delete(name);
    return boxNone();
  }
  const moduleObj = getModule(objBits);
  if (moduleObj) {
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    const nameBits = boxPtr({ type: 'str', value: name });
    dictDelete(dict, nameBits);
    return boxNone();
  }
  const instanceAttrsMap = getInstanceAttrMap(objBits);
  if (instanceAttrsMap) {
    instanceAttrsMap.delete(name);
    return boxNone();
  }
  return boxNone();
};
let lastException = boxNone();
const exceptionStack = [];
const activeExceptionStack = [];
const activeExceptionFallback = [];
const generatorExceptionStacks = new Map();
let generatorRaise = false;
let asyncRaise = false;
const lastNonNone = (stack) => {
  for (let i = stack.length - 1; i >= 0; i -= 1) {
    const bits = stack[i];
    if (!isNone(bits)) {
      return bits;
    }
  }
  return boxNone();
};
const exceptionDepth = () => exceptionStack.length;
const exceptionSetDepth = (depth) => {
  const target = Math.max(0, depth);
  while (exceptionStack.length > target) {
    exceptionStack.pop();
  }
  while (exceptionStack.length < target) {
    exceptionStack.push(1);
  }
  while (activeExceptionStack.length > target) {
    activeExceptionStack.pop();
  }
  while (activeExceptionStack.length < target) {
    activeExceptionStack.push(boxNone());
  }
};
const exceptionArgsItems = (argsBits) => {
  if (isNone(argsBits) || argsBits === 0n) return [];
  const obj = getObj(argsBits);
  if (obj && obj.type === 'tuple') return obj.items;
  if (obj && obj.type === 'list') return obj.items;
  return [argsBits];
};
const exceptionNormalizeArgs = (argsBits) => {
  if (isNone(argsBits) || argsBits === 0n) return tupleFromArray([]);
  const obj = getObj(argsBits);
  if (obj && obj.type === 'tuple') return argsBits;
  if (obj && obj.type === 'list') return tupleFromArray([...obj.items]);
  return tupleFromArray([argsBits]);
};
const exceptionArgs = (msgBits) => tupleFromArray([msgBits]);
const exceptionArgsFromIterable = (argsBits) => {
  const obj = getObj(argsBits);
  if (obj && obj.type === 'tuple') return argsBits;
  if (obj && obj.type === 'list') return tupleFromArray([...obj.items]);
  const errMsg = `'${typeName(argsBits)}' object is not iterable`;
  const items = collectIterableValues(argsBits, errMsg);
  if (items === null) return null;
  return tupleFromArray(items);
};
const exceptionMessageFromArgs = (argsBits) => {
  const items = exceptionArgsItems(argsBits);
  if (items.length === 0) return boxPtr({ type: 'str', value: '' });
  if (items.length === 1) return baseImports.str_from_obj(items[0]);
  return baseImports.str_from_obj(argsBits);
};
const exceptionMessageForKind = (kindBits, argsBits) => {
  const items = exceptionArgsItems(argsBits);
  if (items.length === 1 && getStr(kindBits) === 'KeyError') {
    return boxPtr({ type: 'str', value: reprStringFromBits(items[0]) });
  }
  return exceptionMessageFromArgs(argsBits);
};
const exceptionClass = (kindBits) => {
  const name = getStrObj(kindBits);
  if (name === null) {
    throw new Error('TypeError: exception kind must be a str');
  }
  return getExceptionClassForName(name);
};
const exceptionEnsureDict = (exc) => {
  if (!exc.dictBits || isNone(exc.dictBits)) {
    exc.dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
  }
  return exc.dictBits;
};
const exceptionSetValueFromArgs = (exc, argsBits) => {
  if (getStr(exc.kindBits) !== 'StopIteration') return;
  const items = exceptionArgsItems(argsBits);
  exc.valueBits = items.length ? items[0] : boxNone();
};
const formatTupleRepr = (items) => {
  const rendered = items.map((item) => reprStringFromBits(item));
  if (items.length === 1) {
    return `(${rendered[0]},)`;
  }
  return `(${rendered.join(', ')})`;
};
const exceptionReprFromArgs = (kind, argsBits) => {
  const items = exceptionArgsItems(argsBits);
  if (items.length === 0) return `${kind}()`;
  if (items.length === 1) return `${kind}(${reprStringFromBits(items[0])})`;
  return `${kind}${formatTupleRepr(items)}`;
};
const reprStringFromBits = (val) => {
  if (isTag(val, TAG_INT)) return unboxInt(val).toString();
  if (isFloat(val)) return formatFloat(bitsToFloat(val));
  if (isTag(val, TAG_BOOL)) return (val & 1n) === 1n ? 'True' : 'False';
  if (isTag(val, TAG_NONE)) return 'None';
  const obj = getObj(val);
  if (obj && obj.type === 'str') return formatStringRepr(obj.value);
  if (obj && obj.type === 'exception') {
    const kind = getStr(obj.kindBits) || 'Exception';
    return exceptionReprFromArgs(kind, obj.argsBits || boxNone());
  }
  if (obj && obj.type === 'bigint') return obj.value.toString();
  if (obj && obj.type === 'tuple') return formatTupleRepr(obj.items);
  if (obj && obj.type === 'list') {
    return `[${obj.items.map((item) => reprStringFromBits(item)).join(', ')}]`;
  }
  if (obj && obj.type === 'dict') {
    const pairs = obj.entries.map(
      ([key, value]) => `${reprStringFromBits(key)}: ${reprStringFromBits(value)}`,
    );
    return `{${pairs.join(', ')}}`;
  }
  if (obj && (obj.type === 'set' || obj.type === 'frozenset')) {
    const items = [...obj.items].map((item) => reprStringFromBits(item));
    if (items.length === 0) {
      return obj.type === 'frozenset' ? 'frozenset()' : 'set()';
    }
    const body = `{${items.join(', ')}}`;
    return obj.type === 'frozenset' ? `frozenset(${body})` : body;
  }
  return '<obj>';
};
const exceptionNew = (kindBits, argsBits) => {
  const normArgsBits = exceptionNormalizeArgs(argsBits);
  const msgBits = exceptionMessageForKind(kindBits, normArgsBits);
  const classBits = exceptionClass(kindBits);
  const exc = {
    type: 'exception',
    kindBits,
    msgBits,
    argsBits: normArgsBits,
    classBits,
    dictBits: boxNone(),
    valueBits: boxNone(),
    causeBits: boxNone(),
    contextBits: boxNone(),
    suppressBits: boxBool(false),
    traceBits: boxNone(),
  };
  exceptionSetValueFromArgs(exc, normArgsBits);
  return boxPtr(exc);
};
const exceptionNewFromClass = (classBits, argsBits) => {
  const cls = getClass(classBits);
  if (!cls) {
    throw new Error('TypeError: exception class must be a type');
  }
  let kindBits = null;
  const nameBits = cls.attrs ? cls.attrs.get('__name__') : undefined;
  if (nameBits !== undefined && getStrObj(nameBits) !== null) {
    kindBits = nameBits;
  } else if (cls.name) {
    kindBits = boxPtr({ type: 'str', value: cls.name });
  } else {
    kindBits = boxPtr({ type: 'str', value: 'Exception' });
  }
  const normArgsBits = exceptionNormalizeArgs(argsBits);
  const msgBits = exceptionMessageForKind(kindBits, normArgsBits);
  const exc = {
    type: 'exception',
    kindBits,
    msgBits,
    argsBits: normArgsBits,
    classBits,
    dictBits: boxNone(),
    valueBits: boxNone(),
    causeBits: boxNone(),
    contextBits: boxNone(),
    suppressBits: boxBool(false),
    traceBits: boxNone(),
  };
  exceptionSetValueFromArgs(exc, normArgsBits);
  return boxPtr(exc);
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
const exceptionSetValue = (excBits, valueBits) => {
  const exc = getException(excBits);
  if (!exc) return boxNone();
  exc.valueBits = valueBits;
  return boxNone();
};
const exceptionContextSet = (excBits) => {
  if (!activeExceptionStack.length || isNone(excBits)) return boxNone();
  const exc = getException(excBits);
  if (!exc) {
    throw new Error('TypeError: expected exception object');
  }
  activeExceptionStack[activeExceptionStack.length - 1] = excBits;
  return boxNone();
};
const exceptionSetLast = (excBits) => {
  const exc = getException(excBits);
  if (!exc) {
    throw new Error('TypeError: expected exception object');
  }
  const traceBits = frameStackTraceBits();
  exc.traceBits = traceBits === null ? boxNone() : traceBits;
  const activeContext = lastNonNone(activeExceptionStack);
  const fallbackContext = lastNonNone(activeExceptionFallback);
  const context = !isNone(activeContext) ? activeContext : fallbackContext;
  const candidate = !isNone(lastException) ? lastException : context;
  if (isNone(exc.contextBits) && !isNone(candidate) && candidate !== excBits) {
    exc.contextBits = candidate;
  }
  lastException = excBits;
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
  if (exc.msgBits && !isNone(exc.msgBits)) return exc.msgBits;
  const argsBits = exc.argsBits || boxNone();
  const msgBits = exceptionMessageForKind(exc.kindBits, argsBits);
  exc.msgBits = msgBits;
  return msgBits;
};
const exceptionLast = () => lastException;
const exceptionActive = () => {
  const active = lastNonNone(activeExceptionStack);
  if (!isNone(active)) return active;
  return lastNonNone(activeExceptionFallback);
};
const exceptionClear = () => {
  lastException = boxNone();
  return boxNone();
};
const exceptionPending = () => (isNone(lastException) ? 0n : 1n);
const exceptionPush = () => {
  exceptionStack.push(1);
  activeExceptionStack.push(boxNone());
  return boxNone();
};
const exceptionPop = () => {
  if (!exceptionStack.length) {
    throw new Error('RuntimeError: exception handler stack underflow');
  }
  exceptionStack.pop();
  activeExceptionStack.pop();
  return boxNone();
};
const frameStackPush = (codeBits) => {
  let line = 0;
  const codeObj = getCode(codeBits);
  if (codeObj) {
    const rawLine = Number(codeObj.firstlineno);
    line = Number.isFinite(rawLine) ? Math.trunc(rawLine) : 0;
  }
  frameStack.push({ codeBits, line });
};
const frameStackSetLine = (line) => {
  if (!frameStack.length) return;
  frameStack[frameStack.length - 1].line = line;
};
const frameStackPop = () => {
  if (frameStack.length) {
    frameStack.pop();
  }
};
const frameObjectBits = (codeBits, line) => {
  // TODO(introspection, owner:runtime, milestone:TC2, priority:P1, status:partial): expand frame fields (f_back, f_globals, f_locals) and keep f_lasti/f_lineno updated.
  if (!memory) return boxNone();
  const classBits = getFrameType();
  const frameBits = allocInstanceForClass(classBits);
  if (isNone(frameBits)) return boxNone();
  const attrs = getInstanceAttrMap(frameBits);
  if (attrs) {
    attrs.set('f_code', codeBits);
    attrs.set('f_lineno', boxInt(BigInt(line)));
    attrs.set('f_lasti', boxInt(-1n));
  }
  return frameBits;
};
const tracebackObjectBits = (frameBits, line, nextBits) => {
  if (!memory) return boxNone();
  const classBits = getTracebackType();
  const tbBits = allocInstanceForClass(classBits);
  if (isNone(tbBits)) return boxNone();
  const attrs = getInstanceAttrMap(tbBits);
  if (attrs) {
    attrs.set('tb_frame', frameBits);
    attrs.set('tb_lineno', boxInt(BigInt(line)));
    attrs.set('tb_next', nextBits);
  }
  return tbBits;
};
const frameStackTraceBits = () => {
  if (!frameStack.length) return null;
  let nextBits = boxNone();
  let built = false;
  for (let idx = frameStack.length - 1; idx >= 0; idx -= 1) {
    const entry = frameStack[idx];
    const codeObj = getCode(entry.codeBits);
    if (!codeObj) continue;
    let line = entry.line || 0;
    if (!line) {
      const rawLine = Number(codeObj.firstlineno);
      line = Number.isFinite(rawLine) ? Math.trunc(rawLine) : 0;
    }
    if (!Number.isFinite(line)) {
      line = 0;
    }
    const frameBits = frameObjectBits(entry.codeBits, line);
    if (isNone(frameBits)) continue;
    const tbBits = tracebackObjectBits(frameBits, line, nextBits);
    if (isNone(tbBits)) continue;
    nextBits = tbBits;
    built = true;
  }
  return built ? nextBits : null;
};
const formatTraceback = (traceBits) => {
  if (isNone(traceBits)) return '';
  let out = 'Traceback (most recent call last):\\n';
  let current = traceBits;
  let depth = 0;
  while (!isNone(current) && depth < 512) {
    const frameBits = lookupAttr(current, 'tb_frame');
    const lineBits = lookupAttr(current, 'tb_lineno');
    let line = 0;
    if (lineBits !== undefined && isIntLike(lineBits)) {
      const rawLine = Number(unboxIntLike(lineBits));
      line = Number.isFinite(rawLine) ? Math.trunc(rawLine) : 0;
    }
    let filename = '<unknown>';
    let name = '<module>';
    if (frameBits !== undefined && !isNone(frameBits)) {
      const codeBits = lookupAttr(frameBits, 'f_code');
      const codeObj = codeBits !== undefined ? getCode(codeBits) : null;
      if (codeObj) {
        const rawFilename = getStr(codeObj.filenameBits);
        const rawName = getStr(codeObj.nameBits);
        if (rawFilename) filename = rawFilename;
        if (rawName) name = rawName;
      }
      if (!line) {
        const fLineBits = lookupAttr(frameBits, 'f_lineno');
        if (fLineBits !== undefined && isIntLike(fLineBits)) {
          const rawLine = Number(unboxIntLike(fLineBits));
          line = Number.isFinite(rawLine) ? Math.trunc(rawLine) : 0;
        }
      }
    }
    out += `  File "${filename}", line ${line}, in ${name}\n`;
    const nextBits = lookupAttr(current, 'tb_next');
    current = nextBits === undefined ? boxNone() : nextBits;
    depth += 1;
  }
  if (!isNone(current)) {
    out += '  <traceback truncated>\\n';
  }
  return out;
};
const raiseException = (excBits) => {
  let exc = getException(excBits);
  if (!exc) {
    const cls = getClass(excBits);
    if (!cls || !isSubclass(excBits, getBaseExceptionClass())) {
      const errExc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: 'exceptions must derive from BaseException',
          }),
        ),
      );
      excBits = errExc;
      exc = getException(excBits);
    } else {
      const instBits = exceptionNewFromClass(excBits, tupleFromArray([]));
      if (!isNone(instBits)) {
        const initBits = lookupClassAttr(excBits, '__init__');
        if (initBits !== undefined && !isNone(initBits)) {
          const builder = baseImports.callargs_new(boxInt(0), boxInt(0));
          const args = getCallArgs(builder);
          if (args) {
            args.pos.unshift(instBits);
          }
          baseImports.call_bind(initBits, builder);
          if (exceptionPending() !== 0n) return boxNone();
        }
        excBits = instBits;
        exc = getException(excBits);
      }
    }
  }
  if (exc) {
    const traceBits = frameStackTraceBits();
    exc.traceBits = traceBits === null ? boxNone() : traceBits;
  }
  const activeContext = lastNonNone(activeExceptionStack);
  const fallbackContext = lastNonNone(activeExceptionFallback);
  const context = !isNone(activeContext) ? activeContext : fallbackContext;
  const candidate = !isNone(lastException) ? lastException : context;
  if (
    exc &&
    isNone(exc.contextBits) &&
    !isNone(candidate) &&
    candidate !== excBits
  ) {
    exc.contextBits = candidate;
  }
  lastException = excBits;
  if (!exceptionStack.length && !generatorRaise && !asyncRaise) {
    const kind = exc ? getStr(exc.kindBits) : 'Exception';
    let msg = '';
    if (exc) {
      const msgBits = exceptionMessageForKind(exc.kindBits, exc.argsBits || boxNone());
      msg = getStr(msgBits);
    }
    let rendered = '';
    if (exc && exc.traceBits && !isNone(exc.traceBits)) {
      rendered += formatTraceback(exc.traceBits);
    }
    rendered += msg ? `${kind}: ${msg}` : kind;
    throw new Error(rendered);
  }
  return boxNone();
};
const generatorFlags = (addr) =>
  memView().getBigInt64(addr - HEADER_FLAGS_OFFSET, true);
const generatorSetFlag = (addr, flag, enabled) => {
  const view = memView();
  let flags = view.getBigInt64(addr - HEADER_FLAGS_OFFSET, true);
  if (enabled) {
    flags |= flag;
  } else {
    flags &= ~flag;
  }
  view.setBigInt64(addr - HEADER_FLAGS_OFFSET, flags, true);
};
const generatorIsRunning = (addr) =>
  (generatorFlags(addr) & GEN_FLAG_RUNNING) !== 0n;
const generatorMarkStarted = (addr) => generatorSetFlag(addr, GEN_FLAG_STARTED, true);
const generatorSetRunning = (addr, running) =>
  generatorSetFlag(addr, GEN_FLAG_RUNNING, running);
const generatorFrameBits = (addr) => memView().getBigInt64(addr + GEN_FRAME_OFFSET, true);
const generatorSetFrameBits = (addr, bits) =>
  memView().setBigInt64(addr + GEN_FRAME_OFFSET, bits, true);
const generatorYieldFromBits = (addr) =>
  memView().getBigInt64(addr + GEN_YIELD_FROM_OFFSET, true);
const generatorSetYieldFromBits = (addr, bits) =>
  memView().setBigInt64(addr + GEN_YIELD_FROM_OFFSET, bits, true);
const generatorClearIntrospection = (addr) => {
  generatorSetYieldFromBits(addr, boxNone());
  generatorSetFrameBits(addr, boxNone());
  generatorSetRunning(addr, false);
};
const frameNew = (lasti) => {
  const addr = allocRaw(8);
  if (!addr) return boxNone();
  const bits = boxPtrAddr(addr);
  const attrs = getInstanceAttrMap(bits);
  if (attrs) {
    attrs.set('f_lasti', boxInt(BigInt(lasti)));
  }
  return bits;
};
const frameSetLasti = (frameBits, lasti) => {
  if (!isPtr(frameBits)) return;
  const attrs = getInstanceAttrMap(frameBits);
  if (attrs) {
    attrs.set('f_lasti', boxInt(BigInt(lasti)));
  }
};
const generatorResume = (gen) => {
  if (!isGenerator(gen) || !memory || !table) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const addr = ptrAddr(gen);
  const closedBits = memView().getBigInt64(addr + GEN_CLOSED_OFFSET, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  if (generatorIsRunning(addr)) {
    throw new Error('ValueError: generator already executing');
  }
  if ((generatorFlags(addr) & GEN_FLAG_STARTED) === 0n) {
    generatorMarkStarted(addr);
    frameSetLasti(generatorFrameBits(addr), 0);
  }
  const callerDepth = exceptionDepth();
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = memView().getBigInt64(addr + GEN_EXC_DEPTH_OFFSET, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  const pollIdx = memView().getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = getTableFunc(pollIdx);
  const prevRaise = generatorRaise;
  generatorRaise = true;
  generatorSetRunning(addr, true);
  let res;
  try {
    res = poll ? poll(BigInt(addr)) : tupleFromArray([boxNone(), boxBool(true)]);
  } finally {
    generatorRaise = prevRaise;
    generatorSetRunning(addr, false);
  }
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  memView().setBigInt64(addr + GEN_EXC_DEPTH_OFFSET, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
    generatorClearIntrospection(addr);
    return raiseException(excBits);
  }
  if (res) {
    const pair = getTuple(res);
    if (pair && pair.items.length >= 2) {
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
        generatorClearIntrospection(addr);
      }
    }
  }
  return res;
};
const generatorSend = (gen, sendVal) => {
  if (!isGenerator(gen) || !memory || !table) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const addr = ptrAddr(gen);
  const closedBits = memView().getBigInt64(addr + GEN_CLOSED_OFFSET, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  if (generatorIsRunning(addr)) {
    throw new Error('ValueError: generator already executing');
  }
  const started = (generatorFlags(addr) & GEN_FLAG_STARTED) !== 0n;
  if (!started && !isNone(sendVal)) {
    throw new Error(
      "TypeError: can't send non-None value to a just-started generator",
    );
  }
  if (!started) {
    generatorMarkStarted(addr);
    frameSetLasti(generatorFrameBits(addr), 0);
  }
  const callerDepth = exceptionDepth();
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = memView().getBigInt64(addr + GEN_EXC_DEPTH_OFFSET, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  memView().setBigInt64(addr + GEN_SEND_OFFSET, sendVal, true);
  memView().setBigInt64(addr + GEN_THROW_OFFSET, boxNone(), true);
  const pollIdx = memView().getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = getTableFunc(pollIdx);
  const prevRaise = generatorRaise;
  generatorRaise = true;
  generatorSetRunning(addr, true);
  let res;
  try {
    res = poll ? poll(BigInt(addr)) : tupleFromArray([boxNone(), boxBool(true)]);
  } finally {
    generatorRaise = prevRaise;
    generatorSetRunning(addr, false);
  }
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  memView().setBigInt64(addr + GEN_EXC_DEPTH_OFFSET, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
    generatorClearIntrospection(addr);
    return raiseException(excBits);
  }
  if (res) {
    const pair = getTuple(res);
    if (pair && pair.items.length >= 2) {
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
        generatorClearIntrospection(addr);
      }
    }
  }
  return res;
};
const generatorThrow = (gen, exc) => {
  if (!isGenerator(gen) || !memory || !table) {
    return tupleFromArray([boxNone(), boxBool(true)]);
  }
  const addr = ptrAddr(gen);
  const closedBits = memView().getBigInt64(addr + GEN_CLOSED_OFFSET, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) {
    return raiseException(exc);
  }
  if (generatorIsRunning(addr)) {
    throw new Error('ValueError: generator already executing');
  }
  if ((generatorFlags(addr) & GEN_FLAG_STARTED) === 0n) {
    generatorMarkStarted(addr);
    frameSetLasti(generatorFrameBits(addr), 0);
  }
  const callerDepth = exceptionDepth();
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = memView().getBigInt64(addr + GEN_EXC_DEPTH_OFFSET, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  memView().setBigInt64(addr + GEN_THROW_OFFSET, exc, true);
  memView().setBigInt64(addr + GEN_SEND_OFFSET, boxNone(), true);
  const pollIdx = memView().getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = getTableFunc(pollIdx);
  if (!poll) {
    activeExceptionFallback.pop();
    return raiseException(exc);
  }
  const prevRaise = generatorRaise;
  generatorRaise = true;
  generatorSetRunning(addr, true);
  let res;
  try {
    res = poll(BigInt(addr));
  } finally {
    generatorRaise = prevRaise;
    generatorSetRunning(addr, false);
  }
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  memView().setBigInt64(addr + GEN_EXC_DEPTH_OFFSET, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
    generatorClearIntrospection(addr);
    return raiseException(excBits);
  }
  if (res) {
    const pair = getTuple(res);
    if (pair && pair.items.length >= 2) {
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
        generatorClearIntrospection(addr);
      }
    }
  }
  return res;
};
const generatorClose = (gen) => {
  if (!isGenerator(gen) || !memory || !table) {
    return boxNone();
  }
  const addr = ptrAddr(gen);
  const closedBits = memView().getBigInt64(addr + GEN_CLOSED_OFFSET, true);
  const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
  if (closed) return boxNone();
  if (generatorIsRunning(addr)) {
    throw new Error('ValueError: generator already executing');
  }
  const callerDepth = exceptionDepth();
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = memView().getBigInt64(addr + GEN_EXC_DEPTH_OFFSET, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  const exc = exceptionNew(
    boxPtr({ type: 'str', value: 'GeneratorExit' }),
    boxNone(),
  );
  memView().setBigInt64(addr + GEN_THROW_OFFSET, exc, true);
  memView().setBigInt64(addr + GEN_SEND_OFFSET, boxNone(), true);
  const pollIdx = memView().getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = getTableFunc(pollIdx);
  if (!poll) {
    memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
    generatorClearIntrospection(addr);
    return boxNone();
  }
  const prevRaise = generatorRaise;
  generatorRaise = true;
  generatorSetRunning(addr, true);
  let res;
  try {
    res = poll(BigInt(addr));
  } finally {
    generatorRaise = prevRaise;
    generatorSetRunning(addr, false);
  }
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  memView().setBigInt64(addr + GEN_EXC_DEPTH_OFFSET, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    const excObj = getException(excBits);
    const isExit = excObj && getStr(excObj.kindBits) === 'GeneratorExit';
    if (isExit) {
      memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
      generatorClearIntrospection(addr);
      return boxNone();
    }
    memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
    generatorClearIntrospection(addr);
    return raiseException(excBits);
  }
  if (res) {
    const pair = getTuple(res);
    if (pair) {
      const doneBits = pair.items[1];
      const done = isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n;
      if (!done) {
        const errExc = exceptionNew(
          boxPtr({ type: 'str', value: 'RuntimeError' }),
          exceptionArgs(
            boxPtr({ type: 'str', value: 'generator ignored GeneratorExit' }),
          ),
        );
        raiseException(errExc);
      }
    }
  }
  memView().setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(true), true);
  generatorClearIntrospection(addr);
  return boxNone();
};
const asyncgenPoll = (taskPtr) => {
  const addr = expectPtrAddr(taskPtr, 'asyncgen_poll');
  if (addr === 0 || !memory || !table) return boxNone();
  const state = Number(memView().getBigInt64(addr - HEADER_STATE_OFFSET, true));
  const asyncgenBits = memView().getBigInt64(addr + 0, true);
  const opBits = memView().getBigInt64(addr + 8, true);
  const argBits = memView().getBigInt64(addr + 16, true);
  const asyncgenObj = getAsyncGenerator(asyncgenBits);
  if (!asyncgenObj) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'expected async generator' })),
    );
    return raiseException(exc);
  }
  const genBits = asyncgenObj.genBits;
  if (!isGenerator(genBits)) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'expected generator' })),
    );
    return raiseException(exc);
  }
  const genAddr = ptrAddr(genBits);
  const runningBits = asyncgenObj.runningBits;
  const taskBits = boxPtrAddr(addr);
  if (state === 0) {
    if (!isNone(runningBits) && runningBits !== taskBits) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'RuntimeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: 'anext()/asend()/athrow()/aclose() already running',
          }),
        ),
      );
      return raiseException(exc);
    }
  } else if (!isNone(runningBits) && runningBits !== taskBits) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'RuntimeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: 'anext()/asend()/athrow()/aclose() already running',
        }),
      ),
    );
    return raiseException(exc);
  }
  if (generatorIsRunning(genAddr)) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'RuntimeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: 'anext()/asend()/athrow()/aclose() already running',
        }),
      ),
    );
    return raiseException(exc);
  }
  const op = isTag(opBits, TAG_INT) ? Number(unboxInt(opBits)) : Number(opBits);
  let res;
  if (state !== 0) {
    res = generatorResume(genBits);
  } else if (op === Number(ASYNCGEN_OP_ANEXT)) {
    const closedBits = memView().getBigInt64(genAddr + GEN_CLOSED_OFFSET, true);
    const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
    if (closed) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'StopAsyncIteration' }),
        boxNone(),
      );
      return raiseException(exc);
    }
    const pendingThrow = memView().getBigInt64(genAddr + GEN_THROW_OFFSET, true);
    if (!isNone(pendingThrow)) {
      res = generatorResume(genBits);
    } else {
      res = generatorSend(genBits, boxNone());
    }
  } else if (op === Number(ASYNCGEN_OP_ASEND)) {
    const closedBits = memView().getBigInt64(genAddr + GEN_CLOSED_OFFSET, true);
    const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
    if (closed) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'StopAsyncIteration' }),
        boxNone(),
      );
      return raiseException(exc);
    }
    const started = (generatorFlags(genAddr) & GEN_FLAG_STARTED) !== 0n;
    if (!started && !isNone(argBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: "can't send non-None value to a just-started async generator",
          }),
        ),
      );
      return raiseException(exc);
    }
    const pendingThrow = memView().getBigInt64(genAddr + GEN_THROW_OFFSET, true);
    if (!isNone(pendingThrow)) {
      res = generatorResume(genBits);
    } else {
      res = generatorSend(genBits, argBits);
    }
  } else if (op === Number(ASYNCGEN_OP_ATHROW)) {
    const closedBits = memView().getBigInt64(genAddr + GEN_CLOSED_OFFSET, true);
    const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
    if (closed) return boxNone();
    res = generatorThrow(genBits, argBits);
  } else if (op === Number(ASYNCGEN_OP_ACLOSE)) {
    const closedBits = memView().getBigInt64(genAddr + GEN_CLOSED_OFFSET, true);
    const closed = isTag(closedBits, TAG_BOOL) && (closedBits & 1n) === 1n;
    if (closed) return boxNone();
    const started = (generatorFlags(genAddr) & GEN_FLAG_STARTED) !== 0n;
    if (!started) {
      memView().setBigInt64(genAddr + GEN_CLOSED_OFFSET, boxBool(true), true);
      return boxNone();
    }
    res = generatorThrow(genBits, argBits);
  } else {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(
        boxPtr({ type: 'str', value: 'invalid async generator op' }),
      ),
    );
    return raiseException(exc);
  }
  if (exceptionPending() !== 0n) {
    if (asyncgenObj.runningBits === taskBits) {
      asyncgenObj.runningBits = boxNone();
    }
    memView().setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
    return res;
  }
  if (isPending(res)) {
    asyncgenObj.runningBits = taskBits;
    memView().setBigInt64(addr - HEADER_STATE_OFFSET, 1n, true);
    return res;
  }
  if (asyncgenObj.runningBits === taskBits) {
    asyncgenObj.runningBits = boxNone();
  }
  memView().setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
  const pair = getTuple(res);
  if (pair && pair.items.length >= 2) {
    const valBits = pair.items[0];
    const doneBits = pair.items[1];
    const done = isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n;
    if (op === Number(ASYNCGEN_OP_ACLOSE)) {
      memView().setBigInt64(genAddr + GEN_CLOSED_OFFSET, boxBool(true), true);
      if (!done) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'RuntimeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: 'async generator ignored GeneratorExit',
            }),
          ),
        );
        return raiseException(exc);
      }
      return boxNone();
    }
    if (!done) {
      return valBits;
    }
    if (op === Number(ASYNCGEN_OP_ANEXT) || op === Number(ASYNCGEN_OP_ASEND)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'StopAsyncIteration' }),
        boxNone(),
      );
      return raiseException(exc);
    }
    return boxNone();
  }
  if (op === Number(ASYNCGEN_OP_ACLOSE)) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'RuntimeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: 'async generator ignored GeneratorExit',
        }),
      ),
    );
    return raiseException(exc);
  }
  return res;
};
const generatorPairToValue = (pairBits) => {
  const pair = getTuple(pairBits);
  if (!pair || pair.items.length < 2) {
    throw new Error('TypeError: object is not an iterator');
  }
  const valBits = pair.items[0];
  const doneBits = pair.items[1];
  if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
    const argsBits = isNone(valBits)
      ? tupleFromArray([])
      : tupleFromArray([valBits]);
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'StopIteration' }),
      argsBits,
    );
    return raiseException(exc);
  }
  return valBits;
};
const generatorPairToValueSafe = (pairBits) => {
  const prevRaise = generatorRaise;
  generatorRaise = true;
  try {
    return generatorPairToValue(pairBits);
  } finally {
    generatorRaise = prevRaise;
  }
};
const generatorIterMethod = (selfBits) => selfBits;
const generatorNextMethod = (selfBits) => {
  const pairBits = generatorSend(selfBits, boxNone());
  if (exceptionPending() !== 0n) return boxNone();
  return generatorPairToValueSafe(pairBits);
};
const generatorSendMethod = (selfBits, sendVal) => {
  const pairBits = generatorSend(selfBits, sendVal);
  if (exceptionPending() !== 0n) return boxNone();
  return generatorPairToValueSafe(pairBits);
};
const generatorThrowMethod = (selfBits, excBits) => {
  const pairBits = generatorThrow(selfBits, excBits);
  if (exceptionPending() !== 0n) return boxNone();
  return generatorPairToValueSafe(pairBits);
};
const generatorCloseMethod = (selfBits) => generatorClose(selfBits);
const wrapTableFunc = (fn, arity) => {
  if (typeof WebAssembly.Function !== 'function') return fn;
  const params = new Array(arity).fill('i64');
  return new WebAssembly.Function({ parameters: params, results: ['i64'] }, fn);
};
const getOrAddTableFunc = (fn, arity) => {
  const cached = tableFuncCache.get(fn);
  if (cached !== undefined) return cached;
  let idx;
  if (typeof WebAssembly.Function === 'function') {
    if (!table) return null;
    idx = table.length;
    table.grow(1);
    table.set(idx, wrapTableFunc(fn, arity));
  } else {
    idx = hostTable.length + HOST_TABLE_FLAG;
    hostTable.push(fn);
  }
  tableFuncCache.set(fn, idx);
  return idx;
};
const getGeneratorMethodBits = (name) => {
  const cached = generatorMethodBits.get(name);
  if (cached !== undefined) return cached;
  let idx = null;
  let arity = 0;
  if (name === 'send') {
    if (generatorSendMethodIdx === null) {
      generatorSendMethodIdx = getOrAddTableFunc(generatorSendMethod, 2);
    }
    idx = generatorSendMethodIdx;
    arity = 2;
  } else if (name === 'throw') {
    if (generatorThrowMethodIdx === null) {
      generatorThrowMethodIdx = getOrAddTableFunc(generatorThrowMethod, 2);
    }
    idx = generatorThrowMethodIdx;
    arity = 2;
  } else if (name === 'close') {
    if (generatorCloseMethodIdx === null) {
      generatorCloseMethodIdx = getOrAddTableFunc(generatorCloseMethod, 1);
    }
    idx = generatorCloseMethodIdx;
    arity = 1;
  } else if (name === '__iter__') {
    if (generatorIterMethodIdx === null) {
      generatorIterMethodIdx = getOrAddTableFunc(generatorIterMethod, 1);
    }
    idx = generatorIterMethodIdx;
    arity = 1;
  } else if (name === '__next__') {
    if (generatorNextMethodIdx === null) {
      generatorNextMethodIdx = getOrAddTableFunc(generatorNextMethod, 1);
    }
    idx = generatorNextMethodIdx;
    arity = 1;
  }
  if (idx === null) return boxNone();
  const bits = baseImports.func_new(BigInt(idx), 0n, BigInt(arity));
  generatorMethodBits.set(name, bits);
  return bits;
};
const asyncgenRunning = (asyncgenObj) => {
  if (!asyncgenObj) return false;
  if (!isNone(asyncgenObj.runningBits)) return true;
  const genBits = asyncgenObj.genBits;
  if (!isGenerator(genBits) || !memory) return false;
  const addr = ptrAddr(genBits);
  return generatorIsRunning(addr);
};
const asyncgenAwaitBits = (asyncgenObj) => {
  if (!asyncgenObj) return boxNone();
  const runningBits = asyncgenObj.runningBits;
  if (isNone(runningBits)) return boxNone();
  if (!isPtr(runningBits) || heap.has(runningBits & POINTER_MASK)) return boxNone();
  const addr = ptrAddr(runningBits);
  const awaited = taskWaitingOn.get(addr);
  return awaited === undefined ? boxNone() : awaited;
};
const asyncgenCodeBits = (_asyncgenObj) => boxNone();
const asyncgenFutureNew = (asyncgenBits, opKind, argBits) => {
  const asyncgenObj = getAsyncGenerator(asyncgenBits);
  if (!asyncgenObj) {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'expected async generator' })),
    );
    return raiseException(exc);
  }
  if (!memory || !table) return boxNone();
  let pollIdx = asyncgenPollIdx;
  if (pollIdx === null) {
    pollIdx = getOrAddTableFunc(baseImports.asyncgen_poll, 1);
    if (pollIdx === null) return boxNone();
    asyncgenPollIdx = pollIdx;
  }
  const addr = allocRaw(24);
  if (!addr) return boxNone();
  const view = new DataView(memory.buffer);
  view.setUint32(addr - HEADER_POLL_FN_OFFSET, pollIdx, true);
  view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
  view.setBigInt64(addr + 0, asyncgenBits, true);
  view.setBigInt64(addr + 8, boxInt(opKind), true);
  view.setBigInt64(addr + 16, argBits, true);
  return boxPtrAddr(addr);
};
const asyncgenAiterMethod = (selfBits) => selfBits;
const asyncgenAnextMethod = (selfBits) =>
  asyncgenFutureNew(selfBits, ASYNCGEN_OP_ANEXT, boxNone());
const asyncgenAsendMethod = (selfBits, valBits) =>
  asyncgenFutureNew(selfBits, ASYNCGEN_OP_ASEND, valBits);
const asyncgenAthrowMethod = (selfBits, excBits) =>
  asyncgenFutureNew(selfBits, ASYNCGEN_OP_ATHROW, excBits);
const asyncgenAcloseMethod = (selfBits) => {
  const exc = exceptionNew(
    boxPtr({ type: 'str', value: 'GeneratorExit' }),
    boxNone(),
  );
  return asyncgenFutureNew(selfBits, ASYNCGEN_OP_ACLOSE, exc);
};
const getAsyncGeneratorMethodBits = (name) => {
  const cached = asyncgenMethodBits.get(name);
  if (cached !== undefined) return cached;
  let idx = null;
  let arity = 0;
  if (name === '__aiter__') {
    if (asyncgenAiterMethodIdx === null) {
      asyncgenAiterMethodIdx = getOrAddTableFunc(asyncgenAiterMethod, 1);
    }
    idx = asyncgenAiterMethodIdx;
    arity = 1;
  } else if (name === '__anext__') {
    if (asyncgenAnextMethodIdx === null) {
      asyncgenAnextMethodIdx = getOrAddTableFunc(asyncgenAnextMethod, 1);
    }
    idx = asyncgenAnextMethodIdx;
    arity = 1;
  } else if (name === 'asend') {
    if (asyncgenAsendMethodIdx === null) {
      asyncgenAsendMethodIdx = getOrAddTableFunc(asyncgenAsendMethod, 2);
    }
    idx = asyncgenAsendMethodIdx;
    arity = 2;
  } else if (name === 'athrow') {
    if (asyncgenAthrowMethodIdx === null) {
      asyncgenAthrowMethodIdx = getOrAddTableFunc(asyncgenAthrowMethod, 2);
    }
    idx = asyncgenAthrowMethodIdx;
    arity = 2;
  } else if (name === 'aclose') {
    if (asyncgenAcloseMethodIdx === null) {
      asyncgenAcloseMethodIdx = getOrAddTableFunc(asyncgenAcloseMethod, 1);
    }
    idx = asyncgenAcloseMethodIdx;
    arity = 1;
  }
  if (idx === null) return boxNone();
  const bits = baseImports.func_new(BigInt(idx), 0n, BigInt(arity));
  asyncgenMethodBits.set(name, bits);
  return bits;
};
const builtinMethodBits = new Map();
const FUNC_DEFAULT_NONE = 1;
const FUNC_DEFAULT_DICT_POP = 2;
const FUNC_DEFAULT_DICT_UPDATE = 3;
const builtinMethodNamesForClass = (className) => {
  if (className === 'object') return ['__init__'];
  if (className === 'list') {
    return [
      'append',
      'extend',
      'insert',
      'remove',
      'pop',
      'clear',
      'copy',
      'reverse',
      'count',
      'index',
      'sort',
      '__iter__',
      '__len__',
      '__contains__',
      '__reversed__',
    ];
  }
  if (className === 'dict') {
    return [
      'keys',
      'values',
      'items',
      'get',
      'setdefault',
      'pop',
      'update',
      'clear',
      'copy',
      'popitem',
      '__iter__',
      '__len__',
      '__contains__',
      '__reversed__',
    ];
  }
  if (className === 'tuple') return ['count', 'index'];
  if (className === 'str') {
    return [
      'upper',
      'lower',
      'strip',
      'lstrip',
      'rstrip',
      'startswith',
      'endswith',
      '__iter__',
      '__len__',
      '__contains__',
    ];
  }
  if (className === 'bytes' || className === 'bytearray') {
    return [
      '__iter__',
      '__len__',
      '__contains__',
      '__reversed__',
      'find',
      'split',
      'replace',
      'startswith',
      'endswith',
      'count',
    ];
  }
  if (className === 'set') {
    return [
      'add',
      'discard',
      'remove',
      'pop',
      'clear',
      'update',
      'union',
      'intersection',
      'difference',
      'symmetric_difference',
      'intersection_update',
      'difference_update',
      'symmetric_difference_update',
      'isdisjoint',
      'issubset',
      'issuperset',
      'copy',
      '__iter__',
      '__len__',
      '__contains__',
    ];
  }
  if (className === 'frozenset') {
    return [
      'union',
      'intersection',
      'difference',
      'symmetric_difference',
      'isdisjoint',
      'issubset',
      'issuperset',
      'copy',
      '__iter__',
      '__len__',
      '__contains__',
    ];
  }
  if (className === 'memoryview') return ['tobytes', 'cast'];
  return [];
};
const collectDirNamesFromDict = (names, dictBits) => {
  const dict = getDict(dictBits);
  if (!dict) return;
  for (const [keyBits] of dict.entries) {
    const key = getStrObj(keyBits);
    if (key !== null) names.add(key);
  }
};
const collectDirNamesFromInstanceAttrs = (names, objBits) => {
  if (!isPtr(objBits)) return;
  if (heap.has(objBits & POINTER_MASK)) return;
  const attrs = instanceAttrs.get(ptrAddr(objBits));
  if (!attrs) return;
  for (const name of attrs.keys()) {
    names.add(name);
  }
};
const collectDirNamesFromClass = (names, classBits) => {
  const mro = classMroList(classBits);
  for (const currentBits of mro) {
    const cls = getClass(currentBits);
    if (!cls) continue;
    for (const name of cls.attrs.keys()) {
      names.add(name);
    }
    for (const name of builtinMethodNamesForClass(cls.name)) {
      names.add(name);
    }
  }
};
const PY_HASH_BITS = 61n;
const PY_HASH_MODULUS = (1n << PY_HASH_BITS) - 1n;
const PY_HASH_INF = 314159n;
const PY_HASH_NONE = 0xfca86420n;
const PY_HASHSEED_MAX = 4294967295n;
const HASH_MASK_64 = (1n << 64n) - 1n;
const fixHash = (hash) => (hash === -1n ? -2n : hash);
const toSigned64 = (value) => {
  const masked = value & HASH_MASK_64;
  return masked >= (1n << 63n) ? masked - (1n << 64n) : masked;
};
const rotl64 = (value, shift) =>
  ((value << BigInt(shift)) | (value >> (64n - BigInt(shift)))) & HASH_MASK_64;
const add64 = (lhs, rhs) => (lhs + rhs) & HASH_MASK_64;
const mul64 = (lhs, rhs) => (lhs * rhs) & HASH_MASK_64;
const expMod = (exp) => {
  const bits = Number(PY_HASH_BITS);
  if (exp >= 0) return exp % bits;
  return bits - 1 - ((-1 - exp) % bits);
};
const pow2Mod = (exp) => {
  let value = 1n;
  for (let i = 0; i < exp; i += 1) {
    value <<= 1n;
    if (value >= PY_HASH_MODULUS) value -= PY_HASH_MODULUS;
  }
  return value;
};
const reduceMersenne = (value) => {
  let out = value;
  out = (out & PY_HASH_MODULUS) + (out >> PY_HASH_BITS);
  out = (out & PY_HASH_MODULUS) + (out >> PY_HASH_BITS);
  if (out >= PY_HASH_MODULUS) out -= PY_HASH_MODULUS;
  return out;
};
const mulModMersenne = (lhs, rhs) => reduceMersenne(lhs * rhs);
const frexp = (value) => {
  if (value === 0) return [0, 0];
  const bits = floatToBits(value);
  let exp = Number((bits >> 52n) & 0x7ffn);
  let mant = bits & ((1n << 52n) - 1n);
  if (exp === 0) {
    let e = -1022;
    while ((mant & (1n << 52n)) === 0n) {
      mant <<= 1n;
      e -= 1;
    }
    exp = e;
    mant &= (1n << 52n) - 1n;
  } else {
    exp -= 1022;
  }
  const fracBits = (1022n << 52n) | mant;
  FLOAT_VIEW.setBigUint64(0, fracBits, true);
  return [FLOAT_VIEW.getFloat64(0, true), exp];
};
const bytesToU64LE = (bytes, offset) => {
  let value = 0n;
  for (let i = 0; i < 8; i += 1) {
    value |= BigInt(bytes[offset + i]) << (8n * BigInt(i));
  }
  return value;
};
const lcgHashSeed = (seed) => {
  let x = seed >>> 0;
  const out = new Uint8Array(16);
  for (let i = 0; i < out.length; i += 1) {
    x = (x * 214013 + 2531011) >>> 0;
    out[i] = (x >> 16) & 0xff;
  }
  return out;
};
const randomHashSecret = () => {
  if (typeof crypto !== 'undefined' && crypto.randomBytes) {
    const bytes = crypto.randomBytes(16);
    return {
      k0: bytesToU64LE(bytes, 0),
      k1: bytesToU64LE(bytes, 8),
    };
  }
  return { k0: 0n, k1: 0n };
};
const initHashSecret = () => {
  const seedText =
    typeof process !== 'undefined' && process.env ? process.env.PYTHONHASHSEED : undefined;
  if (seedText === undefined || seedText === 'random') {
    return randomHashSecret();
  }
  const seed = Number(seedText);
  if (!Number.isFinite(seed) || seed < 0 || seed > Number(PY_HASHSEED_MAX)) {
    throw new Error(
      `Fatal Python error: PYTHONHASHSEED must be \"random\" or an integer in range [0; ${PY_HASHSEED_MAX}]`,
    );
  }
  if (seed === 0) return { k0: 0n, k1: 0n };
  const bytes = lcgHashSeed(seed);
  return {
    k0: bytesToU64LE(bytes, 0),
    k1: bytesToU64LE(bytes, 8),
  };
};
const HASH_SECRET = initHashSecret();
const sipHash13 = (bytes, k0, k1) => {
  let v0 = 0x736f6d6570736575n ^ k0;
  let v1 = 0x646f72616e646f6dn ^ k1;
  let v2 = 0x6c7967656e657261n ^ k0;
  let v3 = 0x7465646279746573n ^ k1;
  let tail = 0n;
  let ntail = 0;
  let totalLen = 0n;
  const sipRound = () => {
    v0 = add64(v0, v1);
    v1 = rotl64(v1, 13);
    v1 ^= v0;
    v0 = rotl64(v0, 32);
    v2 = add64(v2, v3);
    v3 = rotl64(v3, 16);
    v3 ^= v2;
    v0 = add64(v0, v3);
    v3 = rotl64(v3, 21);
    v3 ^= v0;
    v2 = add64(v2, v1);
    v1 = rotl64(v1, 17);
    v1 ^= v2;
    v2 = rotl64(v2, 32);
  };
  const processBlock = (block) => {
    v3 ^= block;
    sipRound();
    v0 ^= block;
  };
  for (const byte of bytes) {
    totalLen += 1n;
    tail |= BigInt(byte) << (8n * BigInt(ntail));
    ntail += 1;
    if (ntail === 8) {
      processBlock(tail);
      tail = 0n;
      ntail = 0;
    }
  }
  const b = tail | ((totalLen & 0xffn) << 56n);
  processBlock(b);
  v2 ^= 0xffn;
  sipRound();
  sipRound();
  sipRound();
  return (v0 ^ v1 ^ v2 ^ v3) & HASH_MASK_64;
};
const hashBytesWithSecret = (bytes, secret) => {
  if (bytes.length === 0) return 0n;
  const hash = sipHash13(bytes, secret.k0, secret.k1);
  return fixHash(toSigned64(hash));
};
const hashBytes = (bytes) => hashBytesWithSecret(bytes, HASH_SECRET);
const hashString = (text) => {
  if (text.length === 0) return 0n;
  let maxCodepoint = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0);
    if (code > maxCodepoint) maxCodepoint = code;
  }
  const bytes = [];
  if (maxCodepoint <= 0xff) {
    for (const ch of text) {
      bytes.push(ch.codePointAt(0) & 0xff);
    }
  } else if (maxCodepoint <= 0xffff) {
    for (const ch of text) {
      const code = ch.codePointAt(0);
      bytes.push(code & 0xff, (code >> 8) & 0xff);
    }
  } else {
    for (const ch of text) {
      const code = ch.codePointAt(0);
      bytes.push(code & 0xff, (code >> 8) & 0xff, (code >> 16) & 0xff, (code >> 24) & 0xff);
    }
  }
  return hashBytesWithSecret(bytes, HASH_SECRET);
};
const hashInt = (val) => {
  let mag = val;
  let sign = 1n;
  if (mag < 0n) {
    sign = -1n;
    mag = -mag;
  }
  let hash = mag % PY_HASH_MODULUS;
  if (sign < 0n) hash = -hash;
  return fixHash(hash);
};
const hashBigInt = (val) => {
  let mag = val;
  let sign = 1n;
  if (mag < 0n) {
    sign = -1n;
    mag = -mag;
  }
  let hash = mag % PY_HASH_MODULUS;
  if (sign < 0n) hash = -hash;
  return fixHash(hash);
};
const hashFloat = (val) => {
  if (Number.isNaN(val)) return 0n;
  if (!Number.isFinite(val)) {
    return val > 0 ? PY_HASH_INF : -PY_HASH_INF;
  }
  if (val === 0) return 0n;
  const sign = val < 0 ? -1n : 1n;
  const value = Math.abs(val);
  let [frac, exp] = frexp(value);
  let hash = 0n;
  while (frac !== 0) {
    frac *= 1 << 28;
    const intpart = Math.floor(frac);
    frac -= intpart;
    hash = ((hash << 28n) & PY_HASH_MODULUS) | BigInt(intpart);
    exp -= 28;
  }
  const expValue = expMod(exp);
  hash = mulModMersenne(hash, pow2Mod(expValue));
  const signed = hash * sign;
  return fixHash(signed);
};
const hashComplex = (re, im) => {
  const reHash = hashFloat(re);
  const imHash = hashFloat(im);
  let hash = reHash + imHash * 1000003n;
  if (hash === -1n) hash = -2n;
  return hash;
};
const hashPointer = (ptrBits) => fixHash(toSigned64((ptrBits >> 4n) & HASH_MASK_64));
const hashTupleBits = (tupleBits) => {
  const tuple = getTuple(tupleBits);
  if (!tuple) return 0n;
  const XXPRIME_1 = 11400714785074694791n;
  const XXPRIME_2 = 14029467366897019727n;
  const XXPRIME_5 = 2870177450012600261n;
  let acc = XXPRIME_5;
  for (const elem of tuple.items) {
    const lane = hashBitsSigned(elem);
    if (exceptionPending() !== 0n) return 0n;
    const laneU64 = lane & HASH_MASK_64;
    acc = add64(acc, mul64(laneU64, XXPRIME_2));
    acc = rotl64(acc, 31);
    acc = mul64(acc, XXPRIME_1);
  }
  acc = add64(acc, BigInt(tuple.items.length) ^ (XXPRIME_5 ^ 3527539n));
  if (acc === HASH_MASK_64) return 1546275796n;
  return toSigned64(acc);
};
const hashSliceBits = (sliceBits) => {
  const slice = getSlice(sliceBits);
  if (!slice) return 0n;
  const lanes = [
    hashBitsSigned(slice.start),
    hashBitsSigned(slice.stop),
    hashBitsSigned(slice.step),
  ];
  if (exceptionPending() !== 0n) return 0n;
  const XXPRIME_1 = 11400714785074694791n;
  const XXPRIME_2 = 14029467366897019727n;
  const XXPRIME_5 = 2870177450012600261n;
  let acc = XXPRIME_5;
  for (const lane of lanes) {
    const laneU64 = lane & HASH_MASK_64;
    acc = add64(acc, mul64(laneU64, XXPRIME_2));
    acc = rotl64(acc, 31);
    acc = mul64(acc, XXPRIME_1);
  }
  if (acc === HASH_MASK_64) return 1546275796n;
  return toSigned64(acc);
};
const shuffleFrozensetHash = (hash) => {
  const mixed = (hash ^ 89869747n) ^ (hash << 16n);
  return mul64(mixed, 3644798167n);
};
const hashFrozensetBits = (setBits) => {
  const setObj = getFrozenSet(setBits);
  if (!setObj) return 0n;
  let hash = 0n;
  for (const elem of setObj.items) {
    hash ^= shuffleFrozensetHash(hashBits(elem));
  }
  if (setObj.items.size & 1) {
    hash ^= shuffleFrozensetHash(0n);
  }
  hash ^= (BigInt(setObj.items.size) + 1n) * 1927868237n;
  hash ^= (hash >> 11n) ^ (hash >> 25n);
  hash = add64(mul64(hash, 69069n), 907133923n);
  if (hash === HASH_MASK_64) hash = 590923713n;
  return toSigned64(hash);
};
const hashUnhashable = (bits) => {
  const exc = exceptionNew(
    boxPtr({ type: 'str', value: 'TypeError' }),
    exceptionArgs(
      boxPtr({
        type: 'str',
        value: `unhashable type: '${typeName(bits)}'`,
      }),
    ),
  );
  raiseException(exc);
  return 0n;
};
const hashBits = (bits) => hashBitsSigned(bits) & HASH_MASK_64;
const hashBitsSigned = (bits) => {
  if (isTag(bits, TAG_INT)) return hashInt(unboxInt(bits));
  if (isTag(bits, TAG_BOOL)) return hashInt((bits & 1n) === 1n ? 1n : 0n);
  if (isTag(bits, TAG_NONE)) return PY_HASH_NONE;
  if (isFloat(bits)) return hashFloat(bitsToFloat(bits));
  const obj = getObj(bits);
  if (obj) {
    if (
      obj.type === 'list' ||
      obj.type === 'dict' ||
      obj.type === 'set' ||
      obj.type === 'bytearray' ||
      obj.type === 'memoryview' ||
      obj.type === 'list_builder' ||
      obj.type === 'dict_builder' ||
      obj.type === 'set_builder' ||
      obj.type === 'dict_keys' ||
      obj.type === 'dict_values' ||
      obj.type === 'dict_items' ||
      obj.type === 'callargs'
    ) {
      return hashUnhashable(bits);
    }
    if (obj.type === 'str') return hashString(obj.value);
    if (obj.type === 'bytes') return hashBytes(obj.data);
    if (obj.type === 'bigint') return hashBigInt(obj.value);
    if (obj.type === 'complex') return hashComplex(obj.re, obj.im);
    if (obj.type === 'tuple') return hashTupleBits(bits);
    if (obj.type === 'slice') return hashSliceBits(bits);
    if (obj.type === 'frozenset') return hashFrozensetBits(bits);
  }
  if (isPtr(bits) && !heap.has(bits & POINTER_MASK)) {
    return hashPointer(bits);
  }
  return hashPointer(bits);
};
const getBuiltinMethodBits = (classBits, name) => {
  const cls = getClass(classBits);
  if (!cls) return boxNone();
  const className = cls.name;
  const key = `${className}.${name}`;
  const cached = builtinMethodBits.get(key);
  if (cached !== undefined) return cached;
  let fn = null;
  let arity = 0;
  let defaultKind = 0;
  if (className === 'object') {
    if (name === '__init__') {
      fn = (_selfBits) => boxNone();
      arity = 1;
    } else if (name === '__init_subclass__') {
      fn = (_selfBits) => boxNone();
      arity = 1;
    }
  } else if (className === 'list') {
    if (name === 'append') {
      fn = (selfBits, valBits) => baseImports.list_append(selfBits, valBits);
      arity = 2;
    } else if (name === 'extend') {
      fn = (selfBits, otherBits) => baseImports.list_extend(selfBits, otherBits);
      arity = 2;
    } else if (name === 'insert') {
      fn = (selfBits, idxBits, valBits) =>
        baseImports.list_insert(selfBits, idxBits, valBits);
      arity = 3;
    } else if (name === 'remove') {
      fn = (selfBits, valBits) => baseImports.list_remove(selfBits, valBits);
      arity = 2;
    } else if (name === 'pop') {
      fn = (selfBits, idxBits) => baseImports.list_pop(selfBits, idxBits);
      arity = 2;
    } else if (name === 'clear') {
      fn = (selfBits) => baseImports.list_clear(selfBits);
      arity = 1;
    } else if (name === 'copy') {
      fn = (selfBits) => baseImports.list_copy(selfBits);
      arity = 1;
    } else if (name === 'reverse') {
      fn = (selfBits) => baseImports.list_reverse(selfBits);
      arity = 1;
    } else if (name === 'count') {
      fn = (selfBits, valBits) => baseImports.list_count(selfBits, valBits);
      arity = 2;
    } else if (name === 'index') {
      fn = (selfBits, valBits, startBits, stopBits) =>
        baseImports.list_index_range(selfBits, valBits, startBits, stopBits);
      arity = 4;
    } else if (name === 'sort') {
      fn = (selfBits, keyBits, reverseBits) =>
        baseImports.list_sort(selfBits, keyBits, reverseBits);
      arity = 3;
    } else if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    } else if (name === '__reversed__') {
      fn = (selfBits) => baseImports.reversed_builtin(selfBits);
      arity = 1;
    }
  } else if (className === 'dict') {
    if (name === 'keys') {
      fn = (selfBits) => baseImports.dict_keys(selfBits);
      arity = 1;
    } else if (name === 'values') {
      fn = (selfBits) => baseImports.dict_values(selfBits);
      arity = 1;
    } else if (name === 'items') {
      fn = (selfBits) => baseImports.dict_items(selfBits);
      arity = 1;
    } else if (name === 'get') {
      fn = (selfBits, keyBits, defaultBits) =>
        baseImports.dict_get(selfBits, keyBits, defaultBits);
      arity = 3;
      defaultKind = FUNC_DEFAULT_NONE;
    } else if (name === 'setdefault') {
      fn = (selfBits, keyBits, defaultBits) =>
        baseImports.dict_setdefault(selfBits, keyBits, defaultBits);
      arity = 3;
      defaultKind = FUNC_DEFAULT_NONE;
    } else if (name === 'pop') {
      fn = (selfBits, keyBits, defaultBits, hasDefaultBits) =>
        baseImports.dict_pop(selfBits, keyBits, defaultBits, hasDefaultBits);
      arity = 4;
      defaultKind = FUNC_DEFAULT_DICT_POP;
    } else if (name === 'update') {
      fn = (selfBits, otherBits) => baseImports.dict_update(selfBits, otherBits);
      arity = 2;
      defaultKind = FUNC_DEFAULT_DICT_UPDATE;
    } else if (name === 'clear') {
      fn = (selfBits) => baseImports.dict_clear(selfBits);
      arity = 1;
    } else if (name === 'copy') {
      fn = (selfBits) => baseImports.dict_copy(selfBits);
      arity = 1;
    } else if (name === 'popitem') {
      fn = (selfBits) => baseImports.dict_popitem(selfBits);
      arity = 1;
    } else if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    } else if (name === '__reversed__') {
      fn = (selfBits) => baseImports.reversed_builtin(selfBits);
      arity = 1;
    }
  } else if (className === 'tuple') {
    if (name === 'count') {
      fn = (selfBits, valBits) => baseImports.tuple_count(selfBits, valBits);
      arity = 2;
    } else if (name === 'index') {
      fn = (selfBits, valBits) => baseImports.tuple_index(selfBits, valBits);
      arity = 2;
    }
  } else if (className === 'str') {
    if (name === 'upper') {
      fn = (selfBits) => baseImports.string_upper(selfBits);
      arity = 1;
    } else if (name === 'lower') {
      fn = (selfBits) => baseImports.string_lower(selfBits);
      arity = 1;
    } else if (name === 'strip') {
      fn = (selfBits, charsBits) => baseImports.string_strip(selfBits, charsBits);
      arity = 2;
      defaultKind = FUNC_DEFAULT_NONE;
    } else if (name === 'lstrip') {
      fn = (selfBits, charsBits) => baseImports.string_lstrip(selfBits, charsBits);
      arity = 2;
      defaultKind = FUNC_DEFAULT_NONE;
    } else if (name === 'rstrip') {
      fn = (selfBits, charsBits) => baseImports.string_rstrip(selfBits, charsBits);
      arity = 2;
      defaultKind = FUNC_DEFAULT_NONE;
    } else if (name === 'startswith') {
      if (process.env.MOLT_DEBUG_STR_STARTSWITH === '1') {
        console.error('getBuiltinMethodBits(str.startswith)');
      }
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getStrObj(selfBits);
        if (hay === null) return boxNone();
        const range = normalizeBytesRange(
          hay.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'startswith() start must be int',
          'startswith() end must be int',
        );
        if (!range) return boxNone();
        const slice = hay.slice(range.start, range.end);
        const tuple = getTuple(needleBits);
        if (tuple) {
          for (const item of tuple.items) {
            const needle = getStrObj(item);
            if (needle === null) return boxNone();
            if (slice.startsWith(needle)) return boxBool(true);
          }
          return boxBool(false);
        }
        const needle = getStrObj(needleBits);
        if (needle === null) return boxNone();
        return boxBool(slice.startsWith(needle));
      };
      arity = 6;
    } else if (name === 'endswith') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getStrObj(selfBits);
        if (hay === null) return boxNone();
        const range = normalizeBytesRange(
          hay.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'endswith() start must be int',
          'endswith() end must be int',
        );
        if (!range) return boxNone();
        const slice = hay.slice(range.start, range.end);
        const tuple = getTuple(needleBits);
        if (tuple) {
          for (const item of tuple.items) {
            const needle = getStrObj(item);
            if (needle === null) return boxNone();
            if (slice.endsWith(needle)) return boxBool(true);
          }
          return boxBool(false);
        }
        const needle = getStrObj(needleBits);
        if (needle === null) return boxNone();
        return boxBool(slice.endsWith(needle));
      };
      arity = 6;
    } else if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    }
  } else if (className === 'bytes') {
    if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    } else if (name === '__reversed__') {
      fn = (selfBits) => baseImports.reversed_builtin(selfBits);
      arity = 1;
    } else if (name === 'find') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytes(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'find() start must be int',
          'find() end must be int',
        );
        if (!range) return boxNone();
        return boxInt(bytesFindInRange(hay.data, needle.data, range.start, range.end));
      };
      arity = 6;
    } else if (name === 'split') {
      fn = (selfBits, sepBits) => baseImports.bytes_split(selfBits, sepBits);
      arity = 2;
    } else if (name === 'replace') {
      fn = (selfBits, oldBits, newBits, countBits) => {
        const hay = getBytes(selfBits);
        if (!hay) return boxNone();
        const oldObj = getBytes(oldBits) || getBytearray(oldBits);
        const newObj = getBytes(newBits) || getBytearray(newBits);
        if (!oldObj || !newObj) return boxNone();
        let count = null;
        if (isIntLike(countBits)) {
          count = Number(unboxIntLike(countBits));
        } else {
          const idx = indexFromBitsWithOverflow(
            countBits,
            `'${typeName(countBits)}' object cannot be interpreted as an integer`,
            null,
          );
          if (idx === null) return boxNone();
          count = idx;
        }
        const replaced = bytesReplaceLimited(hay.data, oldObj.data, newObj.data, count);
        return boxPtr({ type: 'bytes', data: replaced });
      };
      arity = 4;
    } else if (name === 'startswith') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytes(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'startswith() start must be int',
          'startswith() end must be int',
        );
        if (!range) return boxNone();
        return boxBool(
          bytesStartsWithInRange(hay.data, needle.data, range.start, range.end),
        );
      };
      arity = 6;
    } else if (name === 'endswith') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytes(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'endswith() start must be int',
          'endswith() end must be int',
        );
        if (!range) return boxNone();
        return boxBool(
          bytesEndsWithInRange(hay.data, needle.data, range.start, range.end),
        );
      };
      arity = 6;
    } else if (name === 'count') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) =>
        baseImports.bytes_count_slice(
          selfBits,
          needleBits,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
        );
      arity = 6;
    }
  } else if (className === 'bytearray') {
    if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    } else if (name === '__reversed__') {
      fn = (selfBits) => baseImports.reversed_builtin(selfBits);
      arity = 1;
    } else if (name === 'find') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytearray(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'find() start must be int',
          'find() end must be int',
        );
        if (!range) return boxNone();
        return boxInt(bytesFindInRange(hay.data, needle.data, range.start, range.end));
      };
      arity = 6;
    } else if (name === 'split') {
      fn = (selfBits, sepBits) => baseImports.bytearray_split(selfBits, sepBits);
      arity = 2;
    } else if (name === 'replace') {
      fn = (selfBits, oldBits, newBits, countBits) => {
        const hay = getBytearray(selfBits);
        if (!hay) return boxNone();
        const oldObj = getBytes(oldBits) || getBytearray(oldBits);
        const newObj = getBytes(newBits) || getBytearray(newBits);
        if (!oldObj || !newObj) return boxNone();
        let count = null;
        if (isIntLike(countBits)) {
          count = Number(unboxIntLike(countBits));
        } else {
          const idx = indexFromBitsWithOverflow(
            countBits,
            `'${typeName(countBits)}' object cannot be interpreted as an integer`,
            null,
          );
          if (idx === null) return boxNone();
          count = idx;
        }
        const replaced = bytesReplaceLimited(hay.data, oldObj.data, newObj.data, count);
        return boxPtr({ type: 'bytearray', data: replaced });
      };
      arity = 4;
    } else if (name === 'startswith') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytearray(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'startswith() start must be int',
          'startswith() end must be int',
        );
        if (!range) return boxNone();
        return boxBool(
          bytesStartsWithInRange(hay.data, needle.data, range.start, range.end),
        );
      };
      arity = 6;
    } else if (name === 'endswith') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
        const hay = getBytearray(selfBits);
        if (!hay) return boxNone();
        const needle = getBytes(needleBits) || getBytearray(needleBits);
        if (!needle) return boxNone();
        const range = normalizeBytesRange(
          hay.data.length,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
          'endswith() start must be int',
          'endswith() end must be int',
        );
        if (!range) return boxNone();
        return boxBool(
          bytesEndsWithInRange(hay.data, needle.data, range.start, range.end),
        );
      };
      arity = 6;
    } else if (name === 'count') {
      fn = (selfBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) =>
        baseImports.bytearray_count_slice(
          selfBits,
          needleBits,
          startBits,
          endBits,
          hasStartBits,
          hasEndBits,
        );
      arity = 6;
    }
  } else if (className === 'set') {
    if (name === 'add') {
      fn = (selfBits, valBits) => baseImports.set_add(selfBits, valBits);
      arity = 2;
    } else if (name === 'discard') {
      fn = (selfBits, valBits) => baseImports.set_discard(selfBits, valBits);
      arity = 2;
    } else if (name === 'remove') {
      fn = (selfBits, valBits) => baseImports.set_remove(selfBits, valBits);
      arity = 2;
    } else if (name === 'pop') {
      fn = (selfBits) => baseImports.set_pop(selfBits);
      arity = 1;
    } else if (name === 'clear') {
      fn = (selfBits) => setClearBits(selfBits);
      arity = 1;
    } else if (name === 'update') {
      fn = (selfBits, othersBits) => setUpdateMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'union') {
      fn = (selfBits, othersBits) => setUnionMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'intersection') {
      fn = (selfBits, othersBits) => setIntersectionMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'difference') {
      fn = (selfBits, othersBits) => setDifferenceMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'symmetric_difference') {
      fn = (selfBits, otherBits) => setSymdiffBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'intersection_update') {
      fn = (selfBits, othersBits) => setIntersectionUpdateMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'difference_update') {
      fn = (selfBits, othersBits) => setDifferenceUpdateMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'symmetric_difference_update') {
      fn = (selfBits, otherBits) => setSymdiffUpdateBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'isdisjoint') {
      fn = (selfBits, otherBits) => setIsDisjointBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'issubset') {
      fn = (selfBits, otherBits) => setIsSubsetBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'issuperset') {
      fn = (selfBits, otherBits) => setIsSupersetBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'copy') {
      fn = (selfBits) => setCopyBits(selfBits);
      arity = 1;
    } else if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    }
  } else if (className === 'frozenset') {
    if (name === 'union') {
      fn = (selfBits, othersBits) => setUnionMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'intersection') {
      fn = (selfBits, othersBits) => setIntersectionMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'difference') {
      fn = (selfBits, othersBits) => setDifferenceMulti(selfBits, othersBits);
      arity = 2;
    } else if (name === 'symmetric_difference') {
      fn = (selfBits, otherBits) => setSymdiffBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'isdisjoint') {
      fn = (selfBits, otherBits) => setIsDisjointBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'issubset') {
      fn = (selfBits, otherBits) => setIsSubsetBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'issuperset') {
      fn = (selfBits, otherBits) => setIsSupersetBits(selfBits, otherBits);
      arity = 2;
    } else if (name === 'copy') {
      fn = (selfBits) => setCopyBits(selfBits);
      arity = 1;
    } else if (name === '__iter__') {
      fn = (selfBits) => baseImports.iter(selfBits);
      arity = 1;
    } else if (name === '__len__') {
      fn = (selfBits) => baseImports.len(selfBits);
      arity = 1;
    } else if (name === '__contains__') {
      fn = (selfBits, itemBits) => baseImports.contains(selfBits, itemBits);
      arity = 2;
    }
  } else if (className === 'memoryview') {
    if (name === 'tobytes') {
      fn = (selfBits) => baseImports.memoryview_tobytes(selfBits);
      arity = 1;
    } else if (name === 'cast') {
      fn = (selfBits, formatBits, shapeBits, hasShapeBits) =>
        baseImports.memoryview_cast(selfBits, formatBits, shapeBits, hasShapeBits);
      arity = 4;
    }
  } else if (className === 'complex') {
    if (name === 'conjugate') {
      fn = (selfBits) => {
        const obj = getComplex(selfBits);
        if (!obj) {
          throw new Error('TypeError: complex.conjugate expects complex');
        }
        return boxComplex(obj.re, -obj.im);
      };
      arity = 1;
    }
  }
  if (!fn) return boxNone();
  const idx = getOrAddTableFunc(fn, arity);
  if (idx === null) return boxNone();
  const bits = baseImports.func_new(BigInt(idx), 0n, BigInt(arity));
  const func = getFunction(bits);
  if (func) {
    func.builtinName = key;
    if (defaultKind) {
      func.defaultKind = defaultKind;
    }
  }
  builtinMethodBits.set(key, bits);
  return bits;
};
const bindBuiltinCall = (funcBits, func, args) => {
  const out = [...args.pos];
  const name = func && typeof func.builtinName === 'string' ? func.builtinName : null;
  lastBuiltinName = name;
  const isSetMethod = name && (name.startsWith('set.') || name.startsWith('frozenset.'));
  if (args.kwNames.length) {
    if (name === 'memoryview.cast') {
      if (args.kwNames.length !== 1) {
        throw new Error('TypeError: cast() takes at most 2 arguments');
      }
      if (out.length !== 2) {
        return baseImports.call_arity_error(BigInt(2), BigInt(out.length));
      }
      const kwName = getStrObj(args.kwNames[0]);
      if (kwName !== 'shape') {
        throw new Error(`TypeError: cast() got an unexpected keyword argument '${kwName}'`);
      }
      out.push(args.kwValues[0]);
    } else if (!isSetMethod) {
      throw new Error('TypeError: keywords are not supported for this builtin');
    }
  }
  const arity = func ? func.arity : out.length;
  if (name === 'object.__init__' && out.length > 1) {
    const selfBits = out[0];
    console.error(
      `object.__init__ called with ${out.length} args on ${typeName(selfBits)}`,
    );
  }
  if (name === 'list.pop') {
    if (out.length === 1) {
      out.push(missingSentinel());
    } else if (out.length !== 2) {
      return baseImports.call_arity_error(BigInt(2), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'list.index') {
    while (out.length < 4) {
      out.push(missingSentinel());
    }
    if (out.length !== 4) {
      return baseImports.call_arity_error(BigInt(4), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'list.sort') {
    if (out.length === 1) {
      out.push(boxNone());
      out.push(boxBool(false));
    } else if (out.length === 2) {
      out.push(boxBool(false));
    } else if (out.length !== 3) {
      return baseImports.call_arity_error(BigInt(3), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'dict.get' || name === 'dict.setdefault') {
    if (out.length === 2) {
      out.push(boxNone());
    } else if (out.length !== 3) {
      return baseImports.call_arity_error(BigInt(3), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'dict.pop') {
    if (out.length === 2) {
      out.push(boxNone());
      out.push(boxInt(0));
    } else if (out.length === 3) {
      out.push(boxInt(1));
    } else if (out.length !== 4) {
      return baseImports.call_arity_error(BigInt(4), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'dict.update') {
    if (out.length === 1) {
      out.push(missingSentinel());
    } else if (out.length !== 2) {
      return baseImports.call_arity_error(BigInt(2), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (
    name === 'bytes.find' ||
    name === 'bytearray.find' ||
    name === 'bytes.startswith' ||
    name === 'bytearray.startswith' ||
    name === 'bytes.endswith' ||
    name === 'bytearray.endswith' ||
    name === 'str.startswith' ||
    name === 'str.endswith' ||
    name === 'bytes.count' ||
    name === 'bytearray.count'
  ) {
    if (out.length === 2) {
      out.push(boxNone());
      out.push(boxNone());
      out.push(boxInt(0));
      out.push(boxInt(0));
    } else if (out.length === 3) {
      out.push(boxNone());
      out.push(boxInt(1));
      out.push(boxInt(0));
    } else if (out.length === 4) {
      out.push(boxInt(1));
      out.push(boxInt(1));
    } else if (out.length !== 6) {
      return baseImports.call_arity_error(BigInt(4), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'bytes.replace' || name === 'bytearray.replace') {
    if (out.length === 3) {
      out.push(boxInt(-1));
    } else if (out.length !== 4) {
      return baseImports.call_arity_error(BigInt(4), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (name === 'memoryview.cast') {
    if (out.length === 2) {
      out.push(boxNone());
      out.push(boxInt(0));
    } else if (out.length === 3) {
      out.push(boxInt(1));
    } else if (out.length !== 4) {
      return baseImports.call_arity_error(BigInt(3), BigInt(out.length));
    }
    return callFunctionBits(funcBits, out);
  }
  if (isSetMethod) {
    const [ownerName, methodName] = name.split('.');
    if (out.length < 1) {
      return baseImports.call_arity_error(BigInt(1), BigInt(out.length));
    }
    const selfBits = out[0];
    const isOwner =
      ownerName === 'set' ? Boolean(getSet(selfBits)) : Boolean(getFrozenSet(selfBits));
    if (!isOwner) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `descriptor '${methodName}' for '${ownerName}' objects doesn't apply to a '${typeName(selfBits)}' object`,
          }),
        ),
      );
      return raiseException(exc);
    }
    if (args.kwNames.length) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `${name}() takes no keyword arguments`,
          }),
        ),
      );
      return raiseException(exc);
    }
    const method = methodName;
    const given = out.length - 1;
    if (
      method === 'union' ||
      method === 'intersection' ||
      method === 'difference' ||
      method === 'update' ||
      method === 'intersection_update' ||
      method === 'difference_update'
    ) {
      if (out.length < 1) {
        return baseImports.call_arity_error(BigInt(1), BigInt(out.length));
      }
      const othersBits = tupleFromArray(out.slice(1));
      return callFunctionBits(funcBits, [out[0], othersBits]);
    }
    if (
      method === 'symmetric_difference' ||
      method === 'symmetric_difference_update' ||
      method === 'isdisjoint' ||
      method === 'issubset' ||
      method === 'issuperset'
    ) {
      if (given !== 1) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `${name}() takes exactly one argument (${given} given)`,
            }),
          ),
        );
        return raiseException(exc);
      }
      return callFunctionBits(funcBits, out);
    }
    if (method === 'copy' || method === 'clear') {
      if (given !== 0) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `${name}() takes no arguments (${given} given)`,
            }),
          ),
        );
        return raiseException(exc);
      }
      return callFunctionBits(funcBits, out);
    }
  }
  const missing = arity - out.length;
  if (missing < 0) {
    return baseImports.call_arity_error(BigInt(arity), BigInt(out.length));
  }
  if (missing === 0) {
    return callFunctionBits(funcBits, out);
  }
  if (func && func.defaultKind === FUNC_DEFAULT_NONE && missing === 1) {
    out.push(boxNone());
    return callFunctionBits(funcBits, out);
  }
  if (func && func.defaultKind === FUNC_DEFAULT_DICT_POP) {
    if (missing === 1) {
      out.push(boxInt(1));
      return callFunctionBits(funcBits, out);
    }
    if (missing === 2) {
      out.push(boxNone());
      out.push(boxInt(0));
      return callFunctionBits(funcBits, out);
    }
  }
  if (func && func.defaultKind === FUNC_DEFAULT_DICT_UPDATE && missing === 1) {
    out.push(missingSentinel());
    return callFunctionBits(funcBits, out);
  }
  return baseImports.call_arity_error(BigInt(arity), BigInt(out.length));
};
"""

BASE_IMPORTS = """\
  runtime_init: () => {
    if (!moduleCache.get('builtins')) {
      const nameBits = boxPtr({ type: 'str', value: 'builtins' });
      const moduleBits = baseImports.module_new(nameBits);
      baseImports.module_cache_set(nameBits, moduleBits);
    }
    return 1n;
  },
  runtime_shutdown: () => 1n,
  unsupported_import: (name) => {
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'RuntimeError' }),
      exceptionArgs(boxPtr({ type: 'str', value: `${name} unsupported` })),
    );
    return raiseException(exc);
  },
  print_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      console.log(unboxInt(val).toString());
      return;
    }
    if (isFloat(val)) {
      console.log(formatFloat(bitsToFloat(val)));
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
    if (isPtr(val)) {
      const str = getStrObj(val);
      if (str !== null) {
        console.log(str);
        return;
      }
    }
    console.log(val.toString());
  },
  print_newline: () => console.log(''),
  alloc: (size) => {
    const addr = allocRaw(size);
    if (!addr) return boxNone();
    return boxPtrAddr(addr);
  },
  alloc_class: (size, classBits) => {
    const addr = allocRaw(size);
    if (!addr) return boxNone();
    if (classBits !== 0n && !getClass(classBits)) {
      throw new Error('TypeError: class must be a type object');
    }
    const objBits = boxPtrAddr(addr);
    if (classBits !== 0n) {
      instanceClasses.set(ptrAddr(objBits), classBits);
    }
    return objBits;
  },
  alloc_class_trusted: (size, classBits) => {
    const addr = allocRaw(size);
    if (!addr) return boxNone();
    const objBits = boxPtrAddr(addr);
    if (classBits !== 0n) {
      instanceClasses.set(ptrAddr(objBits), classBits);
    }
    return objBits;
  },
  alloc_class_static: (size, classBits) => {
    const addr = allocRaw(size);
    if (!addr) return boxNone();
    const objBits = boxPtrAddr(addr);
    if (classBits !== 0n) {
      instanceClasses.set(ptrAddr(objBits), classBits);
    }
    return objBits;
  },
  async_sleep: (taskPtr) => {
    const addr = expectPtrAddr(taskPtr, 'async_sleep');
    if (addr === 0 || !memory) return boxNone();
    const view = memView();
    const stateBits = view.getBigInt64(addr - HEADER_STATE_OFFSET, true);
    const state = Number(stateBits);
    if (state === 0) {
      const delayBits = view.getBigInt64(addr + 0, true);
      let delay = numberFromVal(delayBits);
      if (delay === null || !Number.isFinite(delay) || delay <= 0) delay = 0;
      if (delay > 0) {
        const now =
          typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
            ? process.hrtime.bigint()
            : BigInt(Date.now()) * 1000000n;
        const deadline = Number(now - MONO_START) / 1e9 + delay;
        view.setBigInt64(addr + 0, boxFloat(deadline), true);
      }
      view.setBigInt64(addr - HEADER_STATE_OFFSET, 1n, true);
      return boxPending();
    }
    const deadlineBits = view.getBigInt64(addr + 0, true);
    const deadline = numberFromVal(deadlineBits);
    if (deadline !== null && Number.isFinite(deadline) && deadline > 0) {
      const now =
        typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
          ? process.hrtime.bigint()
          : BigInt(Date.now()) * 1000000n;
      const nowSecs = Number(now - MONO_START) / 1e9;
      if (nowSecs < deadline) return boxPending();
    }
    return view.getBigInt64(addr + 8, true);
  },
  anext_default_poll: (taskPtr) => {
    const addr = expectPtrAddr(taskPtr, 'anext_default_poll');
    if (addr === 0 || !memory || !table) return boxNone();
    const view = new DataView(memory.buffer);
    const state = Number(view.getBigInt64(addr - HEADER_STATE_OFFSET, true));
    const iterBits = view.getBigInt64(addr + 0, true);
    const defaultBits = view.getBigInt64(addr + 8, true);
    if (state === 0) {
      const iterObj = iterBits;
      let attr = lookupAttr(iterObj, '__anext__');
      if (attr === undefined) {
        throw new Error('TypeError: object is not an async iterator');
      }
      if (getFunction(attr)) {
        attr = makeBoundMethod(attr, iterObj);
      }
      const awaitBits = callCallable0(attr);
      view.setBigInt64(addr + 16, awaitBits, true);
      view.setBigInt64(addr - HEADER_STATE_OFFSET, 1n, true);
    }
    const awaitBits = view.getBigInt64(addr + 16, true);
    const awaitPtrBits = awaitBits;
    if (!isPtr(awaitPtrBits) || heap.has(awaitPtrBits & POINTER_MASK)) return boxNone();
    const awaitAddr = ptrAddr(awaitPtrBits);
    const pollIdx = view.getUint32(awaitAddr - HEADER_POLL_FN_OFFSET, true);
    const poll = getTableFunc(pollIdx);
    if (!poll) return boxNone();
    const prevAsync = asyncRaise;
    asyncRaise = true;
    let res;
    try {
      res = poll(BigInt(awaitAddr));
    } finally {
      asyncRaise = prevAsync;
    }
    if (isPending(res)) return res;
    if (exceptionPending() !== 0n) {
      const excBits = exceptionLast();
      const kindBits = exceptionKind(excBits);
      if (getStr(kindBits) === 'StopAsyncIteration') {
        exceptionClear();
        return defaultBits;
      }
    }
    return res;
  },
  future_poll: (futureBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory || !table) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    if (tokenIsCancelled(currentTokenId)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'CancelledError' }),
        boxNone(),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    const view = new DataView(memory.buffer);
    const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    const poll = getTableFunc(pollIdx);
    if (!poll) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const prevAsync = asyncRaise;
    asyncRaise = true;
    let res;
    try {
      res = poll(BigInt(addr));
    } finally {
      asyncRaise = prevAsync;
    }
    if (currentTaskPtr !== 0 && addr !== currentTaskPtr) {
      if (isPending(res)) {
        taskWaitingOn.set(currentTaskPtr, ptrBits);
      } else {
        taskWaitingOn.delete(currentTaskPtr);
      }
    }
    const cancelKey = addr.toString();
    if (cancelPending.has(cancelKey)) {
      cancelPending.delete(cancelKey);
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'CancelledError' }),
        boxNone(),
      );
      raiseException(exc);
      return boxNone();
    }
    return res;
  },
  future_poll_fn: (futureBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory || !table) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return -1n;
    }
    const addr = ptrAddr(ptrBits);
    const view = new DataView(memory.buffer);
    const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    const poll = getTableFunc(pollIdx);
    if (!poll) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return -1n;
    }
    return BigInt(pollIdx);
  },
  future_cancel: (futureBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    cancelPending.add(addr.toString());
    return boxNone();
  },
  future_cancel_msg: (futureBits, msgBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    cancelPending.add(addr.toString());
    return boxNone();
  },
  future_cancel_clear: (futureBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    cancelPending.delete(addr.toString());
    return boxNone();
  },
  promise_new: () => {
    if (!memory || !table) return boxNone();
    let pollIdx = promisePollIdx;
    if (pollIdx === null) {
      pollIdx = getOrAddTableFunc(baseImports.promise_poll, 1);
      if (pollIdx === null) return boxNone();
      promisePollIdx = pollIdx;
    }
    return baseImports.task_new(BigInt(pollIdx), 8n, TASK_KIND_FUTURE);
  },
  promise_poll: (objBits) => {
    const ptrBits = objBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory) return boxNone();
    const addr = ptrAddr(ptrBits);
    const view = memView();
    const state = Number(view.getBigInt64(addr - HEADER_STATE_OFFSET, true));
    if (state === 0) return boxPending();
    const payload = view.getBigInt64(addr + 0, true);
    if (state === 1) return payload;
    if (state === 2) {
      raiseException(payload);
      return boxNone();
    }
    return boxNone();
  },
  promise_set_result: (futureBits, resultBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    const view = memView();
    let pollIdx = promisePollIdx;
    if (pollIdx === null) {
      pollIdx = getOrAddTableFunc(baseImports.promise_poll, 1);
      if (pollIdx === null) return boxNone();
      promisePollIdx = pollIdx;
    }
    const headerPoll = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    if (headerPoll !== pollIdx) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not a promise' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const state = Number(view.getBigInt64(addr - HEADER_STATE_OFFSET, true));
    if (state !== 0) return boxNone();
    view.setBigInt64(addr + 0, resultBits, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 1n, true);
    const toWake = [];
    for (const [taskAddr, awaitedBits] of taskWaitingOn.entries()) {
      if (awaitedBits === ptrBits) {
        toWake.push(taskAddr);
      }
    }
    for (const taskAddr of toWake) {
      taskWaitingOn.delete(taskAddr);
      const key = taskAddr.toString();
      if (!runnableTasks.has(key)) {
        runnableTasks.add(key);
        runnableQueue.push(taskAddr);
      }
    }
    return boxNone();
  },
  promise_set_exception: (futureBits, excBits) => {
    const ptrBits = futureBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(ptrBits);
    const view = memView();
    let pollIdx = promisePollIdx;
    if (pollIdx === null) {
      pollIdx = getOrAddTableFunc(baseImports.promise_poll, 1);
      if (pollIdx === null) return boxNone();
      promisePollIdx = pollIdx;
    }
    const headerPoll = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    if (headerPoll !== pollIdx) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not a promise' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const state = Number(view.getBigInt64(addr - HEADER_STATE_OFFSET, true));
    if (state !== 0) return boxNone();
    view.setBigInt64(addr + 0, excBits, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 2n, true);
    const toWake = [];
    for (const [taskAddr, awaitedBits] of taskWaitingOn.entries()) {
      if (awaitedBits === ptrBits) {
        toWake.push(taskAddr);
      }
    }
    for (const taskAddr of toWake) {
      taskWaitingOn.delete(taskAddr);
      const key = taskAddr.toString();
      if (!runnableTasks.has(key)) {
        runnableTasks.add(key);
        runnableQueue.push(taskAddr);
      }
    }
    return boxNone();
  },
  sleep_register: (taskPtr, futurePtr) => {
    expectPtrAddr(taskPtr, 'sleep_register');
    expectPtrAddr(futurePtr, 'sleep_register');
    return 0n;
  },
  spawn: (taskBits) => {
    const ptrBits = taskBits;
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory || !table) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'task must be awaitable' })),
      );
      raiseException(exc);
      return;
    }
    const addr = ptrAddr(ptrBits);
    const key = addr.toString();
    if (runnableTasks.has(key)) return;
    runnableTasks.add(key);
    runnableQueue.push(addr);
  },
  block_on: (taskPtr) => {
    if (!memory || !table) return 0n;
    let addr = 0;
    if (typeof taskPtr === 'number') {
      if (!Number.isInteger(taskPtr) || taskPtr < 0) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
        );
        raiseException(exc);
        return 0n;
      }
      addr = taskPtr;
    } else if (taskPtr === 0n) {
      addr = 0;
    } else if (isPtr(taskPtr)) {
      addr = ptrAddr(taskPtr);
    } else {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'object is not awaitable' })),
      );
      raiseException(exc);
      return 0n;
    }
    if (addr === 0) return 0n;
    const prevTask = currentTaskPtr;
    const prevToken = currentTokenId;
    currentTaskPtr = addr;
    const token = ensureTaskToken(addr);
    setCurrentTokenId(token);
    const rootBits = boxPtrAddr(addr);
    while (true) {
      const res = baseImports.future_poll(rootBits);
      if (!isPending(res)) {
        setCurrentTokenId(prevToken);
        currentTaskPtr = prevTask;
        clearTaskToken(addr);
        return res;
      }
      const pendingCount = runnableQueue.length;
      for (let i = 0; i < pendingCount; i += 1) {
        const taskAddr = runnableQueue.shift();
        const key = taskAddr.toString();
        if (!runnableTasks.has(key)) continue;
        const prevInnerTask = currentTaskPtr;
        const prevInnerToken = currentTokenId;
        currentTaskPtr = taskAddr;
        const innerToken = ensureTaskToken(taskAddr);
        setCurrentTokenId(innerToken);
        const taskBits = boxPtrAddr(taskAddr);
        const taskRes = baseImports.future_poll(taskBits);
        setCurrentTokenId(prevInnerToken);
        currentTaskPtr = prevInnerTask;
        if (isPending(taskRes)) {
          runnableQueue.push(taskAddr);
          continue;
        }
        runnableTasks.delete(key);
        clearTaskToken(taskAddr);
        taskWaitingOn.delete(taskAddr);
      }
    }
  },
  cancel_token_new: (parentBits) => {
    ensureRootToken();
    const parentId = tokenIdFromBits(parentBits);
    const resolved = parentId === 0n ? currentTokenId : parentId;
    const id = nextCancelTokenId++;
    cancelTokens.set(id, { parent: resolved, cancelled: false, refs: 1n });
    return boxInt(id);
  },
  cancel_token_clone: (tokenBits) => {
    const id = tokenIdFromBits(tokenBits);
    retainToken(id);
    return boxNone();
  },
  cancel_token_drop: (tokenBits) => {
    const id = tokenIdFromBits(tokenBits);
    releaseToken(id);
    return boxNone();
  },
  cancel_token_cancel: (tokenBits) => {
    const id = tokenIdFromBits(tokenBits);
    const entry = cancelTokens.get(id);
    if (entry) entry.cancelled = true;
    return boxNone();
  },
  cancel_token_is_cancelled: (tokenBits) => {
    const id = tokenIdFromBits(tokenBits);
    return boxBool(tokenIsCancelled(id));
  },
  cancel_token_set_current: (tokenBits) => {
    const id = tokenIdFromBits(tokenBits);
    const prev = setCurrentTokenId(id);
    if (currentTaskPtr !== 0) {
      registerTaskToken(currentTaskPtr, currentTokenId);
    }
    return boxInt(prev);
  },
  cancel_token_get_current: () => {
    ensureRootToken();
    return boxInt(currentTokenId);
  },
  cancelled: () => boxBool(tokenIsCancelled(currentTokenId)),
  cancel_current: () => {
    const entry = cancelTokens.get(currentTokenId);
    if (entry) entry.cancelled = true;
    return boxNone();
  },
  chan_new: (capacity) => {
    const id = nextChanId++;
    chanQueues.set(id, []);
    const cap = Number(unboxInt(capacity));
    chanCaps.set(id, cap);
    return id;
  },
  chan_send: (chan, val) => {
    const queue = chanQueues.get(chan);
    if (!queue) return boxPending();
    const cap = chanCaps.get(chan) || 0;
    if (cap > 0 && queue.length >= cap) return boxPending();
    queue.push(val);
    return 0n;
  },
  chan_recv: (chan) => {
    const queue = chanQueues.get(chan);
    if (!queue || queue.length === 0) return boxPending();
    return queue.shift();
  },
  chan_drop: (chan) => {
    chanQueues.delete(chan);
    chanCaps.delete(chan);
  },
  add: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxInt(unboxIntLike(a) + unboxIntLike(b));
    }
    if (getComplex(a) || getComplex(b)) {
      const lc = complexFromValStrict(a);
      const rc = complexFromValStrict(b);
      if ((lc && lc.overflow) || (rc && rc.overflow)) {
        throw new Error('OverflowError: int too large to convert to float');
      }
      if (lc && rc) {
        return boxComplex(lc.re + rc.re, lc.im + rc.im);
      }
      return boxNone();
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      return boxFloat(lf + rf);
    }
    const lstr = getStrObj(a);
    const rstr = getStrObj(b);
    if (lstr !== null && rstr !== null) {
      return boxPtr({ type: 'str', value: `${lstr}${rstr}` });
    }
    const lbytes = getBytes(a);
    const rbytes = getBytes(b);
    if (lbytes && rbytes) {
      return boxPtr({
        type: 'bytes',
        data: Uint8Array.from([...lbytes.data, ...rbytes.data]),
      });
    }
    const lba = getBytearray(a);
    const rba = getBytearray(b);
    if (lba && rba) {
      return boxPtr({
        type: 'bytearray',
        data: Uint8Array.from([...lba.data, ...rba.data]),
      });
    }
    const llist = getList(a);
    const rlist = getList(b);
    if (llist && rlist) {
      return listFromArray([...llist.items, ...rlist.items]);
    }
    const ltuple = getTuple(a);
    const rtuple = getTuple(b);
    if (ltuple && rtuple) {
      return tupleFromArray([...ltuple.items, ...rtuple.items]);
    }
    return boxNone();
  },
  inplace_add: (a, b) => {
    const list = getList(a);
    if (list) {
      baseImports.list_extend(a, b);
      return a;
    }
    const bytearray = getBytearray(a);
    if (bytearray) {
      const bytes = getBytes(b);
      const other = getBytearray(b);
      if (!bytes && !other) {
        throw new Error(`TypeError: can't concat ${typeName(b)} to bytearray`);
      }
      const leftData = bytearray.data;
      let rightData = bytes ? bytes.data : other.data;
      if (other && other === bytearray) {
        rightData = Uint8Array.from(bytearray.data);
      }
      const out = new Uint8Array(leftData.length + rightData.length);
      out.set(leftData, 0);
      out.set(rightData, leftData.length);
      bytearray.data = out;
      return a;
    }
    return baseImports.add(a, b);
  },
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
  sub: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxInt(unboxIntLike(a) - unboxIntLike(b));
    }
    if (getComplex(a) || getComplex(b)) {
      const lc = complexFromValStrict(a);
      const rc = complexFromValStrict(b);
      if ((lc && lc.overflow) || (rc && rc.overflow)) {
        throw new Error('OverflowError: int too large to convert to float');
      }
      if (lc && rc) {
        return boxComplex(lc.re - rc.re, lc.im - rc.im);
      }
      return boxNone();
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      return boxFloat(lf - rf);
    }
    const lset = getSetOpItems(a);
    const rset = getSetOpItems(b);
    if (lset && rset) {
      const outItems = new Set();
      for (const item of lset.items) {
        if (!rset.items.has(item)) {
          outItems.add(item);
        }
      }
      const outType = lset.isView || rset.isView ? 'set' : lset.type;
      return boxPtr({ type: outType, items: outItems });
    }
    return boxNone();
  },
  inplace_sub: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetInplaceRhs(b);
      if (!other) {
        throw new Error(
          `TypeError: unsupported operand type(s) for -=: '${typeName(a)}' and '${typeName(
            b,
          )}'`,
        );
      }
      baseImports.set_difference_update(a, b);
      return a;
    }
    return baseImports.sub(a, b);
  },
  bit_or: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      const li = unboxIntLike(a);
      const ri = unboxIntLike(b);
      if (isTag(a, TAG_BOOL) && isTag(b, TAG_BOOL)) {
        return boxBool((li | ri) !== 0n);
      }
      return boxInt(li | ri);
    }
    const lset = getSetOpItems(a);
    const rset = getSetOpItems(b);
    if (lset && rset) {
      const outItems = new Set(lset.items);
      for (const item of rset.items) {
        outItems.add(item);
      }
      const outType = lset.isView || rset.isView ? 'set' : lset.type;
      return boxPtr({ type: outType, items: outItems });
    }
    return boxNone();
  },
  bit_and: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      const li = unboxIntLike(a);
      const ri = unboxIntLike(b);
      if (isTag(a, TAG_BOOL) && isTag(b, TAG_BOOL)) {
        return boxBool((li & ri) !== 0n);
      }
      return boxInt(li & ri);
    }
    const lset = getSetOpItems(a);
    const rset = getSetOpItems(b);
    if (lset && rset) {
      const outItems = new Set();
      for (const item of lset.items) {
        if (rset.items.has(item)) {
          outItems.add(item);
        }
      }
      const outType = lset.isView || rset.isView ? 'set' : lset.type;
      return boxPtr({ type: outType, items: outItems });
    }
    return boxNone();
  },
  bit_xor: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      const li = unboxIntLike(a);
      const ri = unboxIntLike(b);
      if (isTag(a, TAG_BOOL) && isTag(b, TAG_BOOL)) {
        return boxBool((li ^ ri) !== 0n);
      }
      return boxInt(li ^ ri);
    }
    const lset = getSetOpItems(a);
    const rset = getSetOpItems(b);
    if (lset && rset) {
      const outItems = new Set();
      for (const item of lset.items) {
        if (!rset.items.has(item)) {
          outItems.add(item);
        }
      }
      for (const item of rset.items) {
        if (!lset.items.has(item)) {
          outItems.add(item);
        }
      }
      const outType = lset.isView || rset.isView ? 'set' : lset.type;
      return boxPtr({ type: outType, items: outItems });
    }
    return boxNone();
  },
  invert: (a) => {
    if (isIntLike(a)) {
      return boxInt(~unboxIntLike(a));
    }
    return boxNone();
  },
  inplace_bit_or: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetInplaceRhs(b);
      if (!other) {
        throw new Error(
          `TypeError: unsupported operand type(s) for |=: '${typeName(a)}' and '${typeName(
            b,
          )}'`,
        );
      }
      baseImports.set_update(a, b);
      return a;
    }
    return baseImports.bit_or(a, b);
  },
  inplace_bit_and: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetInplaceRhs(b);
      if (!other) {
        throw new Error(
          `TypeError: unsupported operand type(s) for &=: '${typeName(a)}' and '${typeName(
            b,
          )}'`,
        );
      }
      baseImports.set_intersection_update(a, b);
      return a;
    }
    return baseImports.bit_and(a, b);
  },
  inplace_bit_xor: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetInplaceRhs(b);
      if (!other) {
        throw new Error(
          `TypeError: unsupported operand type(s) for ^=: '${typeName(a)}' and '${typeName(
            b,
          )}'`,
        );
      }
      baseImports.set_symdiff_update(a, b);
      return a;
    }
    return baseImports.bit_xor(a, b);
  },
  lshift: (a, b) => {
    if (!isIntLike(a) || !isIntLike(b)) {
      throw new Error(
        `TypeError: unsupported operand type(s) for <<: '${typeName(a)}' and '${typeName(
          b,
        )}'`,
      );
    }
    const shift = unboxIntLike(b);
    if (shift < 0n) {
      throw new Error('ValueError: negative shift count');
    }
    if (shift >= 63n) {
      return boxInt(0);
    }
    return boxInt(unboxIntLike(a) << shift);
  },
  rshift: (a, b) => {
    if (!isIntLike(a) || !isIntLike(b)) {
      throw new Error(
        `TypeError: unsupported operand type(s) for >>: '${typeName(a)}' and '${typeName(
          b,
        )}'`,
      );
    }
    const shift = unboxIntLike(b);
    if (shift < 0n) {
      throw new Error('ValueError: negative shift count');
    }
    if (shift >= 63n) {
      return unboxIntLike(a) >= 0n ? boxInt(0) : boxInt(-1);
    }
    return boxInt(unboxIntLike(a) >> shift);
  },
  matmul: (a, b) => {
    const la = getObj(a);
    const lb = getObj(b);
    if (la && lb && la.type === 'buffer2d' && lb.type === 'buffer2d') {
      return boxNone();
    }
    throw new Error(
      `TypeError: unsupported operand type(s) for @: '${typeName(a)}' and '${typeName(
        b,
      )}'`,
    );
  },
  mul: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxInt(unboxIntLike(a) * unboxIntLike(b));
    }
    if (getComplex(a) || getComplex(b)) {
      const lc = complexFromValStrict(a);
      const rc = complexFromValStrict(b);
      if ((lc && lc.overflow) || (rc && rc.overflow)) {
        throw new Error('OverflowError: int too large to convert to float');
      }
      if (lc && rc) {
        const re = lc.re * rc.re - lc.im * rc.im;
        const im = lc.im * rc.re + lc.re * rc.im;
        return boxComplex(re, im);
      }
      return boxNone();
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      return boxFloat(lf * rf);
    }
    return boxNone();
  },
  inplace_mul: (a, b) => {
    const list = getList(a);
    if (list) {
      if (!isIntLike(b)) {
        throw new Error(
          `TypeError: can't multiply sequence by non-int of type '${typeName(b)}'`,
        );
      }
      const count = Number(unboxIntLike(b));
      if (count <= 0) {
        list.items.length = 0;
        return a;
      }
      if (count === 1) {
        return a;
      }
      const snapshot = [...list.items];
      list.items.length = 0;
      for (let i = 0; i < count; i += 1) {
        list.items.push(...snapshot);
      }
      return a;
    }
    const bytearray = getBytearray(a);
    if (bytearray) {
      if (!isIntLike(b)) {
        throw new Error(
          `TypeError: can't multiply sequence by non-int of type '${typeName(b)}'`,
        );
      }
      const count = Number(unboxIntLike(b));
      if (count <= 0) {
        bytearray.data = new Uint8Array(0);
        return a;
      }
      if (count === 1) {
        return a;
      }
      const snapshot = Uint8Array.from(bytearray.data);
      const out = new Uint8Array(snapshot.length * count);
      for (let i = 0; i < count; i += 1) {
        out.set(snapshot, i * snapshot.length);
      }
      bytearray.data = out;
      return a;
    }
    return baseImports.mul(a, b);
  },
  div: (a, b) => {
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (getComplex(a) || getComplex(b)) {
      const lc = complexFromValStrict(a);
      const rc = complexFromValStrict(b);
      if ((lc && lc.overflow) || (rc && rc.overflow)) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'OverflowError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: 'int too large to convert to float',
            }),
          ),
        );
        return raiseException(exc);
      }
      if (lc && rc) {
        const denom = rc.re * rc.re + rc.im * rc.im;
        if (denom === 0) {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'ZeroDivisionError' }),
            exceptionArgs(boxPtr({ type: 'str', value: 'division by zero' })),
          );
          return raiseException(exc);
        }
        const re = (lc.re * rc.re + lc.im * rc.im) / denom;
        const im = (lc.im * rc.re - lc.re * rc.im) / denom;
        return boxComplex(re, im);
      }
      return boxNone();
    }
    if (lf === null || rf === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: 'unsupported operand type(s) for /',
          }),
        ),
      );
      return raiseException(exc);
    }
    if (rf === 0) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'ZeroDivisionError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'division by zero' })),
      );
      return raiseException(exc);
    }
    return boxFloat(lf / rf);
  },
  floordiv: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      const li = unboxIntLike(a);
      const ri = unboxIntLike(b);
      if (ri === 0n) {
        throw new Error('ZeroDivisionError: integer division or modulo by zero');
      }
      let q = li / ri;
      const r = li % ri;
      if (r !== 0n && (r > 0n) !== (ri > 0n)) {
        q -= 1n;
      }
      return boxInt(q);
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf === null || rf === null) {
      throw new Error('TypeError: unsupported operand type(s) for //');
    }
    if (rf === 0) {
      throw new Error('ZeroDivisionError: float floor division by zero');
    }
    return boxFloat(Math.floor(lf / rf));
  },
  mod: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      const li = unboxIntLike(a);
      const ri = unboxIntLike(b);
      if (ri === 0n) {
        throw new Error('ZeroDivisionError: integer division or modulo by zero');
      }
      let rem = li % ri;
      if (rem !== 0n && (rem > 0n) !== (ri > 0n)) {
        rem += ri;
      }
      return boxInt(rem);
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf === null || rf === null) {
      throw new Error('TypeError: unsupported operand type(s) for %');
    }
    if (rf === 0) {
      throw new Error('ZeroDivisionError: float modulo');
    }
    let rem = lf % rf;
    if (rem !== 0 && (rem > 0) !== (rf > 0)) {
      rem += rf;
    }
    return boxFloat(rem);
  },
  pow: (a, b) => {
    if (getComplex(a) || getComplex(b)) {
      const base = complexFromValStrict(a);
      const exp = complexFromValStrict(b);
      if ((base && base.overflow) || (exp && exp.overflow)) {
        throw new Error('OverflowError: int too large to convert to float');
      }
      if (base && exp) {
        const out = complexPow(base, exp);
        if (!out) {
          throw new Error('ZeroDivisionError: zero to a negative or complex power');
        }
        return boxComplex(out.re, out.im);
      }
      throw new Error('TypeError: unsupported operand type(s) for **');
    }
    const isIntPair = isIntLike(a) && isIntLike(b);
    if (isIntPair) {
      const base = unboxIntLike(a);
      const exp = unboxIntLike(b);
      if (exp >= 0n) {
        let result = 1n;
        let baseVal = base;
        let expVal = exp;
        const max = (1n << 46n) - 1n;
        const min = -(1n << 46n);
        while (expVal > 0n) {
          if (expVal & 1n) {
            result *= baseVal;
            if (result > max || result < min) {
              result = null;
              break;
            }
          }
          expVal >>= 1n;
          if (expVal) {
            baseVal *= baseVal;
            if (baseVal > max || baseVal < min) {
              result = null;
              break;
            }
          }
        }
        if (result !== null) {
          return boxInt(result);
        }
      }
      const lf = Number(base);
      const rf = Number(exp);
      if (lf === 0 && rf < 0) {
        throw new Error('ZeroDivisionError: 0.0 cannot be raised to a negative power');
      }
      return boxFloat(Math.pow(lf, rf));
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf === null || rf === null) {
      throw new Error('TypeError: unsupported operand type(s) for **');
    }
    if (lf === 0 && rf < 0) {
      throw new Error('ZeroDivisionError: 0.0 cannot be raised to a negative power');
    }
    if (lf < 0 && Number.isFinite(rf) && !Number.isInteger(rf)) {
      const out = complexPow({ re: lf, im: 0 }, { re: rf, im: 0 });
      if (out) {
        return boxComplex(out.re, out.im);
      }
    }
    return boxFloat(Math.pow(lf, rf));
  },
  pow_mod: (a, b, m) => {
    if (!isIntLike(a) || !isIntLike(b) || !isIntLike(m)) {
      throw new Error(
        'TypeError: pow() 3rd argument not allowed unless all arguments are integers',
      );
    }
    const base = unboxIntLike(a);
    const exp = unboxIntLike(b);
    const mod = unboxIntLike(m);
    if (mod === 0n) {
      throw new Error('ValueError: pow() 3rd argument cannot be 0');
    }
    const modPy = (val, modulus) => {
      let rem = val % modulus;
      if (rem !== 0n && (rem > 0n) !== (modulus > 0n)) {
        rem += modulus;
      }
      return rem;
    };
    const modPow = (baseVal, expVal, modulus) => {
      let result = 1n;
      let baseMod = modPy(baseVal, modulus);
      let expBits = expVal;
      while (expBits > 0n) {
        if (expBits & 1n) {
          result = modPy(result * baseMod, modulus);
        }
        expBits >>= 1n;
        if (expBits > 0n) {
          baseMod = modPy(baseMod * baseMod, modulus);
        }
      }
      return modPy(result, modulus);
    };
    if (exp < 0n) {
      const modAbs = mod < 0n ? -mod : mod;
      const baseMod = modPy(base, modAbs);
      const egcd = (x, y) => {
        if (y === 0n) return [x, 1n, 0n];
        const [g, a1, b1] = egcd(y, x % y);
        return [g, b1, a1 - (x / y) * b1];
      };
      const [g, x] = egcd(baseMod, modAbs);
      if (g !== 1n && g !== -1n) {
        throw new Error('ValueError: base is not invertible for the given modulus');
      }
      const inv = modPy(x, modAbs);
      const invMod = modPy(inv, mod);
      return boxInt(modPow(invMod, -exp, mod));
    }
    return boxInt(modPow(base, exp, mod));
  },
  round: (val, ndigits, hasNdigits) => {
    const hasDigits = isTag(hasNdigits, TAG_BOOL) && (hasNdigits & 1n) === 1n;
    if (isIntLike(val)) {
      if (!hasDigits) return boxInt(unboxIntLike(val));
      if (isNone(ndigits)) return boxInt(unboxIntLike(val));
      if (!isIntLike(ndigits)) {
        throw new Error('TypeError: round() ndigits must be int');
      }
      const nd = unboxIntLike(ndigits);
      if (nd >= 0n) return boxInt(unboxIntLike(val));
      const exp = Number(-nd);
      if (exp > 38) return boxInt(0);
      let pow = 1n;
      for (let i = 0; i < exp; i += 1) {
        pow *= 10n;
      }
      const value = unboxIntLike(val);
      const div = value / pow;
      const rem = value % pow;
      const absRem = rem < 0n ? -rem : rem;
      const twice = absRem * 2n;
      let rounded = div;
      if (twice > pow) {
        rounded += value >= 0n ? 1n : -1n;
      } else if (twice === pow && (div & 1n) !== 0n) {
        rounded += value >= 0n ? 1n : -1n;
      }
      return boxInt(rounded * pow);
    }
    if (isFloat(val)) {
      const num = bitsToFloat(val);
      const roundHalfEven = (x) => {
        if (!Number.isFinite(x)) return x;
        const floor = Math.floor(x);
        const ceil = Math.ceil(x);
        const diffFloor = Math.abs(x - floor);
        const diffCeil = Math.abs(ceil - x);
        if (diffFloor < diffCeil) return floor;
        if (diffCeil < diffFloor) return ceil;
        if (Math.abs(floor) > Number.MAX_SAFE_INTEGER) return floor;
        return floor % 2 === 0 ? floor : ceil;
      };
      if (!hasDigits || isNone(ndigits)) {
        if (Number.isNaN(num)) {
          throw new Error('ValueError: cannot convert float NaN to integer');
        }
        if (!Number.isFinite(num)) {
          throw new Error('OverflowError: cannot convert float infinity to integer');
        }
        return boxInt(BigInt(Math.trunc(roundHalfEven(num))));
      }
      if (!isIntLike(ndigits)) {
        throw new Error('TypeError: round() ndigits must be int');
      }
      const nd = Number(unboxIntLike(ndigits));
      if (!Number.isFinite(num)) return boxFloat(num);
      if (nd === 0) return boxFloat(roundHalfEven(num));
      if (nd > 0) {
        if (nd > 308) return boxFloat(num);
        const text = num.toFixed(nd);
        const parsed = Number.parseFloat(text);
        return boxFloat(Number.isNaN(parsed) ? num : parsed);
      }
      const factor = 10 ** -nd;
      if (!Number.isFinite(factor)) {
        return boxFloat(num < 0 ? -0.0 : 0.0);
      }
      if (factor === 0) return boxFloat(num);
      const scaled = num / factor;
      return boxFloat(roundHalfEven(scaled) * factor);
    }
    const roundAttr = lookupAttr(val, '__round__');
    if (roundAttr !== undefined) {
      const arity = callableArity(roundAttr);
      if (arity <= 1) {
        if (hasDigits && !isNone(ndigits)) {
          return callCallable1(roundAttr, ndigits);
        }
        return callCallable0(roundAttr);
      }
      const arg = hasDigits && !isNone(ndigits) ? ndigits : boxNone();
      return callCallable1(roundAttr, arg);
    }
    throw new Error('TypeError: round() expects a real number');
  },
  trunc: (val) => {
    if (isIntLike(val)) return boxInt(unboxIntLike(val));
    if (isFloat(val)) {
      const num = bitsToFloat(val);
      if (Number.isNaN(num)) {
        throw new Error('ValueError: cannot convert float NaN to integer');
      }
      if (!Number.isFinite(num)) {
        throw new Error('OverflowError: cannot convert float infinity to integer');
      }
      return boxInt(BigInt(Math.trunc(num)));
    }
    const truncAttr = lookupAttr(val, '__trunc__');
    if (truncAttr !== undefined) {
      return callCallable0(truncAttr);
    }
    throw new Error('TypeError: trunc() expects a real number');
  },
  lt: (a, b) => {
    const builtin = compareBuiltinBool(a, b, '<');
    if (builtin.kind === 'bool') return boxBool(builtin.value);
    if (builtin.kind === 'error') return boxNone();
    const rich = richCompareBool(a, b, '__lt__', '__gt__');
    if (rich.kind === 'bool') return boxBool(rich.value);
    if (rich.kind === 'error') return boxNone();
    return compareTypeError('<', a, b);
  },
  le: (a, b) => {
    const builtin = compareBuiltinBool(a, b, '<=');
    if (builtin.kind === 'bool') return boxBool(builtin.value);
    if (builtin.kind === 'error') return boxNone();
    const rich = richCompareBool(a, b, '__le__', '__ge__');
    if (rich.kind === 'bool') return boxBool(rich.value);
    if (rich.kind === 'error') return boxNone();
    return compareTypeError('<=', a, b);
  },
  gt: (a, b) => {
    const builtin = compareBuiltinBool(a, b, '>');
    if (builtin.kind === 'bool') return boxBool(builtin.value);
    if (builtin.kind === 'error') return boxNone();
    const rich = richCompareBool(a, b, '__gt__', '__lt__');
    if (rich.kind === 'bool') return boxBool(rich.value);
    if (rich.kind === 'error') return boxNone();
    return compareTypeError('>', a, b);
  },
  ge: (a, b) => {
    const builtin = compareBuiltinBool(a, b, '>=');
    if (builtin.kind === 'bool') return boxBool(builtin.value);
    if (builtin.kind === 'error') return boxNone();
    const rich = richCompareBool(a, b, '__ge__', '__le__');
    if (rich.kind === 'bool') return boxBool(rich.value);
    if (rich.kind === 'error') return boxNone();
    return compareTypeError('>=', a, b);
  },
  eq: (a, b) => {
    const lComplexObj = getComplex(a);
    const rComplexObj = getComplex(b);
    if (lComplexObj || rComplexObj) {
      const lc = complexFromValLossy(a);
      const rc = complexFromValLossy(b);
      if (lc && rc) {
        if (
          Number.isNaN(lc.re) ||
          Number.isNaN(lc.im) ||
          Number.isNaN(rc.re) ||
          Number.isNaN(rc.im)
        ) {
          return boxBool(false);
        }
        return boxBool(lc.re === rc.re && lc.im === rc.im);
      }
      return boxBool(false);
    }
    const ln = numberFromVal(a);
    const rn = numberFromVal(b);
    if (ln !== null && rn !== null) {
      if (Number.isNaN(ln) || Number.isNaN(rn)) return boxBool(false);
      return boxBool(ln === rn);
    }
    if (isTag(a, TAG_NONE) && isTag(b, TAG_NONE)) return boxBool(true);
    if (isPtr(a) && isPtr(b)) {
      const left = getObj(a);
      const right = getObj(b);
      if (left && right) {
        if (left.type === 'str' && right.type === 'str') {
          return boxBool(left.value === right.value);
        }
        if (left.type === 'bytes' && right.type === 'bytes') {
          return boxBool(
            Buffer.from(left.data).equals(Buffer.from(right.data)),
          );
        }
        if (left.type === 'bytearray' && right.type === 'bytearray') {
          return boxBool(
            Buffer.from(left.data).equals(Buffer.from(right.data)),
          );
        }
        if (left.type === 'bytes' && right.type === 'bytearray') {
          return boxBool(
            Buffer.from(left.data).equals(Buffer.from(right.data)),
          );
        }
        if (left.type === 'bytearray' && right.type === 'bytes') {
          return boxBool(
            Buffer.from(left.data).equals(Buffer.from(right.data)),
          );
        }
        if (left.type === 'list' && right.type === 'list') {
          if (left.items.length !== right.items.length) return boxBool(false);
          for (let i = 0; i < left.items.length; i += 1) {
            if (!isTruthyBits(baseImports.eq(left.items[i], right.items[i]))) {
              return boxBool(false);
            }
          }
          return boxBool(true);
        }
        if (left.type === 'tuple' && right.type === 'tuple') {
          if (left.items.length !== right.items.length) return boxBool(false);
          for (let i = 0; i < left.items.length; i += 1) {
            if (!isTruthyBits(baseImports.eq(left.items[i], right.items[i]))) {
              return boxBool(false);
            }
          }
          return boxBool(true);
        }
        if (left.type === 'dict' && right.type === 'dict') {
          if (left.entries.length !== right.entries.length) {
            return boxBool(false);
          }
          for (const [keyBits, valBits] of left.entries) {
            const otherVal = dictGetValue(right, keyBits);
            if (otherVal === null) return boxBool(false);
            if (!isTruthyBits(baseImports.eq(valBits, otherVal))) {
              return boxBool(false);
            }
          }
          return boxBool(true);
        }
      }
    }
    return boxBool(a === b);
  },
  string_eq: (a, b) => {
    const left = getStrObj(a);
    const right = getStrObj(b);
    if (left === null || right === null) return boxBool(false);
    return boxBool(left === right);
  },
  is: (a, b) => {
    if (process.env.MOLT_DEBUG_IS === '1' && (isTag(a, TAG_NONE) || isTag(b, TAG_NONE))) {
      console.error(`is(a=0x${a.toString(16)}, b=0x${b.toString(16)}) -> ${a === b}`);
    }
    return boxBool(a === b);
  },
  closure_load: (ptr, offset) => {
    if (!memory) return boxNone();
    const base = expectPtrAddr(ptr, 'closure_load');
    if (!base) return boxNone();
    const addr = base + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  closure_store: (ptr, offset, val) => {
    if (!memory) return boxNone();
    const base = expectPtrAddr(ptr, 'closure_store');
    if (!base) return boxNone();
    const addr = base + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  not: (val) => {
    return boxBool(baseImports.is_truthy(val) === 0n);
  },
  contains: (container, item) => {
    const list = getList(container);
    if (list) return boxBool(list.items.includes(item));
    const tup = getTuple(container);
    if (tup) return boxBool(tup.items.includes(item));
    const setLike = getSetLike(container);
    if (setLike) return boxBool(setLike.items.has(item));
    return boxBool(false);
  },
  guard_type: (val, expected) => val,
  guard_layout_ptr: (obj, classBits, expected) => {
    if (obj === 0n) return boxBool(false);
    if (!getClass(classBits)) return boxBool(false);
    const addr = expectPtrAddr(obj, 'guard_layout_ptr');
    const clsBits = instanceClasses.get(addr);
    if (clsBits === undefined || clsBits !== classBits) return boxBool(false);
    const version = classLayoutVersion(classBits);
    if (version === null) return boxBool(false);
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    return boxBool(version === expectedVersion);
  },
  class_field_offset_name: (classBits, name) => {
    if (!getClass(classBits)) return null;
    const cached = classFieldOffsets.get(classBits);
    if (cached) {
      const offset = cached.get(name);
      if (offset !== undefined) return offset;
    }
    const offsetsBits = lookupClassAttr(classBits, '__molt_field_offsets__');
    const offsets = offsetsBits !== undefined ? getDict(offsetsBits) : null;
    if (!offsets) return null;
    const nameBits = boxPtr({ type: 'str', value: name });
    const offsetBits = dictGetValue(offsets, nameBits);
    if (offsetBits === null || !isIntLike(offsetBits)) return null;
    const offset = Number(unboxIntLike(offsetBits));
    return offset >= 0 ? offset : null;
  },
  guarded_field_get_ptr: (obj, classBits, expected, offset, namePtr, nameLen) => {
    const name = readUtf8(namePtr, nameLen);
    if (obj === 0n) {
      throw new Error('AttributeError: object has no attribute');
    }
    const base = expectPtrAddr(obj, 'guarded_field_get_ptr');
    const objBits = boxPtrAddr(base);
    if (!getClass(classBits)) {
      return getAttrValue(objBits, name);
    }
    const clsBits = instanceClasses.get(base);
    if (clsBits === undefined || clsBits !== classBits) {
      return getAttrValue(objBits, name);
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return getAttrValue(objBits, name);
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      const actualOffset = baseImports.class_field_offset_name(classBits, name);
      if (actualOffset !== null && memory) {
        const addr = base + actualOffset;
        const view = new DataView(memory.buffer);
        return view.getBigInt64(addr, true);
      }
      return getAttrValue(objBits, name);
    }
    if (!memory) return boxNone();
    const actualOffset = baseImports.class_field_offset_name(classBits, name);
    const addr = base + Number(actualOffset === null ? offset : actualOffset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  guarded_field_set_ptr: (
    obj,
    classBits,
    expected,
    offset,
    val,
    namePtr,
    nameLen,
  ) => {
    const name = readUtf8(namePtr, nameLen);
    if (obj === 0n) {
      throw new Error('AttributeError: object has no attribute');
    }
    const base = expectPtrAddr(obj, 'guarded_field_set_ptr');
    const objBits = boxPtrAddr(base);
    if (!getClass(classBits)) {
      return setAttrValue(objBits, name, val);
    }
    const clsBits = instanceClasses.get(base);
    if (clsBits === undefined || clsBits !== classBits) {
      return setAttrValue(objBits, name, val);
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return setAttrValue(objBits, name, val);
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      return setAttrValue(objBits, name, val);
    }
    if (!memory) return boxNone();
    const actualOffset = baseImports.class_field_offset_name(classBits, name);
    const addr = base + Number(actualOffset === null ? offset : actualOffset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  guarded_field_init_ptr: (
    obj,
    classBits,
    expected,
    offset,
    val,
    namePtr,
    nameLen,
  ) => {
    const name = readUtf8(namePtr, nameLen);
    if (obj === 0n) {
      throw new Error('AttributeError: object has no attribute');
    }
    const base = expectPtrAddr(obj, 'guarded_field_init_ptr');
    const objBits = boxPtrAddr(base);
    if (!getClass(classBits)) {
      return setAttrValue(objBits, name, val);
    }
    const clsBits = instanceClasses.get(base);
    if (clsBits === undefined || clsBits !== classBits) {
      return setAttrValue(objBits, name, val);
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return setAttrValue(objBits, name, val);
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      return setAttrValue(objBits, name, val);
    }
    if (!memory) return boxNone();
    const actualOffset = baseImports.class_field_offset_name(classBits, name);
    const addr = base + Number(actualOffset === null ? offset : actualOffset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  handle_resolve: (bits) => {
    if (!isPtr(bits)) return 0;
    const id = bits & POINTER_MASK;
    const obj = heap.get(id);
    if (obj && obj.memAddr) return obj.memAddr;
    if (!obj) return ptrAddr(bits);
    return 0;
  },
  inc_ref_obj: (_val) => {},
  get_attr_ptr: (obj, namePtr, nameLen) => {
    const addr = expectPtrAddr(obj, 'get_attr_ptr');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return getAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen));
  },
  get_attr_generic: (obj, namePtr, nameLen) => {
    const addr = expectPtrAddr(obj, 'get_attr_generic');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return getAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen));
  },
  get_attr_object: (obj, namePtr, nameLen) => {
    const name = readUtf8(namePtr, nameLen);
    lastAttrName = name;
    lastAttrObjType = typeName(obj);
    if (
      process.env.MOLT_DEBUG_GETATTR_STARTSWITH === '1' &&
      name === 'startswith'
    ) {
      console.error(`get_attr_object startswith on ${typeName(obj)}`);
    }
    return getAttrValue(obj, name);
  },
  get_attr_special: (obj, namePtr, nameLen) =>
    getAttrSpecialValue(obj, readUtf8(namePtr, nameLen)),
  set_attr_ptr: (obj, namePtr, nameLen, val) => {
    const addr = expectPtrAddr(obj, 'set_attr_ptr');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return setAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen), val);
  },
  set_attr_generic: (obj, namePtr, nameLen, val) => {
    const addr = expectPtrAddr(obj, 'set_attr_generic');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return setAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen), val);
  },
  set_attr_object: (obj, namePtr, nameLen, val) =>
    setAttrValue(obj, readUtf8(namePtr, nameLen), val),
  del_attr_ptr: (obj, namePtr, nameLen) => {
    const addr = expectPtrAddr(obj, 'del_attr_ptr');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return delAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen));
  },
  del_attr_generic: (obj, namePtr, nameLen) => {
    const addr = expectPtrAddr(obj, 'del_attr_generic');
    if (!addr) {
      throw new Error('AttributeError: object has no attribute');
    }
    return delAttrValue(boxPtrAddr(addr), readUtf8(namePtr, nameLen));
  },
  del_attr_object: (obj, namePtr, nameLen) =>
    delAttrValue(obj, readUtf8(namePtr, nameLen)),
  object_field_get: (obj, offset) => {
    if (!memory) return boxNone();
    if (!isPtr(obj)) {
      throw new Error('TypeError: object field access on non-object');
    }
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  object_field_set: (obj, offset, val) => {
    if (!memory) return boxNone();
    if (!isPtr(obj)) {
      throw new Error('TypeError: object field access on non-object');
    }
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_init: (obj, offset, val) => {
    if (!memory) return boxNone();
    if (!isPtr(obj)) {
      throw new Error('TypeError: object field access on non-object');
    }
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_get_ptr: (obj, offset) => {
    if (!memory) return boxNone();
    const base = expectPtrAddr(obj, 'object_field_get_ptr');
    if (!base) return boxNone();
    const addr = base + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  object_field_set_ptr: (obj, offset, val) => {
    if (!memory) return boxNone();
    const base = expectPtrAddr(obj, 'object_field_set_ptr');
    if (!base) return boxNone();
    const addr = base + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_init_ptr: (obj, offset, val) => {
    if (!memory) return boxNone();
    const base = expectPtrAddr(obj, 'object_field_init_ptr');
    if (!base) return boxNone();
    const addr = base + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  module_new: (nameBits) => {
    const name = getStrObj(nameBits);
    const dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
    const dict = getDict(dictBits);
    if (dict) {
      const keyBits = boxPtr({ type: 'str', value: '__name__' });
      dictSetValue(dict, keyBits, nameBits);
    }
    const moduleBits = boxPtr({ type: 'module', name: name ?? '<module>', dictBits });
    if (dict && name === 'importlib.machinery') {
      const loaderKey = boxPtr({ type: 'str', value: 'MOLT_LOADER' });
      const loaderBits = boxPtr({ type: 'molt_loader' });
      dictSetValue(dict, loaderKey, loaderBits);
    }
    if (name === 'builtins') {
      installIntrinsics(moduleBits);
    }
    return moduleBits;
  },
  module_cache_get: (nameBits) => {
    const name = getStrObj(nameBits);
    if (name === null) return boxNone();
    const moduleBits = moduleCache.get(name);
    return moduleBits === undefined ? boxNone() : moduleBits;
  },
  module_cache_set: (nameBits, moduleBits) => {
    const name = getStrObj(nameBits);
    if (name === null) return boxNone();
    moduleCache.set(name, moduleBits);
    return boxNone();
  },
  module_get_attr: (moduleBits, nameBits) => {
    const name = getStrObj(nameBits);
    const moduleObj = getModule(moduleBits);
    if (!moduleObj || name === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'module attribute access expects module' }),
        ),
      );
      return raiseException(exc);
    }
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    if (name === '__dict__') return moduleObj.dictBits;
    const val = dictGetValue(dict, nameBits);
    if (val !== null) return val;
    const moduleName = moduleObj.name ?? '';
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'AttributeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: `module '${moduleName}' has no attribute '${name}'`,
        }),
      ),
    );
    return raiseException(exc);
  },
  module_get_global: (moduleBits, nameBits) => {
    const name = getStrObj(nameBits);
    const moduleObj = getModule(moduleBits);
    if (!moduleObj || name === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'module attribute access expects module' }),
        ),
      );
      return raiseException(exc);
    }
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    const val = dictGetValue(dict, nameBits);
    if (val !== null) return val;
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'NameError' }),
      exceptionArgs(
        boxPtr({ type: 'str', value: `name '${name}' is not defined` }),
      ),
    );
    return raiseException(exc);
  },
  module_get_name: (moduleBits, nameBits) => {
    return baseImports.module_get_attr(moduleBits, nameBits);
  },
  module_del_global: (moduleBits, nameBits) => {
    const name = getStrObj(nameBits);
    const moduleObj = getModule(moduleBits);
    if (!moduleObj || name === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: 'module attribute access expects module',
          }),
        ),
      );
      return raiseException(exc);
    }
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    if (dictDelete(dict, nameBits)) return boxNone();
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'NameError' }),
      exceptionArgs(
        boxPtr({ type: 'str', value: `name '${name}' is not defined` }),
      ),
    );
    return raiseException(exc);
  },
  module_import_star: (srcBits, dstBits) => {
    const srcModule = getModule(srcBits);
    const dstModule = getModule(dstBits);
    if (!srcModule || !dstModule) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'module import expects module' }),
        ),
      );
      return raiseException(exc);
    }
    const srcDict = getDict(srcModule.dictBits);
    const dstDict = getDict(dstModule.dictBits);
    if (!srcDict || !dstDict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    const moduleName = srcModule.name ?? '';
    const allNameBits = boxPtr({ type: 'str', value: '__all__' });
    const allBits = dictGetValue(srcDict, allNameBits);
    if (allBits !== null) {
      const iterBits = baseImports.iter(allBits);
      if (isNone(iterBits)) return boxNone();
      while (true) {
        const pairBits = baseImports.iter_next(iterBits);
        const tuple = getTuple(pairBits);
        if (!tuple || tuple.items.length < 2) {
          return boxNone();
        }
        const doneBits = tuple.items[1];
        if (isTruthyBits(doneBits)) {
          break;
        }
        const nameBits = tuple.items[0];
        const name = getStrObj(nameBits);
        if (name === null) {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'TypeError' }),
            exceptionArgs(
              boxPtr({
                type: 'str',
                value: `Item in ${moduleName}.__all__ must be str, not ${typeName(nameBits)}`,
              }),
            ),
          );
          return raiseException(exc);
        }
        const val = dictGetValue(srcDict, nameBits);
        if (val === null) {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'AttributeError' }),
            exceptionArgs(
              boxPtr({
                type: 'str',
                value: `module '${moduleName}' has no attribute '${name}'`,
              }),
            ),
          );
          return raiseException(exc);
        }
        dictSetValue(dstDict, nameBits, val);
      }
      return boxNone();
    }
    for (const [keyBits, val] of srcDict.entries) {
      const name = getStrObj(keyBits);
      if (name === null) continue;
      if (name.length > 0 && name[0] === '_') {
        continue;
      }
      dictSetValue(dstDict, keyBits, val);
    }
    return boxNone();
  },
  module_set_attr: (moduleBits, nameBits, val) => {
    const name = getStrObj(nameBits);
    const moduleObj = getModule(moduleBits);
    if (!moduleObj || name === null) return boxNone();
    const dict = getDict(moduleObj.dictBits);
    if (!dict) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'module dict missing' })),
      );
      return raiseException(exc);
    }
    dictSetValue(dict, nameBits, val);
    return boxNone();
  },
  get_attr_name: (obj, nameBits) => {
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: attribute name must be str');
    }
    return getAttrValue(obj, name);
  },
  get_attr_name_default: (obj, nameBits, defaultVal) => {
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: attribute name must be str');
    }
    const val = lookupAttr(obj, name);
    return val === undefined ? defaultVal : val;
  },
  has_attr_name: (obj, nameBits) => {
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: attribute name must be str');
    }
    return boxBool(lookupAttr(obj, name) !== undefined);
  },
  set_attr_name: (obj, nameBits, val) => {
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: attribute name must be str');
    }
    return setAttrValue(obj, name, val);
  },
  del_attr_name: (obj, nameBits) => {
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: attribute name must be str');
    }
    return delAttrValue(obj, name);
  },
  is_truthy: (val) => {
    if (isTag(val, TAG_BOOL)) {
      return (val & 1n) === 1n ? 1n : 0n;
    }
    if (isTag(val, TAG_INT)) {
      return unboxInt(val) !== 0n ? 1n : 0n;
    }
    if (isFloat(val)) {
      const num = bitsToFloat(val);
      return num !== 0 ? 1n : 0n;
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
      if (obj && obj.type === 'dict') return obj.entries.length ? 1n : 0n;
      if (obj && (obj.type === 'set' || obj.type === 'frozenset'))
        return obj.items.size ? 1n : 0n;
      if (obj && obj.type === 'complex')
        return obj.re !== 0 || obj.im !== 0 ? 1n : 0n;
      if (obj && obj.type === 'asyncgen') return 1n;
      if (obj && obj.type === 'iter') return 1n;
    }
    return 0n;
  },
  json_parse_scalar: (ptr, _len, out) => {
    expectPtrAddr(ptr, 'json_parse_scalar');
    expectPtrAddr(out, 'json_parse_scalar');
    return 0;
  },
  json_parse_scalar_obj: (bits) => {
    const obj = getObj(bits);
    if (!obj || obj.type !== 'str') return boxNone();
    return boxNone();
  },
  msgpack_parse_scalar: (ptr, _len, out) => {
    expectPtrAddr(ptr, 'msgpack_parse_scalar');
    expectPtrAddr(out, 'msgpack_parse_scalar');
    return 0;
  },
  msgpack_parse_scalar_obj: (bits) => {
    const obj = getObj(bits);
    if (!obj || (obj.type !== 'bytes' && obj.type !== 'bytearray')) return boxNone();
    return boxNone();
  },
  cbor_parse_scalar: (ptr, _len, out) => {
    expectPtrAddr(ptr, 'cbor_parse_scalar');
    expectPtrAddr(out, 'cbor_parse_scalar');
    return 0;
  },
  cbor_parse_scalar_obj: (bits) => {
    const obj = getObj(bits);
    if (!obj || (obj.type !== 'bytes' && obj.type !== 'bytearray')) return boxNone();
    return boxNone();
  },
  string_from_bytes: (ptr, len, out) => {
    if (!memory) return 0;
    const view = new DataView(memory.buffer);
    const addr = expectPtrAddr(ptr, 'string_from_bytes');
    const outAddr = expectPtrAddr(out, 'string_from_bytes');
    if (!addr || !outAddr) return 0;
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const value = Buffer.from(bytes).toString('utf8');
    const boxed = boxPtr({ type: 'str', value });
    view.setBigInt64(outAddr, boxed, true);
    return 0;
  },
  bytes_from_bytes: (ptr, len, out) => {
    if (!memory) return 0;
    const view = new DataView(memory.buffer);
    const addr = expectPtrAddr(ptr, 'bytes_from_bytes');
    const outAddr = expectPtrAddr(out, 'bytes_from_bytes');
    if (!addr || !outAddr) return 0;
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const boxed = boxPtr({ type: 'bytes', data: Uint8Array.from(bytes) });
    view.setBigInt64(outAddr, boxed, true);
    return 0;
  },
  bigint_from_str: (ptr, len) => {
    if (!memory) return boxNone();
    const addr = expectPtrAddr(ptr, 'bigint_from_str');
    if (!addr) return boxNone();
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const text = Buffer.from(bytes).toString('utf8').trim();
    try {
      return boxPtr({ type: 'bigint', value: BigInt(text) });
    } catch {
      return boxNone();
    }
  },
  memoryview_new: (bits) => {
    const view = getMemoryview(bits);
    if (view) {
      const shape = memoryviewShape(view);
      const strides = memoryviewStrides(view);
      return boxPtr({
        type: 'memoryview',
        ownerBits: view.ownerBits,
        offset: view.offset,
        len: shape.length ? shape[0] : 0,
        itemsize: view.itemsize,
        stride: strides.length ? strides[0] : 0,
        readonly: view.readonly,
        formatBits: view.formatBits,
        ndim: shape.length,
        shape: shape.slice(),
        strides: strides.slice(),
      });
    }
    const bytes = getBytes(bits);
    const bytearray = getBytearray(bits);
    if (bytes || bytearray) {
      const data = bytes ? bytes.data : bytearray.data;
      const formatBits = boxPtr({ type: 'str', value: 'B' });
      return boxPtr({
        type: 'memoryview',
        ownerBits: bits,
        offset: 0,
        len: data.length,
        itemsize: 1,
        stride: 1,
        readonly: bytes !== null,
        formatBits,
        ndim: 1,
        shape: [data.length],
        strides: [1],
      });
    }
    throw new Error('TypeError: memoryview expects a bytes-like object');
  },
  memoryview_cast: (viewBits, formatBits, shapeBits, hasShapeBits) => {
    const view = getMemoryview(viewBits);
    if (!view) {
      throw new Error("TypeError: cast() argument 'view' must be a memoryview");
    }
    const formatStr = getStrObj(formatBits);
    if (formatStr === null) {
      throw new Error(
        `TypeError: cast() argument 'format' must be str, not ${typeName(formatBits)}`,
      );
    }
    const fmt = memoryviewFormatFromStr(formatStr);
    if (!fmt) {
      throw new Error(
        "ValueError: memoryview: destination format must be a native single character format prefixed with an optional '@'",
      );
    }
    if (!memoryviewIsCContiguousView(view)) {
      throw new Error('TypeError: memoryview: casts are restricted to C-contiguous views');
    }
    const nbytes = memoryviewNbytes(view);
    if (nbytes === null) return boxNone();
    const hasShape = isTruthyBits(hasShapeBits);
    let shape = [];
    if (hasShape) {
      const list = getList(shapeBits);
      const tuple = getTuple(shapeBits);
      if (!list && !tuple) {
        throw new Error('TypeError: shape must be a list or a tuple');
      }
      const items = list ? list.items : tuple.items;
      for (const elem of items) {
        if (!isTag(elem, TAG_INT)) {
          throw new Error('TypeError: memoryview.cast(): elements of shape must be integers');
        }
        const value = Number(unboxInt(elem));
        if (value <= 0) {
          throw new Error(
            'ValueError: memoryview.cast(): elements of shape must be integers > 0',
          );
        }
        shape.push(value);
      }
    } else {
      if (fmt.itemsize === 0 || nbytes % fmt.itemsize !== 0) {
        throw new Error('TypeError: memoryview: length is not a multiple of itemsize');
      }
      shape = [nbytes / fmt.itemsize];
    }
    const product = memoryviewShapeProduct(shape);
    if (product === null) return boxNone();
    if (Number(product) * fmt.itemsize !== nbytes) {
      throw new Error('TypeError: memoryview: product(shape) * itemsize != buffer size');
    }
    const strides = new Array(shape.length).fill(0);
    let stride = fmt.itemsize;
    for (let idx = shape.length - 1; idx >= 0; idx -= 1) {
      strides[idx] = stride;
      stride *= Math.max(1, shape[idx]);
    }
    const outFormatBits = boxPtr({ type: 'str', value: formatStr });
    return boxPtr({
      type: 'memoryview',
      ownerBits: view.ownerBits,
      offset: view.offset,
      len: shape.length ? shape[0] : 0,
      itemsize: fmt.itemsize,
      stride: strides.length ? strides[0] : 0,
      readonly: view.readonly,
      formatBits: outFormatBits,
      ndim: shape.length,
      shape,
      strides,
    });
  },
  memoryview_tobytes: (bits) => {
    const view = getMemoryview(bits);
    if (!view) {
      throw new Error('TypeError: tobytes expects a memoryview');
    }
    const data = memoryviewCollectBytes(view);
    if (data === null) return boxNone();
    return boxPtr({ type: 'bytes', data: Uint8Array.from(data) });
  },
  str_from_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      return boxPtr({ type: 'str', value: unboxInt(val).toString() });
    }
    if (isFloat(val)) {
      return boxPtr({ type: 'str', value: formatFloat(bitsToFloat(val)) });
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
    if (obj && obj.type === 'exception') {
      const argsBits = obj.argsBits || boxNone();
      const msgBits = exceptionMessageFromArgs(argsBits);
      obj.msgBits = msgBits;
      return msgBits;
    }
    if (obj && obj.type === 'bigint') {
      return boxPtr({ type: 'str', value: obj.value.toString() });
    }
    if (obj && obj.type === 'complex') {
      return boxPtr({ type: 'str', value: formatComplex(obj.re, obj.im) });
    }
    const strAttr = lookupAttr(val, '__str__');
    if (strAttr !== undefined) {
      let res;
      try {
        res = callCallable0(strAttr);
      } catch (err) {
        if (exceptionPending() !== 0n) {
          return boxPtr({ type: 'str', value: '<object>' });
        }
        throw err;
      }
      if (exceptionPending() !== 0n) {
        return boxPtr({ type: 'str', value: '<object>' });
      }
      const strVal = getStrObj(res);
      if (strVal !== null) {
        return res;
      }
    }
    if (
      obj &&
      (obj.type === 'list' ||
        obj.type === 'tuple' ||
        obj.type === 'dict' ||
        obj.type === 'set' ||
        obj.type === 'frozenset')
    ) {
      return boxPtr({ type: 'str', value: reprStringFromBits(val) });
    }
    return boxPtr({ type: 'str', value: '<obj>' });
  },
  repr_from_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      return boxPtr({ type: 'str', value: unboxInt(val).toString() });
    }
    if (isFloat(val)) {
      return boxPtr({ type: 'str', value: formatFloat(bitsToFloat(val)) });
    }
    if (isTag(val, TAG_BOOL)) {
      return boxPtr({ type: 'str', value: (val & 1n) === 1n ? 'True' : 'False' });
    }
    if (isTag(val, TAG_NONE)) {
      return boxPtr({ type: 'str', value: 'None' });
    }
    const obj = getObj(val);
    if (obj && obj.type === 'str') {
      return boxPtr({ type: 'str', value: formatStringRepr(obj.value) });
    }
    if (obj && obj.type === 'exception') {
      const kind = getStr(obj.kindBits) || 'Exception';
      const argsBits = obj.argsBits || boxNone();
      const rendered = exceptionReprFromArgs(kind, argsBits);
      return boxPtr({ type: 'str', value: rendered });
    }
    if (obj && obj.type === 'bigint') {
      return boxPtr({ type: 'str', value: obj.value.toString() });
    }
    if (obj && obj.type === 'complex') {
      return boxPtr({ type: 'str', value: formatComplex(obj.re, obj.im) });
    }
    return boxPtr({ type: 'str', value: '<obj>' });
  },
  ascii_from_obj: (val) => {
    if (isTag(val, TAG_INT)) {
      return boxPtr({ type: 'str', value: unboxInt(val).toString() });
    }
    if (isFloat(val)) {
      return boxPtr({ type: 'str', value: formatFloat(bitsToFloat(val)) });
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
    if (obj && obj.type === 'bigint') {
      return boxPtr({ type: 'str', value: obj.value.toString() });
    }
    return boxPtr({ type: 'str', value: '<obj>' });
  },
  bin_builtin: (val) => formatIntBase(val, 2, '0b'),
  oct_builtin: (val) => formatIntBase(val, 8, '0o'),
  hex_builtin: (val) => formatIntBase(val, 16, '0x'),
  int_from_obj: (val, baseBits, hasBase) => {
    const hasB = isTag(hasBase, TAG_BOOL) && (hasBase & 1n) === 1n;
    const parseLiteral = (text, base) => {
      let trimmed = text.trim();
      if (!trimmed) throw new Error('ValueError: invalid literal for int()');
      let sign = 1n;
      if (trimmed.startsWith('+')) {
        trimmed = trimmed.slice(1);
      } else if (trimmed.startsWith('-')) {
        sign = -1n;
        trimmed = trimmed.slice(1);
      }
      let baseVal = base;
      if (baseVal === 0) {
        if (trimmed.startsWith('0x') || trimmed.startsWith('0X')) {
          baseVal = 16;
          trimmed = trimmed.slice(2);
        } else if (trimmed.startsWith('0o') || trimmed.startsWith('0O')) {
          baseVal = 8;
          trimmed = trimmed.slice(2);
        } else if (trimmed.startsWith('0b') || trimmed.startsWith('0B')) {
          baseVal = 2;
          trimmed = trimmed.slice(2);
        } else {
          baseVal = 10;
        }
      } else if (baseVal === 16 && (trimmed.startsWith('0x') || trimmed.startsWith('0X'))) {
        trimmed = trimmed.slice(2);
      } else if (baseVal === 8 && (trimmed.startsWith('0o') || trimmed.startsWith('0O'))) {
        trimmed = trimmed.slice(2);
      } else if (baseVal === 2 && (trimmed.startsWith('0b') || trimmed.startsWith('0B'))) {
        trimmed = trimmed.slice(2);
      }
      trimmed = trimmed.replace(/_/g, '');
      if (!trimmed) throw new Error('ValueError: invalid literal for int()');
      const digits = '0123456789abcdefghijklmnopqrstuvwxyz';
      let acc = 0n;
      const baseBig = BigInt(baseVal);
      const lower = trimmed.toLowerCase();
      for (const ch of lower) {
        const val = digits.indexOf(ch);
        if (val < 0 || val >= baseVal) {
          throw new Error('ValueError: invalid literal for int()');
        }
        acc = acc * baseBig + BigInt(val);
      }
      return acc * sign;
    };
    let baseVal = 10;
    if (hasB) {
      if (!isIntLike(baseBits)) {
        throw new Error('TypeError: int() base must be int');
      }
      baseVal = Number(unboxIntLike(baseBits));
      if (baseVal !== 0 && (baseVal < 2 || baseVal > 36)) {
        throw new Error('ValueError: base must be 0 or between 2 and 36');
      }
    }
    if (hasB) {
      const obj = getObj(val);
      if (!obj || (obj.type !== 'str' && obj.type !== 'bytes' && obj.type !== 'bytearray')) {
        throw new Error('TypeError: int() can\\'t convert non-string with explicit base');
      }
    }
    if (!hasB) {
      if (getComplex(val)) {
        throw new Error(
          `TypeError: int() argument must be a string, a bytes-like object or a real number, not '${typeName(
            val,
          )}'`,
        );
      }
      if (isIntLike(val)) return boxInt(unboxIntLike(val));
      if (isFloat(val)) {
        const num = bitsToFloat(val);
        if (Number.isNaN(num)) {
          throw new Error('ValueError: cannot convert float NaN to integer');
        }
        if (!Number.isFinite(num)) {
          throw new Error('OverflowError: cannot convert float infinity to integer');
        }
        return boxInt(BigInt(Math.trunc(num)));
      }
    }
    const obj = getObj(val);
    if (obj && (obj.type === 'str' || obj.type === 'bytes' || obj.type === 'bytearray')) {
      const text =
        obj.type === 'str' ? obj.value : Buffer.from(obj.data).toString('utf8');
      const num = parseLiteral(text, hasB ? baseVal : 10);
      return boxInt(num);
    }
    if (!hasB) {
      const intAttr = lookupAttr(val, '__int__');
      if (intAttr !== undefined) {
        const res = callCallable0(intAttr);
        if (!isIntLike(res)) {
          throw new Error(`TypeError: __int__ returned non-int (type ${typeName(res)})`);
        }
        return boxInt(unboxIntLike(res));
      }
      const indexAttr = lookupAttr(val, '__index__');
      if (indexAttr !== undefined) {
        const res = callCallable0(indexAttr);
        if (!isIntLike(res)) {
          throw new Error(`TypeError: __index__ returned non-int (type ${typeName(res)})`);
        }
        return boxInt(unboxIntLike(res));
      }
    }
    if (hasB) {
      throw new Error('ValueError: invalid literal for int()');
    }
    throw new Error('TypeError: int() argument must be a string or a number');
  },
  float_from_obj: (val) => {
    if (isFloat(val)) return val;
    if (isIntLike(val)) return boxFloat(Number(unboxIntLike(val)));
    if (getComplex(val)) {
      throw new Error(
        `TypeError: float() argument must be a string or a real number, not '${typeName(
          val,
        )}'`,
      );
    }
    const obj = getObj(val);
    if (obj && obj.type === 'str') {
      const text = obj.value.trim();
      const lowered = text.toLowerCase();
      if (lowered === 'nan' || lowered === '+nan' || lowered === '-nan') {
        return boxFloat(NaN);
      }
      if (
        lowered === 'inf' ||
        lowered === '+inf' ||
        lowered === 'infinity' ||
        lowered === '+infinity'
      ) {
        return boxFloat(Infinity);
      }
      if (lowered === '-inf' || lowered === '-infinity') {
        return boxFloat(-Infinity);
      }
      const parsed = Number(text);
      if (!Number.isNaN(parsed)) {
        return boxFloat(parsed);
      }
      throw new Error(`ValueError: could not convert string to float: '${obj.value}'`);
    }
    if (obj && (obj.type === 'bytes' || obj.type === 'bytearray')) {
      const bytes = Buffer.from(obj.data);
      const text = bytes.toString('utf8').trim();
      const lowered = text.toLowerCase();
      if (lowered === 'nan' || lowered === '+nan' || lowered === '-nan') {
        return boxFloat(NaN);
      }
      if (
        lowered === 'inf' ||
        lowered === '+inf' ||
        lowered === 'infinity' ||
        lowered === '+infinity'
      ) {
        return boxFloat(Infinity);
      }
      if (lowered === '-inf' || lowered === '-infinity') {
        return boxFloat(-Infinity);
      }
      const parsed = Number(text);
      if (!Number.isNaN(parsed)) {
        return boxFloat(parsed);
      }
      throw new Error(
        `ValueError: could not convert string to float: '${bytes.toString('utf8')}'`,
      );
    }
    if (isPtr(val)) {
      const floatAttr = lookupAttr(val, '__float__');
      if (floatAttr !== undefined) {
        const res = callCallable0(floatAttr);
        if (!isFloat(res)) {
          throw new Error(
            `TypeError: ${typeName(val)}.__float__ returned non-float (type ${typeName(
              res,
            )})`,
          );
        }
        return res;
      }
      const indexAttr = lookupAttr(val, '__index__');
      if (indexAttr !== undefined) {
        const res = callCallable0(indexAttr);
        if (!isIntLike(res)) {
          throw new Error(
            `TypeError: __index__ returned non-int (type ${typeName(res)})`,
          );
        }
        return boxFloat(Number(unboxIntLike(res)));
      }
    }
    throw new Error('TypeError: float() argument must be a string or a number');
  },
  complex_from_obj: (val, imagBits, hasImagBits) => {
    const hasImag = isTruthyBits(hasImagBits);
    if (!hasImag) {
      const obj = getObj(val);
      if (obj && obj.type === 'complex') {
        return val;
      }
      if (isFloat(val)) return boxComplex(bitsToFloat(val), 0);
      if (isIntLike(val)) return boxComplex(Number(unboxIntLike(val)), 0);
      if (obj && obj.type === 'bigint') {
        const num = Number(obj.value);
        if (!Number.isFinite(num)) {
          throw new Error('OverflowError: int too large to convert to float');
        }
        return boxComplex(num, 0);
      }
      if (obj && obj.type === 'str') {
        const parsed = parseComplexFromString(obj.value);
        if (!parsed) {
          throw new Error('ValueError: complex() arg is a malformed string');
        }
        return boxComplex(parsed.re, parsed.im);
      }
      if (obj && (obj.type === 'bytes' || obj.type === 'bytearray')) {
        throw new Error(
          `TypeError: complex() argument must be a string or a number, not ${typeName(
            val,
          )}`,
        );
      }
      const complexAttr = lookupAttr(val, '__complex__');
      if (complexAttr !== undefined) {
        const res = callCallable0(complexAttr);
        const resObj = getComplex(res);
        if (resObj) return res;
        throw new Error(
          `TypeError: ${typeName(val)}.__complex__ returned non-complex (type ${typeName(
            res,
          )})`,
        );
      }
      const floatAttr = lookupAttr(val, '__float__');
      if (floatAttr !== undefined) {
        const res = callCallable0(floatAttr);
        if (!isFloat(res)) {
          throw new Error(
            `TypeError: ${typeName(val)}.__float__ returned non-float (type ${typeName(
              res,
            )})`,
          );
        }
        return boxComplex(bitsToFloat(res), 0);
      }
      const indexAttr = lookupAttr(val, '__index__');
      if (indexAttr !== undefined) {
        const res = callCallable0(indexAttr);
        if (!isIntLike(res)) {
          throw new Error(`TypeError: __index__ returned non-int (type ${typeName(res)})`);
        }
        return boxComplex(Number(unboxIntLike(res)), 0);
      }
      throw new Error('TypeError: complex() argument must be a string or a number');
    }
    const realObj = getObj(val);
    if (
      realObj &&
      (realObj.type === 'str' ||
        realObj.type === 'bytes' ||
        realObj.type === 'bytearray')
    ) {
      throw new Error(
        `TypeError: complex() argument 'real' must be a real number, not ${typeName(
          val,
        )}`,
      );
    }
    const imagObj = getObj(imagBits);
    if (
      imagObj &&
      (imagObj.type === 'str' ||
        imagObj.type === 'bytes' ||
        imagObj.type === 'bytearray')
    ) {
      throw new Error(
        `TypeError: complex() argument 'imag' must be a real number, not ${typeName(
          imagBits,
        )}`,
      );
    }
    const real = complexFromValStrict(val);
    if (!real) {
      throw new Error(
        `TypeError: complex() argument 'real' must be a real number, not ${typeName(
          val,
        )}`,
      );
    }
    if (real.overflow) {
      throw new Error('OverflowError: int too large to convert to float');
    }
    const imag = complexFromValStrict(imagBits);
    if (!imag) {
      throw new Error(
        `TypeError: complex() argument 'imag' must be a real number, not ${typeName(
          imagBits,
        )}`,
      );
    }
    if (imag.overflow) {
      throw new Error('OverflowError: int too large to convert to float');
    }
    const re = real.re - imag.im;
    const im = real.im + imag.re;
    return boxComplex(re, im);
  },
  len: (val) => {
    const list = getList(val);
    if (list) return boxInt(list.items.length);
    const tup = getTuple(val);
    if (tup) return boxInt(tup.items.length);
    const setLike = getSetLike(val);
    if (setLike) return boxInt(BigInt(setLike.items.size));
    const dict = getDict(val);
    if (dict) return boxInt(dict.entries.length);
    const dictView = getDictKeysView(val) || getDictValuesView(val) || getDictItemsView(val);
    if (dictView) {
      const target = getDict(dictView.dictBits);
      if (target) return boxInt(target.entries.length);
    }
    const bytes = getBytes(val);
    if (bytes) return boxInt(bytes.data.length);
    const bytearray = getBytearray(val);
    if (bytearray) return boxInt(bytearray.data.length);
    const view = getMemoryview(val);
    if (view) {
      const shape = memoryviewShape(view);
      if (shape.length === 0) {
        throw new Error('TypeError: 0-dim memory has no length');
      }
      return boxInt(shape[0]);
    }
    const strVal = getStrObj(val);
    if (strVal !== null) return boxInt(Array.from(strVal).length);
    return boxInt(0);
  },
  slice: () => boxNone(),
  slice_new: (start, stop, step) =>
    boxPtr({ type: 'slice', start, stop, step }),
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
  list_append: (listBits, valBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    list.items.push(valBits);
    return boxNone();
  },
  list_pop: (listBits, indexBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const missing = missingSentinel();
    const len = list.items.length;
    const lenBig = BigInt(len);
    let idxBig;
    if (indexBits === missing || isNone(indexBits)) {
      idxBig = lenBig - 1n;
    } else {
      idxBig = indexBigIntFromBits(indexBits, LIST_INDEX_ERR);
      if (idxBig === null) return boxNone();
    }
    if (idxBig < 0n) idxBig += lenBig;
    if (idxBig < 0n || idxBig >= lenBig) {
      throw new Error('IndexError: pop index out of range');
    }
    return list.items.splice(Number(idxBig), 1)[0] ?? boxNone();
  },
  list_extend: (listBits, otherBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const otherList = getList(otherBits);
    const otherTuple = getTuple(otherBits);
    if (otherList) {
      const snapshot = otherList === list ? [...list.items] : otherList.items;
      list.items.push(...snapshot);
      return boxNone();
    }
    if (otherTuple) {
      list.items.push(...otherTuple.items);
      return boxNone();
    }
    const iterBits = baseImports.iter(otherBits);
    if (isNone(iterBits)) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const tuple = getTuple(pairBits);
      if (!tuple || tuple.items.length < 2) {
        throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      list.items.push(tuple.items[0]);
    }
    return boxNone();
  },
  list_insert: (listBits, indexBits, valBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    if (!isIntLike(indexBits)) return boxNone();
    let idx = Number(unboxIntLike(indexBits));
    if (idx < 0) idx += list.items.length;
    if (idx < 0) idx = 0;
    if (idx > list.items.length) idx = list.items.length;
    list.items.splice(idx, 0, valBits);
    return boxNone();
  },
  list_remove: (listBits, valBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const idx = list.items.findIndex((item) => item === valBits);
    if (idx >= 0) list.items.splice(idx, 1);
    return boxNone();
  },
  list_clear: (listBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    list.items.length = 0;
    return boxNone();
  },
  list_copy: (listBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    return listFromArray([...list.items]);
  },
  list_reverse: (listBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    list.items.reverse();
    return boxNone();
  },
  list_sort: (listBits, keyBits, reverseBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const useKey = !isNone(keyBits);
    const reverse = isTruthyBits(reverseBits);
    const items = [];
    for (const valBits of list.items) {
      const keyVal = useKey ? callCallable1(keyBits, valBits) : valBits;
      if (useKey && exceptionPending() !== 0n) return boxNone();
      items.push({ key: keyVal, val: valBits, idx: items.length });
    }
    let error = null;
    items.sort((left, right) => {
      if (error) return 0;
      const outcome = compareObjects(left.key, right.key);
      if (outcome.kind === 'ordered') {
        if (outcome.ordering !== 0) {
          return reverse ? -outcome.ordering : outcome.ordering;
        }
      } else if (outcome.kind === 'notComparable') {
        error = { kind: 'notComparable', left: left.key, right: right.key };
        return 0;
      } else if (outcome.kind === 'error') {
        error = { kind: 'exception' };
        return 0;
      }
      return left.idx - right.idx;
    });
    if (error) {
      if (error.kind === 'exception') return boxNone();
      compareTypeError('<', error.left, error.right);
    }
    list.items = items.map((item) => item.val);
    return boxNone();
  },
  list_count: (listBits, valBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    let count = 0;
    for (const item of list.items) {
      if (item === valBits) count += 1;
    }
    return boxInt(count);
  },
  list_index: (listBits, valBits) => {
    const missing = missingSentinel();
    return baseImports.list_index_range(listBits, valBits, missing, missing);
  },
  list_index_range: (listBits, valBits, startBits, stopBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const len = list.items.length;
    const missing = missingSentinel();
    const listIndexBound = (bits) => {
      if (isNone(bits)) {
        throw new Error(`TypeError: ${LIST_INDEX_ERR}`);
      }
      const idx = indexBigIntFromBits(bits, LIST_INDEX_ERR);
      if (idx === null) return null;
      let value = idx;
      const lenBig = BigInt(len);
      if (value < 0n) value += lenBig;
      if (value < 0n) return 0;
      if (value > lenBig) return len;
      return Number(value);
    };
    const start = startBits === missing ? 0 : listIndexBound(startBits);
    if (start === null) return boxNone();
    const stop = stopBits === missing ? len : listIndexBound(stopBits);
    if (stop === null) return boxNone();
    const startIdx = Math.min(Math.max(start, 0), len);
    const stopIdx = Math.min(Math.max(stop, 0), len);
    if (startIdx < stopIdx) {
      for (let idx = startIdx; idx < stopIdx; idx += 1) {
        if (list.items[idx] === valBits) {
          return boxInt(idx);
        }
      }
    }
    throw new Error('ValueError: list.index(x): x not in list');
  },
  heapq_heapify: (listBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const heap = list.items;
    const len = heap.length;
    if (len < 2) return boxNone();
    for (let idx = Math.floor(len / 2) - 1; idx >= 0; idx -= 1) {
      const ok = heapSiftUp(heap, idx);
      if (ok === null) return boxNone();
    }
    return boxNone();
  },
  heapq_heappush: (listBits, itemBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const heap = list.items;
    heap.push(itemBits);
    const ok = heapSiftDown(heap, 0, heap.length - 1);
    if (ok === null) return boxNone();
    return boxNone();
  },
  heapq_heappop: (listBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const heap = list.items;
    if (heap.length === 0) {
      throw new Error('IndexError: index out of range');
    }
    const last = heap.pop();
    if (heap.length === 0) {
      return last;
    }
    const returnBits = heap[0];
    heap[0] = last;
    const ok = heapSiftUp(heap, 0);
    if (ok === null) return boxNone();
    return returnBits;
  },
  heapq_heapreplace: (listBits, itemBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const heap = list.items;
    if (heap.length === 0) {
      throw new Error('IndexError: index out of range');
    }
    const returnBits = heap[0];
    heap[0] = itemBits;
    const ok = heapSiftUp(heap, 0);
    if (ok === null) return boxNone();
    return returnBits;
  },
  heapq_heappushpop: (listBits, itemBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    const heap = list.items;
    if (heap.length !== 0) {
      const lt = heapLt(heap[0], itemBits);
      if (lt === null) return boxNone();
      if (lt) {
        const returnBits = heap[0];
        heap[0] = itemBits;
        const ok = heapSiftUp(heap, 0);
        if (ok === null) return boxNone();
        return returnBits;
      }
      return itemBits;
    }
    return itemBits;
  },
  tuple_from_list: (val) => {
    const list = getList(val);
    if (list) return tupleFromArray([...list.items]);
    const tup = getTuple(val);
    if (tup) return val;
    return boxNone();
  },
  dict_new: () => boxPtr({ type: 'dict', entries: [], lookup: new Map() }),
  dict_from_obj: (val) => {
    const dict = getDict(val);
    if (dict) return val;
    return boxNone();
  },
  dict_set: (dictBits, keyBits, valBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    dictSetValue(dict, keyBits, valBits);
    return dictBits;
  },
  dict_get: (dictBits, keyBits, defaultBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const val = dictGetValue(dict, keyBits);
    return val === null ? defaultBits : val;
  },
  dict_pop: (dictBits, keyBits, defaultBits, hasDefaultBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const val = dictGetValue(dict, keyBits);
    if (val === null) {
      const hasDefault = isTruthyBits(hasDefaultBits);
      if (hasDefault) return defaultBits;
      throw new Error(`KeyError: ${reprStringFromBits(keyBits)}`);
    }
    dictDelete(dict, keyBits);
    return val;
  },
  dict_popitem: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    if (dict.entries.length === 0) {
      throw new Error('KeyError: popitem(): dictionary is empty');
    }
    const entry = dict.entries.pop();
    dict.lookup = new Map();
    for (let i = 0; i < dict.entries.length; i++) {
      dict.lookup.set(dictKey(dict.entries[i][0]), i);
    }
    if (!entry) return boxNone();
    return tupleFromArray([entry[0], entry[1]]);
  },
  dict_setdefault: (dictBits, keyBits, defaultBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const val = dictGetValue(dict, keyBits);
    if (val === null) {
      dictSetValue(dict, keyBits, defaultBits);
      return defaultBits;
    }
    return val;
  },
  dict_update: (dictBits, otherBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const other = getDict(otherBits);
    if (!other) return boxNone();
    for (const [keyBits, valBits] of other.entries) {
      dictSetValue(dict, keyBits, valBits);
    }
    return boxNone();
  },
  dict_update_kwstar: (dictBits, otherBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const other = getDict(otherBits);
    if (!other) {
      throw new Error('TypeError: argument after ** must be a mapping');
    }
    for (const [keyBits, valBits] of other.entries) {
      const keyStr = getStrObj(keyBits);
      if (keyStr === null) {
        throw new Error('TypeError: keywords must be strings');
      }
      dictSetValue(dict, keyBits, valBits);
    }
    return boxNone();
  },
  dict_keys: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    return boxPtr({ type: 'dict_keys', dictBits });
  },
  dict_values: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    return boxPtr({ type: 'dict_values', dictBits });
  },
  dict_items: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    return boxPtr({ type: 'dict_items', dictBits });
  },
  dict_copy: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const out = { type: 'dict', entries: [], lookup: new Map() };
    for (const [keyBits, valBits] of dict.entries) {
      dictSetValue(out, keyBits, valBits);
    }
    return boxPtr(out);
  },
  dict_clear: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    dict.entries = [];
    dict.lookup = new Map();
    return boxNone();
  },
  set_new: () => boxPtr({ type: 'set', items: new Set() }),
  set_add: (setBits, keyBits) => {
    const set = getSet(setBits);
    if (set) {
      set.items.add(keyBits);
    }
    return boxNone();
  },
  frozenset_new: () => boxPtr({ type: 'frozenset', items: new Set() }),
  frozenset_add: (setBits, keyBits) => {
    const set = getFrozenSet(setBits);
    if (set) {
      set.items.add(keyBits);
    }
    return boxNone();
  },
  set_discard: (setBits, keyBits) => {
    const set = getSet(setBits);
    if (set) {
      set.items.delete(keyBits);
    }
    return boxNone();
  },
  set_remove: (setBits, keyBits) => {
    const set = getSet(setBits);
    if (set && set.items.has(keyBits)) {
      set.items.delete(keyBits);
      return boxNone();
    }
    throw new Error('KeyError: set.remove(x): x not in set');
  },
  set_pop: (setBits) => {
    const set = getSet(setBits);
    if (!set || !set.items.size) {
      throw new Error('KeyError: pop from an empty set');
    }
    const iter = set.items.values();
    const value = iter.next().value;
    set.items.delete(value);
    return value;
  },
  set_update: (setBits, otherBits) => {
    const set = getSet(setBits);
    const other = getSet(otherBits) || getFrozenSet(otherBits);
    if (set && other) {
      for (const item of other.items) {
        set.items.add(item);
      }
      return boxNone();
    }
    if (!set) return boxNone();
    const iterBits = baseImports.iter(otherBits);
    if (isNone(iterBits)) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const tuple = getTuple(pairBits);
      if (!tuple || tuple.items.length < 2) {
        throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      set.items.add(tuple.items[0]);
    }
    return boxNone();
  },
  set_intersection_update: (setBits, otherBits) => {
    const set = getSet(setBits);
    const other = getSet(otherBits) || getFrozenSet(otherBits);
    if (set && other) {
      for (const item of [...set.items]) {
        if (!other.items.has(item)) {
          set.items.delete(item);
        }
      }
      return boxNone();
    }
    if (!set) return boxNone();
    const iterBits = baseImports.iter(otherBits);
    if (isNone(iterBits)) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    const otherItems = new Set();
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const tuple = getTuple(pairBits);
      if (!tuple || tuple.items.length < 2) {
        throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      otherItems.add(tuple.items[0]);
    }
    for (const item of [...set.items]) {
      if (!otherItems.has(item)) {
        set.items.delete(item);
      }
    }
    return boxNone();
  },
  set_difference_update: (setBits, otherBits) => {
    const set = getSet(setBits);
    const other = getSet(otherBits) || getFrozenSet(otherBits);
    if (set && other) {
      for (const item of other.items) {
        set.items.delete(item);
      }
      return boxNone();
    }
    if (!set) return boxNone();
    const iterBits = baseImports.iter(otherBits);
    if (isNone(iterBits)) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const tuple = getTuple(pairBits);
      if (!tuple || tuple.items.length < 2) {
        throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      set.items.delete(tuple.items[0]);
    }
    return boxNone();
  },
  set_symdiff_update: (setBits, otherBits) => {
    const set = getSet(setBits);
    const other = getSet(otherBits) || getFrozenSet(otherBits);
    if (set && other) {
      const leftItems = [...set.items];
      const rightItems = [...other.items];
      const leftLookup = new Set(leftItems);
      const newItems = new Set();
      for (const item of leftItems) {
        if (!other.items.has(item)) {
          newItems.add(item);
        }
      }
      for (const item of rightItems) {
        if (!leftLookup.has(item)) {
          newItems.add(item);
        }
      }
      set.items = newItems;
      return boxNone();
    }
    if (!set) return boxNone();
    const iterBits = baseImports.iter(otherBits);
    if (isNone(iterBits)) {
      throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
    }
    const otherItems = new Set();
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const tuple = getTuple(pairBits);
      if (!tuple || tuple.items.length < 2) {
        throw new Error(`TypeError: '${typeName(otherBits)}' object is not iterable`);
      }
      const doneBits = tuple.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      otherItems.add(tuple.items[0]);
    }
    const leftItems = [...set.items];
    const leftLookup = new Set(leftItems);
    const newItems = new Set();
    for (const item of leftItems) {
      if (!otherItems.has(item)) {
        newItems.add(item);
      }
    }
    for (const item of otherItems) {
      if (!leftLookup.has(item)) {
        newItems.add(item);
      }
    }
    set.items = newItems;
    return boxNone();
  },
  tuple_count: (tupleBits, valBits) => {
    const tuple = getTuple(tupleBits);
    if (!tuple) return boxNone();
    let count = 0;
    for (const item of tuple.items) {
      if (item === valBits) count += 1;
    }
    return boxInt(count);
  },
  tuple_index: (tupleBits, valBits) => {
    const tuple = getTuple(tupleBits);
    if (!tuple) return boxNone();
    for (let i = 0; i < tuple.items.length; i += 1) {
      if (tuple.items[i] === valBits) {
        return boxInt(i);
      }
    }
    throw new Error('ValueError: tuple.index(x): x not in tuple');
  },
  iter: (val) => {
    if (isGenerator(val)) {
      return val;
    }
    const enumObj = getEnumerate(val);
    if (enumObj) {
      return val;
    }
    if (
      getCallIter(val) ||
      getReversed(val) ||
      getZipIter(val) ||
      getMapIter(val) ||
      getFilterIter(val)
    ) {
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
    const setLike = getSetLike(val);
    if (setLike) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    const dict = getDict(val);
    if (dict) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    if (getDictKeysView(val) || getDictValuesView(val) || getDictItemsView(val)) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    const bytes = getBytes(val);
    if (bytes) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    const bytearray = getBytearray(val);
    if (bytearray) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    if (getStrObj(val) !== null) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: `'${typeName(val)}' object is not iterable`,
        }),
      ),
    );
    return raiseException(exc);
  },
  iter_sentinel: (callableBits, sentinelBits) => {
    if (!isTruthyBits(baseImports.is_callable(callableBits))) {
      throw new Error('TypeError: iter(v, w): v must be callable');
    }
    return boxPtr({ type: 'call_iter', callable: callableBits, sentinel: sentinelBits });
  },
  enumerate: (iterable, startBits, hasStartBits) => {
    let start = 0n;
    if (isTruthyBits(hasStartBits)) {
      if (isIntLike(startBits)) {
        start = unboxIntLike(startBits);
      } else {
        const indexAttr = lookupAttr(startBits, '__index__');
        if (indexAttr !== undefined) {
          const res = callCallable0(indexAttr);
          if (!isIntLike(res)) {
            throw new Error(
              `TypeError: __index__ returned non-int (type ${typeName(res)})`,
            );
          }
          start = unboxIntLike(res);
        } else {
          throw new Error('TypeError: enumerate() start must be an integer');
        }
      }
    }
    const iterBits = baseImports.iter(iterable);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
    }
    return boxPtr({ type: 'enumerate', iterBits, index: start });
  },
  aiter: (val) => {
    const iterObj = val;
    let attr = lookupAttr(iterObj, '__aiter__');
    if (attr === undefined) {
      throw new Error('TypeError: object is not async iterable');
    }
    if (getFunction(attr)) {
      attr = makeBoundMethod(attr, iterObj);
    }
    return callCallable0(attr);
  },
  iter_next: (val) => {
    const enumObj = getEnumerate(val);
    if (enumObj) {
      const next = iterNextInternal(enumObj.iterBits);
      const nextTuple = getTuple(next);
      if (!nextTuple || nextTuple.items.length < 2) return next;
      const done = nextTuple.items[1];
      if (isTag(done, TAG_BOOL) && (done & 1n) === 1n) {
        return next;
      }
      const indexBits = boxInt(enumObj.index);
      enumObj.index += 1n;
      const pair = tupleFromArray([indexBits, nextTuple.items[0]]);
      return tupleFromArray([pair, boxBool(false)]);
    }
    return iterNextInternal(val);
  },
  anext: (val) => {
    const iterObj = val;
    let attr = lookupAttr(iterObj, '__anext__');
    if (attr === undefined) {
      const addr = isPtr(iterObj) ? ptrAddr(iterObj) : -1;
      const hasClass = isPtr(iterObj) && instanceClasses.has(addr);
      throw new Error(
        `TypeError: object is not an async iterator (got ${typeName(val)}, ` +
          `addr=${addr}, hasClass=${hasClass})`,
      );
    }
    if (getFunction(attr)) {
      attr = makeBoundMethod(attr, iterObj);
    }
    return callCallable0(attr);
  },
  task_new: (pollFn, closureSize, kind) => {
    if (kind === TASK_KIND_GENERATOR) {
      return baseImports.generator_new(pollFn, closureSize);
    }
    if (kind !== TASK_KIND_FUTURE) {
      throw new Error(`TypeError: unknown task kind ${kind}`);
    }
    const size = Number(closureSize);
    const addr = allocRaw(size);
    if (!addr || !memory) return boxNone();
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr - HEADER_POLL_FN_OFFSET, pollFn, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
    const slots = Math.floor(size / 8);
    for (let i = 0; i < slots; i += 1) {
      view.setBigInt64(addr + i * 8, boxNone(), true);
    }
    return boxPtrAddr(addr);
  },
  task_register_token_owned: (taskBits, tokenBits) => {
    if (!isPtr(taskBits) || heap.has(taskBits & POINTER_MASK)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'task must be awaitable' })),
      );
      raiseException(exc);
      return boxNone();
    }
    const addr = ptrAddr(taskBits);
    const tokenId = tokenIdFromBits(tokenBits);
    registerTaskToken(addr, tokenId === 0n ? 1n : tokenId);
    return boxNone();
  },
  generator_new: (pollFn, closureSize) => {
    const size = Number(closureSize);
    const addr = allocRaw(size);
    if (!addr || !memory) return boxNone();
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr - HEADER_POLL_FN_OFFSET, pollFn, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
    const slots = Math.floor(size / 8);
    for (let i = 0; i < slots; i += 1) {
      view.setBigInt64(addr + i * 8, boxNone(), true);
    }
    if (size >= GEN_CONTROL_SIZE) {
      view.setBigInt64(addr + GEN_SEND_OFFSET, boxNone(), true);
      view.setBigInt64(addr + GEN_THROW_OFFSET, boxNone(), true);
      view.setBigInt64(addr + GEN_CLOSED_OFFSET, boxBool(false), true);
      view.setBigInt64(addr + GEN_EXC_DEPTH_OFFSET, boxInt(1), true);
      view.setBigInt64(addr + GEN_FRAME_OFFSET, frameNew(-1), true);
      view.setBigInt64(addr + GEN_YIELD_FROM_OFFSET, boxNone(), true);
    }
    return boxPtrAddr(addr);
  },
  asyncgen_new: (genBits) => {
    if (!isGenerator(genBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'expected generator' })),
      );
      return raiseException(exc);
    }
    const objBits = boxPtr({
      type: 'asyncgen',
      genBits,
      runningBits: boxNone(),
    });
    asyncgenRegistry.add(objBits);
    return objBits;
  },
  asyncgen_shutdown: () => {
    const gens = Array.from(asyncgenRegistry);
    asyncgenRegistry.clear();
    for (const genBits of gens) {
      const asyncgenObj = getAsyncGenerator(genBits);
      if (!asyncgenObj) continue;
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'GeneratorExit' }),
        boxNone(),
      );
      const futureBits = asyncgenFutureNew(
        genBits,
        ASYNCGEN_OP_ACLOSE,
        exc,
      );
      if (!isNone(futureBits)) {
        baseImports.block_on(futureBits);
        if (exceptionPending() !== 0n) {
          exceptionClear();
        }
      }
    }
    return boxNone();
  },
  asyncgen_poll: (taskPtr) => asyncgenPoll(taskPtr),
  generator_send: (gen, sendVal) => generatorSend(gen, sendVal),
  generator_throw: (gen, exc) => generatorThrow(gen, exc),
  generator_close: (gen) => generatorClose(gen),
  is_generator: (val) => boxBool(isGenerator(val)),
  is_bound_method: (val) => boxBool(isBoundMethod(val)),
  function_is_generator: (val) => {
    const attr = lookupAttr(val, '__molt_is_generator__');
    if (attr === undefined) return boxBool(false);
    return boxBool(isTruthyBits(attr));
  },
  function_is_coroutine: (val) => {
    const attr = lookupAttr(val, '__molt_is_coroutine__');
    if (attr === undefined) return boxBool(false);
    return boxBool(isTruthyBits(attr));
  },
  function_default_kind: (val) => {
    const func = getFunction(val);
    return func && typeof func.defaultKind === 'number'
      ? BigInt(func.defaultKind)
      : 0n;
  },
  function_closure_bits: (val) => {
    const func = getFunction(val);
    return func && func.closure ? func.closure : 0n;
  },
  call_arity_error: (expected, got) => {
    const name = lastBuiltinName ? ` for ${lastBuiltinName}` : '';
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'TypeError' }),
      exceptionArgs(
        boxPtr({
          type: 'str',
          value: `call arity mismatch (expected ${expected}, got ${got})${name}`,
        }),
      ),
    );
    return raiseException(exc);
  },
  callargs_new: (_posCap, _kwCap) =>
    boxPtr({ type: 'callargs', pos: [], kwNames: [], kwValues: [] }),
  callargs_push_pos: (builder, val) => {
    const args = getCallArgs(builder);
    if (!args) return boxNone();
    args.pos.push(val);
    return boxNone();
  },
  callargs_push_kw: (builder, nameBits, valBits) => {
    const args = getCallArgs(builder);
    if (!args) return boxNone();
    const name = getStrObj(nameBits);
    if (name === null) {
      throw new Error('TypeError: keywords must be strings');
    }
    for (const existing of args.kwNames) {
      const existingName = getStrObj(existing);
      if (existingName === name) {
        throw new Error(`TypeError: got multiple values for keyword argument '${name}'`);
      }
    }
    args.kwNames.push(nameBits);
    args.kwValues.push(valBits);
    return boxNone();
  },
  callargs_expand_star: (builder, iterable) => {
    const args = getCallArgs(builder);
    if (!args) return boxNone();
    const iterBits = baseImports.iter(iterable);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
    }
    while (true) {
      const pair = iterNextInternal(iterBits);
      const tuple = getTuple(pair);
      if (!tuple || tuple.items.length < 2) return boxNone();
      const done = tuple.items[1];
      if (isTruthyBits(done)) break;
      args.pos.push(tuple.items[0]);
    }
    return boxNone();
  },
  callargs_expand_kwstar: (builder, mapping) => {
    const args = getCallArgs(builder);
    if (!args) return boxNone();
    const dict = getDict(mapping);
    if (!dict) {
      throw new Error('TypeError: argument after ** must be a dict');
    }
    for (const entry of dict.entries) {
      const nameBits = entry[0];
      const valBits = entry[1];
      const name = getStrObj(nameBits);
      if (name === null) {
        throw new Error('TypeError: keywords must be strings');
      }
      for (const existing of args.kwNames) {
        const existingName = getStrObj(existing);
        if (existingName === name) {
          throw new Error(`TypeError: got multiple values for keyword argument '${name}'`);
        }
      }
      args.kwNames.push(nameBits);
      args.kwValues.push(valBits);
    }
    return boxNone();
  },
  call_bind: (callBits, builderBits) => {
    const args = getCallArgs(builderBits);
    if (!args) return boxNone();
    if (process.env.MOLT_DEBUG_CALL_BIND_NONE === '1' && isTag(callBits, TAG_NONE)) {
      const payload = callBits & POINTER_MASK;
      const payloadHex = payload.toString(16);
      const top = frameStack.length ? frameStack[frameStack.length - 1] : null;
      const codeObj = top ? getCode(top.codeBits) : null;
      const name = codeObj && codeObj.name ? codeObj.name : '<no-code>';
      const pending = exceptionPending() !== 0n;
      const argTypes = args.pos
        .map((bit) => {
          const s = getStrObj(bit);
          if (s !== null) return `str:${s}`;
          return typeName(bit);
        })
        .join(', ');
      console.error(
        `call_bind None payload=0x${payloadHex} in ${name} args=[${argTypes}] pending=${pending} lastAttr=${lastAttrName ?? '<none>'} lastAttrType=${lastAttrObjType ?? '<none>'}`,
      );
    }
    const cls = getClass(callBits);
    if (cls) {
      const typeBits = getBuiltinType(101);
      const handleTypeCall = () => {
        const nameBits = args.pos[0];
        const basesBitsRaw = args.pos[1];
        const namespaceBits = args.pos[2];
        const name = getStrObj(nameBits);
        if (name === null) {
          throw new Error('TypeError: type() name must be a str');
        }
        const classBits = baseImports.class_new(nameBits);
        let basesBits = basesBitsRaw;
        const basesTuple = getTuple(basesBitsRaw);
        if (basesTuple && basesTuple.items.length === 0) {
          basesBits = getBuiltinType(100);
        }
        baseImports.class_set_base(classBits, basesBits);
        const dict = getDict(namespaceBits);
        if (!dict) {
          throw new Error('TypeError: type() namespace must be a dict');
        }
        for (const entry of dict.entries) {
          const keyBits = entry[0];
          const valBits = entry[1];
          const key = getStrObj(keyBits);
          if (key === null) {
            throw new Error('TypeError: type() attribute name must be str');
          }
          setAttrValue(classBits, key, valBits);
        }
        return classBits;
      };
      if (callBits === typeBits) {
        if (args.pos.length === 0) return typeBits;
        if (args.pos.length === 1) return typeOfBits(args.pos[0]);
        if (args.pos.length === 3) return handleTypeCall();
        return baseImports.call_arity_error(BigInt(3), BigInt(args.pos.length));
      }
      if (isSubclass(callBits, typeBits) && args.pos.length === 3) {
        return handleTypeCall();
      }
      if (isSubclass(callBits, getBaseExceptionClass())) {
        const excArgsBits = tupleFromArray([...args.pos]);
        const excBits = exceptionNewFromClass(callBits, excArgsBits);
        const initBits = lookupClassAttr(callBits, '__init__');
        if (initBits === undefined || isNone(initBits)) {
          return excBits;
        }
        args.pos.unshift(excBits);
        baseImports.call_bind(initBits, builderBits);
        return excBits;
      }
      const instBits = allocInstanceForClass(callBits);
      const initBits = lookupClassAttr(callBits, '__init__');
      if (initBits === undefined) {
        return instBits;
      }
      const initFunc = getFunction(initBits);
      if (
        initFunc &&
        initFunc.builtinName === 'object.__init__' &&
        args.pos.length > 0
      ) {
        const clsName = cls && cls.name ? cls.name : '<class>';
        console.error(
          `object.__init__ invoked for ${clsName} with ${args.pos.length} args`,
        );
      }
      args.pos.unshift(instBits);
      baseImports.call_bind(initBits, builderBits);
      return instBits;
    }
    let funcBits = callBits;
    let selfBits = null;
    const bound = getBoundMethod(callBits);
    if (bound) {
      funcBits = bound.func;
      selfBits = bound.self;
    } else if (!getFunction(callBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `'${typeName(callBits)}' object is not callable`,
          }),
        ),
      );
      return raiseException(exc);
    }
    const func = getFunction(funcBits);
    if (!func) {
      throw new Error('TypeError: call expects function object');
    }
    if (selfBits !== null) {
      args.pos.unshift(selfBits);
    }
    if (typeof func.builtinName === 'string') {
      return bindBuiltinCall(funcBits, func, args);
    }
    const nameBits = func.attrs ? func.attrs.get('__name__') : undefined;
    const nameStr = nameBits !== undefined ? getStrObj(nameBits) : null;
    if (nameStr === 'sorted') {
      return bindBuiltinCall(funcBits, func, args);
    }
    const attrs = func.attrs || new Map();
    const argNamesBits = attrs.get('__molt_arg_names__');
    if (argNamesBits === undefined) {
      return bindBuiltinCall(funcBits, func, args);
    }
    const argNamesTuple = getTuple(argNamesBits);
    if (!argNamesTuple) {
      return bindBuiltinCall(funcBits, func, args);
    }
    const argNameBits = [...argNamesTuple.items];
    const argNames = argNameBits.map((bit) => {
      const name = getStrObj(bit);
      if (name === null) {
        throw new Error('TypeError: call expects function object');
      }
      return name;
    });
    const posonlyBits = attrs.get('__molt_posonly__') ?? boxInt(0);
    const posonly = Number(unboxIntLike(posonlyBits));
    const kwonlyBits = attrs.get('__molt_kwonly_names__') ?? boxNone();
    const kwonlyTuple = isNone(kwonlyBits) ? null : getTuple(kwonlyBits);
    const kwonlyNameBits = kwonlyTuple ? [...kwonlyTuple.items] : [];
    const kwonlyNames = kwonlyNameBits.map((bit) => {
      const name = getStrObj(bit);
      if (name === null) {
        throw new Error('TypeError: call expects function object');
      }
      return name;
    });
    const varargBits = attrs.get('__molt_vararg__') ?? boxNone();
    const varkwBits = attrs.get('__molt_varkw__') ?? boxNone();
    const hasVararg = !isNone(varargBits);
    const hasVarkw = !isNone(varkwBits);
    const defaultsBits = attrs.get('__defaults__') ?? boxNone();
    const defaultsTuple = isNone(defaultsBits) ? null : getTuple(defaultsBits);
    const defaults = defaultsTuple ? [...defaultsTuple.items] : [];
    const kwdefaultsBits = attrs.get('__kwdefaults__') ?? boxNone();
    const kwdefaults = isNone(kwdefaultsBits) ? null : getDict(kwdefaultsBits);
    const totalPos = argNames.length;
    const kwonlyStart = totalPos + (hasVararg ? 1 : 0);
    const totalParams = kwonlyStart + kwonlyNames.length + (hasVarkw ? 1 : 0);
    const slots = new Array(totalParams).fill(undefined);
    const extraPos = [];
    const posArgs = [...args.pos];
    for (let i = 0; i < posArgs.length; i++) {
      if (i < totalPos) {
        slots[i] = posArgs[i];
      } else if (hasVararg) {
        extraPos.push(posArgs[i]);
      } else {
        throw new Error('TypeError: too many positional arguments');
      }
    }
    const extraKwPairs = [];
    for (let i = 0; i < args.kwNames.length; i++) {
      const nameBits = args.kwNames[i];
      const valBits = args.kwValues[i];
      const name = getStrObj(nameBits);
      if (name === null) {
        throw new Error('TypeError: keywords must be strings');
      }
      let matched = false;
      const posIdx = argNames.indexOf(name);
      if (posIdx !== -1) {
        if (posIdx < posonly) {
          throw new Error(`TypeError: got positional-only argument '${name}' passed as keyword`);
        }
        if (slots[posIdx] !== undefined) {
          throw new Error(`TypeError: got multiple values for argument '${name}'`);
        }
        slots[posIdx] = valBits;
        matched = true;
      }
      if (!matched) {
        const kwIdx = kwonlyNames.indexOf(name);
        if (kwIdx !== -1) {
          const slotIdx = kwonlyStart + kwIdx;
          if (slots[slotIdx] !== undefined) {
            throw new Error(`TypeError: got multiple values for argument '${name}'`);
          }
          slots[slotIdx] = valBits;
          matched = true;
        }
      }
      if (!matched) {
        if (hasVarkw) {
          extraKwPairs.push([nameBits, valBits]);
        } else {
          throw new Error(`TypeError: got an unexpected keyword '${name}'`);
        }
      }
    }
    const defaultStart = Math.max(0, totalPos - defaults.length);
    for (let i = 0; i < totalPos; i++) {
      if (slots[i] !== undefined) continue;
      if (i >= defaultStart) {
        slots[i] = defaults[i - defaultStart];
      } else {
        throw new Error(`TypeError: missing required argument '${argNames[i]}'`);
      }
    }
    for (let i = 0; i < kwonlyNames.length; i++) {
      const slotIdx = kwonlyStart + i;
      if (slots[slotIdx] !== undefined) continue;
      let val = null;
      if (kwdefaults) {
        val = dictGetValue(kwdefaults, kwonlyNameBits[i]);
      }
      if (val !== null) {
        slots[slotIdx] = val;
      } else {
        throw new Error(
          `TypeError: missing required keyword-only argument '${kwonlyNames[i]}'`,
        );
      }
    }
    if (hasVararg) {
      slots[totalPos] = tupleFromArray(extraPos);
    }
    if (hasVarkw) {
      const dict = { type: 'dict', entries: [], lookup: new Map() };
      for (const [nameBits, valBits] of extraKwPairs) {
        dictSetValue(dict, nameBits, valBits);
      }
      slots[kwonlyStart + kwonlyNames.length] = boxPtr(dict);
    }
    const finalArgs = slots.map((val) => val ?? boxNone());
    if (func.arity !== undefined && func.arity !== finalArgs.length) {
      throw new Error(
        `TypeError: call arity mismatch (expected ${func.arity}, got ${finalArgs.length})`,
      );
    }
    const isGenBits = attrs.get('__molt_is_generator__');
    if (isGenBits !== undefined && isTruthyBits(isGenBits)) {
      const payload = [];
      if (func.closure && func.closure !== 0n && isPtr(func.closure)) {
        payload.push(func.closure);
      }
      payload.push(...finalArgs);
      let payloadBytes = payload.length * 8;
      const sizeBits = attrs.get('__molt_closure_size__');
      if (sizeBits !== undefined && isIntLike(sizeBits)) {
        const size = Number(unboxIntLike(sizeBits));
        if (size > payloadBytes) {
          payloadBytes = size;
        }
      }
      const genBits = baseImports.generator_new(
        BigInt(func.idx),
        BigInt(payloadBytes),
      );
      if (isNone(genBits) || !memory) return genBits;
      const addr = ptrAddr(genBits);
      const view = new DataView(memory.buffer);
      for (let i = 0; i < payload.length; i += 1) {
        view.setBigInt64(
          addr + GEN_CONTROL_SIZE + i * 8,
          payload[i],
          true,
        );
      }
      return genBits;
    }
    return callFunctionBits(funcBits, finalArgs);
  },
  is_callable: (val) => {
    if (getFunction(val) || getBoundMethod(val)) return boxBool(true);
    const attr = lookupAttr(val, '__call__');
    return boxBool(attr !== undefined);
  },
  is_function_obj: (val) => boxBool(getFunction(val) !== null),
  index: (seq, idxBits) => {
    const list = getList(seq);
    const tup = getTuple(seq);
    const bytes = getBytes(seq);
    const bytearray = getBytearray(seq);
    const strVal = getStrObj(seq);
    const items = list ? list.items : tup ? tup.items : null;
    if (items) {
      const errMsg = list
        ? `list indices must be integers or slices, not ${typeName(idxBits)}`
        : `tuple indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        null,
      );
      if (idx === null) return boxNone();
      let pos = idx;
      if (pos < 0) pos += items.length;
      if (pos < 0 || pos >= items.length) {
        throw new Error(
          `IndexError: ${list ? 'list index out of range' : 'tuple index out of range'}`,
        );
      }
      return items[pos];
    }
    if (bytes || bytearray) {
      const errMsg = bytes
        ? `byte indices must be integers or slices, not ${typeName(idxBits)}`
        : `bytearray indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        null,
      );
      if (idx === null) return boxNone();
      const data = bytes ? bytes.data : bytearray.data;
      let pos = idx;
      if (pos < 0) pos += data.length;
      if (pos < 0 || pos >= data.length) {
        throw new Error(
          `IndexError: ${bytearray ? 'bytearray index out of range' : 'index out of range'}`,
        );
      }
      return boxInt(data[pos]);
    }
    const view = getMemoryview(seq);
    if (view) {
      const fmt = memoryviewFormatFromBits(view.formatBits);
      if (!fmt) return boxNone();
      const owner = getBytes(view.ownerBits) || getBytearray(view.ownerBits);
      if (!owner) return boxNone();
      const data = owner.data;
      const shape = memoryviewShape(view);
      const strides = memoryviewStrides(view);
      const ndim = shape.length;
      if (ndim === 0) {
        const tup = getTuple(idxBits);
        if (tup && tup.items.length === 0) {
          const val = memoryviewReadScalar(data, view.offset, fmt);
          return val === null ? boxNone() : val;
        }
        throw new Error('TypeError: invalid indexing of 0-dim memory');
      }
      const tup = getTuple(idxBits);
      if (tup) {
        let hasSlice = false;
        let allSlice = true;
        for (const elem of tup.items) {
          const slice = getSlice(elem);
          if (slice) {
            hasSlice = true;
          } else {
            allSlice = false;
          }
        }
        if (hasSlice) {
          if (allSlice) {
            throw new Error('NotImplementedError: multi-dimensional slicing is not implemented');
          }
          throw new Error('TypeError: memoryview: invalid slice key');
        }
        const indices = [];
        for (const elem of tup.items) {
          const idx = indexFromBitsWithOverflow(elem, 'memoryview: invalid slice key', null);
          if (idx === null) return boxNone();
          indices.push(idx);
        }
        if (indices.length < ndim) {
          throw new Error('NotImplementedError: sub-views are not implemented');
        }
        if (indices.length > ndim) {
          throw new Error(
            `TypeError: cannot index ${ndim}-dimension view with ${indices.length}-element tuple`,
          );
        }
        if (shape.length !== strides.length) return boxNone();
        let pos = view.offset;
        for (let dim = 0; dim < indices.length; dim += 1) {
          let i = indices[dim];
          const dimLen = shape[dim];
          if (i < 0) i += dimLen;
          if (i < 0 || i >= dimLen) {
            throw new Error(`IndexError: index out of bounds on dimension ${dim + 1}`);
          }
          pos += i * strides[dim];
        }
        if (pos < 0 || pos + fmt.itemsize > data.length) {
          throw new Error('IndexError: index out of bounds on dimension 1');
        }
        const val = memoryviewReadScalar(data, pos, fmt);
        return val === null ? boxNone() : val;
      }
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        if (shape.length === 0) {
          throw new Error('TypeError: invalid indexing of 0-dim memory');
        }
        const indices = normalizeSliceIndices(
          shape[0],
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        const newOffset = view.offset + indices.start * strides[0];
        const newStride = strides[0] * indices.step;
        const sliceIndices = collectSliceIndices(indices.start, indices.stop, indices.step);
        const newLen = sliceIndices.length;
        const newShape = shape.slice();
        const newStrides = strides.slice();
        newShape[0] = newLen;
        newStrides[0] = newStride;
        return boxPtr({
          type: 'memoryview',
          ownerBits: view.ownerBits,
          offset: newOffset,
          len: newLen,
          itemsize: view.itemsize,
          stride: newStride,
          readonly: view.readonly,
          formatBits: view.formatBits,
          ndim,
          shape: newShape,
          strides: newStrides,
        });
      }
      const idx = indexFromBitsWithOverflow(
        idxBits,
        'memoryview: invalid slice key',
        null,
      );
      if (idx === null) return boxNone();
      if (ndim > 1) {
        throw new Error('NotImplementedError: multi-dimensional sub-views are not implemented');
      }
      if (shape.length === 0) {
        throw new Error('TypeError: invalid indexing of 0-dim memory');
      }
      let i = idx;
      const len = shape[0];
      if (i < 0) i += len;
      if (i < 0 || i >= len) {
        throw new Error('IndexError: index out of bounds on dimension 1');
      }
      const pos = view.offset + i * strides[0];
      if (pos < 0 || pos + fmt.itemsize > data.length) {
        throw new Error('IndexError: index out of bounds on dimension 1');
      }
      const val = memoryviewReadScalar(data, pos, fmt);
      return val === null ? boxNone() : val;
    }
    if (strVal !== null) {
      const errMsg = `string indices must be integers, not '${typeName(idxBits)}'`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        null,
      );
      if (idx === null) return boxNone();
      const chars = Array.from(strVal);
      let pos = idx;
      if (pos < 0) pos += chars.length;
      if (pos < 0 || pos >= chars.length) {
        throw new Error('IndexError: string index out of range');
      }
      return boxPtr({ type: 'str', value: chars[pos] });
    }
    const dict = getDict(seq);
    if (dict) {
      const val = dictGetValue(dict, idxBits);
      return val === null ? boxNone() : val;
    }
    const dictView =
      getDictKeysView(seq) || getDictValuesView(seq) || getDictItemsView(seq);
    if (dictView) {
      if (!isIntLike(idxBits)) return boxNone();
      let idx = Number(unboxIntLike(idxBits));
      const viewDict = getDict(dictView.dictBits);
      if (!viewDict) return boxNone();
      const len = viewDict.entries.length;
      if (idx < 0) idx += len;
      if (idx < 0 || idx >= len) return boxNone();
      const [keyBits, valBits] = viewDict.entries[idx];
      if (dictView.type === 'dict_items') {
        return tupleFromArray([keyBits, valBits]);
      }
      return dictView.type === 'dict_keys' ? keyBits : valBits;
    }
    return boxNone();
  },
  store_index: (seq, idxBits, val) => {
    const list = getList(seq);
    if (list) {
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        const indices = normalizeSliceIndices(
          list.items.length,
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        const newItems = collectIterableValues(
          val,
          'must assign iterable to extended slice',
        );
        if (newItems === null) return boxNone();
        if (indices.step === 1) {
          let start = indices.start;
          let stop = indices.stop;
          if (start > stop) stop = start;
          list.items.splice(start, stop - start, ...newItems);
          return seq;
        }
        const sliceIndices = collectSliceIndices(
          indices.start,
          indices.stop,
          indices.step,
        );
        if (sliceIndices.length !== newItems.length) {
          throw new Error(
            `ValueError: attempt to assign sequence of size ${newItems.length} to extended slice of size ${sliceIndices.length}`,
          );
        }
        for (let i = 0; i < sliceIndices.length; i++) {
          list.items[sliceIndices[i]] = newItems[i];
        }
        return seq;
      }
      const errMsg = `list indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        null,
      );
      if (idx === null) return boxNone();
      let i = idx;
      if (i < 0) i += list.items.length;
      if (i < 0 || i >= list.items.length) {
        throw new Error('IndexError: list assignment index out of range');
      }
      list.items[i] = val;
      return seq;
    }
    const dict = getDict(seq);
    if (dict) {
      dictSetValue(dict, idxBits, val);
      return seq;
    }
    const bytearray = getBytearray(seq);
    if (bytearray) {
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        const indices = normalizeSliceIndices(
          bytearray.data.length,
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        const srcBytes = collectBytearrayAssignBytes(val);
        if (srcBytes === null) return boxNone();
        if (indices.step === 1) {
          let start = indices.start;
          let stop = indices.stop;
          if (start > stop) stop = start;
          bytearray.data.splice(start, stop - start, ...srcBytes);
          return seq;
        }
        const sliceIndices = collectSliceIndices(
          indices.start,
          indices.stop,
          indices.step,
        );
        if (sliceIndices.length !== srcBytes.length) {
          throw new Error(
            `ValueError: attempt to assign bytes of size ${srcBytes.length} to extended slice of size ${sliceIndices.length}`,
          );
        }
        for (let i = 0; i < sliceIndices.length; i++) {
          bytearray.data[sliceIndices[i]] = srcBytes[i];
        }
        return seq;
      }
      const errMsg = `bytearray indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        "cannot fit 'int' into an index-sized integer",
      );
      if (idx === null) return boxNone();
      let i = idx;
      if (i < 0) i += bytearray.data.length;
      if (i < 0 || i >= bytearray.data.length) {
        throw new Error('IndexError: bytearray index out of range');
      }
      let value = getBigIntValue(val);
      if (value === null) {
        const indexAttr = lookupAttr(val, '__index__');
        if (indexAttr !== undefined) {
          const res = callCallable0(indexAttr);
          if (exceptionPending() !== 0n) return boxNone();
          value = getBigIntValue(res);
          if (value === null) {
            throw new Error(
              `TypeError: __index__ returned non-int (type ${typeName(res)})`,
            );
          }
        }
      }
      if (value === null) {
        throw new Error(
          `TypeError: '${typeName(val)}' object cannot be interpreted as an integer`,
        );
      }
      if (value < 0n || value > 255n) {
        throw new Error('ValueError: byte must be in range(0, 256)');
      }
      bytearray.data[i] = Number(value);
      return seq;
    }
    const view = getMemoryview(seq);
    if (view) {
      if (view.readonly) {
        throw new Error('TypeError: cannot modify read-only memory');
      }
      const owner = getBytearray(view.ownerBits);
      if (!owner) {
        throw new Error('TypeError: memoryview is not writable');
      }
      const fmt = memoryviewFormatFromBits(view.formatBits);
      if (!fmt) return boxNone();
      const data = owner.data;
      const shape = memoryviewShape(view);
      const strides = memoryviewStrides(view);
      const ndim = shape.length;
      if (ndim === 0) {
        const tup = getTuple(idxBits);
        if (tup && tup.items.length === 0) {
          const ok = memoryviewWriteScalar(data, view.offset, fmt, val);
          return ok ? seq : boxNone();
        }
        throw new Error('TypeError: invalid indexing of 0-dim memory');
      }
      const tup = getTuple(idxBits);
      if (tup) {
        let hasSlice = false;
        let allSlice = true;
        for (const elem of tup.items) {
          const slice = getSlice(elem);
          if (slice) {
            hasSlice = true;
          } else {
            allSlice = false;
          }
        }
        if (hasSlice) {
          if (allSlice) {
            throw new Error(
              'NotImplementedError: memoryview slice assignments are currently restricted to ndim = 1',
            );
          }
          throw new Error('TypeError: memoryview: invalid slice key');
        }
        const indices = [];
        for (const elem of tup.items) {
          const idx = indexFromBitsWithOverflow(elem, 'memoryview: invalid slice key', null);
          if (idx === null) return boxNone();
          indices.push(idx);
        }
        if (indices.length < ndim) {
          throw new Error('NotImplementedError: sub-views are not implemented');
        }
        if (indices.length > ndim) {
          throw new Error(
            `TypeError: cannot index ${ndim}-dimension view with ${indices.length}-element tuple`,
          );
        }
        if (shape.length !== strides.length) return boxNone();
        let pos = view.offset;
        for (let dim = 0; dim < indices.length; dim += 1) {
          let i = indices[dim];
          const dimLen = shape[dim];
          if (i < 0) i += dimLen;
          if (i < 0 || i >= dimLen) {
            throw new Error(`IndexError: index out of bounds on dimension ${dim + 1}`);
          }
          pos += i * strides[dim];
        }
        if (pos < 0 || pos + fmt.itemsize > data.length) {
          throw new Error('IndexError: index out of bounds on dimension 1');
        }
        const ok = memoryviewWriteScalar(data, pos, fmt, val);
        return ok ? seq : boxNone();
      }
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        if (ndim !== 1) {
          throw new Error(
            'NotImplementedError: memoryview slice assignments are currently restricted to ndim = 1',
          );
        }
        if (shape.length === 0) {
          throw new Error('TypeError: invalid indexing of 0-dim memory');
        }
        const indices = normalizeSliceIndices(
          shape[0],
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        const sliceIndices = collectSliceIndices(
          indices.start,
          indices.stop,
          indices.step,
        );
        const elemCount = sliceIndices.length;
        let srcBytes = null;
        const bytes = getBytes(val);
        const bytearray = getBytearray(val);
        const srcView = getMemoryview(val);
        if (bytes || bytearray) {
          if (fmt.code !== 'B') {
            throw new Error(
              'ValueError: memoryview assignment: lvalue and rvalue have different structures',
            );
          }
          srcBytes = Array.from(bytes ? bytes.data : bytearray.data);
        } else if (srcView) {
          const srcFmt = memoryviewFormatFromBits(srcView.formatBits);
          if (!srcFmt) return boxNone();
          const srcShape = memoryviewShape(srcView);
          if (srcFmt.code !== fmt.code || srcShape.length !== 1 || srcShape[0] !== elemCount) {
            throw new Error(
              'ValueError: memoryview assignment: lvalue and rvalue have different structures',
            );
          }
          const buf = memoryviewCollectBytes(srcView);
          if (buf === null) return boxNone();
          srcBytes = buf;
        } else {
          throw new Error(
            `TypeError: a bytes-like object is required, not '${typeName(val)}'`,
          );
        }
        const expected = elemCount * fmt.itemsize;
        if (srcBytes.length !== expected) {
          throw new Error(
            'ValueError: memoryview assignment: lvalue and rvalue have different structures',
          );
        }
        let pos = view.offset + indices.start * strides[0];
        const stepStride = strides[0] * indices.step;
        for (let i = 0; i < srcBytes.length; i += fmt.itemsize) {
          if (pos < 0 || pos + fmt.itemsize > data.length) return boxNone();
          for (let j = 0; j < fmt.itemsize; j += 1) {
            data[pos + j] = srcBytes[i + j];
          }
          pos += stepStride;
        }
        return seq;
      }
      const idx = indexFromBitsWithOverflow(
        idxBits,
        'memoryview: invalid slice key',
        null,
      );
      if (idx === null) return boxNone();
      if (ndim !== 1) {
        throw new Error('NotImplementedError: sub-views are not implemented');
      }
      if (shape.length === 0) {
        throw new Error('TypeError: invalid indexing of 0-dim memory');
      }
      let i = idx;
      const len = shape[0];
      if (i < 0) i += len;
      if (i < 0 || i >= len) {
        throw new Error('IndexError: index out of bounds on dimension 1');
      }
      const pos = view.offset + i * strides[0];
      if (pos < 0 || pos + fmt.itemsize > data.length) {
        throw new Error('IndexError: index out of bounds on dimension 1');
      }
      const ok = memoryviewWriteScalar(data, pos, fmt, val);
      return ok ? seq : boxNone();
    }
    return boxNone();
  },
  del_index: (seq, idxBits) => {
    const list = getList(seq);
    if (list) {
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        const indices = normalizeSliceIndices(
          list.items.length,
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        if (indices.step === 1) {
          let start = indices.start;
          let stop = indices.stop;
          if (start > stop) stop = start;
          list.items.splice(start, stop - start);
          return seq;
        }
        const sliceIndices = collectSliceIndices(
          indices.start,
          indices.stop,
          indices.step,
        );
        if (indices.step > 0) {
          for (let i = sliceIndices.length - 1; i >= 0; i--) {
            list.items.splice(sliceIndices[i], 1);
          }
        } else {
          for (let i = 0; i < sliceIndices.length; i++) {
            list.items.splice(sliceIndices[i], 1);
          }
        }
        return seq;
      }
      const errMsg = `list indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        null,
      );
      if (idx === null) return boxNone();
      let i = idx;
      if (i < 0) i += list.items.length;
      if (i < 0 || i >= list.items.length) {
        throw new Error('IndexError: list assignment index out of range');
      }
      list.items.splice(i, 1);
      return seq;
    }
    const bytearray = getBytearray(seq);
    if (bytearray) {
      const sliceObj = getSlice(idxBits);
      if (sliceObj) {
        const indices = normalizeSliceIndices(
          bytearray.data.length,
          sliceObj.start,
          sliceObj.stop,
          sliceObj.step,
        );
        if (indices === null) return boxNone();
        if (indices.step === 1) {
          let start = indices.start;
          let stop = indices.stop;
          if (start > stop) stop = start;
          bytearray.data.splice(start, stop - start);
          return seq;
        }
        const sliceIndices = collectSliceIndices(
          indices.start,
          indices.stop,
          indices.step,
        );
        if (indices.step > 0) {
          for (let i = sliceIndices.length - 1; i >= 0; i--) {
            bytearray.data.splice(sliceIndices[i], 1);
          }
        } else {
          for (let i = 0; i < sliceIndices.length; i++) {
            bytearray.data.splice(sliceIndices[i], 1);
          }
        }
        return seq;
      }
      const errMsg = `bytearray indices must be integers or slices, not ${typeName(idxBits)}`;
      const idx = indexFromBitsWithOverflow(
        idxBits,
        errMsg,
        "cannot fit 'int' into an index-sized integer",
      );
      if (idx === null) return boxNone();
      let i = idx;
      if (i < 0) i += bytearray.data.length;
      if (i < 0 || i >= bytearray.data.length) {
        throw new Error('IndexError: bytearray index out of range');
      }
      bytearray.data.splice(i, 1);
      return seq;
    }
    const view = getMemoryview(seq);
    if (view) {
      throw new Error('TypeError: cannot delete memory');
    }
    return boxNone();
  },
  bytes_find: (hayBits, needleBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    return boxInt(bytesFindInRange(hay.data, needle.data, 0, hay.data.length));
  },
  bytes_find_slice: (hayBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(-1);
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (needle) {
      const n = needle.data;
      if (n.length === 0) {
        if (startGtLen) return boxInt(-1);
        return boxInt(start);
      }
      return boxInt(bytesFindInRange(hay.data, n, start, end));
    }
    const needleInt = getBigIntValue(needleBits);
    if (needleInt !== null) {
      if (needleInt < 0n || needleInt > 255n) {
        throw new Error('ValueError: byte must be in range(0, 256)');
      }
      const byte = Number(needleInt);
      for (let i = start; i < end; i += 1) {
        if (hay.data[i] === byte) return boxInt(i);
      }
      return boxInt(-1);
    }
    return boxNone();
  },
  bytearray_find: (hayBits, needleBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    return boxInt(bytesFindInRange(hay.data, needle.data, 0, hay.data.length));
  },
  bytearray_find_slice: (hayBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(-1);
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (needle) {
      const n = needle.data;
      if (n.length === 0) {
        if (startGtLen) return boxInt(-1);
        return boxInt(start);
      }
      return boxInt(bytesFindInRange(hay.data, n, start, end));
    }
    const needleInt = getBigIntValue(needleBits);
    if (needleInt !== null) {
      if (needleInt < 0n || needleInt > 255n) {
        throw new Error('ValueError: byte must be in range(0, 256)');
      }
      const byte = Number(needleInt);
      for (let i = start; i < end; i += 1) {
        if (hay.data[i] === byte) return boxInt(i);
      }
      return boxInt(-1);
    }
    return boxNone();
  },
  bytes_startswith: (hayBits, needleBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length > h.length) return boxBool(false);
    for (let i = 0; i < n.length; i += 1) {
      if (h[i] !== n[i]) return boxBool(false);
    }
    return boxBool(true);
  },
  bytes_startswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemBytes = getBytes(item) || getBytearray(item);
        if (!itemBytes) return boxNone();
        const n = itemBytes.data;
        if (n.length === 0 && startGtLen) continue;
        if (bytesStartsWithInRange(hay.data, n, start, end)) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const n = needle.data;
    if (n.length === 0 && startGtLen) return boxBool(false);
    return boxBool(bytesStartsWithInRange(hay.data, n, start, end));
  },
  bytearray_startswith: (hayBits, needleBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length > h.length) return boxBool(false);
    for (let i = 0; i < n.length; i += 1) {
      if (h[i] !== n[i]) return boxBool(false);
    }
    return boxBool(true);
  },
  bytearray_startswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemBytes = getBytes(item) || getBytearray(item);
        if (!itemBytes) return boxNone();
        const n = itemBytes.data;
        if (n.length === 0 && startGtLen) continue;
        if (bytesStartsWithInRange(hay.data, n, start, end)) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const n = needle.data;
    if (n.length === 0 && startGtLen) return boxBool(false);
    return boxBool(bytesStartsWithInRange(hay.data, n, start, end));
  },
  bytes_endswith: (hayBits, needleBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length > h.length) return boxBool(false);
    const offset = h.length - n.length;
    for (let i = 0; i < n.length; i += 1) {
      if (h[offset + i] !== n[i]) return boxBool(false);
    }
    return boxBool(true);
  },
  bytes_endswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemBytes = getBytes(item) || getBytearray(item);
        if (!itemBytes) return boxNone();
        const n = itemBytes.data;
        if (n.length === 0 && startGtLen) continue;
        if (bytesEndsWithInRange(hay.data, n, start, end)) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const n = needle.data;
    if (n.length === 0 && startGtLen) return boxBool(false);
    return boxBool(bytesEndsWithInRange(hay.data, n, start, end));
  },
  bytearray_endswith: (hayBits, needleBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length > h.length) return boxBool(false);
    const offset = h.length - n.length;
    for (let i = 0; i < n.length; i += 1) {
      if (h[offset + i] !== n[i]) return boxBool(false);
    }
    return boxBool(true);
  },
  bytearray_endswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemBytes = getBytes(item) || getBytearray(item);
        if (!itemBytes) return boxNone();
        const n = itemBytes.data;
        if (n.length === 0 && startGtLen) continue;
        if (bytesEndsWithInRange(hay.data, n, start, end)) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const n = needle.data;
    if (n.length === 0 && startGtLen) return boxBool(false);
    return boxBool(bytesEndsWithInRange(hay.data, n, start, end));
  },
  bytes_count: (hayBits, needleBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length === 0) return boxInt(h.length + 1);
    let count = 0;
    let i = 0;
    while (i + n.length <= h.length) {
      let match = true;
      for (let j = 0; j < n.length; j += 1) {
        if (h[i + j] !== n[j]) {
          match = false;
          break;
        }
      }
      if (match) {
        count += 1;
        i += n.length;
      } else {
        i += 1;
      }
    }
    return boxInt(count);
  },
  bytearray_count: (hayBits, needleBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const h = hay.data;
    const n = needle.data;
    if (n.length === 0) return boxInt(h.length + 1);
    let count = 0;
    let i = 0;
    while (i + n.length <= h.length) {
      let match = true;
      for (let j = 0; j < n.length; j += 1) {
        if (h[i + j] !== n[j]) {
          match = false;
          break;
        }
      }
      if (match) {
        count += 1;
        i += n.length;
      } else {
        i += 1;
      }
    }
    return boxInt(count);
  },
  bytes_count_slice: (hayBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(0);
    const slice = hay.data.slice(start, end);
    const n = needle.data;
    if (n.length === 0) {
      if (startGtLen) return boxInt(0);
      return boxInt(end - start + 1);
    }
    let count = 0;
    let i = 0;
    while (i + n.length <= slice.length) {
      let match = true;
      for (let j = 0; j < n.length; j += 1) {
        if (slice[i + j] !== n[j]) {
          match = false;
          break;
        }
      }
      if (match) {
        count += 1;
        i += n.length;
      } else {
        i += 1;
      }
    }
    return boxInt(count);
  },
  bytearray_count_slice: (hayBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const total = hay.data.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(0);
    const slice = hay.data.slice(start, end);
    const n = needle.data;
    if (n.length === 0) {
      if (startGtLen) return boxInt(0);
      return boxInt(end - start + 1);
    }
    let count = 0;
    let i = 0;
    while (i + n.length <= slice.length) {
      let match = true;
      for (let j = 0; j < n.length; j += 1) {
        if (slice[i + j] !== n[j]) {
          match = false;
          break;
        }
      }
      if (match) {
        count += 1;
        i += n.length;
      } else {
        i += 1;
      }
    }
    return boxInt(count);
  },
  string_find: () => boxNone(),
  string_find_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getStrObj(hayBits);
    const needle = getStrObj(needleBits);
    if (hay === null || needle === null) return boxNone();
    const hayChars = Array.from(hay);
    const needleChars = Array.from(needle);
    const total = hayChars.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(-1);
    if (needleChars.length === 0) {
      if (startGtLen) return boxInt(-1);
      return boxInt(start);
    }
    const limit = end - needleChars.length;
    for (let i = start; i <= limit; i += 1) {
      let match = true;
      for (let j = 0; j < needleChars.length; j += 1) {
        if (hayChars[i + j] !== needleChars[j]) {
          match = false;
          break;
        }
      }
      if (match) return boxInt(i);
    }
    return boxInt(-1);
  },
  string_format: (val, specBits) => {
    const spec = getStrObj(specBits);
    if (spec === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `format spec must be a str, not ${typeName(specBits)}`,
          }),
        ),
      );
      return raiseException(exc);
    }
    if (spec.length === 0) {
      return baseImports.str_from_obj(val);
    }
    const raiseFormatError = (err) => {
      const msg = err && err.message ? err.message : String(err);
      const match = msg.match(/^([A-Za-z_]+Error):\\s*(.*)$/);
      const kind = match ? match[1] : 'ValueError';
      const text = match ? match[2] : msg;
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: kind }),
        exceptionArgs(boxPtr({ type: 'str', value: text })),
      );
      return raiseException(exc);
    };
    let parsed;
    try {
      parsed = parseFormatSpec(spec);
    } catch (err) {
      return raiseFormatError(err);
    }
    let rendered = '';
    try {
      rendered = formatWithSpec(val, parsed);
    } catch (err) {
      return raiseFormatError(err);
    }
    return boxPtr({ type: 'str', value: rendered });
  },
  string_startswith: () => boxBool(false),
  string_startswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getStrObj(hayBits);
    if (hay === null) return boxNone();
    const hayChars = Array.from(hay);
    const total = hayChars.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const matchPrefix = (needleChars) => {
      if (needleChars.length === 0) return !startGtLen;
      const sliceLen = end - start;
      if (needleChars.length > sliceLen) return false;
      for (let i = 0; i < needleChars.length; i += 1) {
        if (hayChars[start + i] !== needleChars[i]) return false;
      }
      return true;
    };
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemStr = getStrObj(item);
        if (itemStr === null) return boxNone();
        if (matchPrefix(Array.from(itemStr))) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getStrObj(needleBits);
    if (needle === null) return boxNone();
    return boxBool(matchPrefix(Array.from(needle)));
  },
  string_endswith: () => boxBool(false),
  string_endswith_slice: (
    hayBits,
    needleBits,
    startBits,
    endBits,
    hasStartBits,
    hasEndBits,
  ) => {
    const hay = getStrObj(hayBits);
    if (hay === null) return boxNone();
    const hayChars = Array.from(hay);
    const total = hayChars.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxBool(false);
    const matchSuffix = (needleChars) => {
      if (needleChars.length === 0) return !startGtLen;
      const sliceLen = end - start;
      if (needleChars.length > sliceLen) return false;
      const offset = end - needleChars.length;
      for (let i = 0; i < needleChars.length; i += 1) {
        if (hayChars[offset + i] !== needleChars[i]) return false;
      }
      return true;
    };
    const tuple = getTuple(needleBits);
    if (tuple) {
      if (!tuple.items.length) return boxBool(false);
      for (const item of tuple.items) {
        const itemStr = getStrObj(item);
        if (itemStr === null) return boxNone();
        if (matchSuffix(Array.from(itemStr))) return boxBool(true);
      }
      return boxBool(false);
    }
    const needle = getStrObj(needleBits);
    if (needle === null) return boxNone();
    return boxBool(matchSuffix(Array.from(needle)));
  },
  string_count: (hayBits, needleBits) => {
    const hay = getStrObj(hayBits);
    const needle = getStrObj(needleBits);
    if (hay === null || needle === null) return boxNone();
    const hayChars = Array.from(hay);
    const needleChars = Array.from(needle);
    return boxInt(stringCountFromChars(hayChars, needleChars));
  },
  string_count_slice: (hayBits, needleBits, startBits, endBits, hasStartBits, hasEndBits) => {
    const hay = getStrObj(hayBits);
    const needle = getStrObj(needleBits);
    if (hay === null || needle === null) return boxNone();
    const hayChars = Array.from(hay);
    const needleChars = Array.from(needle);
    const total = hayChars.length;
    const bounds = sliceBoundsFromArgs(
      startBits,
      endBits,
      hasStartBits,
      hasEndBits,
      total,
    );
    if (!bounds) return boxNone();
    const { start, end, startGtLen } = bounds;
    if (end < start) return boxInt(0);
    const slice = hayChars.slice(start, end);
    if (needleChars.length === 0) {
      if (startGtLen) return boxInt(0);
      return boxInt(end - start + 1);
    }
    return boxInt(stringCountFromChars(slice, needleChars));
  },
  string_join: (sepBits, itemsBits) => {
    const sep = getStrObj(sepBits);
    if (sep === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'join expects a str separator' }),
        ),
      );
      return raiseException(exc);
    }
    const parts = [];
    let firstBits = null;
    const pushItem = (itemBits, idx) => {
      const text = getStrObj(itemBits);
      if (text === null) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `sequence item ${idx}: expected str instance, ${typeName(itemBits)} found`,
            }),
          ),
        );
        raiseException(exc);
        return false;
      }
      if (idx === 0) {
        firstBits = itemBits;
      }
      parts.push(text);
      return true;
    };
    const list = getList(itemsBits);
    const tuple = getTuple(itemsBits);
    if (list || tuple) {
      const items = list ? list.items : tuple.items;
      for (let idx = 0; idx < items.length; idx += 1) {
        if (!pushItem(items[idx], idx)) return boxNone();
      }
      if (!items.length) {
        return boxPtr({ type: 'str', value: '' });
      }
      if (items.length === 1 && firstBits !== null) {
        return firstBits;
      }
      return boxPtr({ type: 'str', value: parts.join(sep) });
    }
    const iterBits = baseImports.iter(itemsBits);
    if (isNone(iterBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'can only join an iterable' })),
      );
      return raiseException(exc);
    }
    let idx = 0;
    while (true) {
      const pairBits = iterNextInternal(iterBits);
      if (exceptionPending() !== 0n) return boxNone();
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) return boxNone();
      const doneBits = pair.items[1];
      if (isTruthyBits(doneBits)) break;
      if (!pushItem(pair.items[0], idx)) return boxNone();
      idx += 1;
    }
    if (!parts.length) {
      return boxPtr({ type: 'str', value: '' });
    }
    return boxPtr({ type: 'str', value: parts.join(sep) });
  },
  string_split: (hayBits, needleBits) => {
    const hay = getStrObj(hayBits);
    if (hay === null) return boxNone();
    if (isNone(needleBits)) {
      const parts = stringSplitWhitespaceMax(hay, -1);
      return boxPtr({ type: 'list', items: parts.map((item) => boxPtr({ type: 'str', value: item })) });
    }
    const needle = getStrObj(needleBits);
    if (needle === null) return boxNone();
    const parts = stringSplitSepMax(hay, needle, -1);
    return boxPtr({ type: 'list', items: parts.map((item) => boxPtr({ type: 'str', value: item })) });
  },
  string_split_max: (hayBits, needleBits, maxsplitBits) => {
    const hay = getStrObj(hayBits);
    if (hay === null) return boxNone();
    const maxsplit = splitMaxsplitFromBits(maxsplitBits);
    if (maxsplit === null) return boxNone();
    if (isNone(needleBits)) {
      const parts = stringSplitWhitespaceMax(hay, maxsplit);
      return boxPtr({ type: 'list', items: parts.map((item) => boxPtr({ type: 'str', value: item })) });
    }
    const needle = getStrObj(needleBits);
    if (needle === null) return boxNone();
    const parts = stringSplitSepMax(hay, needle, maxsplit);
    return boxPtr({ type: 'list', items: parts.map((item) => boxPtr({ type: 'str', value: item })) });
  },
  string_lower: (haystack) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    return boxPtr({ type: 'str', value: str.toLowerCase() });
  },
  string_upper: (haystack) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    return boxPtr({ type: 'str', value: str.toUpperCase() });
  },
  string_capitalize: (haystack) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    if (!str.length) return boxPtr({ type: 'str', value: '' });
    const chars = Array.from(str);
    const first = chars[0].toUpperCase();
    const rest = chars.slice(1).join('').toLowerCase();
    return boxPtr({ type: 'str', value: first + rest });
  },
  string_strip: (haystack, charsBits) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    if (isNone(charsBits)) {
      return boxPtr({ type: 'str', value: str.trim() });
    }
    const chars = getStrObj(charsBits);
    if (chars === null) {
      throw new Error('TypeError: strip arg must be None or str');
    }
    return boxPtr({ type: 'str', value: stringStripChars(str, chars) });
  },
  string_lstrip: (haystack, charsBits) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    if (isNone(charsBits)) {
      return boxPtr({ type: 'str', value: str.trimStart() });
    }
    const chars = getStrObj(charsBits);
    if (chars === null) {
      throw new Error('TypeError: lstrip arg must be None or str');
    }
    return boxPtr({ type: 'str', value: stringLStripChars(str, chars) });
  },
  string_rstrip: (haystack, charsBits) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    if (isNone(charsBits)) {
      return boxPtr({ type: 'str', value: str.trimEnd() });
    }
    const chars = getStrObj(charsBits);
    if (chars === null) {
      throw new Error('TypeError: rstrip arg must be None or str');
    }
    return boxPtr({ type: 'str', value: stringRStripChars(str, chars) });
  },
  bytes_split: (hayBits, needleBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    if (isNone(needleBits)) {
      const parts = bytesSplitWhitespaceMax(hay.data, -1);
      return boxPtr({
        type: 'list',
        items: parts.map((part) => boxPtr({ type: 'bytes', data: part })),
      });
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const parts = bytesSplitSepMax(hay.data, needle.data, -1);
    return boxPtr({
      type: 'list',
      items: parts.map((part) => boxPtr({ type: 'bytes', data: part })),
    });
  },
  bytes_split_max: (hayBits, needleBits, maxsplitBits) => {
    const hay = getBytes(hayBits);
    if (!hay) return boxNone();
    const maxsplit = splitMaxsplitFromBits(maxsplitBits);
    if (maxsplit === null) return boxNone();
    if (isNone(needleBits)) {
      const parts = bytesSplitWhitespaceMax(hay.data, maxsplit);
      return boxPtr({
        type: 'list',
        items: parts.map((part) => boxPtr({ type: 'bytes', data: part })),
      });
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const parts = bytesSplitSepMax(hay.data, needle.data, maxsplit);
    return boxPtr({
      type: 'list',
      items: parts.map((part) => boxPtr({ type: 'bytes', data: part })),
    });
  },
  bytearray_split: (hayBits, needleBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    if (isNone(needleBits)) {
      const parts = bytesSplitWhitespaceMax(hay.data, -1);
      return boxPtr({
        type: 'list',
        items: parts.map((part) => boxPtr({ type: 'bytearray', data: part })),
      });
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const parts = bytesSplitSepMax(hay.data, needle.data, -1);
    return boxPtr({
      type: 'list',
      items: parts.map((part) => boxPtr({ type: 'bytearray', data: part })),
    });
  },
  bytearray_split_max: (hayBits, needleBits, maxsplitBits) => {
    const hay = getBytearray(hayBits);
    if (!hay) return boxNone();
    const maxsplit = splitMaxsplitFromBits(maxsplitBits);
    if (maxsplit === null) return boxNone();
    if (isNone(needleBits)) {
      const parts = bytesSplitWhitespaceMax(hay.data, maxsplit);
      return boxPtr({
        type: 'list',
        items: parts.map((part) => boxPtr({ type: 'bytearray', data: part })),
      });
    }
    const needle = getBytes(needleBits) || getBytearray(needleBits);
    if (!needle) return boxNone();
    const parts = bytesSplitSepMax(hay.data, needle.data, maxsplit);
    return boxPtr({
      type: 'list',
      items: parts.map((part) => boxPtr({ type: 'bytearray', data: part })),
    });
  },
  string_replace: (_hay, _needle, _repl, _count) => boxNone(),
  bytes_replace: (_hay, _needle, _repl, _count) => boxNone(),
  bytearray_replace: (_hay, _needle, _repl, _count) => boxNone(),
  bytes_from_obj: (val) => {
    const bytes = getBytes(val);
    if (bytes) return val;
    const bytearray = getBytearray(val);
    if (bytearray) {
      return boxPtr({ type: 'bytes', data: Uint8Array.from(bytearray.data) });
    }
    if (getStrObj(val) !== null) {
      throw new Error('TypeError: string argument without an encoding');
    }
    const emitRangeError = () => {
      throw new Error('ValueError: bytes must be in range(0, 256)');
    };
    const toCount = (bits) => {
      const value = getBigIntValue(bits);
      if (value === null) return null;
      if (value < 0n) {
        throw new Error('ValueError: negative count');
      }
      if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new Error("OverflowError: cannot fit 'int' into an index-sized integer");
      }
      return Number(value);
    };
    const byteFromItem = (itemBits) => {
      let value = getBigIntValue(itemBits);
      if (value === null) {
        const indexAttr = lookupAttr(itemBits, '__index__');
        if (indexAttr !== undefined) {
          const res = callCallable0(indexAttr);
          if (exceptionPending() !== 0n) return null;
          value = getBigIntValue(res);
          if (value === null) {
            throw new Error(
              `TypeError: __index__ returned non-int (type ${typeName(res)})`,
            );
          }
        }
      }
      if (value === null) {
        throw new Error(
          `TypeError: '${typeName(itemBits)}' object cannot be interpreted as an integer`,
        );
      }
      if (value < 0n || value > 255n) {
        emitRangeError();
      }
      return Number(value);
    };
    const directCount = toCount(val);
    if (directCount !== null) {
      return boxPtr({ type: 'bytes', data: new Uint8Array(directCount) });
    }
    const indexAttr = lookupAttr(val, '__index__');
    if (indexAttr !== undefined) {
      const res = callCallable0(indexAttr);
      if (exceptionPending() !== 0n) return boxNone();
      const count = toCount(res);
      if (count === null) {
        throw new Error(
          `TypeError: __index__ returned non-int (type ${typeName(res)})`,
        );
      }
      return boxPtr({ type: 'bytes', data: new Uint8Array(count) });
    }
    const list = getList(val);
    const tuple = getTuple(val);
    if (list || tuple) {
      const items = list ? list.items : tuple.items;
      const out = [];
      for (const item of items) {
        const byte = byteFromItem(item);
        if (byte === null) return boxNone();
        out.push(byte);
      }
      return boxPtr({ type: 'bytes', data: Uint8Array.from(out) });
    }
    const iterBits = baseImports.iter(val);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: cannot convert '${typeName(val)}' object to bytes`);
    }
    const out = [];
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = pair.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      const byte = byteFromItem(pair.items[0]);
      if (byte === null) return boxNone();
      out.push(byte);
    }
    return boxPtr({ type: 'bytes', data: Uint8Array.from(out) });
  },
  bytearray_from_obj: (val) => {
    const bytes = getBytes(val);
    if (bytes) {
      return boxPtr({ type: 'bytearray', data: Uint8Array.from(bytes.data) });
    }
    const bytearray = getBytearray(val);
    if (bytearray) {
      return boxPtr({ type: 'bytearray', data: Uint8Array.from(bytearray.data) });
    }
    if (getStrObj(val) !== null) {
      throw new Error('TypeError: string argument without an encoding');
    }
    const emitRangeError = () => {
      throw new Error('ValueError: byte must be in range(0, 256)');
    };
    const toCount = (bits) => {
      const value = getBigIntValue(bits);
      if (value === null) return null;
      if (value < 0n) {
        throw new Error('ValueError: negative count');
      }
      if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new Error("OverflowError: cannot fit 'int' into an index-sized integer");
      }
      return Number(value);
    };
    const byteFromItem = (itemBits) => {
      let value = getBigIntValue(itemBits);
      if (value === null) {
        const indexAttr = lookupAttr(itemBits, '__index__');
        if (indexAttr !== undefined) {
          const res = callCallable0(indexAttr);
          if (exceptionPending() !== 0n) return null;
          value = getBigIntValue(res);
          if (value === null) {
            throw new Error(
              `TypeError: __index__ returned non-int (type ${typeName(res)})`,
            );
          }
        }
      }
      if (value === null) {
        throw new Error(
          `TypeError: '${typeName(itemBits)}' object cannot be interpreted as an integer`,
        );
      }
      if (value < 0n || value > 255n) {
        emitRangeError();
      }
      return Number(value);
    };
    const directCount = toCount(val);
    if (directCount !== null) {
      return boxPtr({ type: 'bytearray', data: new Uint8Array(directCount) });
    }
    const indexAttr = lookupAttr(val, '__index__');
    if (indexAttr !== undefined) {
      const res = callCallable0(indexAttr);
      if (exceptionPending() !== 0n) return boxNone();
      const count = toCount(res);
      if (count === null) {
        throw new Error(
          `TypeError: __index__ returned non-int (type ${typeName(res)})`,
        );
      }
      return boxPtr({ type: 'bytearray', data: new Uint8Array(count) });
    }
    const list = getList(val);
    const tuple = getTuple(val);
    if (list || tuple) {
      const items = list ? list.items : tuple.items;
      const out = [];
      for (const item of items) {
        const byte = byteFromItem(item);
        if (byte === null) return boxNone();
        out.push(byte);
      }
      return boxPtr({ type: 'bytearray', data: Uint8Array.from(out) });
    }
    const iterBits = baseImports.iter(val);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(
        `TypeError: cannot convert '${typeName(val)}' object to bytearray`,
      );
    }
    const out = [];
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = pair.items[1];
      if (isTruthyBits(doneBits)) {
        break;
      }
      const byte = byteFromItem(pair.items[0]);
      if (byte === null) return boxNone();
      out.push(byte);
    }
    return boxPtr({ type: 'bytearray', data: Uint8Array.from(out) });
  },
  bytes_from_str: (val, encodingBits, errorsBits) => {
    const text = getStrObj(val);
    if (text === null) return boxNone();
    let encoding = 'utf8';
    const enc = getStrObj(encodingBits);
    if (enc) {
      const normalized = enc.toLowerCase();
      if (normalized === 'utf-16' || normalized === 'utf16') {
        encoding = 'utf16le';
      } else if (normalized === 'latin-1' || normalized === 'latin1') {
        encoding = 'latin1';
      } else {
        encoding = normalized;
      }
    }
    const data = Buffer.from(text, encoding);
    return boxPtr({ type: 'bytes', data: Uint8Array.from(data) });
  },
  bytearray_from_str: (val, encodingBits, errorsBits) => {
    const bytes = baseImports.bytes_from_str(val, encodingBits, errorsBits);
    const obj = getBytes(bytes);
    if (!obj) return boxNone();
    return boxPtr({ type: 'bytearray', data: Uint8Array.from(obj.data) });
  },
  intarray_from_seq: () => boxNone(),
  buffer2d_new: () => boxNone(),
  buffer2d_get: () => boxNone(),
  buffer2d_set: () => boxNone(),
  buffer2d_matmul: () => boxNone(),
  dataclass_new: () => boxNone(),
  dataclass_get: () => boxNone(),
  dataclass_set: () => boxNone(),
  dataclass_set_class: () => boxNone(),
  class_new: (nameBits) => {
    const name = getStrObj(nameBits);
    const classBits = boxPtr({
      type: 'class',
      name: name ?? '<class>',
      attrs: new Map(),
      baseBits: boxNone(),
      basesBits: null,
      mroBits: null,
    });
    classLayoutVersions.set(classBits, 0n);
    return classBits;
  },
  class_set_base: (classBits, baseBits) => {
    setClassBases(classBits, baseBits);
    return boxNone();
  },
  class_apply_set_name: (classBits) => {
    const cls = getClass(classBits);
    if (!cls) return boxNone();
    for (const [name, valBits] of cls.attrs.entries()) {
      const setName = lookupAttr(valBits, '__set_name__');
      if (setName !== undefined && isTruthyBits(baseImports.is_callable(setName))) {
        const nameBits = boxPtr({ type: 'str', value: name });
        callCallable2(setName, classBits, nameBits);
      }
    }
    return boxNone();
  },
  builtin_type: (tagBits) => {
    if (!isTag(tagBits, TAG_INT)) {
      throw new Error('TypeError: builtin type tag must be int');
    }
    const tag = Number(unboxInt(tagBits));
    return getBuiltinType(tag);
  },
  type_of: (objBits) => typeOfBits(objBits),
  class_layout_version: (classBits) => {
    if (!getClass(classBits)) {
      throw new Error('TypeError: class must be a type object');
    }
    const version = classLayoutVersion(classBits);
    return boxInt(version ?? 0n);
  },
  class_set_layout_version: (classBits, versionBits) => {
    if (!getClass(classBits)) {
      throw new Error('TypeError: class must be a type object');
    }
    if (!isIntLike(versionBits)) {
      throw new Error('TypeError: layout version must be int');
    }
    const version = unboxIntLike(versionBits);
    if (version < 0n) {
      throw new Error('TypeError: layout version must be non-negative');
    }
    classLayoutVersions.set(classBits, version);
    return boxNone();
  },
  isinstance: (objBits, classBits) => {
    const tuple = getTuple(classBits);
    if (tuple) {
      for (const item of tuple.items) {
        if (getClass(item) && isSubclass(typeOfBits(objBits), item)) {
          return boxBool(true);
        }
      }
      return boxBool(false);
    }
    if (!getClass(classBits)) {
      throw new Error('TypeError: isinstance() arg 2 must be a type or tuple of types');
    }
    return boxBool(isSubclass(typeOfBits(objBits), classBits));
  },
  issubclass: (subBits, classBits) => {
    if (!getClass(subBits)) {
      throw new Error('TypeError: issubclass() arg 1 must be a class');
    }
    const tuple = getTuple(classBits);
    if (tuple) {
      for (const item of tuple.items) {
        if (!getClass(item)) {
          throw new Error(
            'TypeError: issubclass() arg 2 must be a class or tuple of classes'
          );
        }
        if (isSubclass(subBits, item)) {
          return boxBool(true);
        }
      }
      return boxBool(false);
    }
    if (!getClass(classBits)) {
      throw new Error('TypeError: issubclass() arg 2 must be a class or tuple of classes');
    }
    return boxBool(isSubclass(subBits, classBits));
  },
  object_new: () => {
    const objBits = boxPtrAddr(heapPtr);
    heapPtr = align(heapPtr + HEADER_SIZE + 8, 8);
    instanceClasses.set(ptrAddr(objBits), getBuiltinType(100));
    return objBits;
  },
  func_new: (fnIdx, trampolineIdx, arity) => {
    const addr = allocRaw(48);
    if (addr && memory) {
      const view = new DataView(memory.buffer);
      view.setBigInt64(addr, fnIdx, true);
      view.setBigInt64(addr + 8, arity, true);
      view.setBigInt64(addr + 40, trampolineIdx, true);
    }
    const tramp = Number(trampolineIdx);
    return boxPtr({
      type: 'function',
      idx: Number(fnIdx),
      arity: Number(arity),
      trampoline: Number.isFinite(tramp) ? tramp : 0,
      attrs: new Map(),
      memAddr: addr || null,
    });
  },
  func_new_closure: (fnIdx, trampolineIdx, arity, closureBits) => {
    const bits = baseImports.func_new(fnIdx, trampolineIdx, arity);
    const func = getFunction(bits);
    if (func) {
      func.closure = closureBits;
      if (func.memAddr && memory) {
        const view = new DataView(memory.buffer);
        view.setBigInt64(func.memAddr + 24, closureBits, true);
      }
    }
    return bits;
  },
  bound_method_new: (funcBits, selfBits) => {
    const addr = allocRaw(16);
    if (addr && memory) {
      const view = new DataView(memory.buffer);
      view.setBigInt64(addr, funcBits, true);
      view.setBigInt64(addr + 8, selfBits, true);
    }
    return boxPtr({
      type: 'bound_method',
      func: funcBits,
      self: selfBits,
      memAddr: addr || null,
    });
  },
  super_new: (typeBits, objBits) =>
    boxPtr({ type: 'super', startBits: typeBits, objBits }),
  classmethod_new: () => boxNone(),
  staticmethod_new: () => boxNone(),
  property_new: () => boxNone(),
  object_set_class: (objBits, classBits) => {
    const addr = expectPtrAddr(objBits, 'object_set_class');
    if (addr) {
      instanceClasses.set(addr, classBits);
    }
    return boxNone();
  },
  context_null: (val) => val,
  id: (val) => val,
  hash_builtin: (val) => {
    const hash = hashBitsSigned(val);
    if (exceptionPending() !== 0n) return boxNone();
    return boxIntOrBigint(hash);
  },
  ord: (val) => {
    const str = getStrObj(val);
    if (str !== null) {
      const chars = Array.from(str);
      if (chars.length !== 1) {
        throw new Error(
          `TypeError: ord() expected a character, but string of length ${chars.length} found`,
        );
      }
      return boxInt(BigInt(chars[0].codePointAt(0)));
    }
    const bytes = getBytes(val);
    if (bytes) {
      if (bytes.data.length !== 1) {
        throw new Error(
          `TypeError: ord() expected a character, but string of length ${bytes.data.length} found`,
        );
      }
      return boxInt(BigInt(bytes.data[0]));
    }
    const bytearray = getBytearray(val);
    if (bytearray) {
      if (bytearray.data.length !== 1) {
        throw new Error(
          `TypeError: ord() expected a character, but string of length ${bytearray.data.length} found`,
        );
      }
      return boxInt(BigInt(bytearray.data[0]));
    }
    throw new Error(`TypeError: ord() expected string of length 1, but ${typeName(val)} found`);
  },
  chr: (val) => {
    let codePoint = null;
    if (isIntLike(val)) {
      codePoint = unboxIntLike(val);
    } else {
      const indexAttr = lookupAttr(val, '__index__');
      if (indexAttr !== undefined) {
        const res = callCallable0(indexAttr);
        if (!isIntLike(res)) {
          throw new Error(`TypeError: __index__ returned non-int (type ${typeName(res)})`);
        }
        codePoint = unboxIntLike(res);
      }
    }
    if (codePoint === null) {
      throw new Error(
        `TypeError: '${typeName(val)}' object cannot be interpreted as an integer`,
      );
    }
    if (codePoint < 0n || codePoint > 0x10ffffn) {
      throw new Error('ValueError: chr() arg not in range(0x110000)');
    }
    return boxPtr({ type: 'str', value: String.fromCodePoint(Number(codePoint)) });
  },
  abs_builtin: (val) => {
    const big = getBigIntValue(val);
    if (big !== null) {
      const absVal = big < 0n ? -big : big;
      if (isTag(val, TAG_INT) || isTag(val, TAG_BOOL)) {
        return boxInt(absVal);
      }
      const obj = getObj(val);
      if (obj && obj.type === 'bigint') {
        return boxPtr({ type: 'bigint', value: absVal });
      }
      return boxInt(absVal);
    }
    if (isFloat(val)) {
      return boxFloat(Math.abs(bitsToFloat(val)));
    }
    const complexObj = getComplex(val);
    if (complexObj) {
      return boxFloat(Math.hypot(complexObj.re, complexObj.im));
    }
    const absAttr = lookupAttr(val, '__abs__');
    if (absAttr !== undefined) {
      return callCallable0(absAttr);
    }
    throw new Error(`TypeError: bad operand type for abs(): '${typeName(val)}'`);
  },
  divmod_builtin: (a, b) => {
    const li = getBigIntValue(a);
    const ri = getBigIntValue(b);
    if (li !== null && ri !== null) {
      if (ri === 0n) {
        throw new Error('ZeroDivisionError: division by zero');
      }
      let q = li / ri;
      let r = li % ri;
      if (r !== 0n && (r > 0n) !== (ri > 0n)) {
        q -= 1n;
        r += ri;
      }
      return tupleFromArray([boxInt(q), boxInt(r)]);
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      if (rf === 0) {
        throw new Error('ZeroDivisionError: division by zero');
      }
      const q = Math.floor(lf / rf);
      let r = lf % rf;
      if (r !== 0 && (r > 0) !== (rf > 0)) {
        r += rf;
      }
      return tupleFromArray([boxFloat(q), boxFloat(r)]);
    }
    throw new Error(
      `TypeError: unsupported operand type(s) for divmod(): '${typeName(
        a,
      )}' and '${typeName(b)}'`,
    );
  },
  getargv: () => {
    const items = runtimeArgv.map((arg) => boxPtr({ type: 'str', value: arg }));
    return listFromArray(items);
  },
  getrecursionlimit: () => boxInt(BigInt(recursionLimit)),
  setrecursionlimit: (limitBits) => {
    if (!isIntLike(limitBits)) {
      throw new Error(
        `TypeError: '${typeName(limitBits)}' object cannot be interpreted as an integer`,
      );
    }
    const limit = Number(unboxIntLike(limitBits));
    if (limit < 1) {
      throw new Error('ValueError: recursion limit must be greater or equal than 1');
    }
    if (limit <= recursionDepth) {
      throw new Error(
        `RecursionError: cannot set the recursion limit to ${limit} at the recursion depth ${recursionDepth}: the limit is too low`,
      );
    }
    recursionLimit = limit;
    return boxNone();
  },
  recursion_guard_enter: () => {
    if (recursionDepth + 1 > recursionLimit) {
      throw new Error('RecursionError: maximum recursion depth exceeded');
    }
    recursionDepth += 1;
    return 1n;
  },
  recursion_guard_exit: () => {
    if (recursionDepth > 0) {
      recursionDepth -= 1;
    }
  },
  trace_enter_slot: (codeId) => {
    const idx = Number(codeId);
    let codeBits = boxNone();
    if (codeSlots && Number.isSafeInteger(idx) && idx >= 0 && idx < codeSlots.length) {
      codeBits = codeSlots[idx];
    }
    frameStackPush(codeBits);
    return codeBits;
  },
  trace_set_line: (lineBits) => {
    const lineRaw = isIntLike(lineBits) ? Number(unboxIntLike(lineBits)) : 0;
    const line = Number.isFinite(lineRaw) ? lineRaw : 0;
    frameStackSetLine(line);
    return boxNone();
  },
  trace_exit: () => {
    frameStackPop();
    return boxNone();
  },
  code_slots_init: (countBits) => {
    if (codeSlots !== null) return boxNone();
    const count = Number(countBits);
    if (!Number.isSafeInteger(count) || count < 0) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'MemoryError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code slot count too large' }),
        ),
      );
      return raiseException(exc);
    }
    codeSlots = new Array(count).fill(boxNone());
    return boxNone();
  },
  code_slot_set: (codeIdBits, codeBits) => {
    if (codeSlots === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'RuntimeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code slots not initialized' }),
        ),
      );
      return raiseException(exc);
    }
    const idx = Number(codeIdBits);
    if (!Number.isSafeInteger(idx) || idx < 0 || idx >= codeSlots.length) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'IndexError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'code slot out of range' })),
      );
      return raiseException(exc);
    }
    if (!isNone(codeBits) && !getCode(codeBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code slot expects code object' }),
        ),
      );
      return raiseException(exc);
    }
    codeSlots[idx] = codeBits;
    return boxNone();
  },
  code_new: (
    filenameBits,
    nameBits,
    firstlinenoBits,
    linetableBits,
    varnamesBits,
    argcountBits,
    posonlyargcountBits,
    kwonlyargcountBits,
  ) => {
    const filename = getStrObj(filenameBits);
    if (filename === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'code filename must be str' })),
      );
      return raiseException(exc);
    }
    const name = getStrObj(nameBits);
    if (name === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'code name must be str' })),
      );
      return raiseException(exc);
    }
    if (!isNone(linetableBits) && !getTuple(linetableBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code linetable must be tuple or None' }),
        ),
      );
      return raiseException(exc);
    }
    if (isNone(varnamesBits)) {
      varnamesBits = tupleFromArray([]);
    } else if (!getTuple(varnamesBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code varnames must be tuple or None' }),
        ),
      );
      return raiseException(exc);
    }
    if (!isIntLike(argcountBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'code argcount must be int' })),
      );
      return raiseException(exc);
    }
    if (!isIntLike(posonlyargcountBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code posonlyargcount must be int' }),
        ),
      );
      return raiseException(exc);
    }
    if (!isIntLike(kwonlyargcountBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code kwonlyargcount must be int' }),
        ),
      );
      return raiseException(exc);
    }
    let firstlineno = 0;
    if (isIntLike(firstlinenoBits)) {
      const rawLine = Number(unboxIntLike(firstlinenoBits));
      firstlineno = Number.isFinite(rawLine) ? Math.trunc(rawLine) : 0;
    }
    const argcount = Number(unboxIntLike(argcountBits));
    const posonlyargcount = Number(unboxIntLike(posonlyargcountBits));
    const kwonlyargcount = Number(unboxIntLike(kwonlyargcountBits));
    if (argcount < 0 || posonlyargcount < 0 || kwonlyargcount < 0) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'ValueError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'code arg counts must be >= 0' }),
        ),
      );
      return raiseException(exc);
    }
    return boxPtr({
      type: 'code',
      filenameBits,
      nameBits,
      firstlineno,
      linetableBits,
      varnamesBits,
      argcount,
      posonlyargcount,
      kwonlyargcount,
    });
  },
  compile_builtin: (
    sourceBits,
    filenameBits,
    modeBits,
    flagsBits,
    dontInheritBits,
    optimizeBits,
  ) => {
    const source = getStrObj(sourceBits);
    if (source === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 1 must be a string' })),
      );
      return raiseException(exc);
    }
    const filename = getStrObj(filenameBits);
    if (filename === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 2 must be a string' })),
      );
      return raiseException(exc);
    }
    const mode = getStrObj(modeBits);
    if (mode === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 3 must be a string' })),
      );
      return raiseException(exc);
    }
    if (mode !== 'exec' && mode !== 'eval' && mode !== 'single') {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'ValueError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: "compile() mode must be 'exec', 'eval' or 'single'" }),
        ),
      );
      return raiseException(exc);
    }
    if (!isIntLike(flagsBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 4 must be int' })),
      );
      return raiseException(exc);
    }
    if (!isIntLike(dontInheritBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 5 must be int' })),
      );
      return raiseException(exc);
    }
    if (!isIntLike(optimizeBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'compile() arg 6 must be int' })),
      );
      return raiseException(exc);
    }
    const compileCheckNonlocal = (rawSource) => {
      const isIdent = (name) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
      const parseNameList = (raw) =>
        raw
          .split(',')
          .map((part) => part.trim())
          .filter((name) => isIdent(name));
      const parseParamNames = (raw) =>
        raw
          .split(',')
          .map((part) => part.trim())
          .filter((part) => part.length > 0)
          .map((part) => part.replace(/^\*+/, '').trim())
          .filter((part) => part.length > 0)
          .map((part) => part.split('=')[0].split(':')[0].trim())
          .filter((part) => isIdent(part));
      const bindingInOuter = (scopes, name) => {
        for (let i = scopes.length - 1; i >= 1; i -= 1) {
          const scope = scopes[i];
          if (scope.assigned.has(name) || scope.params.has(name)) {
            return true;
          }
        }
        return false;
      };
      const scopes = [
        {
          indent: 0,
          assigned: new Set(),
          globals: new Set(),
          nonlocals: new Set(),
          params: new Set(),
        },
      ];
      let pendingDefIndent = null;
      let pendingParams = [];
      const lines = rawSource.split(/\\r?\\n/);
      for (const raw of lines) {
        const stripped = raw.trimStart();
        if (!stripped || stripped.startsWith('#')) {
          continue;
        }
        const indent = raw.length - stripped.length;
        if (pendingDefIndent !== null && indent > pendingDefIndent) {
          const scope = {
            indent,
            assigned: new Set(),
            globals: new Set(),
            nonlocals: new Set(),
            params: new Set(),
          };
          for (const name of pendingParams) {
            scope.params.add(name);
            scope.assigned.add(name);
          }
          scopes.push(scope);
          pendingDefIndent = null;
          pendingParams = [];
        }
        while (scopes.length > 1 && indent < scopes[scopes.length - 1].indent) {
          const scope = scopes.pop();
          for (const name of scope.nonlocals) {
            if (!bindingInOuter(scopes, name)) {
              return `no binding for nonlocal '${name}' found`;
            }
          }
        }
        if (stripped.startsWith('def ') || stripped.startsWith('async def ')) {
          const header = stripped.startsWith('async def ')
            ? stripped.slice('async def '.length)
            : stripped.slice('def '.length);
          const name = header.split('(')[0].trim();
          if (isIdent(name)) {
            scopes[scopes.length - 1].assigned.add(name);
          }
          const start = header.indexOf('(');
          const end = header.lastIndexOf(')');
          if (start >= 0 && end > start) {
            pendingParams = parseParamNames(header.slice(start + 1, end));
          }
          pendingDefIndent = indent;
          continue;
        }
        if (stripped.startsWith('global ')) {
          for (const name of parseNameList(stripped.slice('global '.length))) {
            const scope = scopes[scopes.length - 1];
            if (scope.nonlocals.has(name)) {
              return `name '${name}' is nonlocal and global`;
            }
            scope.globals.add(name);
          }
          continue;
        }
        if (stripped.startsWith('nonlocal ')) {
          for (const name of parseNameList(stripped.slice('nonlocal '.length))) {
            const scope = scopes[scopes.length - 1];
            if (scope.globals.has(name)) {
              return `name '${name}' is nonlocal and global`;
            }
            scope.nonlocals.add(name);
          }
          continue;
        }
        if (
          stripped.includes('=') &&
          !stripped.includes('==') &&
          !stripped.includes('!=') &&
          !stripped.startsWith('return ') &&
          !stripped.startsWith('yield ') &&
          !stripped.startsWith('raise ') &&
          !stripped.startsWith('assert ')
        ) {
          const lhs = stripped.split('=', 1)[0].trim();
          for (const name of parseNameList(lhs)) {
            scopes[scopes.length - 1].assigned.add(name);
          }
        }
      }
      while (scopes.length > 1) {
        const scope = scopes.pop();
        for (const name of scope.nonlocals) {
          if (!bindingInOuter(scopes, name)) {
            return `no binding for nonlocal '${name}' found`;
          }
        }
      }
      return null;
    };
    const nonlocalMessage = compileCheckNonlocal(source);
    if (nonlocalMessage !== null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'SyntaxError' }),
        exceptionArgs(boxPtr({ type: 'str', value: nonlocalMessage })),
      );
      return raiseException(exc);
    }
    const nameBits = boxPtr({ type: 'str', value: '<module>' });
    return baseImports.code_new(
      filenameBits,
      nameBits,
      boxInt(1),
      boxNone(),
      tupleFromArray([]),
      boxInt(0),
      boxInt(0),
      boxInt(0),
    );
  },
  sum_builtin: (iterableBits, startBits) => {
    const iterBits = baseImports.iter(iterableBits);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterableBits)}' object is not iterable`);
    }
    let total = startBits;
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        return total;
      }
      total = baseImports.add(total, pair.items[0]);
    }
  },
  min_builtin: (argsBits, keyBits, defaultBits) => {
    const args = getTuple(argsBits);
    if (!args || args.items.length === 0) {
      throw new Error('TypeError: min expected at least 1 argument, got 0');
    }
    const missing = missingSentinel();
    const hasDefault = defaultBits !== missing;
    if (args.items.length > 1 && hasDefault) {
      throw new Error(
        'TypeError: Cannot specify a default for min() with multiple positional arguments',
      );
    }
    const useKey = !isTag(keyBits, TAG_NONE);
    if (args.items.length === 1) {
      const iterBits = baseImports.iter(args.items[0]);
      if (isTag(iterBits, TAG_NONE)) {
        throw new Error(`TypeError: '${typeName(args.items[0])}' object is not iterable`);
      }
      let best = null;
      let bestKey = null;
      while (true) {
        const pairBits = baseImports.iter_next(iterBits);
        const pair = getTuple(pairBits);
        if (!pair || pair.items.length < 2) {
          throw new Error('TypeError: object is not an iterator');
        }
        const doneBits = pair.items[1];
        if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
          if (best === null) {
            if (hasDefault) {
              return defaultBits;
            }
            throw new Error('ValueError: min() arg is an empty sequence');
          }
          return best;
        }
        const valBits = pair.items[0];
        if (best === null) {
          best = valBits;
          bestKey = useKey ? callCallable1(keyBits, valBits) : valBits;
          if (useKey && exceptionPending() !== 0n) return boxNone();
          continue;
        }
        const candKey = useKey ? callCallable1(keyBits, valBits) : valBits;
        if (useKey && exceptionPending() !== 0n) return boxNone();
        const cmp = compareKeys(candKey, bestKey, '<');
        if (cmp === null) return boxNone();
        if (cmp < 0) {
          best = valBits;
          bestKey = candKey;
        }
      }
    }
    let best = args.items[0];
    let bestKey = useKey ? callCallable1(keyBits, best) : best;
    if (useKey && exceptionPending() !== 0n) return boxNone();
    for (const valBits of args.items.slice(1)) {
      const candKey = useKey ? callCallable1(keyBits, valBits) : valBits;
      if (useKey && exceptionPending() !== 0n) return boxNone();
      const cmp = compareKeys(candKey, bestKey, '<');
      if (cmp === null) return boxNone();
      if (cmp < 0) {
        best = valBits;
        bestKey = candKey;
      }
    }
    return best;
  },
  max_builtin: (argsBits, keyBits, defaultBits) => {
    const args = getTuple(argsBits);
    if (!args || args.items.length === 0) {
      throw new Error('TypeError: max expected at least 1 argument, got 0');
    }
    const missing = missingSentinel();
    const hasDefault = defaultBits !== missing;
    if (args.items.length > 1 && hasDefault) {
      throw new Error(
        'TypeError: Cannot specify a default for max() with multiple positional arguments',
      );
    }
    const useKey = !isTag(keyBits, TAG_NONE);
    if (args.items.length === 1) {
      const iterBits = baseImports.iter(args.items[0]);
      if (isTag(iterBits, TAG_NONE)) {
        throw new Error(`TypeError: '${typeName(args.items[0])}' object is not iterable`);
      }
      let best = null;
      let bestKey = null;
      while (true) {
        const pairBits = baseImports.iter_next(iterBits);
        const pair = getTuple(pairBits);
        if (!pair || pair.items.length < 2) {
          throw new Error('TypeError: object is not an iterator');
        }
        const doneBits = pair.items[1];
        if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
          if (best === null) {
            if (hasDefault) {
              return defaultBits;
            }
            throw new Error('ValueError: max() arg is an empty sequence');
          }
          return best;
        }
        const valBits = pair.items[0];
        if (best === null) {
          best = valBits;
          bestKey = useKey ? callCallable1(keyBits, valBits) : valBits;
          if (useKey && exceptionPending() !== 0n) return boxNone();
          continue;
        }
        const candKey = useKey ? callCallable1(keyBits, valBits) : valBits;
        if (useKey && exceptionPending() !== 0n) return boxNone();
        const cmp = compareKeys(candKey, bestKey, '>');
        if (cmp === null) return boxNone();
        if (cmp > 0) {
          best = valBits;
          bestKey = candKey;
        }
      }
    }
    let best = args.items[0];
    let bestKey = useKey ? callCallable1(keyBits, best) : best;
    if (useKey && exceptionPending() !== 0n) return boxNone();
    for (const valBits of args.items.slice(1)) {
      const candKey = useKey ? callCallable1(keyBits, valBits) : valBits;
      if (useKey && exceptionPending() !== 0n) return boxNone();
      const cmp = compareKeys(candKey, bestKey, '>');
      if (cmp === null) return boxNone();
      if (cmp > 0) {
        best = valBits;
        bestKey = candKey;
      }
    }
    return best;
  },
  sorted_builtin: (iterBits, keyBits, reverseBits) => {
    const iterObj = baseImports.iter(iterBits);
    if (isTag(iterObj, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterBits)}' object is not iterable`);
    }
    const useKey = !isTag(keyBits, TAG_NONE);
    const reverse = isTruthyBits(reverseBits);
    const items = [];
    while (true) {
      const pairBits = baseImports.iter_next(iterObj);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        break;
      }
      const valBits = pair.items[0];
      const keyVal = useKey ? callCallable1(keyBits, valBits) : valBits;
      if (useKey && exceptionPending() !== 0n) return boxNone();
      items.push({ key: keyVal, val: valBits, idx: items.length });
    }
    let error = null;
    items.sort((left, right) => {
      if (error) return 0;
      const outcome = compareObjects(left.key, right.key);
      if (outcome.kind === 'ordered') {
        if (outcome.ordering !== 0) {
          return reverse ? -outcome.ordering : outcome.ordering;
        }
      } else if (outcome.kind === 'notComparable') {
        error = { kind: 'notComparable', left: left.key, right: right.key };
        return 0;
      } else if (outcome.kind === 'error') {
        error = { kind: 'exception' };
        return 0;
      }
      return left.idx - right.idx;
    });
    if (error) {
      if (error.kind === 'exception') return boxNone();
      compareTypeError('<', error.left, error.right);
    }
    return listFromArray(items.map((item) => item.val));
  },
  map_builtin: (funcBits, iterablesBits) => {
    const iterables = getTuple(iterablesBits);
    if (!iterables) {
      throw new Error('TypeError: map expects a tuple');
    }
    if (iterables.items.length === 0) {
      throw new Error('TypeError: map() must have at least two arguments');
    }
    const iters = [];
    for (const iterable of iterables.items) {
      const iterBits = baseImports.iter(iterable);
      if (isTag(iterBits, TAG_NONE)) {
        throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
      }
      iters.push(iterBits);
    }
    return boxPtr({ type: 'map', func: funcBits, iters });
  },
  filter_builtin: (funcBits, iterableBits) => {
    const iterBits = baseImports.iter(iterableBits);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterableBits)}' object is not iterable`);
    }
    return boxPtr({ type: 'filter', func: funcBits, iterBits });
  },
  zip_builtin: (iterablesBits) => {
    const iterables = getTuple(iterablesBits);
    if (!iterables) {
      throw new Error('TypeError: zip expects a tuple');
    }
    const iters = [];
    for (const iterable of iterables.items) {
      const iterBits = baseImports.iter(iterable);
      if (isTag(iterBits, TAG_NONE)) {
        throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
      }
      iters.push(iterBits);
    }
    return boxPtr({ type: 'zip', iters });
  },
  reversed_builtin: (seqBits) => {
    const list = getList(seqBits);
    if (list) {
      return boxPtr({ type: 'reversed', target: seqBits, idx: list.items.length });
    }
    const tup = getTuple(seqBits);
    if (tup) {
      return boxPtr({ type: 'reversed', target: seqBits, idx: tup.items.length });
    }
    const bytes = getBytes(seqBits);
    if (bytes) {
      return boxPtr({ type: 'reversed', target: seqBits, idx: bytes.data.length });
    }
    const bytearray = getBytearray(seqBits);
    if (bytearray) {
      return boxPtr({ type: 'reversed', target: seqBits, idx: bytearray.data.length });
    }
    const dict = getDict(seqBits);
    if (dict) {
      return boxPtr({ type: 'reversed', target: seqBits, idx: dict.entries.length });
    }
    const strVal = getStrObj(seqBits);
    if (strVal !== null) {
      return boxPtr({
        type: 'reversed',
        target: seqBits,
        idx: Array.from(strVal).length,
      });
    }
    throw new Error(`TypeError: '${typeName(seqBits)}' object is not reversible`);
  },
  context_enter: (val) => val,
  context_exit: () => boxBool(false),
  context_unwind: () => boxBool(false),
  context_depth: () => boxInt(0),
  context_unwind_to: () => boxNone(),
  env_get: () => boxNone(),
  getpid: () => {
    const pid = typeof process !== 'undefined' ? process.pid : 0;
    return boxInt(BigInt(pid ?? 0));
  },
  time_monotonic: () => {
    const now =
      typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
        ? process.hrtime.bigint()
        : BigInt(Date.now()) * 1000000n;
    const delta = now - MONO_START;
    return boxFloat(Number(delta) / 1e9);
  },
  time_monotonic_ns: () => {
    const now =
      typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
        ? process.hrtime.bigint()
        : BigInt(Date.now()) * 1000000n;
    const delta = now - MONO_START;
    return boxIntOrBigint(delta);
  },
  time_perf_counter: () => {
    const now =
      typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
        ? process.hrtime.bigint()
        : BigInt(Date.now()) * 1000000n;
    const delta = now - MONO_START;
    return boxFloat(Number(delta) / 1e9);
  },
  time_perf_counter_ns: () => {
    const now =
      typeof process !== 'undefined' && process.hrtime && process.hrtime.bigint
        ? process.hrtime.bigint()
        : BigInt(Date.now()) * 1000000n;
    const delta = now - MONO_START;
    return boxIntOrBigint(delta);
  },
  time_process_time: () => {
    if (typeof process !== 'undefined' && process.cpuUsage) {
      const usage = process.cpuUsage();
      const totalUs = (usage.user || 0) + (usage.system || 0);
      return boxFloat(totalUs / 1e6);
    }
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'OSError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'process_time unavailable' })),
    );
    return raiseException(exc);
  },
  time_process_time_ns: () => {
    if (typeof process !== 'undefined' && process.cpuUsage) {
      const usage = process.cpuUsage();
      const totalUs = BigInt((usage.user || 0) + (usage.system || 0));
      return boxIntOrBigint(totalUs * 1000n);
    }
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'OSError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'process_time unavailable' })),
    );
    return raiseException(exc);
  },
  time_time: () => boxFloat(Date.now() / 1000),
  time_time_ns: () => boxIntOrBigint(BigInt(Date.now()) * 1000000n),
  time_localtime: (secsBits) => {
    const parseSeconds = (bits) => {
      if (isTag(bits, TAG_NONE)) {
        return { ok: true, value: Date.now() / 1000 };
      }
      if (isFloat(bits)) {
        const val = bitsToFloat(bits);
        if (!Number.isFinite(val)) {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'OverflowError' }),
            exceptionArgs(
              boxPtr({
                type: 'str',
                value: 'timestamp out of range for platform time_t',
              }),
            ),
          );
          return { ok: false, bits: raiseException(exc) };
        }
        return { ok: true, value: val };
      }
      const big = getBigIntValue(bits);
      if (big !== null) {
        return { ok: true, value: Number(big) };
      }
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `an integer is required (got type ${typeName(bits)})`,
          }),
        ),
      );
      return { ok: false, bits: raiseException(exc) };
    };
    const result = parseSeconds(secsBits);
    if (!result.ok) return result.bits;
    const date = new Date(result.value * 1000);
    const year = date.getFullYear();
    const month = date.getMonth() + 1;
    const day = date.getDate();
    const hour = date.getHours();
    const minute = date.getMinutes();
    const second = date.getSeconds();
    const wday = (date.getDay() + 6) % 7;
    const start = new Date(year, 0, 1).getTime();
    const yday = Math.floor((date.getTime() - start) / 86400000) + 1;
    const jan = new Date(year, 0, 1);
    const jul = new Date(year, 6, 1);
    const stdOffset = Math.max(jan.getTimezoneOffset(), jul.getTimezoneOffset());
    const isdst = date.getTimezoneOffset() < stdOffset ? 1 : 0;
    return tupleFromArray([
      boxInt(year),
      boxInt(month),
      boxInt(day),
      boxInt(hour),
      boxInt(minute),
      boxInt(second),
      boxInt(wday),
      boxInt(yday),
      boxInt(isdst),
    ]);
  },
  time_gmtime: (secsBits) => {
    const parseSeconds = (bits) => {
      if (isTag(bits, TAG_NONE)) {
        return { ok: true, value: Date.now() / 1000 };
      }
      if (isFloat(bits)) {
        const val = bitsToFloat(bits);
        if (!Number.isFinite(val)) {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'OverflowError' }),
            exceptionArgs(
              boxPtr({
                type: 'str',
                value: 'timestamp out of range for platform time_t',
              }),
            ),
          );
          return { ok: false, bits: raiseException(exc) };
        }
        return { ok: true, value: val };
      }
      const big = getBigIntValue(bits);
      if (big !== null) {
        return { ok: true, value: Number(big) };
      }
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `an integer is required (got type ${typeName(bits)})`,
          }),
        ),
      );
      return { ok: false, bits: raiseException(exc) };
    };
    const result = parseSeconds(secsBits);
    if (!result.ok) return result.bits;
    const date = new Date(result.value * 1000);
    const year = date.getUTCFullYear();
    const month = date.getUTCMonth() + 1;
    const day = date.getUTCDate();
    const hour = date.getUTCHours();
    const minute = date.getUTCMinutes();
    const second = date.getUTCSeconds();
    const wday = (date.getUTCDay() + 6) % 7;
    const start = Date.UTC(year, 0, 1);
    const yday = Math.floor((date.getTime() - start) / 86400000) + 1;
    return tupleFromArray([
      boxInt(year),
      boxInt(month),
      boxInt(day),
      boxInt(hour),
      boxInt(minute),
      boxInt(second),
      boxInt(wday),
      boxInt(yday),
      boxInt(0),
    ]);
  },
  time_strftime: (fmtBits, timeBits) => {
    const fmt = getStrObj(fmtBits);
    if (fmt === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'strftime() format must be str' })),
      );
      return raiseException(exc);
    }
    if (fmt.includes('\0')) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'ValueError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'embedded null character' })),
      );
      return raiseException(exc);
    }
    const tuple = getTuple(timeBits);
    if (!tuple) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'strftime() argument 2 must be tuple' })),
      );
      return raiseException(exc);
    }
    if (tuple.items.length !== 9) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({ type: 'str', value: 'time tuple must have exactly 9 elements' }),
        ),
      );
      return raiseException(exc);
    }
    const vals = [];
    for (const item of tuple.items) {
      const big = getBigIntValue(item);
      if (big === null) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `an integer is required (got type ${typeName(item)})`,
            }),
          ),
        );
        return raiseException(exc);
      }
      vals.push(Number(big));
    }
    const [year, month, day, hour, minute, second, wday, yday] = vals;
    const WEEKDAY_SHORT = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
    const MONTH_SHORT = [
      'Jan',
      'Feb',
      'Mar',
      'Apr',
      'May',
      'Jun',
      'Jul',
      'Aug',
      'Sep',
      'Oct',
      'Nov',
      'Dec',
    ];
    const padNum = (value, width, pad = '0') => {
      const sign = value < 0 ? '-' : '';
      const abs = Math.abs(value);
      const body = String(abs).padStart(width, pad);
      return `${sign}${body}`;
    };
    let out = '';
    for (let idx = 0; idx < fmt.length; idx++) {
      const ch = fmt[idx];
      if (ch !== '%') {
        out += ch;
        continue;
      }
      idx += 1;
      if (idx >= fmt.length) {
        out += '%';
        break;
      }
      const spec = fmt[idx];
      switch (spec) {
        case '%':
          out += '%';
          break;
        case 'Y':
          out += padNum(year, 4, '0');
          break;
        case 'y':
          out += padNum(((year % 100) + 100) % 100, 2, '0');
          break;
        case 'm':
          out += padNum(month, 2, '0');
          break;
        case 'd':
          out += padNum(day, 2, '0');
          break;
        case 'H':
          out += padNum(hour, 2, '0');
          break;
        case 'M':
          out += padNum(minute, 2, '0');
          break;
        case 'S':
          out += padNum(second, 2, '0');
          break;
        case 'a':
          out += WEEKDAY_SHORT[wday] || '';
          break;
        case 'b':
        case 'h':
          out += MONTH_SHORT[month - 1] || '';
          break;
        case 'c':
          out += `${WEEKDAY_SHORT[wday]} ${MONTH_SHORT[month - 1]} ${padNum(
            day,
            2,
            ' ',
          )} ${padNum(hour, 2, '0')}:${padNum(minute, 2, '0')}:${padNum(
            second,
            2,
            '0',
          )} ${padNum(year, 4, '0')}`;
          break;
        case 'x':
          out += `${padNum(month, 2, '0')}/${padNum(day, 2, '0')}/${padNum(
            ((year % 100) + 100) % 100,
            2,
            '0',
          )}`;
          break;
        case 'X':
          out += `${padNum(hour, 2, '0')}:${padNum(minute, 2, '0')}:${padNum(
            second,
            2,
            '0',
          )}`;
          break;
        case 'Z':
          out += 'UTC';
          break;
        case 'z':
          out += '+0000';
          break;
        default: {
          const exc = exceptionNew(
            boxPtr({ type: 'str', value: 'ValueError' }),
            exceptionArgs(
              boxPtr({
                type: 'str',
                value: `unsupported strftime directive %${spec}`,
              }),
            ),
          );
          return raiseException(exc);
        }
      }
    }
    return boxPtr({ type: 'str', value: out });
  },
  time_timezone: () => boxInt(0),
  time_tzname: () =>
    tupleFromArray([
      boxPtr({ type: 'str', value: 'UTC' }),
      boxPtr({ type: 'str', value: 'UTC' }),
    ]),
  math_log: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    if (Number.isNaN(num)) return floatBits;
    if (num <= 0) {
      throw new Error('ValueError: math domain error');
    }
    return boxFloat(Math.log(num));
  },
  math_log2: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    if (Number.isNaN(num)) return floatBits;
    if (num <= 0) {
      throw new Error('ValueError: math domain error');
    }
    if (Math.log2) return boxFloat(Math.log2(num));
    return boxFloat(Math.log(num) / Math.LN2);
  },
  math_exp: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    return boxFloat(Math.exp(num));
  },
  math_sin: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    return boxFloat(Math.sin(num));
  },
  math_cos: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    return boxFloat(Math.cos(num));
  },
  math_acos: (val) => {
    const floatBits = baseImports.float_from_obj(val);
    const num = bitsToFloat(floatBits);
    if (Number.isNaN(num)) return floatBits;
    if (num < -1 || num > 1) {
      throw new Error('ValueError: math domain error');
    }
    return boxFloat(Math.acos(num));
  },
  math_lgamma: (..._args) => baseImports.unsupported_import('math_lgamma'),
  path_exists: (pathBits) => {
    const path = getStrObj(pathBits);
    if (path === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'path must be str' })),
      );
      return raiseException(exc);
    }
    return boxBool(fs.existsSync(path));
  },
  path_unlink: (pathBits) => {
    const path = getStrObj(pathBits);
    if (path === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'path must be str' })),
      );
      return raiseException(exc);
    }
    try {
      fs.unlinkSync(path);
      return boxNone();
    } catch (err) {
      const msg = err && err.message ? err.message : String(err);
      const code = err && err.code ? err.code : null;
      let kind = 'OSError';
      if (code === 'ENOENT') {
        kind = 'FileNotFoundError';
      } else if (code === 'EACCES' || code === 'EPERM') {
        kind = 'PermissionError';
      } else if (code === 'EISDIR') {
        kind = 'IsADirectoryError';
      }
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: kind }),
        exceptionArgs(boxPtr({ type: 'str', value: msg })),
      );
      return raiseException(exc);
    }
  },
  path_chmod: (..._args) => baseImports.unsupported_import('path_chmod'),
  exception_push: () => exceptionPush(),
  exception_pop: () => exceptionPop(),
  exception_last: () => exceptionLast(),
  exception_active: () => exceptionActive(),
  exception_new: (kind, args) => exceptionNew(kind, args),
  exception_new_from_class: (cls, args) => exceptionNewFromClass(cls, args),
  exception_class: (kind) => exceptionClass(kind),
  exception_clear: () => exceptionClear(),
  exception_pending: () => exceptionPending(),
  exception_kind: (exc) => exceptionKind(exc),
  exception_message: (exc) => exceptionMessage(exc),
  exception_set_cause: (exc, cause) => exceptionSetCause(exc, cause),
  exception_set_value: (exc, value) => exceptionSetValue(exc, value),
  exception_context_set: (exc) => exceptionContextSet(exc),
  exception_set_last: (exc) => exceptionSetLast(exc),
  raise: (exc) => raiseException(exc),
  context_closing: (val) => val,
  bridge_unavailable: () => boxNone(),
  open_builtin: (
    fileBits,
    modeBits,
    _bufferingBits,
    _encodingBits,
    _errorsBits,
    _newlineBits,
    _closefdBits,
    _openerBits,
  ) => {
    if (isNone(modeBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: "open() argument 'mode' must be str, not NoneType",
          }),
        ),
      );
      return raiseException(exc);
    }
    const mode = getStrObj(modeBits);
    if (mode === null) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `open() argument 'mode' must be str, not ${typeName(modeBits)}`,
          }),
        ),
      );
      return raiseException(exc);
    }
    const readable = mode === '' || mode.includes('r') || mode.includes('+');
    const writable = mode.includes('w') || mode.includes('a') || mode.includes('x') || mode.includes('+');
    if (readable) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'PermissionError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'missing fs.read capability' })),
      );
      return raiseException(exc);
    }
    if (writable) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'PermissionError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'missing fs.write capability' })),
      );
      return raiseException(exc);
    }
    const exc = exceptionNew(
      boxPtr({ type: 'str', value: 'ValueError' }),
      exceptionArgs(boxPtr({ type: 'str', value: 'invalid mode' })),
    );
    return raiseException(exc);
  },
  file_open: () => boxNone(),
  file_read: () => boxNone(),
  file_readline: () => boxNone(),
  file_readlines: () => boxNone(),
  file_readinto: () => boxNone(),
  file_readinto1: () => boxNone(),
  file_write: () => boxNone(),
  file_writelines: () => boxNone(),
  file_seek: () => boxNone(),
  file_tell: () => boxNone(),
  file_fileno: () => boxNone(),
  file_truncate: () => boxNone(),
  file_flush: () => boxNone(),
  file_readable: () => boxNone(),
  file_writable: () => boxNone(),
  file_seekable: () => boxNone(),
  file_isatty: () => boxNone(),
  file_close: () => boxNone(),
  file_detach: () => boxNone(),
  file_reconfigure: () => boxNone(),
  db_query: (ptr, _len, out, _token) => {
    expectPtrAddr(ptr, 'db_query');
    if (!memory) return 7;
    const view = new DataView(memory.buffer);
    const outAddr = expectPtrAddr(out, 'db_query');
    if (outAddr === 0) return 2;
    const stream = streamCreate();
    view.setBigInt64(outAddr, stream.handle, true);
    return 0;
  },
  db_exec: (ptr, _len, out, _token) => {
    expectPtrAddr(ptr, 'db_exec');
    if (!memory) return 7;
    const view = new DataView(memory.buffer);
    const outAddr = expectPtrAddr(out, 'db_exec');
    if (outAddr === 0) return 2;
    const stream = streamCreate();
    view.setBigInt64(outAddr, stream.handle, true);
    return 0;
  },
  db_query_obj: (payloadBits, _token) => {
    const data = getBytes(payloadBits) || getBytearray(payloadBits);
    if (!data) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'send expects bytes-like object' })),
      );
      return raiseException(exc);
    }
    const stream = streamCreate();
    return stream.handle;
  },
  db_exec_obj: (payloadBits, _token) => {
    const data = getBytes(payloadBits) || getBytearray(payloadBits);
    if (!data) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'send expects bytes-like object' })),
      );
      return raiseException(exc);
    }
    const stream = streamCreate();
    return stream.handle;
  },
  db_host_poll: () => 0,
  stream_new: (capacity) => streamCreate(capacity).handle,
  stream_send: (handle, ptr, len) => {
    expectPtrAddr(ptr, 'stream_send');
    const bytes = readMemoryBytes(ptr, len);
    const stream = streamGet(handle);
    if (!stream) return boxPending();
    if (!stream.closed) {
      stream.queue.push(bytes);
    }
    return 0n;
  },
  stream_recv: (handle) => {
    const stream = streamGet(handle);
    if (!stream) return boxNone();
    if (stream.queue.length === 0) {
      if (stream.closed) return boxNone();
      baseImports.db_host_poll();
      if (stream.queue.length === 0) {
        return stream.closed ? boxNone() : boxPending();
      }
    }
    const chunk = stream.queue.shift();
    return boxBytes(chunk);
  },
  stream_close: (handle) => {
    const stream = streamGet(handle);
    if (stream) stream.closed = true;
  },
  stream_drop: (handle) => {
    streamRelease(handle);
  },
  ws_connect: (ptr, _len, out) => {
    expectPtrAddr(ptr, 'ws_connect');
    expectPtrAddr(out, 'ws_connect');
    return 0;
  },
  ws_pair: (_capacity, outLeft, outRight) => {
    expectPtrAddr(outLeft, 'ws_pair');
    expectPtrAddr(outRight, 'ws_pair');
    return 0;
  },
  ws_send: (_handle, ptr, _len) => {
    expectPtrAddr(ptr, 'ws_send');
    return 0n;
  },
  ws_connect_obj: (..._args) => baseImports.unsupported_import('ws_connect_obj'),
  ws_pair_obj: (..._args) => baseImports.unsupported_import('ws_pair_obj'),
  ws_send_obj: (..._args) => baseImports.unsupported_import('ws_send_obj'),
  ws_recv: () => 0n,
  ws_close: () => {},
  ws_drop: () => {},
  errno_constants: () => {
    const errno = os.constants && os.constants.errno ? os.constants.errno : {};
    const names = [
      'EACCES',
      'EAGAIN',
      'EALREADY',
      'ECHILD',
      'ECONNABORTED',
      'ECONNREFUSED',
      'ECONNRESET',
      'EEXIST',
      'EINPROGRESS',
      'EINTR',
      'EISDIR',
      'ENOENT',
      'ENOTDIR',
      'EPERM',
      'EPIPE',
      'ESRCH',
      'ETIMEDOUT',
      'EWOULDBLOCK',
      'ESHUTDOWN',
      'ENOTCAPABLE',
    ];
    const dictBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
    const reverseBits = boxPtr({ type: 'dict', entries: [], lookup: new Map() });
    const dict = getDict(dictBits);
    const reverse = getDict(reverseBits);
    if (!dict || !reverse) return boxNone();
    for (const name of names) {
      let value = errno[name];
      if (value === undefined && name === 'EWOULDBLOCK') {
        value = errno.EAGAIN;
      }
      if (value === undefined) {
        continue;
      }
      const nameBits = boxPtr({ type: 'str', value: name });
      const valueBits = boxInt(BigInt(value));
      dictSetValue(dict, nameBits, valueBits);
      dictSetValue(reverse, valueBits, nameBits);
    }
    return tupleFromArray([dictBits, reverseBits]);
  },
  missing: () => missingSentinel(),
  ellipsis: () => ellipsisObj(),
  not_implemented: () => notImplementedSentinel(),
  repr_builtin: (val) => baseImports.repr_from_obj(val),
  format_builtin: (val, specBits) => {
    const spec = getStrObj(specBits);
    if (spec === null) {
      throw new Error(
        `TypeError: format() argument 2 must be str, not ${typeName(specBits)}`,
      );
    }
    if (spec.length === 0) {
      return baseImports.str_from_obj(val);
    }
    return baseImports.string_format(val, specBits);
  },
  callable_builtin: (val) => baseImports.is_callable(val),
  round_builtin: (val, ndigitsBits) => {
    const missing = missingSentinel();
    const hasNdigits = ndigitsBits !== missing;
    const ndigits = hasNdigits ? ndigitsBits : boxNone();
    return baseImports.round(val, ndigits, boxBool(hasNdigits));
  },
  enumerate_builtin: (iterable, startBits) => {
    const missing = missingSentinel();
    const hasStart = startBits !== missing;
    const start = hasStart ? startBits : boxInt(0);
    return baseImports.enumerate(iterable, start, boxBool(hasStart));
  },
  next_builtin: (iterBits, defaultBits) => {
    const missing = missingSentinel();
    const pairBits = baseImports.iter_next(iterBits);
    const pair = getTuple(pairBits);
    if (!pair || pair.items.length < 2) {
      throw new Error('TypeError: object is not an iterator');
    }
    const valBits = pair.items[0];
    const doneBits = pair.items[1];
    if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
      if (defaultBits !== missing) {
        return defaultBits;
      }
      const argsBits = isTag(valBits, TAG_NONE)
        ? tupleFromArray([])
        : tupleFromArray([valBits]);
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'StopIteration' }),
        argsBits,
      );
      return raiseException(exc);
    }
    return valBits;
  },
  any_builtin: (iterable) => {
    const iterBits = baseImports.iter(iterable);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
    }
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const valBits = pair.items[0];
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        return boxBool(false);
      }
      if (baseImports.is_truthy(valBits) === 1n) {
        return boxBool(true);
      }
    }
  },
  all_builtin: (iterable) => {
    const iterBits = baseImports.iter(iterable);
    if (isTag(iterBits, TAG_NONE)) {
      throw new Error(`TypeError: '${typeName(iterable)}' object is not iterable`);
    }
    while (true) {
      const pairBits = baseImports.iter_next(iterBits);
      const pair = getTuple(pairBits);
      if (!pair || pair.items.length < 2) {
        throw new Error('TypeError: object is not an iterator');
      }
      const valBits = pair.items[0];
      const doneBits = pair.items[1];
      if (isTag(doneBits, TAG_BOOL) && (doneBits & 1n) === 1n) {
        return boxBool(true);
      }
      if (baseImports.is_truthy(valBits) === 0n) {
        return boxBool(false);
      }
    }
  },
  getattr_builtin: (objBits, nameBits, defaultBits) => {
    const missing = missingSentinel();
    if (defaultBits === missing) {
      return baseImports.get_attr_name(objBits, nameBits);
    }
    return baseImports.get_attr_name_default(objBits, nameBits, defaultBits);
  },
  vars_builtin: (objBits) => {
    const dictNameBits = boxPtr({ type: 'str', value: '__dict__' });
    const missing = missingSentinel();
    const dictBits = baseImports.get_attr_name_default(objBits, dictNameBits, missing);
    if (exceptionPending() !== 0n) return boxNone();
    if (dictBits === missing) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: 'vars() argument must have __dict__ attribute',
          }),
        ),
      );
      return raiseException(exc);
    }
    return dictBits;
  },
  dir_builtin: (objBits) => {
    const dirMethodBits = lookupAttr(objBits, '__dir__');
    if (dirMethodBits !== undefined) {
      return callCallable0(dirMethodBits);
    }
    const names = new Set();
    const obj = getObj(objBits);
    if (obj && obj.type === 'module') {
      collectDirNamesFromDict(names, obj.dictBits);
    } else {
      collectDirNamesFromInstanceAttrs(names, objBits);
      const dictBits = lookupAttr(objBits, '__dict__');
      if (dictBits !== undefined) {
        collectDirNamesFromDict(names, dictBits);
      }
      const typeBits = getClass(objBits) ? objBits : typeOfBits(objBits);
      collectDirNamesFromClass(names, typeBits);
    }
    const out = Array.from(names);
    out.sort();
    return listFromArray(out.map((name) => boxPtr({ type: 'str', value: name })));
  },
  anext_builtin: (iterBits, defaultBits) => {
    const missing = missingSentinel();
    if (defaultBits === missing) {
      return baseImports.anext(iterBits);
    }
    if (!memory || !table) return boxNone();
    const pollFn = baseImports.anext_default_poll;
    let pollIdx = anextDefaultPollIdx;
    if (pollIdx === null) {
      pollIdx = getOrAddTableFunc(pollFn, 1);
      if (pollIdx === null) return boxNone();
      anextDefaultPollIdx = pollIdx;
    }
    const addr = allocRaw(24);
    if (!addr) return boxNone();
    const view = new DataView(memory.buffer);
    view.setUint32(addr - HEADER_POLL_FN_OFFSET, pollIdx, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
    view.setBigInt64(addr + 0, iterBits, true);
    view.setBigInt64(addr + 8, defaultBits, true);
    view.setBigInt64(addr + 16, boxNone(), true);
    return boxPtrAddr(addr);
  },
  print_builtin: (argsBits, sepBits, endBits, fileBits, flushBits) => {
    const args = getTuple(argsBits);
    if (!args) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'print expects a tuple' })),
      );
      return raiseException(exc);
    }
    const stringArg = (bits, defaultVal, label) => {
      if (isNone(bits)) return defaultVal;
      const text = getStrObj(bits);
      if (text === null) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'TypeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `${label} must be None or a string, not ${typeName(bits)}`,
            }),
          ),
        );
        raiseException(exc);
        return null;
      }
      return text;
    };
    const sep = stringArg(sepBits, ' ', 'sep');
    if (sep === null) return boxNone();
    const end = stringArg(endBits, '\\n', 'end');
    if (end === null) return boxNone();
    const parts = [];
    for (const val of args.items) {
      const strBits = baseImports.str_from_obj(val);
      const text = getStrObj(strBits);
      parts.push(text === null ? '<obj>' : text);
    }
    const output = parts.join(sep) + end;
    const doFlush = baseImports.is_truthy(flushBits) !== 0n;
    if (isNone(fileBits)) {
      if (typeof process !== 'undefined' && process.stdout && process.stdout.write) {
        process.stdout.write(output);
        if (doFlush && process.stdout.flush) {
          process.stdout.flush();
        }
      } else if (output.endsWith('\\n')) {
        console.log(output.slice(0, -1));
      } else {
        console.log(output);
      }
      return boxNone();
    }
    const writeBits = lookupAttr(fileBits, 'write');
    if (writeBits === undefined) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'AttributeError' }),
        exceptionArgs(
          boxPtr({
            type: 'str',
            value: `'${typeName(fileBits)}' object has no attribute 'write'`,
          }),
        ),
      );
      return raiseException(exc);
    }
    const textBits = boxPtr({ type: 'str', value: output });
    callCallable1(writeBits, textBits);
    if (doFlush) {
      const flushMethodBits = lookupAttr(fileBits, 'flush');
      if (flushMethodBits === undefined) {
        const exc = exceptionNew(
          boxPtr({ type: 'str', value: 'AttributeError' }),
          exceptionArgs(
            boxPtr({
              type: 'str',
              value: `'${typeName(fileBits)}' object has no attribute 'flush'`,
            }),
          ),
        );
        return raiseException(exc);
      }
      callCallable0(flushMethodBits);
    }
    return boxNone();
  },
  super_builtin: (typeBits, objBits) => baseImports.super_new(typeBits, objBits),

  io_wait: (..._args) => baseImports.unsupported_import('io_wait'),
  io_wait_new: (..._args) => baseImports.unsupported_import('io_wait_new'),
  ws_wait: (..._args) => baseImports.unsupported_import('ws_wait'),
  ws_wait_new: (..._args) => baseImports.unsupported_import('ws_wait_new'),
  thread_submit: (..._args) => baseImports.unsupported_import('thread_submit'),
  thread_poll: (..._args) => baseImports.unsupported_import('thread_poll'),
  process_spawn: (..._args) => baseImports.unsupported_import('process_spawn'),
  process_wait_future: (..._args) => baseImports.unsupported_import('process_wait_future'),
  process_poll: (..._args) => baseImports.unsupported_import('process_poll'),
  process_pid: (..._args) => baseImports.unsupported_import('process_pid'),
  process_returncode: (..._args) => baseImports.unsupported_import('process_returncode'),
  process_kill: (..._args) => baseImports.unsupported_import('process_kill'),
  process_terminate: (..._args) => baseImports.unsupported_import('process_terminate'),
  process_stdin: (..._args) => baseImports.unsupported_import('process_stdin'),
  process_stdout: (..._args) => baseImports.unsupported_import('process_stdout'),
  process_stderr: (..._args) => baseImports.unsupported_import('process_stderr'),
  process_drop: (..._args) => baseImports.unsupported_import('process_drop'),
  socket_constants: (..._args) => baseImports.unsupported_import('socket_constants'),
  socket_has_ipv6: (..._args) => baseImports.unsupported_import('socket_has_ipv6'),
  socket_new: (..._args) => baseImports.unsupported_import('socket_new'),
  socket_close: (..._args) => baseImports.unsupported_import('socket_close'),
  socket_drop: (..._args) => baseImports.unsupported_import('socket_drop'),
  socket_clone: (..._args) => baseImports.unsupported_import('socket_clone'),
  socket_fileno: (..._args) => baseImports.unsupported_import('socket_fileno'),
  socket_gettimeout: (..._args) => baseImports.unsupported_import('socket_gettimeout'),
  socket_settimeout: (..._args) => baseImports.unsupported_import('socket_settimeout'),
  socket_setblocking: (..._args) => baseImports.unsupported_import('socket_setblocking'),
  socket_getblocking: (..._args) => baseImports.unsupported_import('socket_getblocking'),
  socket_bind: (..._args) => baseImports.unsupported_import('socket_bind'),
  socket_listen: (..._args) => baseImports.unsupported_import('socket_listen'),
  socket_accept: (..._args) => baseImports.unsupported_import('socket_accept'),
  socket_connect: (..._args) => baseImports.unsupported_import('socket_connect'),
  socket_connect_ex: (..._args) => baseImports.unsupported_import('socket_connect_ex'),
  socket_recv: (..._args) => baseImports.unsupported_import('socket_recv'),
  socket_recv_into: (..._args) => baseImports.unsupported_import('socket_recv_into'),
  socket_send: (..._args) => baseImports.unsupported_import('socket_send'),
  socket_sendall: (..._args) => baseImports.unsupported_import('socket_sendall'),
  socket_sendto: (..._args) => baseImports.unsupported_import('socket_sendto'),
  socket_recvfrom: (..._args) => baseImports.unsupported_import('socket_recvfrom'),
  socket_shutdown: (..._args) => baseImports.unsupported_import('socket_shutdown'),
  socket_getsockname: (..._args) => baseImports.unsupported_import('socket_getsockname'),
  socket_getpeername: (..._args) => baseImports.unsupported_import('socket_getpeername'),
  socket_setsockopt: (..._args) => baseImports.unsupported_import('socket_setsockopt'),
  socket_getsockopt: (..._args) => baseImports.unsupported_import('socket_getsockopt'),
  socket_detach: (..._args) => baseImports.unsupported_import('socket_detach'),
  socketpair: (..._args) => baseImports.unsupported_import('socketpair'),
  socket_getaddrinfo: (..._args) => baseImports.unsupported_import('socket_getaddrinfo'),
  socket_getnameinfo: (..._args) => baseImports.unsupported_import('socket_getnameinfo'),
  socket_gethostname: (..._args) => baseImports.unsupported_import('socket_gethostname'),
  socket_getservbyname: (..._args) => baseImports.unsupported_import('socket_getservbyname'),
  socket_getservbyport: (..._args) => baseImports.unsupported_import('socket_getservbyport'),
  socket_inet_pton: (..._args) => baseImports.unsupported_import('socket_inet_pton'),
  socket_inet_ntop: (..._args) => baseImports.unsupported_import('socket_inet_ntop'),
  ne: (a, b) => {
    const res = baseImports.eq(a, b);
    if (exceptionPending() !== 0n) return boxNone();
    return boxBool(baseImports.is_truthy(res) === 0n);
  },
  stream_clone: (..._args) => baseImports.unsupported_import('stream_clone'),
  stream_send_obj: (..._args) => baseImports.unsupported_import('stream_send_obj'),
  sys_set_version_info: (major, minor, micro, releaselevel, serial, version) => {
    sysVersionInfo = tupleFromArray([major, minor, micro, releaselevel, serial]);
    sysVersionStr = version;
    return boxNone();
  },
  sys_version_info: () => {
    if (sysVersionInfo) return sysVersionInfo;
    return tupleFromArray([
      boxInt(0),
      boxInt(0),
      boxInt(0),
      boxPtr({ type: 'str', value: 'final' }),
      boxInt(0),
    ]);
  },
  sys_version: () => {
    if (sysVersionStr) return sysVersionStr;
    return boxPtr({ type: 'str', value: '0.0.0' });
  },
  sys_platform: () => boxPtr({ type: 'str', value: 'wasm' }),
  sys_executable: () => boxNone(),
  sys_stdin: () => boxNone(),
  sys_stdout: () => boxNone(),
  sys_stderr: () => boxNone(),
  stdlib_probe: () => boxBool(true),
  os_name: () => boxPtr({ type: 'str', value: 'posix' }),
  os_close: (_fd) => boxNone(),
  os_dup: (fd) => fd,
  os_get_inheritable: (_fd) => boxBool(false),
  os_set_inheritable: (_fd, _inheritable) => boxNone(),
  os_urandom: (sizeBits) => {
    if (!isIntLike(sizeBits)) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'urandom() argument must be int' })),
      );
      return raiseException(exc);
    }
    const size = Number(unboxIntLike(sizeBits));
    if (!Number.isFinite(size) || size < 0) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'ValueError' }),
        exceptionArgs(boxPtr({ type: 'str', value: 'urandom() argument must be non-negative' })),
      );
      return raiseException(exc);
    }
    if (size === 0) return boxBytes([]);
    let data;
    if (typeof crypto !== 'undefined' && crypto.randomBytes) {
      data = crypto.randomBytes(size);
    } else {
      const buf = new Uint8Array(size);
      for (let i = 0; i < size; i += 1) {
        buf[i] = Math.floor(Math.random() * 256);
      }
      data = buf;
    }
    return boxBytes(data);
  },
  env_snapshot: () => boxPtr({ type: 'dict', entries: [], lookup: new Map() }),
  getcwd: () => boxPtr({ type: 'str', value: '/' }),
  getframe: (_depth) => boxNone(),
  path_listdir: (_path) => listFromArray([]),
  path_mkdir: (_path) => boxNone(),
  path_rmdir: (_path) => boxNone(),
  pending: () => boxPending(),
  chan_send_blocking: (chan, val) => baseImports.chan_send(chan, val),
  chan_try_send: (chan, val) => baseImports.chan_send(chan, val),
  chan_recv_blocking: (chan) => baseImports.chan_recv(chan),
  chan_try_recv: (chan) => baseImports.chan_recv(chan),
  func_new_builtin: (fnIdx, trampolineIdx, arity) =>
    baseImports.func_new(fnIdx, trampolineIdx, arity),
  function_set_builtin: (_funcBits) => boxNone(),
  fn_ptr_code_set: (_fnPtr, _codeBits) => boxNone(),
  asyncgen_hooks_get: () => boxNone(),
  asyncgen_hooks_set: (_hooks, _finalizer) => boxNone(),
  asyncgen_locals: (_obj) => boxNone(),
  asyncgen_locals_register: (_obj, _names, _offsets) => boxNone(),
  gen_locals: (_obj) => boxNone(),
  gen_locals_register: (_obj, _names, _offsets) => boxNone(),
  exception_stack_clear: () => boxNone(),
  exceptiongroup_match: (_exc, _handler) => boxNone(),
  exceptiongroup_combine: (_exc) => boxNone(),
  weakref_register: (_obj, _cb, _finalize) => boxNone(),
  weakref_get: (_handle) => boxNone(),
  weakref_drop: (_handle) => boxNone(),
  struct_calcsize: (_format) => boxNone(),
  struct_pack: (_format, _values) => boxNone(),
  struct_unpack: (_format, _buffer) => boxNone(),
  lock_new: () => {
    const id = nextLockId++;
    lockStates.set(id, { locked: false });
    return boxInt(id);
  },
  lock_acquire: (handle, _blocking, _timeout) => {
    const id = Number(unboxInt(handle));
    const entry = lockStates.get(id);
    if (!entry) return boxBool(false);
    if (entry.locked) return boxBool(false);
    entry.locked = true;
    return boxBool(true);
  },
  lock_release: (handle) => {
    const id = Number(unboxInt(handle));
    const entry = lockStates.get(id);
    if (entry) entry.locked = false;
    return boxNone();
  },
  lock_locked: (handle) => {
    const id = Number(unboxInt(handle));
    const entry = lockStates.get(id);
    return boxBool(!!(entry && entry.locked));
  },
  lock_drop: (handle) => {
    const id = Number(unboxInt(handle));
    lockStates.delete(id);
    return boxNone();
  },
  rlock_new: () => {
    const id = nextRLockId++;
    rlockStates.set(id, { locked: false, count: 0 });
    return boxInt(id);
  },
  rlock_acquire: (handle, _blocking, _timeout) => {
    const id = Number(unboxInt(handle));
    const entry = rlockStates.get(id);
    if (!entry) return boxBool(false);
    entry.locked = true;
    entry.count += 1;
    return boxBool(true);
  },
  rlock_release: (handle) => {
    const id = Number(unboxInt(handle));
    const entry = rlockStates.get(id);
    if (entry && entry.count > 0) {
      entry.count -= 1;
      if (entry.count === 0) entry.locked = false;
    }
    return boxNone();
  },
  rlock_locked: (handle) => {
    const id = Number(unboxInt(handle));
    const entry = rlockStates.get(id);
    return boxBool(!!(entry && entry.locked));
  },
  rlock_drop: (handle) => {
    const id = Number(unboxInt(handle));
    rlockStates.delete(id);
    return boxNone();
  },
  thread_spawn: (_payload) => boxInt(0),
  thread_join: (_handle, _timeout) => boxNone(),
  thread_is_alive: (_handle) => boxBool(false),
  thread_ident: (_handle) => boxNone(),
  thread_native_id: (_handle) => boxNone(),
  thread_current_ident: () => boxNone(),
  thread_current_native_id: () => boxNone(),
  thread_drop: (_handle) => boxNone(),
"""

IMPORT_HELPERS = """\
const readVarUint = (bytes, offset) => {
  let result = 0;
  let shift = 0;
  let pos = offset;
  while (true) {
    if (pos >= bytes.length) {
      throw new Error('Unexpected EOF while reading varuint');
    }
    const byte = bytes[pos++];
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7;
  }
  return [result, pos];
};
const readString = (bytes, offset) => {
  const [len, pos] = readVarUint(bytes, offset);
  const end = pos + len;
  if (end > bytes.length) {
    throw new Error('Unexpected EOF while reading string');
  }
  const value = new TextDecoder('utf-8').decode(bytes.slice(pos, end));
  return [value, end];
};
const readVarInt = (bytes, offset) => {
  let result = 0;
  let shift = 0;
  let pos = offset;
  let byte = 0;
  while (true) {
    if (pos >= bytes.length) {
      throw new Error('Unexpected EOF while reading varint');
    }
    byte = bytes[pos++];
    result |= (byte & 0x7f) << shift;
    shift += 7;
    if ((byte & 0x80) === 0) {
      break;
    }
  }
  if (shift < 32 && (byte & 0x40)) {
    result |= ~0 << shift;
  }
  return [result, pos];
};
const readLimits = (bytes, offset) => {
  const flags = bytes[offset++];
  const [min, pos] = readVarUint(bytes, offset);
  let max = null;
  let next = pos;
  if (flags & 0x01) {
    const [maxVal, posMax] = readVarUint(bytes, pos);
    max = maxVal;
    next = posMax;
  }
  return [{ min, max }, next];
};
const skipConstExpr = (bytes, offset, sectionEnd) => {
  let pos = offset;
  while (pos < sectionEnd && bytes[pos] !== 0x0b) pos += 1;
  if (pos < sectionEnd && bytes[pos] === 0x0b) pos += 1;
  return pos;
};
const readConstExprI32 = (bytes, offset, sectionEnd, globalI32Values) => {
  let pos = offset;
  if (pos >= sectionEnd) return [0, pos, false];
  const opcode = bytes[pos++];
  if (opcode === 0x41) {
    let value;
    [value, pos] = readVarInt(bytes, pos);
    pos = skipConstExpr(bytes, pos, sectionEnd);
    return [value, pos, true];
  }
  if (opcode === 0x23) {
    let globalIdx;
    [globalIdx, pos] = readVarUint(bytes, pos);
    pos = skipConstExpr(bytes, pos, sectionEnd);
    if (globalI32Values[globalIdx] !== undefined) {
      return [globalI32Values[globalIdx], pos, true];
    }
    return [0, pos, false];
  }
  pos = skipConstExpr(bytes, pos, sectionEnd);
  return [0, pos, false];
};
const parseWasmImports = (buffer) => {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 8) {
    throw new Error('Invalid wasm binary');
  }
  let offset = 8;
  let memoryImport = null;
  let tableImport = null;
  let globalImportCount = 0;
  while (offset < bytes.length) {
    const sectionId = bytes[offset++];
    const [sectionSize, sizePos] = readVarUint(bytes, offset);
    offset = sizePos;
    const sectionEnd = offset + sectionSize;
    if (sectionId === 2) {
      let count;
      [count, offset] = readVarUint(bytes, offset);
      for (let idx = 0; idx < count; idx += 1) {
        let moduleName;
        [moduleName, offset] = readString(bytes, offset);
        let fieldName;
        [fieldName, offset] = readString(bytes, offset);
        const kind = bytes[offset++];
        if (kind === 0) {
          const [, next] = readVarUint(bytes, offset);
          offset = next;
        } else if (kind === 1) {
          offset += 1;
          let limits;
          [limits, offset] = readLimits(bytes, offset);
          if (moduleName === 'env' && fieldName === '__indirect_function_table') {
            tableImport = limits;
          }
        } else if (kind === 2) {
          let limits;
          [limits, offset] = readLimits(bytes, offset);
          if (moduleName === 'env' && fieldName === 'memory') {
            memoryImport = limits;
          }
        } else if (kind === 3) {
          offset += 2;
          globalImportCount += 1;
        } else {
          throw new Error(`Unknown import kind ${kind}`);
        }
      }
    } else {
      offset = sectionEnd;
    }
  }
  return { memory: memoryImport, table: tableImport, globalImportCount };
};
const parseWasmDataEnd = (buffer, memoryImport, globalImportCount) => {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 8) {
    throw new Error('Invalid wasm binary');
  }
  let offset = 8;
  let dataEnd = 0;
  let unknownActiveOffset = false;
  let memoryMinPages = memoryImport ? memoryImport.min : null;
  const globalI32Values = [];
  while (offset < bytes.length) {
    const sectionId = bytes[offset++];
    const [sectionSize, sizePos] = readVarUint(bytes, offset);
    offset = sizePos;
    const sectionEnd = offset + sectionSize;
    if (sectionId === 5 && memoryMinPages === null) {
      let count;
      [count, offset] = readVarUint(bytes, offset);
      for (let idx = 0; idx < count; idx += 1) {
        let limits;
        [limits, offset] = readLimits(bytes, offset);
        if (idx === 0) memoryMinPages = limits.min;
      }
    } else if (sectionId === 6) {
      let count;
      [count, offset] = readVarUint(bytes, offset);
      for (let idx = 0; idx < count; idx += 1) {
        const valType = bytes[offset++];
        const mutability = bytes[offset++];
        let valueKnown = false;
        let value = 0;
        if (valType === 0x7f && mutability === 0) {
          [value, offset, valueKnown] = readConstExprI32(
            bytes,
            offset,
            sectionEnd,
            globalI32Values
          );
        } else {
          offset = skipConstExpr(bytes, offset, sectionEnd);
        }
        const globalIdx = (globalImportCount || 0) + idx;
        if (valueKnown) globalI32Values[globalIdx] = value;
      }
    } else if (sectionId === 11) {
      let count;
      [count, offset] = readVarUint(bytes, offset);
      for (let idx = 0; idx < count; idx += 1) {
        const kind = bytes[offset++];
        let segOffset = 0;
        let segOffsetKnown = false;
        let active = false;
        if (kind === 0) {
          active = true;
          [segOffset, offset, segOffsetKnown] = readConstExprI32(
            bytes,
            offset,
            sectionEnd,
            globalI32Values
          );
        } else if (kind === 2) {
          active = true;
          const [, pos] = readVarUint(bytes, offset);
          offset = pos;
          [segOffset, offset, segOffsetKnown] = readConstExprI32(
            bytes,
            offset,
            sectionEnd,
            globalI32Values
          );
        } else if (kind === 1) {
          segOffset = 0;
        } else {
          throw new Error(`Unknown data segment kind ${kind}`);
        }
        let size;
        [size, offset] = readVarUint(bytes, offset);
        if (active) {
          if (!segOffsetKnown) {
            unknownActiveOffset = true;
          } else {
            if (segOffset < 0) segOffset = 0;
            if (segOffset + size > dataEnd) {
              dataEnd = segOffset + size;
            }
          }
        }
        offset += size;
      }
    }
    offset = sectionEnd;
  }
  if (unknownActiveOffset && memoryMinPages !== null) {
    const memoryMinBytes = memoryMinPages * 65536;
    if (memoryMinBytes > dataEnd) dataEnd = memoryMinBytes;
  }
  return dataEnd;
};
const wasmImports = parseWasmImports(wasmBuffer);
const wasmDataEnd = parseWasmDataEnd(
  wasmBuffer,
  wasmImports.memory,
  wasmImports.globalImportCount
);
heapPtr = Math.max(heapPtr, align(wasmDataEnd, 8));
if (wasmImports.memory) {
  const memDesc = { initial: wasmImports.memory.min };
  if (wasmImports.memory.max !== null) {
    memDesc.maximum = wasmImports.memory.max;
  }
  memory = new WebAssembly.Memory(memDesc);
}
if (wasmImports.table) {
  const tableDesc = { initial: wasmImports.table.min, element: 'anyfunc' };
  if (wasmImports.table.max !== null) {
    tableDesc.maximum = wasmImports.table.max;
  }
  table = new WebAssembly.Table(tableDesc);
}
const envImports = {};
if (memory) envImports.memory = memory;
if (table) envImports.__indirect_function_table = table;
envImports.molt_getpid_host = () =>
  BigInt(typeof process !== 'undefined' && process.pid ? process.pid : 0);
envImports.molt_ws_poll_host = () => 0;
"""


def wasm_runner_source(*, extra_js: str = "", import_overrides: str = "") -> str:
    parts = [BASE_PREAMBLE]
    if extra_js:
        parts.append(extra_js.rstrip() + "\n")
    parts.append("const baseImports = {\n")
    parts.append(BASE_IMPORTS.rstrip() + "\n")
    parts.append("};\n")
    parts.append(INTRINSIC_REGISTRY_JS.rstrip() + "\n")
    parts.append("const overrideImports = {\n")
    if import_overrides:
        parts.append(import_overrides.rstrip() + "\n")
    parts.append("};\n")
    parts.append(IMPORT_HELPERS.rstrip() + "\n")
    parts.append(
        "const imports = { molt_runtime: { ...baseImports, ...overrideImports }, env: envImports };\n"
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
