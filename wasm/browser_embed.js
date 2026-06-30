const WASM_MAGIC = 0x6d736100;
const WASM_VERSION = 0x1;
const ENOSYS = 38;
const EINVAL = 22;
const ENOMEM = 12;
const WASI_ERRNO_NOSYS = 52;
const WASI_ERRNO_INVAL = 28;
const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_MASK = 0x0007000000000000n;
const INT_MASK = (1n << 47n) - 1n;
const TYPE_TAG_BYTES = 6;
const TABLE_REF_EXPORT_PREFIX = '__molt_table_ref_';
const NATIVE_CALLABLE_IMPORT_MODULE = 'molt_native';
const UTF8_DECODER = new TextDecoder('utf-8');
const UTF8_ENCODER = new TextEncoder();

const readVarUint = (view, offset) => {
  let result = 0;
  let shift = 0;
  let pos = offset;
  while (true) {
    if (pos >= view.length) {
      throw new Error('Unexpected EOF while reading varuint');
    }
    const byte = view[pos++];
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7;
  }
  return { value: result >>> 0, offset: pos };
};

const readString = (view, offset) => {
  const lenRes = readVarUint(view, offset);
  const len = lenRes.value;
  const start = lenRes.offset;
  const end = start + len;
  if (end > view.length) {
    throw new Error('Unexpected EOF while reading string');
  }
  return { value: UTF8_DECODER.decode(view.subarray(start, end)), offset: end };
};

const readLimits = (view, offset) => {
  if (offset >= view.length) {
    throw new Error('Unexpected EOF while reading limits');
  }
  const flags = view[offset++];
  const minRes = readVarUint(view, offset);
  let max = null;
  offset = minRes.offset;
  if (flags & 0x1) {
    const maxRes = readVarUint(view, offset);
    max = maxRes.value;
    offset = maxRes.offset;
  }
  return { min: minRes.value, max, offset };
};

export const parseMoltWasmImports = (buffer) => {
  const view = new Uint8Array(buffer);
  const header = new DataView(view.buffer, view.byteOffset, view.byteLength);
  if (header.getUint32(0, true) !== WASM_MAGIC) {
    throw new Error('Invalid WASM header');
  }
  if (header.getUint32(4, true) !== WASM_VERSION) {
    throw new Error('Unsupported WASM version');
  }
  let offset = 8;
  const result = { funcImports: [], memory: null, table: null };
  while (offset < view.length) {
    const sectionId = view[offset++];
    const sizeRes = readVarUint(view, offset);
    const size = sizeRes.value;
    offset = sizeRes.offset;
    const end = offset + size;
    if (end > view.length) {
      throw new Error('Unexpected EOF while reading section');
    }
    if (sectionId !== 2) {
      offset = end;
      continue;
    }
    let inner = offset;
    const countRes = readVarUint(view, inner);
    let count = countRes.value;
    inner = countRes.offset;
    while (count > 0) {
      const moduleRes = readString(view, inner);
      const module = moduleRes.value;
      inner = moduleRes.offset;
      const nameRes = readString(view, inner);
      const name = nameRes.value;
      inner = nameRes.offset;
      const kind = view[inner++];
      if (kind === 0) {
        inner = readVarUint(view, inner).offset;
        result.funcImports.push({ module, name });
      } else if (kind === 1) {
        inner += 1;
        const limits = readLimits(view, inner);
        inner = limits.offset;
        result.table = { min: limits.min, max: limits.max };
      } else if (kind === 2) {
        const limits = readLimits(view, inner);
        inner = limits.offset;
        result.memory = { min: limits.min, max: limits.max };
      } else {
        inner = readVarUint(view, inner).offset;
      }
      count -= 1;
    }
    offset = end;
  }
  return result;
};

const mergeLimits = (left, right, label) => {
  if (!left) return right;
  if (!right) return left;
  const min = Math.max(left.min, right.min);
  let max = null;
  if (left.max !== null && right.max !== null) {
    max = Math.max(left.max, right.max);
  } else {
    max = left.max !== null ? left.max : right.max;
  }
  if (max !== null && min > max) {
    throw new Error(`Incompatible ${label} limits`);
  }
  return { min, max };
};

const makeMemory = (limits, initialPages = null) => {
  const min = Math.max(limits?.min ?? 0, initialPages ?? 0, 1);
  const descriptor = { initial: min };
  if (limits?.max !== null && limits?.max !== undefined) {
    descriptor.maximum = Math.max(limits.max, min);
  }
  return new WebAssembly.Memory(descriptor);
};

const makeTable = (limits, initial = null) => {
  const min = Math.max(limits?.min ?? 0, initial ?? 0, 1);
  const descriptor = { element: 'anyfunc', initial: min };
  if (limits?.max !== null && limits?.max !== undefined) {
    descriptor.maximum = Math.max(limits.max, min);
  }
  return new WebAssembly.Table(descriptor);
};

const requireIntegerField = (source, name) => {
  const value = source?.[name];
  if (!Number.isInteger(value)) {
    throw new Error(`manifest.abi.browser_embed.table_layout.${name} must be an integer`);
  }
  return value;
};

const NATIVE_CALLABLE_ABI_OBJECT_CALL_V1 = 'molt.object_call_v1';
const NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1 = 'molt.object_callargs_v1';
const NATIVE_CALLABLE_ABI_FORWARD_F32_V1 = 'molt.forward_f32_v1';

const nativeCallableBrowserSignature = (abi) => {
  if (abi === NATIVE_CALLABLE_ABI_OBJECT_CALL_V1) {
    return { params: ['molt.value...'], result: 'molt.value' };
  }
  if (abi === NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1) {
    return { params: ['molt.callargs'], result: 'molt.value' };
  }
  if (abi === NATIVE_CALLABLE_ABI_FORWARD_F32_V1) {
    return { params: ['bytes.float32'], result: 'bytes.float32' };
  }
  throw new Error(`unsupported browser native callable ABI: ${abi}`);
};

