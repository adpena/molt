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
  builtinTypes.set(tag, clsBits);
  setClassBases(clsBits, baseBits);
  return clsBits;
};
const heap = new Map();
const instanceClasses = new Map();
let nextPtr = 1n << 40n;
let memory = null;
let table = null;
const chanQueues = new Map();
const chanCaps = new Map();
const moduleCache = new Map();
const sleepPending = new Set();
let nextChanId = 1n;
let heapPtr = 1 << 20;
const HEADER_SIZE = 32;
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
const normalizePtrBits = (val) => {
  if (val === 0n) return val;
  if (isPtr(val)) return val;
  return boxPtrAddr(val);
};
const getObj = (val) => heap.get(val & POINTER_MASK);
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
const getClass = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'class') return obj;
  return null;
};
const getModule = (val) => {
  const obj = getObj(val);
  if (obj && obj.type === 'module') return obj;
  return null;
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
const listFromArray = (items) => boxPtr({ type: 'list', items });
const tupleFromArray = (items) => boxPtr({ type: 'tuple', items });
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
    if (obj && obj.type === 'iter') return true;
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
        return boxPtr({ type: 'bound_method', func: attrVal, self: instanceBits });
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
    const clsBits = instanceClasses.get(ptrAddr(objBits));
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
  const pollIdx = view.getUint32(addr - 24, true);
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
  const pollIdx = view.getUint32(addr - 24, true);
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
  const pollIdx = view.getUint32(addr - 24, true);
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
  async_sleep: (taskPtr) => {
    if (taskPtr === 0n) return boxNone();
    const key = taskPtr.toString();
    if (!sleepPending.has(key)) {
      sleepPending.add(key);
      return boxPending();
    }
    return boxNone();
  },
  block_on: (taskPtr) => {
    if (!memory || !table) return 0n;
    const addr = ptrAddr(taskPtr);
    const view = new DataView(memory.buffer);
    const pollIdx = view.getUint32(addr - 24, true);
    const poll = table.get(pollIdx);
    if (!poll) return 0n;
    while (true) {
      const res = poll(taskPtr);
      if (isPending(res)) continue;
      return res;
    }
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
    if (isIntLike(a) && isIntLike(b)) {
      return boxBool(unboxIntLike(a) < unboxIntLike(b));
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      if (Number.isNaN(lf) || Number.isNaN(rf)) return boxBool(false);
      return boxBool(lf < rf);
    }
    return compareTypeError('<', a, b);
  },
  le: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxBool(unboxIntLike(a) <= unboxIntLike(b));
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      if (Number.isNaN(lf) || Number.isNaN(rf)) return boxBool(false);
      return boxBool(lf <= rf);
    }
    return compareTypeError('<=', a, b);
  },
  gt: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxBool(unboxIntLike(a) > unboxIntLike(b));
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      if (Number.isNaN(lf) || Number.isNaN(rf)) return boxBool(false);
      return boxBool(lf > rf);
    }
    return compareTypeError('>', a, b);
  },
  ge: (a, b) => {
    if (isIntLike(a) && isIntLike(b)) {
      return boxBool(unboxIntLike(a) >= unboxIntLike(b));
    }
    const lf = numberFromVal(a);
    const rf = numberFromVal(b);
    if (lf !== null && rf !== null) {
      if (Number.isNaN(lf) || Number.isNaN(rf)) return boxBool(false);
      return boxBool(lf >= rf);
    }
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
  handle_resolve: (bits) => {
    if (!isPtr(bits)) return 0n;
    const id = bits & POINTER_MASK;
    const obj = heap.get(id);
    if (obj && obj.memAddr) return BigInt(obj.memAddr);
    if (!obj) return BigInt(ptrAddr(bits));
    return 0n;
  },
  get_attr_generic: (obj, namePtr, nameLen) =>
    getAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen)),
  get_attr_object: (obj, namePtr, nameLen) =>
    getAttrValue(obj, readUtf8(namePtr, nameLen)),
  set_attr_generic: (obj, namePtr, nameLen, val) =>
    setAttrValue(normalizePtrBits(obj), readUtf8(namePtr, nameLen), val),
  set_attr_object: (obj, namePtr, nameLen, val) =>
    setAttrValue(obj, readUtf8(namePtr, nameLen), val),
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
  list_append: () => boxNone(),
  list_pop: () => boxNone(),
  list_extend: () => boxNone(),
  list_insert: () => boxNone(),
  list_remove: () => boxNone(),
  list_count: () => boxNone(),
  list_index: () => boxNone(),
  tuple_from_list: (val) => {
    const list = getList(val);
    if (list) return tupleFromArray([...list.items]);
    const tup = getTuple(val);
    if (tup) return val;
    return boxNone();
  },
  dict_new: () => boxNone(),
  dict_set: () => boxNone(),
  dict_get: () => boxNone(),
  dict_pop: () => boxNone(),
  dict_keys: () => boxNone(),
  dict_values: () => boxNone(),
  dict_items: () => boxNone(),
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
    }
    return boxNone();
  },
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
    const setLike = getSetLike(val);
    if (setLike) {
      return boxPtr({ type: 'iter', target: val, idx: 0 });
    }
    return boxNone();
  },
  aiter: (val) => {
    const attr = lookupAttr(val, '__aiter__');
    if (attr === undefined) {
      throw new Error('TypeError: object is not async iterable');
    }
    return callCallable0(attr);
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
    const setLike = getSetLike(target);
    const items = list ? list.items : tup ? tup.items : setLike ? [...setLike.items] : null;
    if (!items) return boxNone();
    if (iter.idx >= items.length) {
      return tupleFromArray([boxNone(), boxBool(true)]);
    }
    const value = items[iter.idx];
    iter.idx += 1;
    return tupleFromArray([value, boxBool(false)]);
  },
  anext: (val) => {
    const attr = lookupAttr(val, '__anext__');
    if (attr === undefined) {
      throw new Error('TypeError: object is not an async iterator');
    }
    return callCallable0(attr);
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
    return boxNone();
  },
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
  class_new: (nameBits) => {
    const name = getStrObj(nameBits);
    return boxPtr({
      type: 'class',
      name: name ?? '<class>',
      attrs: new Map(),
      baseBits: boxNone(),
      basesBits: null,
      mroBits: null,
    });
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
