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
const getBigIntValue = (val) => {
  if (isIntLike(val)) return unboxIntLike(val);
  const obj = getObj(val);
  if (obj && obj.type === 'bigint') return obj.value;
  return null;
};
const BUILTIN_TYPE_TAGS = new Map([
  [1, 'int'],
  [2, 'float'],
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
]);
const builtinTypes = new Map();
const builtinBaseTag = (tag) => {
  if (tag === 3) return 1;
  if (tag === 101) return 100;
  if (tag === 100) return null;
  if (tag === 4) return 100;
  return 100;
};
const getBuiltinType = (tag) => {
  if (builtinTypes.has(tag)) return builtinTypes.get(tag);
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
const heap = new Map();
const instanceClasses = new Map();
const classLayoutVersions = new Map();
const instanceAttrs = new Map();
let nextPtr = 1n << 40n;
let memory = null;
let table = null;
const chanQueues = new Map();
const chanCaps = new Map();
const moduleCache = new Map();
const sleepPending = new Set();
const cancelTokens = new Map();
const taskTokens = new Map();
let nextCancelTokenId = 2n;
let currentTokenId = 1n;
let currentTaskPtr = 0n;
let nextChanId = 1n;
let heapPtr = 1 << 20;
let recursionLimit = 1000;
let recursionDepth = 0;
const HEADER_SIZE = 40;
const HEADER_POLL_FN_OFFSET = HEADER_SIZE - 8;
const HEADER_STATE_OFFSET = HEADER_SIZE - 16;
const GEN_CONTROL_SIZE = 32;
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
const boxPtrAddr = (addr) => QNAN | TAG_PTR | (BigInt(addr) & POINTER_MASK);
const ptrAddr = (val) => Number(val & POINTER_MASK);
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
let notImplementedBits = null;
const notImplementedSentinel = () => {
  if (notImplementedBits === null) {
    notImplementedBits = boxPtr({ type: 'not_implemented' });
  }
  return notImplementedBits;
};
let anextDefaultPollIdx = null;
const normalizePtrBits = (val) => {
  if (val === 0n) return val;
  if (isPtr(val)) return val;
  return boxPtrAddr(val);
};
const getObj = (val) => heap.get(val & POINTER_MASK);
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
const callFunctionBits = (funcBits, args) => {
  const func = getFunction(funcBits);
  if (!func || !table) {
    throw new Error('TypeError: call expects function object');
  }
  const fn = table.get(func.idx);
  if (!fn) {
    throw new Error('TypeError: call expects function object');
  }
  if (func.closure && func.closure !== 0n) {
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
    if (func && table) {
      const fn = table.get(func.idx);
      if (fn) return fn.length;
    }
  }
  const func = getFunction(callableBits);
  if (func && table) {
    const fn = table.get(func.idx);
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
const typeName = (val) => {
  if (isTag(val, TAG_NONE)) return 'NoneType';
  if (isTag(val, TAG_BOOL)) return 'bool';
  if (isTag(val, TAG_INT)) return 'int';
  if (isFloat(val)) return 'float';
  const obj = getObj(val);
  if (obj) {
    if (obj.type === 'class') return obj.name ?? 'type';
    if (obj.type === 'str') return 'str';
    if (obj.type === 'bytes') return 'bytes';
    if (obj.type === 'bytearray') return 'bytearray';
    if (obj.type === 'list') return 'list';
    if (obj.type === 'tuple') return 'tuple';
    if (obj.type === 'set') return 'set';
    if (obj.type === 'frozenset') return 'frozenset';
    if (obj.type === 'dict') return 'dict';
    if (obj.type === 'module') return 'module';
    if (obj.type === 'function') return 'function';
    if (obj.type === 'map') return 'map';
    if (obj.type === 'filter') return 'filter';
    if (obj.type === 'zip') return 'zip';
    if (obj.type === 'reversed') return 'reversed';
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
const formatFloat = (val) => {
  if (Number.isNaN(val)) return 'nan';
  if (!Number.isFinite(val)) return val < 0 ? '-inf' : 'inf';
  if (Number.isInteger(val)) return val.toFixed(1);
  return val.toString();
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
const lookupExceptionAttr = (exc, name) => {
  switch (name) {
    case '__cause__':
      return exc.causeBits;
    case '__context__':
      return exc.contextBits;
    case '__suppress_context__':
      return exc.suppressBits;
    default:
      return undefined;
  }
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
  }
  return undefined;
};
const lookupAttr = (objBits, name) => {
  const exc = getException(objBits);
  if (exc) {
    return lookupExceptionAttr(exc, name);
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
  if (func && func.attrs && func.attrs.has(name)) {
    return func.attrs.get(name);
  }
  if (isPtr(objBits) && !heap.has(objBits & POINTER_MASK)) {
    const key = ptrAddr(objBits);
    const attrs = instanceAttrs.get(key);
    if (attrs && attrs.has(name)) {
      return attrs.get(name);
    }
    const clsBits = instanceClasses.get(key);
    if (clsBits !== undefined) {
      const val = lookupClassAttr(clsBits, name, objBits);
      if (val !== undefined) return val;
    }
  }
  return undefined;
};
const lookupSpecialAttr = (objBits, name) => {
  const exc = getException(objBits);
  if (exc) {
    return lookupExceptionAttr(exc, name);
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
    if (obj.type === 'str') return getBuiltinType(5);
    if (obj.type === 'bytes') return getBuiltinType(6);
    if (obj.type === 'bytearray') return getBuiltinType(7);
    if (obj.type === 'list') return getBuiltinType(8);
    if (obj.type === 'tuple') return getBuiltinType(9);
    if (obj.type === 'dict') return getBuiltinType(10);
    if (obj.type === 'set') return getBuiltinType(17);
    if (obj.type === 'frozenset') return getBuiltinType(18);
    if (obj.type === 'memoryview') return getBuiltinType(15);
  }
  if (isPtr(objBits) && !heap.has(objBits & POINTER_MASK)) {
    const clsBits = instanceClasses.get(ptrAddr(objBits));
    if (clsBits !== undefined) return clsBits;
  }
  return getBuiltinType(100);
};
const getAttrValue = (objBits, name) => {
  const val = lookupAttr(objBits, name);
  if (val === undefined) return boxNone();
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
  const instanceAttrsMap = getInstanceAttrMap(objBits);
  if (instanceAttrsMap) {
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
    moduleObj.attrs.delete(name);
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
const exceptionContextSet = (excBits) => {
  if (!activeExceptionStack.length || isNone(excBits)) return boxNone();
  const exc = getException(excBits);
  if (!exc) {
    throw new Error('TypeError: expected exception object');
  }
  activeExceptionStack[activeExceptionStack.length - 1] = excBits;
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
const raiseException = (excBits) => {
  const exc = getException(excBits);
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
  if (!exceptionStack.length && !generatorRaise) {
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
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  view.setBigInt64(addr + 0, sendVal, true);
  view.setBigInt64(addr + 8, boxNone(), true);
  const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = table.get(pollIdx);
  const prevRaise = generatorRaise;
  generatorRaise = true;
  const res = poll
    ? poll(gen)
    : tupleFromArray([boxNone(), boxBool(true)]);
  generatorRaise = prevRaise;
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    return raiseException(excBits);
  }
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
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  view.setBigInt64(addr + 8, exc, true);
  view.setBigInt64(addr + 0, boxNone(), true);
  const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = table.get(pollIdx);
  const prevRaise = generatorRaise;
  generatorRaise = true;
  const res = poll
    ? poll(gen)
    : tupleFromArray([boxNone(), boxBool(true)]);
  generatorRaise = prevRaise;
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
  exceptionSetDepth(newDepth);
  generatorExceptionStacks.set(key, activeExceptionStack.slice());
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...callerStack);
  exceptionSetDepth(callerDepth);
  activeExceptionFallback.pop();
  if (pending) {
    return raiseException(excBits);
  }
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
  const callerStack = activeExceptionStack.slice();
  const callerContext = callerStack.length
    ? callerStack[callerStack.length - 1]
    : boxNone();
  activeExceptionFallback.push(callerContext);
  const key = addr;
  const genStack = generatorExceptionStacks.get(key) || [];
  activeExceptionStack.length = 0;
  activeExceptionStack.push(...genStack);
  const depthBits = view.getBigInt64(addr + 24, true);
  const genDepth = isTag(depthBits, TAG_INT) ? Number(unboxInt(depthBits)) : 0;
  exceptionSetDepth(genDepth);
  const exc = exceptionNew(
    boxPtr({ type: 'str', value: 'GeneratorExit' }),
    boxPtr({ type: 'str', value: '' }),
  );
  view.setBigInt64(addr + 8, exc, true);
  view.setBigInt64(addr + 0, boxNone(), true);
  const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
  const poll = table.get(pollIdx);
  const prevRaise = generatorRaise;
  generatorRaise = true;
  const res = poll ? poll(gen) : null;
  generatorRaise = prevRaise;
  const pending = exceptionPending() !== 0n;
  const excBits = pending ? exceptionLast() : boxNone();
  if (pending) exceptionClear();
  const newDepth = exceptionDepth();
  view.setBigInt64(addr + 24, boxInt(newDepth), true);
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
      view.setBigInt64(addr + 16, boxBool(true), true);
      return boxNone();
    }
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
    if (taskPtr === 0n) return boxNone();
    const key = taskPtr.toString();
    if (!sleepPending.has(key)) {
      sleepPending.add(key);
      return boxPending();
    }
    return boxNone();
  },
  anext_default_poll: (taskPtr) => {
    if (taskPtr === 0n || !memory || !table) return boxNone();
    const addr = ptrAddr(taskPtr);
    const view = new DataView(memory.buffer);
    const state = Number(view.getBigInt64(addr - HEADER_STATE_OFFSET, true));
    const iterBits = view.getBigInt64(addr + 0, true);
    const defaultBits = view.getBigInt64(addr + 8, true);
    if (state === 0) {
    const attr = lookupAttr(normalizePtrBits(iterBits), '__anext__');
      if (attr === undefined) {
        throw new Error('TypeError: object is not an async iterator');
      }
      const awaitBits = callCallable0(attr);
      view.setBigInt64(addr + 16, awaitBits, true);
      view.setBigInt64(addr - HEADER_STATE_OFFSET, 1n, true);
    }
    const awaitBits = view.getBigInt64(addr + 16, true);
    const awaitPtrBits = normalizePtrBits(awaitBits);
    if (!isPtr(awaitPtrBits) || heap.has(awaitPtrBits & POINTER_MASK)) return boxNone();
    const awaitAddr = ptrAddr(awaitPtrBits);
    const pollIdx = view.getUint32(awaitAddr - HEADER_POLL_FN_OFFSET, true);
    const poll = table.get(pollIdx);
    if (!poll) return boxNone();
    const res = poll(awaitBits);
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
  future_poll_fn: (futureBits) => {
    const ptrBits = normalizePtrBits(futureBits);
    if (!isPtr(ptrBits) || heap.has(ptrBits & POINTER_MASK) || !memory || !table) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        boxPtr({ type: 'str', value: 'object is not awaitable' }),
      );
      raiseException(exc);
      return -1n;
    }
    const addr = ptrAddr(ptrBits);
    const view = new DataView(memory.buffer);
    const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    const poll = table.get(pollIdx);
    if (!poll) {
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'TypeError' }),
        boxPtr({ type: 'str', value: 'object is not awaitable' }),
      );
      raiseException(exc);
      return -1n;
    }
    return BigInt(pollIdx);
  },
  sleep_register: (taskPtr, _futurePtr) => {
    return 0n;
  },
  block_on: (taskPtr) => {
    if (!memory || !table) return 0n;
    const addr = ptrAddr(taskPtr);
    const view = new DataView(memory.buffer);
    const pollIdx = view.getUint32(addr - HEADER_POLL_FN_OFFSET, true);
    const poll = table.get(pollIdx);
    if (!poll) return 0n;
    const prevTask = currentTaskPtr;
    const prevToken = currentTokenId;
    currentTaskPtr = taskPtr;
    const token = ensureTaskToken(taskPtr);
    setCurrentTokenId(token);
    while (true) {
      const res = poll(taskPtr);
      if (isPending(res)) continue;
      setCurrentTokenId(prevToken);
      currentTaskPtr = prevTask;
      clearTaskToken(taskPtr);
      return res;
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
    if (currentTaskPtr !== 0n) {
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
  add: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxInt(unboxIntLike(a) + unboxIntLike(b));
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
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      return boxFloat(lf - rf);
    }
    const lset = getSetLike(a);
    const rset = getSetLike(b);
    if (lset && rset) {
      const outItems = new Set();
      for (const item of lset.items) {
        if (!rset.items.has(item)) {
          outItems.add(item);
        }
      }
      return boxPtr({ type: lset.type, items: outItems });
    }
    return boxNone();
  },
  inplace_sub: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetLike(b);
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
    const lset = getSetLike(a);
    const rset = getSetLike(b);
    if (lset && rset) {
      const outItems = new Set(lset.items);
      for (const item of rset.items) {
        outItems.add(item);
      }
      return boxPtr({ type: lset.type, items: outItems });
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
    const lset = getSetLike(a);
    const rset = getSetLike(b);
    if (lset && rset) {
      const outItems = new Set();
      for (const item of lset.items) {
        if (rset.items.has(item)) {
          outItems.add(item);
        }
      }
      return boxPtr({ type: lset.type, items: outItems });
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
    const lset = getSetLike(a);
    const rset = getSetLike(b);
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
      return boxPtr({ type: lset.type, items: outItems });
    }
    return boxNone();
  },
  inplace_bit_or: (a, b) => {
    const set = getSet(a);
    if (set) {
      const other = getSetLike(b);
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
      const other = getSetLike(b);
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
      const other = getSetLike(b);
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
    if (lf === null || rf === null) {
      throw new Error('TypeError: unsupported operand type(s) for /');
    }
    if (rf === 0) {
      throw new Error('ZeroDivisionError: division by zero');
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
      }
    }
    return boxBool(a === b);
  },
  is: (a, b) => boxBool(a === b),
  closure_load: (ptr, offset) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(ptr) + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  closure_store: (ptr, offset, val) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(ptr) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  not: (val) => {
    if (isTag(val, TAG_BOOL)) {
      return boxBool((val & 1n) !== 1n);
    }
    if (isTag(val, TAG_INT)) {
      return boxBool(unboxInt(val) === 0n);
    }
    return boxBool(true);
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
    const ptrBits = normalizePtrBits(obj);
    const clsBits = instanceClasses.get(ptrAddr(ptrBits));
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
  guarded_field_get_ptr: (obj, classBits, expected, offset, namePtr, nameLen) => {
    if (obj === 0n || !getClass(classBits)) {
      return getAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen));
    }
    const ptrBits = normalizePtrBits(obj);
    const clsBits = instanceClasses.get(ptrAddr(ptrBits));
    if (clsBits === undefined || clsBits !== classBits) {
      return getAttrValue(ptrBits, readUtf8(namePtr, nameLen));
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return getAttrValue(ptrBits, readUtf8(namePtr, nameLen));
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      return getAttrValue(ptrBits, readUtf8(namePtr, nameLen));
    }
    if (!memory) return boxNone();
    const addr = ptrAddr(ptrBits) + Number(offset);
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
    if (obj === 0n || !getClass(classBits)) {
      return setAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen), val);
    }
    const ptrBits = normalizePtrBits(obj);
    const clsBits = instanceClasses.get(ptrAddr(ptrBits));
    if (clsBits === undefined || clsBits !== classBits) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    if (!memory) return boxNone();
    const addr = ptrAddr(ptrBits) + Number(offset);
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
    if (obj === 0n || !getClass(classBits)) {
      return setAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen), val);
    }
    const ptrBits = normalizePtrBits(obj);
    const clsBits = instanceClasses.get(ptrAddr(ptrBits));
    if (clsBits === undefined || clsBits !== classBits) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    const version = classLayoutVersion(classBits);
    if (version === null) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    let expectedVersion = expected;
    if (isTag(expected, TAG_INT)) {
      expectedVersion = unboxInt(expected);
    } else if (isTag(expected, TAG_BOOL)) {
      expectedVersion = unboxIntLike(expected);
    }
    if (version !== expectedVersion) {
      return setAttrValue(ptrBits, readUtf8(namePtr, nameLen), val);
    }
    if (!memory) return boxNone();
    const addr = ptrAddr(ptrBits) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  handle_resolve: (bits) => {
    if (!isPtr(bits)) return 0n;
    const id = bits & POINTER_MASK;
    const obj = heap.get(id);
    if (obj && obj.memAddr) return BigInt(obj.memAddr);
    if (!obj) return BigInt(ptrAddr(bits));
    return 0n;
  },
  inc_ref_obj: (_val) => {},
  get_attr_ptr: (obj, namePtr, nameLen) =>
    getAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen)),
  get_attr_generic: (obj, namePtr, nameLen) =>
    getAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen)),
  get_attr_object: (obj, namePtr, nameLen) =>
    getAttrValue(obj, readUtf8(namePtr, nameLen)),
  get_attr_special: (obj, namePtr, nameLen) =>
    getAttrSpecialValue(obj, readUtf8(namePtr, nameLen)),
  set_attr_ptr: (obj, namePtr, nameLen, val) =>
    setAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen), val),
  set_attr_generic: (obj, namePtr, nameLen, val) =>
    setAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen), val),
  set_attr_object: (obj, namePtr, nameLen, val) =>
    setAttrValue(obj, readUtf8(namePtr, nameLen), val),
  del_attr_ptr: (obj, namePtr, nameLen) =>
    delAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen)),
  del_attr_generic: (obj, namePtr, nameLen) =>
    delAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen)),
  del_attr_object: (obj, namePtr, nameLen) =>
    delAttrValue(obj, readUtf8(namePtr, nameLen)),
  object_field_get: (obj, offset) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  object_field_set: (obj, offset, val) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_init: (obj, offset, val) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(obj) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_get_ptr: (obj, offset) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(normalizePtrBits(obj)) + Number(offset);
    const view = new DataView(memory.buffer);
    return view.getBigInt64(addr, true);
  },
  object_field_set_ptr: (obj, offset, val) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(normalizePtrBits(obj)) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  object_field_init_ptr: (obj, offset, val) => {
    if (!memory) return boxNone();
    const addr = ptrAddr(normalizePtrBits(obj)) + Number(offset);
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr, val, true);
    return boxNone();
  },
  module_new: (nameBits) => {
    const name = getStrObj(nameBits);
    return boxPtr({
      type: 'module',
      name: name ?? '<module>',
      attrs: new Map(),
    });
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
    if (!moduleObj || name === null) return boxNone();
    const val = moduleObj.attrs.get(name);
    return val === undefined ? boxNone() : val;
  },
  module_set_attr: (moduleBits, nameBits, val) => {
    const name = getStrObj(nameBits);
    const moduleObj = getModule(moduleBits);
    if (!moduleObj || name === null) return boxNone();
    moduleObj.attrs.set(name, val);
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
    if (obj && (obj.type === 'set' || obj.type === 'frozenset'))
      return obj.items.size ? 1n : 0n;
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
  bigint_from_str: (ptr, len) => {
    if (!memory) return boxNone();
    const addr = Number(ptr);
    const size = Number(len);
    const bytes = new Uint8Array(memory.buffer, addr, size);
    const text = Buffer.from(bytes).toString('utf8').trim();
    try {
      return boxPtr({ type: 'bigint', value: BigInt(text) });
    } catch {
      return boxNone();
    }
  },
  memoryview_new: () => boxNone(),
  memoryview_tobytes: () => boxNone(),
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
    if (obj && obj.type === 'bigint') {
      return boxPtr({ type: 'str', value: obj.value.toString() });
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
      return val;
    }
    if (obj && obj.type === 'bigint') {
      return boxPtr({ type: 'str', value: obj.value.toString() });
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
  len: (val) => {
    const list = getList(val);
    if (list) return boxInt(list.items.length);
    const tup = getTuple(val);
    if (tup) return boxInt(tup.items.length);
    const setLike = getSetLike(val);
    if (setLike) return boxInt(BigInt(setLike.items.size));
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
  list_append: (listBits, valBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    list.items.push(valBits);
    return boxNone();
  },
  list_pop: (listBits, indexBits) => {
    const list = getList(listBits);
    if (!list) return boxNone();
    let idx;
    if (isNone(indexBits)) {
      idx = list.items.length - 1;
    } else if (isIntLike(indexBits)) {
      idx = Number(unboxIntLike(indexBits));
    } else {
      return boxNone();
    }
    if (idx < 0) idx += list.items.length;
    if (idx < 0 || idx >= list.items.length) return boxNone();
    return list.items.splice(idx, 1)[0] ?? boxNone();
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
    const list = getList(listBits);
    if (!list) return boxNone();
    const idx = list.items.findIndex((item) => item === valBits);
    if (idx < 0) return boxNone();
    return boxInt(idx);
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
      throw new Error('KeyError: dict.pop missing key');
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
    const keys = dict.entries.map((entry) => entry[0]);
    return listFromArray(keys);
  },
  dict_values: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const values = dict.entries.map((entry) => entry[1]);
    return listFromArray(values);
  },
  dict_items: (dictBits) => {
    const dict = getDict(dictBits);
    if (!dict) return boxNone();
    const items = dict.entries.map((entry) => tupleFromArray([entry[0], entry[1]]));
    return listFromArray(items);
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
  tuple_count: () => boxNone(),
  tuple_index: () => boxNone(),
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
    return boxNone();
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
    const attr = lookupAttr(normalizePtrBits(val), '__aiter__');
    if (attr === undefined) {
      throw new Error('TypeError: object is not async iterable');
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
    const attr = lookupAttr(normalizePtrBits(val), '__anext__');
    if (attr === undefined) {
      const norm = normalizePtrBits(val);
      const addr = isPtr(norm) ? ptrAddr(norm) : -1;
      const hasClass = isPtr(norm) && instanceClasses.has(addr);
      throw new Error(
        `TypeError: object is not an async iterator (got ${typeName(val)}, ` +
          `addr=${addr}, hasClass=${hasClass})`,
      );
    }
    return callCallable0(attr);
  },
  generator_new: (pollFn, closureSize) => {
    const size = Number(closureSize);
    const addr = allocRaw(size);
    if (!addr || !memory) return boxNone();
    const view = new DataView(memory.buffer);
    view.setBigInt64(addr - HEADER_POLL_FN_OFFSET, pollFn, true);
    view.setBigInt64(addr - HEADER_STATE_OFFSET, 0n, true);
    if (size >= GEN_CONTROL_SIZE) {
      view.setBigInt64(addr + 0, boxNone(), true);
      view.setBigInt64(addr + 8, boxNone(), true);
      view.setBigInt64(addr + 16, boxBool(false), true);
      view.setBigInt64(addr + 24, boxInt(1), true);
    }
    return boxPtrAddr(addr);
  },
  generator_send: (gen, sendVal) => generatorSend(gen, sendVal),
  generator_throw: (gen, exc) => generatorThrow(gen, exc),
  generator_close: (gen) => generatorClose(gen),
  is_generator: (val) => boxBool(isGenerator(val)),
  is_bound_method: (val) => boxBool(isBoundMethod(val)),
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
    throw new Error(`TypeError: call arity mismatch (expected ${expected}, got ${got})`);
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
    const cls = getClass(callBits);
    if (cls) {
      const instBits = allocInstanceForClass(callBits);
      const initBits = lookupClassAttr(callBits, '__init__');
      if (initBits === undefined) {
        return instBits;
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
      throw new Error('TypeError: object is not callable');
    }
    const func = getFunction(funcBits);
    if (!func) {
      throw new Error('TypeError: call expects function object');
    }
    const attrs = func.attrs || new Map();
    const argNamesBits = attrs.get('__molt_arg_names__');
    const argNamesTuple = getTuple(argNamesBits);
    if (!argNamesTuple) {
      throw new Error('TypeError: call expects function object');
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
    if (selfBits !== null) {
      posArgs.unshift(selfBits);
    }
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
    return callFunctionBits(funcBits, finalArgs);
  },
  is_callable: (val) => {
    if (getFunction(val) || getBoundMethod(val)) return boxBool(true);
    const attr = lookupAttr(normalizePtrBits(val), '__call__');
    return boxBool(attr !== undefined);
  },
  is_function_obj: (val) => boxBool(getFunction(val) !== null),
  index: (seq, idxBits) => {
    const idx = Number(unboxInt(idxBits));
    const list = getList(seq);
    const tup = getTuple(seq);
    const bytes = getBytes(seq);
    const bytearray = getBytearray(seq);
    const items = list ? list.items : tup ? tup.items : null;
    if (items) {
      let pos = idx;
      if (pos < 0) pos += items.length;
      if (pos < 0 || pos >= items.length) return boxNone();
      return items[pos];
    }
    if (bytes || bytearray) {
      const data = bytes ? bytes.data : bytearray.data;
      let pos = idx;
      if (pos < 0) pos += data.length;
      if (pos < 0 || pos >= data.length) return boxNone();
      return boxInt(data[pos]);
    }
    return boxNone();
  },
  store_index: (seq, idxBits, val) => {
    const idx = Number(unboxInt(idxBits));
    const list = getList(seq);
    if (list) {
      let i = idx;
      if (i < 0) i += list.items.length;
      if (i < 0 || i >= list.items.length) return boxNone();
      list.items[i] = val;
      return seq;
    }
    const bytearray = getBytearray(seq);
    if (bytearray) {
      let i = idx;
      if (i < 0) i += bytearray.data.length;
      if (i < 0 || i >= bytearray.data.length) return boxNone();
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
    return boxNone();
  },
  bytes_find: () => boxNone(),
  bytearray_find: () => boxNone(),
  string_find: () => boxNone(),
  string_format: () => boxNone(),
  string_startswith: () => boxBool(false),
  string_endswith: () => boxBool(false),
  string_count: () => boxInt(0),
  string_count_slice: () => boxInt(0),
  string_join: () => boxNone(),
  string_split: () => boxNone(),
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
  string_strip: (haystack) => {
    const str = getStrObj(haystack);
    if (str === null) return boxNone();
    return boxPtr({ type: 'str', value: str.trim() });
  },
  bytes_split: () => boxNone(),
  bytearray_split: () => boxNone(),
  string_replace: () => boxNone(),
  bytes_replace: () => boxNone(),
  bytearray_replace: () => boxNone(),
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
      if (setName !== undefined) {
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
  func_new: (fnIdx, arity) => {
    const addr = allocRaw(16);
    if (addr && memory) {
      const view = new DataView(memory.buffer);
      view.setBigInt64(addr, fnIdx, true);
      view.setBigInt64(addr + 8, arity, true);
    }
    return boxPtr({
      type: 'function',
      idx: Number(fnIdx),
      arity: Number(arity),
      attrs: new Map(),
      memAddr: addr || null,
    });
  },
  func_new_closure: (fnIdx, arity, closureBits) => {
    const bits = baseImports.func_new(fnIdx, arity);
    const func = getFunction(bits);
    if (func) {
      func.closure = closureBits;
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
    instanceClasses.set(ptrAddr(objBits), classBits);
    return boxNone();
  },
  context_null: (val) => val,
  id: (val) => val,
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
  exception_push: () => exceptionPush(),
  exception_pop: () => exceptionPop(),
  exception_last: () => exceptionLast(),
  exception_new: (kind, msg) => exceptionNew(kind, msg),
  exception_clear: () => exceptionClear(),
  exception_pending: () => exceptionPending(),
  exception_kind: (exc) => exceptionKind(exc),
  exception_message: (exc) => exceptionMessage(exc),
  exception_set_cause: (exc, cause) => exceptionSetCause(exc, cause),
  exception_context_set: (exc) => exceptionContextSet(exc),
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
  missing: () => missingSentinel(),
  not_implemented: () => notImplementedSentinel(),
  repr_builtin: (val) => baseImports.repr_from_obj(val),
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
      const msgBits = isTag(valBits, TAG_NONE)
        ? boxPtr({ type: 'str', value: '' })
        : baseImports.str_from_obj(valBits);
      const msg = getStrObj(msgBits) ?? '';
      const exc = exceptionNew(
        boxPtr({ type: 'str', value: 'StopIteration' }),
        boxPtr({ type: 'str', value: msg }),
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
  anext_builtin: (iterBits, defaultBits) => {
    const missing = missingSentinel();
    if (defaultBits === missing) {
      return baseImports.anext(iterBits);
    }
    if (!memory || !table) return boxNone();
    const pollFn = baseImports.anext_default_poll;
    let pollIdx = anextDefaultPollIdx;
    if (pollIdx === null) {
      for (let i = 0; i < table.length; i += 1) {
        if (table.get(i) === pollFn) {
          pollIdx = i;
          break;
        }
      }
      if (pollIdx === null) {
        pollIdx = table.length;
        table.grow(1);
        table.set(pollIdx, pollFn);
      }
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
  print_builtin: (argsBits) => {
    const args = getTuple(argsBits);
    if (!args) {
      throw new Error('TypeError: print expects a tuple');
    }
    if (args.items.length === 0) {
      baseImports.print_newline();
      return boxNone();
    }
    if (args.items.length === 1) {
      baseImports.print_obj(args.items[0]);
      return boxNone();
    }
    const parts = [];
    for (const val of args.items) {
      const strBits = baseImports.str_from_obj(val);
      const text = getStrObj(strBits);
      parts.push(text === null ? '<obj>' : text);
    }
    baseImports.print_obj(boxPtr({ type: 'str', value: parts.join(' ') }));
    return boxNone();
  },
  super_builtin: (typeBits, objBits) => baseImports.super_new(typeBits, objBits),
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
const parseWasmImports = (buffer) => {
  const bytes = new Uint8Array(buffer);
  if (bytes.length < 8) {
    throw new Error('Invalid wasm binary');
  }
  let offset = 8;
  let memoryImport = null;
  let tableImport = null;
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
        } else {
          throw new Error(`Unknown import kind ${kind}`);
        }
      }
    } else {
      offset = sectionEnd;
    }
  }
  return { memory: memoryImport, table: tableImport };
};
const wasmImports = parseWasmImports(wasmBuffer);
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