const requireNativeCallableSignature = (symbol, abi, rawSignature) => {
  if (!rawSignature || typeof rawSignature !== 'object' || Array.isArray(rawSignature)) {
    throw new Error(
      `manifest.abi.browser_embed.native_callables.symbols.${symbol}.signature must be an object`,
    );
  }
  const expected = nativeCallableBrowserSignature(abi);
  if (
    !Array.isArray(rawSignature.params) ||
    rawSignature.params.length !== expected.params.length ||
    rawSignature.params.some((value, index) => value !== expected.params[index]) ||
    rawSignature.result !== expected.result
  ) {
    throw new Error(
      `manifest.abi.browser_embed.native_callables.symbols.${symbol}.signature ` +
        `must match ${abi}`,
    );
  }
  return { params: [...expected.params], result: expected.result };
};

const nativeCallableManifestFromAbi = (raw) => {
  if (raw === undefined || raw === null) {
    return { module: NATIVE_CALLABLE_IMPORT_MODULE, symbols: {}, authoritative: false };
  }
  if (!raw || typeof raw !== 'object') {
    throw new Error('manifest.abi.browser_embed.native_callables must be an object');
  }
  const module = raw.module || NATIVE_CALLABLE_IMPORT_MODULE;
  if (module !== NATIVE_CALLABLE_IMPORT_MODULE) {
    throw new Error(
      `manifest.abi.browser_embed.native_callables.module must be ${NATIVE_CALLABLE_IMPORT_MODULE}`,
    );
  }
  const symbols = raw.symbols || {};
  if (!symbols || typeof symbols !== 'object' || Array.isArray(symbols)) {
    throw new Error('manifest.abi.browser_embed.native_callables.symbols must be an object');
  }
  const normalizedSymbols = {};
  for (const [symbol, spec] of Object.entries(symbols)) {
    if (typeof symbol !== 'string' || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(symbol)) {
      throw new Error(
        `manifest.abi.browser_embed.native_callables has invalid symbol ${String(symbol)}`,
      );
    }
    if (!spec || typeof spec !== 'object' || Array.isArray(spec)) {
      throw new Error(
        `manifest.abi.browser_embed.native_callables.symbols.${symbol} must be an object`,
      );
    }
    if (spec.binding !== undefined && spec.binding !== 'direct_symbol') {
      throw new Error(
        `manifest.abi.browser_embed.native_callables.symbols.${symbol}.binding must be direct_symbol`,
      );
    }
    if (typeof spec.abi !== 'string' || spec.abi.length === 0) {
      throw new Error(
        `manifest.abi.browser_embed.native_callables.symbols.${symbol}.abi must be a string`,
      );
    }
    const signature = requireNativeCallableSignature(symbol, spec.abi, spec.signature);
    normalizedSymbols[symbol] = { ...spec, signature };
  }
  return {
    module: NATIVE_CALLABLE_IMPORT_MODULE,
    symbols: normalizedSymbols,
    authoritative: true,
  };
};

const browserAbiFromManifest = (manifest) => {
  const abi = manifest?.abi?.browser_embed;
  if (!abi || typeof abi !== 'object') {
    throw new Error('manifest missing abi.browser_embed');
  }
  if (!Array.isArray(abi.call_indirect_imports) || abi.call_indirect_imports.length === 0) {
    throw new Error('manifest.abi.browser_embed.call_indirect_imports must be non-empty');
  }
  const callIndirectImports = new Set();
  for (const name of abi.call_indirect_imports) {
    if (typeof name !== 'string' || !/^molt_call_indirect\d+$/.test(name)) {
      throw new Error(`invalid manifest call_indirect import: ${String(name)}`);
    }
    callIndirectImports.add(name);
  }
  const runtimeImportFallbacks = abi.runtime_import_fallbacks || {};
  if (!runtimeImportFallbacks || typeof runtimeImportFallbacks !== 'object') {
    throw new Error('manifest.abi.browser_embed.runtime_import_fallbacks must be an object');
  }
  const tableLayout = abi.table_layout || {};
  const legacyTableBase = requireIntegerField(tableLayout, 'legacy_table_base');
  const reservedRuntimeCallableBase = requireIntegerField(
    tableLayout,
    'reserved_runtime_callable_base',
  );
  const reservedRuntimeCallableCount = requireIntegerField(
    tableLayout,
    'reserved_runtime_callable_count',
  );
  return {
    callIndirectImports,
    runtimeImportFallbacks,
    nativeCallables: nativeCallableManifestFromAbi(abi.native_callables),
    tableLayout: {
      legacyTableBase,
      reservedRuntimeCallableBase,
      reservedRuntimeCallableCount,
      reservedRuntimeSharedPrefixLen:
        reservedRuntimeCallableBase + reservedRuntimeCallableCount * 2,
    },
  };
};

const bytesFromBufferSource = (source) => {
  if (source instanceof Uint8Array) {
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }
  if (ArrayBuffer.isView(source)) {
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }
  if (source instanceof ArrayBuffer) {
    return new Uint8Array(source);
  }
  throw new Error(`expected an ArrayBuffer or typed array, got ${Object.prototype.toString.call(source)}`);
};

const copyBytes = (bytes) => {
  const out = new Uint8Array(bytes.byteLength);
  out.set(bytes);
  return out;
};

const fetchArrayBuffer = async (url) => {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load ${url}: HTTP ${response.status}`);
  }
  return await response.arrayBuffer();
};

const fetchJson = async (url) => {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load ${url}: HTTP ${response.status}`);
  }
  return await response.json();
};

const resolveUrl = (path, baseUrl) => new URL(path, baseUrl).href;

const boxInt = (value) => {
  let v = BigInt(value);
  if (v < 0n) {
    v = (1n << 47n) + v;
  }
  return QNAN | TAG_INT | (v & INT_MASK);
};

const isIntBits = (bits) => (bits & (QNAN | TAG_MASK)) === (QNAN | TAG_INT);
const unboxInt = (bits) => {
  let value = bits & INT_MASK;
  if ((value & (1n << 46n)) !== 0n) {
    value -= 1n << 47n;
  }
  return Number(value);
};

const writeBytesToMemory = (memory, ptr, bytes) => {
  if (!memory) return false;
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  if (!Number.isFinite(addr) || addr === 0) return false;
  new Uint8Array(memory.buffer, addr, bytes.length).set(bytes);
  return true;
};

const writeU32ToMemory = (memory, ptr, value) => {
  if (!memory) return false;
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  if (!Number.isFinite(addr) || addr === 0) return false;
  new DataView(memory.buffer).setUint32(addr, Number(value) >>> 0, true);
  return true;
};

