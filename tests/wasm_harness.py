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
const boxBool = (b) => QNAN | TAG_BOOL | (b ? 1n : 0n);
const boxNone = () => QNAN | TAG_NONE;
const boxPending = () => QNAN | TAG_PENDING;
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
  contains: () => boxBool(false),
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
    getAttrValue(obj, readUtf8(namePtr, nameLen)),
  get_attr_object: (obj, namePtr, nameLen) =>
    getAttrValue(obj, readUtf8(namePtr, nameLen)),
  set_attr_generic: (obj, namePtr, nameLen, val) =>
    setAttrValue(obj, readUtf8(namePtr, nameLen), val),
  set_attr_object: (obj, namePtr, nameLen, val) =>
    setAttrValue(obj, readUtf8(namePtr, nameLen), val),
  del_attr_generic: (obj, namePtr, nameLen) =>
    delAttrValue(obj, readUtf8(namePtr, nameLen)),
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
    const items = list ? list.items : tup ? tup.items : null;
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
