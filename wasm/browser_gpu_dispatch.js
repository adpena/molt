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
  const closedWorkers = new WeakSet();
  let nextId = 1;
  const pending = new Map();
  let disposed = false;

  const failPending = (message) => {
    for (const entry of pending.values()) {
      entry.error = message;
      Atomics.store(entry.waiter, 0, 1);
      Atomics.notify(entry.waiter, 0, 1);
    }
    pending.clear();
  };

  const closeWorker = (target) => {
    if (!target) return;
    if (worker === target) worker = null;
    if (closedWorkers.has(target)) return;
    closedWorkers.add(target);
    if (typeof target.terminate === 'function') target.terminate();
  };

  const ensureWorker = () => {
    if (disposed) {
      throw new Error('browser webgpu dispatcher has been disposed');
    }
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
    const activeWorker = new Worker(new URL('./browser_gpu_worker.js', import.meta.url), {
      type: 'module',
    });
    worker = activeWorker;
    activeWorker.addEventListener('error', (event) => {
      const detail =
        event && event.message ? `browser webgpu worker error: ${event.message}` : 'browser webgpu worker failed';
      if (worker === activeWorker) failPending(detail);
      closeWorker(activeWorker);
    });
    return activeWorker;
  };

  return {
    dispatchKernel(request) {
      const activeWorker = ensureWorker();
      const id = nextId;
      nextId += 1;
      // The worker, not this blocked event loop, completes the synchronous import.
      // Shared readbacks are private staging; no late worker can mutate WASM
      // memory after timeout or a failed dispatch.
      const waiter = new Int32Array(new SharedArrayBuffer(8));
      const errorBytes = new Uint8Array(new SharedArrayBuffer(4096));
      const bindings = request.bindings.map((binding) => {
        const bytes = new Uint8Array(new SharedArrayBuffer(binding.bytes.byteLength));
        bytes.set(binding.bytes);
        return { ...binding, bytes };
      });
      const entry = { waiter, error: null };
      pending.set(id, entry);
      activeWorker.postMessage({
        type: 'dispatch',
        id,
        waiter,
        errorBytes,
        request: {
          source: request.source,
          entry: request.entry,
          grid: request.grid,
          workgroupSize: request.workgroupSize,
          bindings,
        },
      });
      const res = Atomics.wait(waiter, 0, 0, dispatchTimeoutMs);
      if (res === 'timed-out') {
        pending.delete(id);
        closeWorker(activeWorker);
        throw new Error('browser webgpu dispatch timed out');
      }
      pending.delete(id);
      if (entry.error) throw new Error(entry.error);
      if (Atomics.load(waiter, 0) !== 1) {
        const length = Atomics.load(waiter, 1);
        throw new Error(UTF8_DECODER.decode(errorBytes.subarray(0, length)) || 'browser webgpu worker failed');
      }
      // Every physical output, including store-occurrence flags, has the
      // original fixed extent. Publish only after the complete dispatch succeeds.
      for (let index = 0; index < bindings.length; index += 1) {
        if (bindings[index].access === 'read_write') {
          request.bindings[index].bytes.set(bindings[index].bytes);
        }
      }
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      failPending('browser webgpu dispatcher has been disposed');
      const activeWorker = worker;
      closeWorker(activeWorker);
    },
  };
};

export const createBrowserGpuHost = (state, options) => {
  const opts = options && typeof options === 'object' ? options : {};
  const ownsDispatcher = !opts.gpuKernelDispatcher;
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

  const dispose = () => {
    if (ownsDispatcher && typeof dispatcher.dispose === 'function') dispatcher.dispose();
  };
  return { gpuWebGpuDispatchHost, dispose };
};