const memoryOffset32 = (ptr, label) => {
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr);
  if (!Number.isInteger(addr) || addr < 0 || addr > 0xffffffff) {
    throw new TypeError(`Expected wasm32 memory offset for ${label}, got ${ptr}`);
  }
  return addr >>> 0;
};

const allocRuntimeTempBytes = (runtime, memory, bytes) => {
  if (!runtime || !memory) {
    throw new Error('runtime not initialized');
  }
  if (typeof runtime.exports?.molt_scratch_alloc !== 'function') {
    throw new Error('runtime is missing required export: molt_scratch_alloc');
  }
  const payload = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const ptr = runtime.exports.molt_scratch_alloc(BigInt(payload.length));
  if (!ptr || ptr === 0n) {
    throw new Error('molt_scratch_alloc failed');
  }
  const payloadPtr = typeof ptr === 'bigint' ? ptr : BigInt(ptr);
  new Uint8Array(memory.buffer, Number(payloadPtr), payload.length).set(payload);
  return { allocPtr: payloadPtr, payloadPtr, size: payload.length };
};

const freeRuntimeTempBytes = (runtime, temp) => {
  if (
    !runtime ||
    typeof runtime.exports?.molt_scratch_free !== 'function' ||
    !temp ||
    !temp.allocPtr
  ) {
    return;
  }
  runtime.exports.molt_scratch_free(temp.allocPtr, BigInt(temp.size ?? 0));
};

const decRefMaybe = (runtime, bits) => {
  if (
    runtime &&
    typeof runtime.exports?.molt_dec_ref_obj === 'function' &&
    bits !== null &&
    bits !== undefined &&
    bits !== 0n &&
    bits !== 0
  ) {
    runtime.exports.molt_dec_ref_obj(bits);
  }
};

const makeBytesObject = (runtime, memory, bytes) => {
  const payload = copyBytes(bytesFromBufferSource(bytes));
  const tempBytes = allocRuntimeTempBytes(runtime, memory, payload);
  const tempOut = allocRuntimeTempBytes(runtime, memory, new Uint8Array(8));
  try {
    const status = runtime.exports.molt_bytes_from_bytes(
      Number(tempBytes.payloadPtr),
      BigInt(payload.length),
      Number(tempOut.payloadPtr),
    );
    if (Number(status) !== 0) {
      throw new Error(`molt_bytes_from_bytes failed with status ${status}`);
    }
    return new DataView(memory.buffer).getBigUint64(Number(tempOut.payloadPtr), true);
  } finally {
    freeRuntimeTempBytes(runtime, tempBytes);
    freeRuntimeTempBytes(runtime, tempOut);
  }
};

const readRuntimeStringBits = (runtime, memory, stringBits) => {
  if (!runtime || !memory || !stringBits || stringBits === 0n) {
    return null;
  }
  if (
    typeof runtime.exports?.molt_string_as_ptr !== 'function' ||
    typeof runtime.exports?.molt_dec_ref_obj !== 'function'
  ) {
    return null;
  }
  const temp = allocRuntimeTempBytes(runtime, memory, new Uint8Array(8));
  try {
    const ptr = runtime.exports.molt_string_as_ptr(stringBits, temp.payloadPtr);
    if (!ptr || ptr === 0n) {
      return null;
    }
    const len = new DataView(memory.buffer).getBigUint64(Number(temp.payloadPtr), true);
    const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
    return UTF8_DECODER.decode(new Uint8Array(memory.buffer, addr, Number(len)));
  } finally {
    freeRuntimeTempBytes(runtime, temp);
  }
};

const pendingRuntimeExceptionMessage = (runtime, memory) => {
  if (!runtime) {
    return null;
  }
  const errPending =
    typeof runtime.exports?.molt_err_pending === 'function' &&
    Number(runtime.exports.molt_err_pending()) !== 0;
  const exceptionPending =
    typeof runtime.exports?.molt_exception_pending === 'function' &&
    Number(runtime.exports.molt_exception_pending()) !== 0;
  const exceptionPendingFast =
    typeof runtime.exports?.molt_exception_pending_fast === 'function' &&
    Number(runtime.exports.molt_exception_pending_fast()) !== 0;
  if (!errPending && !exceptionPending && !exceptionPendingFast) {
    return null;
  }
  let excBits = 0n;
  let shouldDecRefFetched = false;
  if (errPending) {
    const fetched =
      typeof runtime.exports.molt_err_fetch === 'function'
        ? runtime.exports.molt_err_fetch.bind(runtime.exports)
        : null;
    const peeked =
      typeof runtime.exports.molt_err_peek === 'function'
        ? runtime.exports.molt_err_peek.bind(runtime.exports)
        : null;
    excBits = fetched ? fetched() : peeked ? peeked() : 0n;
    shouldDecRefFetched = Boolean(fetched);
  } else if (typeof runtime.exports?.molt_exception_last === 'function') {
    excBits = runtime.exports.molt_exception_last();
    shouldDecRefFetched = true;
  }
  try {
    if (
      excBits &&
      excBits !== 0n &&
      typeof runtime.exports?.molt_object_repr === 'function' &&
      typeof runtime.exports?.molt_dec_ref_obj === 'function'
    ) {
      const reprBits = runtime.exports.molt_object_repr(excBits);
      if (reprBits && reprBits !== 0n) {
        try {
          const repr = readRuntimeStringBits(runtime, memory, reprBits);
          if (repr && repr !== 'None') {
            return `Unhandled Molt exception: ${repr}`;
          }
        } finally {
          runtime.exports.molt_dec_ref_obj(reprBits);
        }
      }
    }
    return 'Unhandled Molt exception';
  } finally {
    if (shouldDecRefFetched && excBits && excBits !== 0n) {
      decRefMaybe(runtime, excBits);
    }
  }
};

