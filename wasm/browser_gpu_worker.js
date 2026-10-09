const GPU_BUFFER_USAGE = globalThis.GPUBufferUsage;
const GPU_MAP_MODE = globalThis.GPUMapMode;

if (!GPU_BUFFER_USAGE || !GPU_MAP_MODE) {
  throw new Error('WebGPU globals are unavailable in browser_gpu_worker.js');
}

let devicePromise = null;
let deviceFailure = null;
const pipelineCache = new Map();

const ensureDevice = async () => {
  if (!devicePromise) {
    devicePromise = (async () => {
      if (!globalThis.navigator || !globalThis.navigator.gpu) {
        throw new Error('navigator.gpu is unavailable in the browser WebGPU worker');
      }
      const adapter = await globalThis.navigator.gpu.requestAdapter();
      if (!adapter) {
        throw new Error('WebGPU adapter is unavailable');
      }
      const device = await adapter.requestDevice();
      device.lost.then((info) => {
        deviceFailure ??= new Error(`WebGPU device lost (${info.reason}): ${info.message}`);
        return { error: deviceFailure };
      });
      device.addEventListener('uncapturederror', (event) => {
        deviceFailure ??= new Error(`uncaptured WebGPU error: ${event.error.message}`);
      });
      return device;
    })();
  }
  return devicePromise;
};

const ensurePipeline = async (device, entry, source) => {
  const key = `${entry}\0${source}`;
  if (!pipelineCache.has(key)) {
    pipelineCache.set(
      key,
      Promise.resolve().then(() => {
        const shader = device.createShaderModule({ code: source });
        return device.createComputePipelineAsync({
          layout: 'auto',
          compute: {
            module: shader,
            entryPoint: entry,
          },
        });
      }),
    );
  }
  return pipelineCache.get(key);
};

const normalizeBytes = (bytes) =>
  bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes || []);

