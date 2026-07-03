const ENOSYS = 38;
const EINVAL = 22;
const ETIMEDOUT = 110;
const UTF8_DECODER = new TextDecoder('utf-8');
const UTF8_ENCODER = new TextEncoder();

const readBytesFromMemory = (memory, ptr, len) => {
  if (!memory) return new Uint8Array(0);
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  const size = typeof len === 'bigint' ? Number(len) : Number(len >>> 0);
  if (!Number.isFinite(addr) || addr === 0 || size <= 0) return new Uint8Array(0);
  return new Uint8Array(memory.buffer, addr, size);
};

const readStringFromMemory = (memory, ptr, len) => {
  const bytes = readBytesFromMemory(memory, ptr, len);
  if (!bytes.length) return '';
  return UTF8_DECODER.decode(bytes);
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

const createWorkerWebGpuDispatcher = (options = {}) => {
  const canBlock =
    typeof SharedArrayBuffer !== 'undefined' &&
    typeof Atomics !== 'undefined' &&
    typeof Atomics.wait === 'function' &&
    typeof Worker === 'function' &&
    typeof document === 'undefined';
  const timeoutRaw =
    options.gpuKernelTimeoutMs ??
    (typeof process !== 'undefined' ? process?.env?.MOLT_GPU_KERNEL_TIMEOUT_MS : undefined);
  const dispatchTimeoutMs = Number.isFinite(Number(timeoutRaw))
    ? Math.max(1, Number(timeoutRaw))
    : 15000;
  let worker = null;
  let nextId = 1;
  const pending = new Map();

  const failPending = (message) => {
    for (const entry of pending.values()) {
      entry.error = message;
      Atomics.store(entry.waiter, 0, 1);
      Atomics.notify(entry.waiter, 0, 1);
    }
    pending.clear();
  };

  const ensureWorker = () => {
    if (worker) {
      return worker;
    }
    if (typeof globalThis.navigator !== 'undefined' && !globalThis.navigator?.gpu) {
      throw new Error('navigator.gpu is unavailable in the browser WebGPU host');
    }
    if (!canBlock) {
      throw new Error(
        'browser webgpu dispatcher requires a worker-like host with SharedArrayBuffer and Atomics.wait'
      );
    }
    worker = new Worker(new URL('./browser_gpu_worker.js', import.meta.url), {
      type: 'module',
    });
    worker.addEventListener('message', (event) => {
      const payload = event && event.data ? event.data : null;
      if (!payload || typeof payload.id !== 'number') {
        return;
      }
      const entry = pending.get(payload.id);
      if (!entry) {
        return;
      }
      pending.delete(payload.id);
      entry.error = payload.error || null;
      entry.outputs = Array.isArray(payload.outputs)
        ? payload.outputs.map((binding) => ({
            binding: binding.binding,
            bytes:
              binding.bytes instanceof Uint8Array
                ? binding.bytes
                : new Uint8Array(binding.bytes || []),
          }))
        : [];
      Atomics.store(entry.waiter, 0, 1);
      Atomics.notify(entry.waiter, 0, 1);
    });
    worker.addEventListener('error', (event) => {
      const detail =
        event && event.message ? `browser webgpu worker error: ${event.message}` : 'browser webgpu worker failed';
      failPending(detail);
      worker = null;
    });
    return worker;
  };

  return {
    dispatchKernel(request) {
      const activeWorker = ensureWorker();
      const id = nextId;
      nextId += 1;
      const waiter = new Int32Array(new SharedArrayBuffer(4));
      const entry = { waiter, error: null, outputs: [] };
      pending.set(id, entry);
      activeWorker.postMessage({
        type: 'dispatch',
        id,
        request: {
          source: request.source,
          entry: request.entry,
          grid: request.grid,
          workgroupSize: request.workgroupSize,
          bindings: request.bindings.map((binding) => ({
            binding: binding.binding,
            name: binding.name,
            kind: binding.kind,
            access: binding.access,
            bytes: binding.bytes.slice(),
          })),
        },
      });
      const res = Atomics.wait(waiter, 0, 0, dispatchTimeoutMs);
      if (res === 'timed-out') {
        pending.delete(id);
        throw new Error('browser webgpu dispatch timed out');
      }
      if (entry.error) {
        throw new Error(entry.error);
      }
      for (const output of entry.outputs) {
        const binding = request.bindings.find((candidate) => candidate.binding === output.binding);
        if (!binding) {
          continue;
        }
        binding.bytes.set(output.bytes.subarray(0, binding.bytes.length));
      }
    },
  };
};

export const createBrowserGpuHost = (state, options) => {
  const opts = options && typeof options === 'object' ? options : {};
  const dispatcher = (() => {
    if (opts.gpuKernelDispatcher && typeof opts.gpuKernelDispatcher.dispatchKernel === 'function') {
      return opts.gpuKernelDispatcher;
    }
    if (typeof opts.gpuKernelDispatcher === 'function') {
      return { dispatchKernel: opts.gpuKernelDispatcher };
    }
    return createWorkerWebGpuDispatcher(opts);
  })();

  const activeMemory = () => {
    if (state && typeof state.memoryProvider === 'function') {
      return state.memoryProvider();
    }
    return state?.appMemory || state?.memory || null;
  };

  const writeHostError = (memory, errPtr, errCap, outErrLenPtr, code, message) => {
    const text = typeof message === 'string' ? message : String(message || '');
    if (memory && outErrLenPtr) {
      writeU32ToMemory(memory, outErrLenPtr, UTF8_ENCODER.encode(text).length);
    }
    if (memory && errPtr && errCap) {
      const encoded = UTF8_ENCODER.encode(text);
      const cap = Math.max(0, Number(errCap));
      const slice = cap > 0 ? encoded.subarray(0, Math.max(0, cap - 1)) : new Uint8Array(0);
      writeBytesToMemory(memory, errPtr, slice);
      if (cap > slice.length) {
        writeBytesToMemory(memory, Number(errPtr) + slice.length, new Uint8Array([0]));
      }
    }
    if (Number(code) === 0) {
      return 0;
    }
    return -Math.abs(Number(code) || EINVAL);
  };

  const classifyDispatchError = (message) => {
    if (typeof message !== 'string' || message.length === 0) {
      return EINVAL;
    }
    if (message.includes('timed out')) {
      return ETIMEDOUT;
    }
    if (message.includes('requires a worker-like host') || message.includes('unavailable')) {
      return ENOSYS;
    }
    return EINVAL;
  };

  const gpuWebGpuDispatchHost = (
    sourcePtr,
    sourceLen,
    entryPtr,
    entryLen,
    bindingsPtr,
    bindingsLen,
    gridRaw,
    workgroupSizeRaw,
    errPtr,
    errCap,
    outErrLenPtr,
  ) => {
    const memory = activeMemory();
    if (!memory) {
      return writeHostError(memory, errPtr, errCap, outErrLenPtr, ENOSYS, 'runtime memory not initialized');
    }
    let launchRecord;
    try {
      const launchText = readStringFromMemory(memory, bindingsPtr, bindingsLen);
      launchRecord = JSON.parse(launchText);
    } catch (err) {
      return writeHostError(
        memory,
        errPtr,
        errCap,
        outErrLenPtr,
        EINVAL,
        'invalid browser webgpu launch record',
      );
    }
    if (!launchRecord || !Array.isArray(launchRecord.bindings)) {
      return writeHostError(
        memory,
        errPtr,
        errCap,
        outErrLenPtr,
        EINVAL,
        'browser webgpu launch record missing bindings',
      );
    }
    const request = {
      source: readStringFromMemory(memory, sourcePtr, sourceLen),
      entry: readStringFromMemory(memory, entryPtr, entryLen),
      grid: typeof gridRaw === 'bigint' ? Number(gridRaw) : Number(gridRaw),
      workgroupSize:
        typeof workgroupSizeRaw === 'bigint' ? Number(workgroupSizeRaw) : Number(workgroupSizeRaw),
      bindings: launchRecord.bindings.map((binding) => ({
        binding: Number(binding.binding),
        name: String(binding.name || ''),
        kind: String(binding.kind || 'buffer'),
        access: String(binding.access || 'read'),
        bytes: readBytesFromMemory(memory, Number(binding.ptr) >>> 0, Number(binding.len) >>> 0),
      })),
    };
    try {
      const result = dispatcher.dispatchKernel(request);
      if (result && typeof result.then === 'function') {
        throw new Error(
          'browser webgpu dispatcher must complete synchronously from the wasm host import',
        );
      }
      return writeHostError(memory, errPtr, errCap, outErrLenPtr, 0, '');
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      return writeHostError(
        memory,
        errPtr,
        errCap,
        outErrLenPtr,
        classifyDispatchError(detail),
        detail,
      );
    }
  };

  return { gpuWebGpuDispatchHost };
};