const readBytesObject = (runtime, memory, bits) => {
  if (!runtime || !memory) {
    throw new Error('runtime not initialized');
  }
  const tagFn = runtime.exports?.molt_type_tag_of_bits;
  if (typeof tagFn === 'function' && Number(tagFn(bits)) !== TYPE_TAG_BYTES) {
    throw new Error('Molt kernel result is not bytes');
  }
  if (typeof runtime.exports?.molt_bytes_as_ptr !== 'function') {
    throw new Error('runtime is missing required export: molt_bytes_as_ptr');
  }
  const tempLen = allocRuntimeTempBytes(runtime, memory, new Uint8Array(8));
  try {
    const ptr = runtime.exports.molt_bytes_as_ptr(
      bits,
      memoryOffset32(tempLen.payloadPtr, 'molt_bytes_as_ptr out_len'),
    );
    const pending = pendingRuntimeExceptionMessage(runtime, memory);
    if (pending) {
      throw new Error(pending);
    }
    if (!ptr || ptr === 0n) {
      throw new Error('molt_bytes_as_ptr returned null');
    }
    const len = new DataView(memory.buffer).getBigUint64(
      memoryOffset32(tempLen.payloadPtr, 'molt_bytes_as_ptr out_len'),
      true,
    );
    const addr = memoryOffset32(ptr, 'molt_bytes_as_ptr result');
    return copyBytes(new Uint8Array(memory.buffer, addr, Number(len)));
  } finally {
    freeRuntimeTempBytes(runtime, tempLen);
  }
};

const makeHostArg = (runtime, memory, value) => {
  if (typeof value === 'number' && Number.isInteger(value)) {
    return boxInt(value);
  }
  if (typeof value === 'bigint') {
    return boxInt(value);
  }
  if (value instanceof Uint8Array || value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    return makeBytesObject(runtime, memory, bytesFromBufferSource(value));
  }
  throw new Error(`unsupported Molt browser embed argument: ${Object.prototype.toString.call(value)}`);
};

const nativeCallableMap = (value) => {
  if (!value) return {};
  if (typeof value !== 'object') {
    throw new Error('nativeCallables must be an object keyed by native symbol');
  }
  return value;
};

const nativeCallableAbiMap = (value) => {
  if (!value) return {};
  if (typeof value !== 'object') {
    throw new Error('nativeCallableAbis must be an object keyed by native symbol');
  }
  return value;
};

const nativeCallableManifestFromOptions = (options) => {
  if (options.nativeCallableManifest !== undefined) {
    return nativeCallableManifestFromAbi(options.nativeCallableManifest);
  }
  const manifestNativeCallables =
    options.manifest?.abi?.browser_embed?.native_callables ||
    options.manifest?.abi?.native_callables;
  return nativeCallableManifestFromAbi(manifestNativeCallables);
};

const byteLengthFromWasm = (symbol, rawLength) => {
  const byteLength = typeof rawLength === 'bigint' ? Number(rawLength) : Number(rawLength);
  if (!Number.isSafeInteger(byteLength) || byteLength < 0) {
    throw new Error(`${symbol} ${NATIVE_CALLABLE_ABI_FORWARD_F32_V1} byte length is invalid`);
  }
  if (byteLength % Float32Array.BYTES_PER_ELEMENT !== 0) {
    throw new Error(
      `${symbol} ${NATIVE_CALLABLE_ABI_FORWARD_F32_V1} byte length ` +
        `${byteLength} is not divisible by ${Float32Array.BYTES_PER_ELEMENT}`,
    );
  }
  return byteLength;
};

const copyForwardF32Result = (symbol, result, output) => {
  if (result === undefined || result === output) {
    return;
  }
  const values = Array.isArray(result) ? new Float32Array(result) : result;
  if (!(values instanceof Float32Array)) {
    throw new Error(`${symbol} ${NATIVE_CALLABLE_ABI_FORWARD_F32_V1} must return or fill Float32Array`);
  }
  if (values.length !== output.length) {
    throw new Error(
      `${symbol} ${NATIVE_CALLABLE_ABI_FORWARD_F32_V1} returned ${values.length} values; ` +
        `expected ${output.length}`,
    );
  }
  output.set(values);
};

const callForwardF32Native = (state, symbol, impl, args) => {
  if (args.length !== 3) {
    throw new Error(
      `${symbol} ${NATIVE_CALLABLE_ABI_FORWARD_F32_V1} expects (inputPtr, byteLength, outputPtr)`,
    );
  }
  const inputPtr = memoryOffset32(args[0], `${symbol} input pointer`);
  const byteLength = byteLengthFromWasm(symbol, args[1]);
  const outputPtr = memoryOffset32(args[2], `${symbol} output pointer`);
  const input = new Float32Array(
    state.memory.buffer,
    inputPtr,
    byteLength / Float32Array.BYTES_PER_ELEMENT,
  );
  const output = new Float32Array(
    state.memory.buffer,
    outputPtr,
    byteLength / Float32Array.BYTES_PER_ELEMENT,
  );
  const result = impl(input, output, {
    abi: NATIVE_CALLABLE_ABI_FORWARD_F32_V1,
    byteLength,
    memory: state.memory,
    runtimeInstance: state.runtimeInstance,
    symbol,
  });
  copyForwardF32Result(symbol, result, output);
  return 0;
};

const normalizeMoltValueHandle = (symbol, abi, value, label) => {
  if (typeof value === 'bigint') {
    return value;
  }
  if (typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value)) {
    return BigInt.asUintN(64, BigInt(value));
  }
  throw new TypeError(
    `${symbol} ${abi} ${label} must be a Molt value handle (i64 BigInt)`,
  );
};

const callObjectNative = (state, symbol, impl, args, abi, expectedArity = null) => {
  if (expectedArity !== null && args.length !== expectedArity) {
    throw new Error(`${symbol} ${abi} expects ${expectedArity} Molt value handle argument(s)`);
  }
  const callArgs = args.map((arg, index) =>
    normalizeMoltValueHandle(symbol, abi, arg, `arg${index}`));
  const result = impl(...callArgs, {
    abi,
    arity: callArgs.length,
    memory: state.memory,
    runtimeInstance: state.runtimeInstance,
    symbol,
  });
  return normalizeMoltValueHandle(symbol, abi, result, 'result');
};