const dispatchKernel = async (request) => {
  const device = await ensureDevice();
  if (deviceFailure) throw deviceFailure;
  if (!Number.isSafeInteger(request.grid) || request.grid < 0 || request.grid > device.limits.maxComputeWorkgroupsPerDimension) {
    throw new Error('WebGPU grid exceeds device dispatch capability');
  }
  if (!Number.isSafeInteger(request.workgroupSize) || request.workgroupSize <= 0 ||
      request.workgroupSize > device.limits.maxComputeWorkgroupSizeX ||
      request.workgroupSize > device.limits.maxComputeInvocationsPerWorkgroup) {
    throw new Error('WebGPU workgroup exceeds device capability');
  }
  const allocations = [];
  const readbacks = [];
  const run = async () => {
    const pipeline = await ensurePipeline(device, request.entry, request.source);
    const bindings = request.bindings.map((binding) => {
      const bytes = normalizeBytes(binding.bytes);
      const size = Math.max(bytes.byteLength, 1);
      const buffer = device.createBuffer({
        size,
        usage:
          GPU_BUFFER_USAGE.STORAGE |
          GPU_BUFFER_USAGE.COPY_DST |
          GPU_BUFFER_USAGE.COPY_SRC,
      });
      allocations.push(buffer);
      if (bytes.byteLength > 0) {
        device.queue.writeBuffer(buffer, 0, bytes);
      }
      return { binding, buffer, size };
    });

    const bindGroup = bindings.length === 0 ? null : device.createBindGroup({
      layout: pipeline.getBindGroupLayout(0),
      entries: bindings.map((entry) => ({
        binding: entry.binding.binding,
        resource: { buffer: entry.buffer },
      })),
    });

    const encoder = device.createCommandEncoder();
    const pass = encoder.beginComputePass();
    pass.setPipeline(pipeline);
    if (bindGroup) pass.setBindGroup(0, bindGroup);
    pass.dispatchWorkgroups(Number(request.grid) || 0);
    pass.end();

    for (const entry of bindings) {
      if (entry.binding.access !== 'read_write') {
        continue;
      }
      const readback = device.createBuffer({
        size: entry.size,
        usage: GPU_BUFFER_USAGE.COPY_DST | GPU_BUFFER_USAGE.MAP_READ,
      });
      allocations.push(readback);
      encoder.copyBufferToBuffer(entry.buffer, 0, readback, 0, entry.size);
      readbacks.push({ binding: entry.binding.binding, readback, size: entry.size });
    }

    device.queue.submit([encoder.finish()]);
    // Queue completion is required even when there is no writable output.
    await device.queue.onSubmittedWorkDone();
    const mapped = await Promise.allSettled(readbacks.map((entry) => entry.readback.mapAsync(GPU_MAP_MODE.READ)));
    const mapFailure = mapped.find((result) => result.status === 'rejected');
    if (mapFailure) throw mapFailure.reason;

    const outputs = readbacks.map((entry) => {
      const range = new Uint8Array(entry.readback.getMappedRange());
      const bytes = new Uint8Array(range.slice());
      entry.readback.unmap();
      return { binding: entry.binding, bytes };
    });
    return { outputs };
  };
  // One invocation per worker: the owning synchronous dispatcher cannot post a
  // second request before this completion. Scopes cover all device work, including
  // upload/pipeline/readback, and are always consumed before success publication.
  const filters = ['internal', 'out-of-memory', 'validation'];
  let pushed = 0;
  try {
    for (const filter of filters) { device.pushErrorScope(filter); pushed += 1; }
    // WebGPU pipeline, queue and mapping promises settle on device loss. Finish
    // the operation before destroying resources; do not race cleanup against it.
    const outcome = await run().then(value => ({ value }), error => ({ error }));
    let scopedError = null;
    while (pushed > 0) {
      pushed -= 1;
      try { const error = await device.popErrorScope(); scopedError ??= error; }
      catch (error) { scopedError ??= error; }
    }
    if (deviceFailure) throw deviceFailure;
    if (scopedError) throw new Error(`WebGPU execution failed: ${scopedError.message}`);
    if (outcome.error) throw outcome.error;
    return outcome.value;
  } finally {
    while (pushed > 0) { pushed -= 1; await device.popErrorScope().catch(() => {}); }
    for (const buffer of allocations) buffer.destroy();
  }
};

globalThis.addEventListener('message', async (event) => {
  const payload = event && event.data ? event.data : null;
  if (!payload || payload.type !== 'dispatch' || typeof payload.id !== 'number') {
    return;
  }
  const waiter = payload.waiter;
  const errorBytes = payload.errorBytes;
  if (!(waiter instanceof Int32Array) || waiter.length !== 2 ||
      !(waiter.buffer instanceof SharedArrayBuffer) ||
      !(errorBytes instanceof Uint8Array) || !(errorBytes.buffer instanceof SharedArrayBuffer)) {
    return;
  }
  try {
    const result = await dispatchKernel(payload.request);
    const expected = new Map(payload.request.bindings
      .filter((binding) => binding.access === 'read_write')
      .map((binding) => [binding.binding, binding]));
    const seen = new Set();
    for (const output of result.outputs) {
      const binding = expected.get(output.binding);
      if (!binding || seen.has(output.binding) || output.bytes.byteLength !== binding.bytes.byteLength) {
        throw new Error('browser webgpu readback differs from physical dispatch plan');
      }
      seen.add(output.binding);
    }
    if (seen.size !== expected.size) throw new Error('browser webgpu readback omitted a physical binding');
    for (const output of result.outputs) expected.get(output.binding).bytes.set(output.bytes);
    Atomics.store(waiter, 0, 1);
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    const encoded = new TextEncoder().encode(detail).subarray(0, errorBytes.length);
    errorBytes.set(encoded);
    Atomics.store(waiter, 1, encoded.length);
    Atomics.store(waiter, 0, 2);
  } finally {
    Atomics.notify(waiter, 0, 1);
  }
});
