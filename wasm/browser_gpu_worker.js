const GPU_BUFFER_USAGE = globalThis.GPUBufferUsage;
const GPU_MAP_MODE = globalThis.GPUMapMode;

if (!GPU_BUFFER_USAGE || !GPU_MAP_MODE) {
  throw new Error('WebGPU globals are unavailable in browser_gpu_worker.js');
}

let devicePromise = null;
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
      return adapter.requestDevice();
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
        return device.createComputePipeline({
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
    if (bytes.byteLength > 0) {
      device.queue.writeBuffer(buffer, 0, bytes);
    }
    return { binding, buffer, size };
  });

  const bindGroup = device.createBindGroup({
    layout: pipeline.getBindGroupLayout(0),
    entries: bindings.map((entry) => ({
      binding: entry.binding.binding,
      resource: { buffer: entry.buffer },
    })),
  });

  const encoder = device.createCommandEncoder();
  const pass = encoder.beginComputePass();
  pass.setPipeline(pipeline);
  pass.setBindGroup(0, bindGroup);
  pass.dispatchWorkgroups(Number(request.grid) || 0);
  pass.end();

  const readbacks = [];
  for (const entry of bindings) {
    if (entry.binding.access !== 'read_write') {
      continue;
    }
    const readback = device.createBuffer({
      size: entry.size,
      usage: GPU_BUFFER_USAGE.COPY_DST | GPU_BUFFER_USAGE.MAP_READ,
    });
    encoder.copyBufferToBuffer(entry.buffer, 0, readback, 0, entry.size);
    readbacks.push({ binding: entry.binding.binding, readback, size: entry.size });
  }

  device.queue.submit([encoder.finish()]);
  await Promise.all(readbacks.map((entry) => entry.readback.mapAsync(GPU_MAP_MODE.READ)));

  const outputs = readbacks.map((entry) => {
    const range = new Uint8Array(entry.readback.getMappedRange());
    const bytes = new Uint8Array(range.slice());
    entry.readback.unmap();
    return { binding: entry.binding, bytes };
  });
  return { outputs };
};

globalThis.addEventListener('message', async (event) => {
  const payload = event && event.data ? event.data : null;
  if (!payload || payload.type !== 'dispatch' || typeof payload.id !== 'number') {
    return;
  }
  try {
    const result = await dispatchKernel(payload.request);
    globalThis.postMessage({ id: payload.id, outputs: result.outputs });
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    globalThis.postMessage({ id: payload.id, error: detail, outputs: [] });
  }
});