export const createMoltNativeCallableImports = (state, appImports, options = {}) => {
  const callables = nativeCallableMap(options.nativeCallables);
  const abiBySymbol = nativeCallableAbiMap(options.nativeCallableAbis);
  const manifestCallables = nativeCallableManifestFromOptions(options);
  const manifestSymbols = manifestCallables.symbols || {};
  const manifestAuthoritative = Boolean(
    options.requireNativeCallableManifest || manifestCallables.authoritative,
  );
  const imports = {};
  for (const entry of appImports?.funcImports || []) {
    if (entry.module !== NATIVE_CALLABLE_IMPORT_MODULE) {
      continue;
    }
    const symbol = entry.name;
    const manifestSpec = manifestSymbols[symbol] || null;
    if (manifestAuthoritative && !manifestSpec) {
      throw new Error(
        `app native callable import ${symbol} missing from ` +
          'manifest.abi.browser_embed.native_callables.symbols',
      );
    }
    imports[symbol] = (...args) => {
      const impl = callables[symbol];
      if (typeof impl !== 'function') {
        throw new Error(`missing browser native callable implementation for ${symbol}`);
      }
      const manifestAbi = manifestSpec?.abi || null;
      const overrideAbi = abiBySymbol[symbol] || impl.abi || options.nativeCallableAbi || null;
      if (manifestAbi && overrideAbi && manifestAbi !== overrideAbi) {
        throw new Error(
          `browser native callable ABI override for ${symbol} conflicts with manifest: ` +
            `${overrideAbi} != ${manifestAbi}`,
        );
      }
      const abi = overrideAbi || manifestAbi || NATIVE_CALLABLE_ABI_OBJECT_CALL_V1;
      if (abi === NATIVE_CALLABLE_ABI_OBJECT_CALL_V1) {
        return callObjectNative(state, symbol, impl, args, abi);
      }
      if (abi === NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1) {
        return callObjectNative(state, symbol, impl, args, abi, 1);
      }
      if (abi === NATIVE_CALLABLE_ABI_FORWARD_F32_V1) {
        return callForwardF32Native(state, symbol, impl, args);
      }
      throw new Error(`unsupported browser native callable ABI for ${symbol}: ${abi}`);
    };
  }
  return imports;
};

const installTableRefs = (instance, table) => {
  if (!instance || !table) {
    return;
  }
  const refs = [];
  for (const [name, value] of Object.entries(instance.exports)) {
    const match = /^__molt_table_ref_(\d+)$/.exec(name);
    if (!match || typeof value !== 'function') {
      continue;
    }
    refs.push({ index: Number(match[1]), fn: value });
  }
  refs.sort((a, b) => a.index - b.index);
  if (refs.length === 0) {
    return;
  }
  const maxIndex = refs[refs.length - 1].index;
  if (maxIndex >= table.length) {
    table.grow(maxIndex + 1 - table.length);
  }
  for (const ref of refs) {
    table.set(ref.index, ref.fn);
  }
};

const ensureTableCapacityForExportedRefs = (instance, table) => {
  if (!instance || !table) {
    return;
  }
  let maxIndex = -1;
  for (const name of Object.keys(instance.exports)) {
    const match = /^__molt_table_ref_(\d+)$/.exec(name);
    if (!match) {
      continue;
    }
    const idx = Number(match[1]);
    if (Number.isInteger(idx) && idx > maxIndex) {
      maxIndex = idx;
    }
  }
  if (maxIndex >= table.length) {
    table.grow(maxIndex + 1 - table.length);
  }
};

const normalizeI64Result = (value) => {
  if (value === undefined || value === null) {
    return 0n;
  }
  return typeof value === 'bigint' ? value : BigInt(value);
};

const normalizeI64BridgeValue = (value, label) => {
  if (typeof value === 'bigint') {
    return value;
  }
  if (typeof value !== 'number' || !Number.isFinite(value) || !Number.isInteger(value)) {
    throw new TypeError(`Expected integer for ${label}, got ${value}`);
  }
  return BigInt.asUintN(64, BigInt(value));
};

const callIsolateImportExport = (fn, args) => {
  if (args.length !== 1) {
    throw new TypeError(`molt_isolate_import expects one i64 handle, got ${args.length}`);
  }
  const handle = normalizeI64BridgeValue(args[0], 'molt_isolate_import handle');
  return normalizeI64BridgeValue(fn(handle), 'molt_isolate_import result');
};

const normalizeValueForKind = (value, kind) => {
  if (kind === 'i64') {
    return normalizeI64Result(value);
  }
  if (kind === 'i32') {
    return typeof value === 'bigint' ? Number(value) : Number(value);
  }
  return value;
};

const normalizeImportResult = (value, resultKind) => {
  if (resultKind === 'i64') {
    return normalizeI64Result(value);
  }
  if (resultKind === 'i32') {
    return typeof value === 'bigint' ? Number(value) : Number(value);
  }
  return value;
};

const callWithSignature = (fn, signature, args) => {
  if (!signature || !Array.isArray(signature.params)) {
    return fn(...args);
  }
  const callArgs = args.map((value, index) =>
    normalizeValueForKind(value, signature.params[index] || null));
  const out = fn(...callArgs);
  return normalizeImportResult(out, signature.result || null);
};

const tableRefExportName = (index) => `${TABLE_REF_EXPORT_PREFIX}${index}`;

const buildRuntimeImports = (appModule, runtimeInstance, manifest, browserAbi) => {
  const imports = {};
  const runtimeImports = manifest?.abi?.runtime_imports || {};
  const manifestNames = new Set(runtimeImports.names || []);
  const signatures = runtimeImports.signatures || {};
  const runtimeExportSignatures = runtimeImports.runtime_export_signatures || {};
  const resultKinds = runtimeImports.result_kinds || {};
  const runtimeExport = (name) => {
    const fn = runtimeInstance.exports[name];
    return typeof fn === 'function' ? fn : null;
  };
  const makeCallBindFallback = (entryName, fallback) => {
    if (!Number.isInteger(fallback.call_arity)) {
      throw new Error(`manifest fallback for ${entryName} missing call_arity`);
    }
    const exports = Array.isArray(fallback.exports) ? fallback.exports : [];
    const [callBindName, callargsNewName, callargsPushPosName] = exports;
    const callBindIc = runtimeExport(callBindName);
    const callargsNew = runtimeExport(callargsNewName);
    const callargsPushPos = runtimeExport(callargsPushPosName);
    if (!callBindIc || !callargsNew || !callargsPushPos) {
      throw new Error(`runtime missing fallback exports for ${entryName}`);
    }
    return (methodBits, ...argBits) => {
      const builderBits = callargsNew(boxInt(fallback.call_arity), boxInt(0));
      for (const argBitsValue of argBits) {
        callargsPushPos(builderBits, argBitsValue);
      }
      return callBindIc(boxInt(0), methodBits, builderBits);
    };
  };
  const resolveFallback = (entryName) => {
    const fallback = browserAbi.runtimeImportFallbacks[entryName] || null;
    if (!fallback) {
      return null;
    }
    if (fallback.strategy === 'call_bind_ic') {
      return makeCallBindFallback(entryName, fallback);
    }
    if (fallback.strategy === 'direct_export') {
      const exports = Array.isArray(fallback.exports) ? fallback.exports : [];
      if (exports.length !== 1) {
        throw new Error(`manifest fallback for ${entryName} must name one export`);
      }
      return runtimeExport(exports[0]);
    }
    throw new Error(`unsupported manifest fallback strategy for ${entryName}: ${fallback.strategy}`);
  };
  for (const entry of WebAssembly.Module.imports(appModule)) {
    if (entry.module !== 'molt_runtime') continue;
    if (!manifestNames.has(entry.name)) {
      throw new Error(`app runtime import ${entry.name} missing from manifest`);
    }
    const signature = signatures[entry.name] || null;
    if (!signature || !Array.isArray(signature.params)) {
      throw new Error(`app runtime import ${entry.name} missing manifest signature`);
    }
    const resultKind = resultKinds[entry.name] || null;
    if (!resultKind) {
      throw new Error(`app runtime import ${entry.name} missing manifest result kind`);
    }
    const exportName = entry.name.startsWith('molt_') ? entry.name : `molt_${entry.name}`;
    imports[entry.name] = (...args) => {
      let fn = runtimeExport(exportName);
      let callSignature = runtimeExportSignatures[entry.name] || signature;
      if (!fn) {
        fn = resolveFallback(entry.name);
        callSignature = signature;
      }
      if (typeof fn !== 'function') {
        throw new Error(`molt_runtime missing export ${exportName}`);
      }
      const callArgs = args.map((value, index) =>
        normalizeValueForKind(value, callSignature.params[index] || null));
      return normalizeImportResult(fn(...callArgs), resultKind);
    };
  }
  return imports;
};

const buildMinimalWasi = (state, logFn) => {
  const unsupported = () => WASI_ERRNO_NOSYS;
  const wasi = {
    args_get: () => 0,
    args_sizes_get: (argcPtr, argvBufSizePtr) => {
      writeU32ToMemory(state.memory, argcPtr, 0);
      writeU32ToMemory(state.memory, argvBufSizePtr, 0);
      return 0;
    },
    environ_get: () => 0,
    environ_sizes_get: (countPtr, sizePtr) => {
      writeU32ToMemory(state.memory, countPtr, 0);
      writeU32ToMemory(state.memory, sizePtr, 0);
      return 0;
    },
    fd_write: (fd, iovs, iovsLen, nwritten) => {
      const memory = state.memory;
      if (!memory) return WASI_ERRNO_NOSYS;
      const view = new DataView(memory.buffer);
      let total = 0;
      let text = '';
      for (let i = 0; i < Number(iovsLen); i += 1) {
        const ptr = view.getUint32(Number(iovs) + i * 8, true);
        const len = view.getUint32(Number(iovs) + i * 8 + 4, true);
        total += len;
        text += UTF8_DECODER.decode(new Uint8Array(memory.buffer, ptr, len));
      }
      if (text && typeof logFn === 'function') {
        logFn(Number(fd) === 2 ? 'stderr' : 'stdout', text);
      }
      if (nwritten) {
        view.setUint32(Number(nwritten), total >>> 0, true);
      }
      return 0;
    },
    random_get: (bufPtr, bufLen) => {
      const memory = state.memory;
      if (!memory) return WASI_ERRNO_NOSYS;
      const bytes = new Uint8Array(memory.buffer, Number(bufPtr), Number(bufLen));
      if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
        crypto.getRandomValues(bytes);
      } else {
        for (let i = 0; i < bytes.length; i += 1) {
          bytes[i] = Math.floor(Math.random() * 256);
        }
      }
      return 0;
    },
    clock_time_get: (_clockId, _precision, outPtr) => {
      const memory = state.memory;
      if (!memory) return WASI_ERRNO_NOSYS;
      new DataView(memory.buffer).setBigUint64(
        Number(outPtr),
        BigInt(Date.now()) * 1000000n,
        true,
      );
      return 0;
    },
    proc_exit: (code) => {
      throw new Error(`Molt browser embed proc_exit(${Number(code)})`);
    },
    poll_oneoff: (_inPtr, _outPtr, _nsubscriptions, outEventsPtr) => {
      if (outEventsPtr) writeU32ToMemory(state.memory, outEventsPtr, 0);
      return 0;
    },
    sched_yield: () => 0,
  };
  return new Proxy(wasi, {
    get(target, name) {
      if (name in target) return target[name];
      if (typeof name === 'string') return unsupported;
      return undefined;
    },
  });
};

const buildMinimalEnv = (state, manifest, browserAbi, logFn) => {
  const stubI32 = () => -ENOSYS;
  const stubI64 = () => -BigInt(ENOSYS);
  const stubZero = () => 0;
  const stubZeroI64 = () => 0n;
  const sharedTableBase = manifest?.wasm_table_base ?? null;
  const tableLayout = browserAbi.tableLayout;
  const appTableRefSignatures = manifest?.abi?.table_refs?.app || {};
  const runtimeTableRefSignatures = manifest?.abi?.table_refs?.runtime || {};
  const remapLegacyRuntimeSharedIdx = (idx) => {
    if (sharedTableBase === null || sharedTableBase <= tableLayout.legacyTableBase) {
      return idx;
    }
    if (
      idx >= tableLayout.legacyTableBase + tableLayout.reservedRuntimeCallableBase &&
      idx < tableLayout.legacyTableBase + tableLayout.reservedRuntimeSharedPrefixLen
    ) {
      return idx - tableLayout.legacyTableBase + sharedTableBase;
    }
    return idx;
  };
  const callIndirect = (name) => (fnIndex, ...args) => {
    const idx = Number(fnIndex);
    const dispatchIdx = remapLegacyRuntimeSharedIdx(idx);
    const directName = tableRefExportName(dispatchIdx);
    const tableFn = state.table ? state.table.get(dispatchIdx) : null;
    if (typeof tableFn === 'function') {
      const signature =
        appTableRefSignatures[directName] || runtimeTableRefSignatures[directName] || null;
      try {
        return callWithSignature(tableFn, signature, args);
      } catch (err) {
        const detail = err && typeof err.message === 'string' ? err.message : String(err);
        const fnName = tableFn.name || '<anon>';
        throw new Error(
          `${name} shared-table entry failed at idx=${dispatchIdx}: ${detail}; ` +
            `fnName=${fnName}; fnLen=${tableFn.length}; argsLen=${args.length}`,
        );
      }
    }
    const rtDirectFn = state.runtimeInstance?.exports?.[directName];
    if (typeof rtDirectFn === 'function') {
      try {
        return callWithSignature(rtDirectFn, runtimeTableRefSignatures[directName] || null, args);
      } catch (err) {
        const detail = err && typeof err.message === 'string' ? err.message : String(err);
        throw new Error(
          `${name} runtime direct export ${directName} failed: ${detail}; ` +
            `fnLen=${rtDirectFn.length}; argsLen=${args.length}`,
        );
      }
    }
    const indirectFn = state.appInstance?.exports?.[name];
    if (typeof indirectFn === 'function') {
      try {
        return indirectFn(fnIndex, ...args);
      } catch (err) {
        const detail = err && typeof err.message === 'string' ? err.message : String(err);
        throw new Error(
          `${name} app export failed at idx=${idx}: ${detail}; ` +
            `fnLen=${indirectFn.length}; argsLen=${args.length}`,
        );
      }
    }
    throw new Error(`${name} missing table entry at ${dispatchIdx}`);
  };
  const env = {
    memory: state.memory,
    __indirect_function_table: state.table,
    molt_db_query_host: stubI32,
    molt_db_exec_host: stubI32,
    molt_db_host_poll: stubZero,
    molt_getpid_host: stubZeroI64,
    molt_os_close_host: stubI32,
    molt_socket_new_host: stubI64,
    molt_socket_close_host: stubI32,
    molt_socket_clone_host: stubI64,
    molt_socket_bind_host: stubI32,
    molt_socket_listen_host: stubI32,
    molt_socket_accept_host: stubI64,
    molt_socket_connect_host: stubI32,
    molt_socket_connect_ex_host: stubI32,
    molt_socket_recv_host: stubI32,
    molt_socket_send_host: stubI32,
    molt_socket_sendto_host: stubI32,
    molt_socket_sendmsg_host: stubI32,
    molt_socket_recvfrom_host: stubI32,
    molt_socket_recvmsg_host: stubI32,
    molt_socket_shutdown_host: stubI32,
    molt_socket_getsockname_host: stubI32,
    molt_socket_getpeername_host: stubI32,
    molt_socket_setsockopt_host: stubI32,
    molt_socket_getsockopt_host: stubI32,
    molt_socket_detach_host: stubI64,
    molt_socket_socketpair_host: stubI32,
    molt_socket_getaddrinfo_host: stubI32,
    molt_socket_gethostname_host: stubI32,
    molt_socket_getservbyname_host: stubI32,
    molt_socket_getservbyport_host: stubI32,
    molt_socket_poll_host: stubI32,
    molt_socket_wait_host: stubI32,
    molt_socket_has_ipv6_host: stubZero,
    molt_ws_connect_host: stubI32,
    molt_ws_poll_host: stubI32,
    molt_ws_send_host: stubI32,
    molt_ws_recv_host: stubI32,
    molt_ws_close_host: stubI32,
    molt_process_spawn_host: stubI32,
    molt_process_wait_host: stubI32,
    molt_process_kill_host: stubI32,
    molt_process_terminate_host: stubI32,
    molt_process_write_host: stubI32,
    molt_process_close_stdin_host: stubI32,
    molt_process_stdio_host: stubI32,
    molt_process_host_poll: stubZero,
    molt_gpu_webgpu_dispatch_host: stubI32,
    molt_time_timezone_host: stubZeroI64,
    molt_time_local_offset_host: stubZeroI64,
    molt_time_tzname_host: (_which, _bufPtr, _bufCap, outLenPtr) => {
      if (outLenPtr) writeU32ToMemory(state.memory, outLenPtr, 0);
      return 0;
    },
    molt_vfs_read: stubI32,
    molt_vfs_write: stubI32,
    molt_vfs_exists: stubZero,
    molt_vfs_unlink: stubI32,
    molt_log_host: (level, ptr, len) => {
      const memory = state.memory;
      if (!memory || typeof logFn !== 'function') return;
      const msg = UTF8_DECODER.decode(new Uint8Array(memory.buffer, Number(ptr), Number(len)));
      logFn(level, msg);
    },
    molt_isolate_import: (...args) => {
      const fn = state.appInstance?.exports?.molt_isolate_import;
      if (typeof fn !== 'function') {
        throw new Error('molt_isolate_import called before app instantiation');
      }
      return callIsolateImportExport(fn, args);
    },
  };
  return new Proxy(env, {
    get(target, name) {
      if (name in target) return target[name];
      if (typeof name === 'string' && browserAbi.callIndirectImports.has(name)) {
        const fn = callIndirect(name);
        target[name] = fn;
        return fn;
      }
      if (typeof name === 'string' && name.startsWith('molt_call_indirect')) {
        throw new Error(`call_indirect import ${name} is not declared by manifest`);
      }
      if (typeof name === 'string' && name.endsWith('_host')) {
        return stubI32;
      }
      return undefined;
    },
  });
};

const resolveExportName = (exports, requested) => {
  if (typeof exports[requested] === 'function') {
    return requested;
  }
  const suffix = `__${requested}`;
  const matches = Object.keys(exports).filter(
    (name) => name.endsWith(suffix) && typeof exports[name] === 'function',
  );
  if (matches.length === 1) {
    return matches[0];
  }
  throw new Error(`app export missing or ambiguous: ${requested}`);
};

const typedArrayFromBytes = (bytes, Constructor, label) => {
  if (bytes.byteLength % Constructor.BYTES_PER_ELEMENT !== 0) {
    throw new Error(
      `${label} result byte length ${bytes.byteLength} is not divisible by ${Constructor.BYTES_PER_ELEMENT}`,
    );
  }
  const copy = copyBytes(bytes);
  return new Constructor(copy.buffer);
};

export const decodeMoltBrowserKernelBytes = (bytes, resultType = 'float32') => {
  const normalized = String(resultType || 'float32').toLowerCase().replace(/[-_]/g, '');
  switch (normalized) {
    case 'bytes':
    case 'uint8':
    case 'uint8array':
      return bytes;
    case 'float32':
    case 'float32array':
    case 'f32':
      return typedArrayFromBytes(bytes, Float32Array, 'float32');
    case 'float64':
    case 'float64array':
    case 'f64':
      return typedArrayFromBytes(bytes, Float64Array, 'float64');
    case 'int32':
    case 'int32array':
      return typedArrayFromBytes(bytes, Int32Array, 'int32');
    case 'uint32':
    case 'uint32array':
      return typedArrayFromBytes(bytes, Uint32Array, 'uint32');
    default:
      throw new Error(`unsupported Molt browser kernel resultType: ${resultType}`);
  }
};

export const loadMoltBrowserEmbed = async (options = {}) => {
  const moduleUrl = options.moduleUrl || import.meta.url;
  const baseForManifest = options.baseUrl || moduleUrl;
  const manifestUrl = options.manifestUrl || resolveUrl('manifest.json', baseForManifest);
  const manifest = options.manifest || await fetchJson(manifestUrl);
  const baseUrl = options.baseUrl || resolveUrl('.', manifestUrl);
  const appPath = manifest?.modules?.app?.path || 'app.wasm';
  const runtimePath = manifest?.modules?.runtime?.path || 'molt_runtime.wasm';
  const appUrl = options.appUrl || options.wasmUrl || resolveUrl(appPath, baseUrl);
  const runtimeUrl = options.runtimeUrl || resolveUrl(runtimePath, baseUrl);
  const browserAbi = browserAbiFromManifest(manifest);
  const [appBytes, runtimeBytes] = await Promise.all([
    fetchArrayBuffer(appUrl),
    fetchArrayBuffer(runtimeUrl),
  ]);
  const appImports = parseMoltWasmImports(appBytes);
  const runtimeImports = parseMoltWasmImports(runtimeBytes);
  const memoryLimits = mergeLimits(appImports.memory, runtimeImports.memory, 'memory');
  const tableLimits = mergeLimits(appImports.table, runtimeImports.table, 'table');
  const memory = makeMemory(memoryLimits, manifest.shared_memory_initial_pages);
  const table = makeTable(tableLimits, manifest.shared_table_initial);
  const state = {
    appInstance: null,
    runtimeInstance: null,
    memory,
    table,
  };
  const env = buildMinimalEnv(state, manifest, browserAbi, options.log || null);
  const wasi = buildMinimalWasi(state, options.log || null);
  const runtimeModule = await WebAssembly.compile(runtimeBytes);
  const runtimeInstance = await WebAssembly.instantiate(runtimeModule, {
    env,
    wasi_snapshot_preview1: wasi,
  });
  state.runtimeInstance = runtimeInstance;
  if (manifest.wasm_table_base !== null && manifest.wasm_table_base !== undefined) {
    const setTableBase = runtimeInstance.exports.molt_set_wasm_table_base;
    if (typeof setTableBase === 'function') {
      setTableBase(BigInt(manifest.wasm_table_base));
    }
  }
  installTableRefs(runtimeInstance, table);
  const appModule = await WebAssembly.compile(appBytes);
  const moltNative = createMoltNativeCallableImports(state, appImports, {
    ...options,
    manifest,
    nativeCallableManifest: browserAbi.nativeCallables,
    requireNativeCallableManifest: true,
  });
  const appInstance = await WebAssembly.instantiate(appModule, {
    env,
    molt_native: moltNative,
    wasi_snapshot_preview1: wasi,
    molt_runtime: buildRuntimeImports(appModule, runtimeInstance, manifest, browserAbi),
  });
  state.appInstance = appInstance;
  ensureTableCapacityForExportedRefs(appInstance, table);
  return {
    appInstance,
    runtimeInstance,
    memory,
    table,
    manifest,
  };
};

export const loadMoltBrowserKernel = async (options = {}) => {
  const embed = await loadMoltBrowserEmbed(options);
  const exportName = options.exportName || options.functionName || 'forward';
  const resolvedExportName = resolveExportName(embed.appInstance.exports, exportName);
  const resultType = options.resultType || options.outputType || 'float32';
  let initialized = false;
  const ensureInitialized = () => {
    if (initialized) {
      return;
    }
    const hostInit = embed.appInstance.exports.molt_host_init;
    const isolateBootstrap = embed.appInstance.exports.molt_isolate_bootstrap;
    if (typeof hostInit === 'function') {
      hostInit();
    } else if (typeof isolateBootstrap === 'function') {
      isolateBootstrap();
    }
    const pending = pendingRuntimeExceptionMessage(embed.runtimeInstance, embed.memory);
    if (pending) {
      throw new Error(pending);
    }
    initialized = true;
  };
  const callBytes = (...args) => {
    ensureInitialized();
    const fn = embed.appInstance.exports[resolvedExportName];
    const argBits = args.map((arg) => makeHostArg(embed.runtimeInstance, embed.memory, arg));
    let resultBits = 0n;
    try {
      resultBits = fn(...argBits);
    } finally {
      for (const bits of argBits) {
        decRefMaybe(embed.runtimeInstance, bits);
      }
    }
    const pending = pendingRuntimeExceptionMessage(embed.runtimeInstance, embed.memory);
    if (pending) {
      decRefMaybe(embed.runtimeInstance, resultBits);
      throw new Error(pending);
    }
    try {
      return readBytesObject(embed.runtimeInstance, embed.memory, resultBits);
    } finally {
      decRefMaybe(embed.runtimeInstance, resultBits);
    }
  };
  const forward = (input, ...extraArgs) => {
    const resultBytes = callBytes(bytesFromBufferSource(input), ...extraArgs);
    return decodeMoltBrowserKernelBytes(resultBytes, resultType);
  };
  return {
    ...embed,
    exportName: resolvedExportName,
    callBytes,
    forward,
  };
};
