// Developer-only physical adapter control. Serve the repository under the
// existing cross-origin-isolated browser host and invoke this exported function.
// It uses the production worker, never a replacement dispatch implementation.
export async function runBrowserGpuCompletionControls() {
  if (!globalThis.crossOriginIsolated || !globalThis.navigator?.gpu ||
      typeof SharedArrayBuffer === 'undefined' || typeof Atomics.waitAsync !== 'function') {
    throw new Error('physical browser control requires WebGPU and isolated shared-memory worker capability');
  }
  const adapter = await navigator.gpu.requestAdapter();
  if (!adapter) throw new Error('physical browser control requires an admitted adapter');
  const probe = await adapter.requestDevice();
  const overLimitGrid = probe.limits.maxComputeWorkgroupsPerDimension + 1;
  probe.destroy();
  const worker = new Worker(new URL('../../wasm/browser_gpu_worker.js', import.meta.url), {type: 'module'});
  const normal = '@group(0) @binding(0) var<storage, read_write> a: array<i32>; @group(0) @binding(1) var<storage, read_write> b: array<i32>; @compute @workgroup_size(1) fn main() { a[0] = 42; b[0] = 17; }';
  // Pipeline creation is valid. The actual supplied four-byte binding fails
  // the eight-byte Pair layout in createBindGroup without a JS exception.
  const invalidBinding = 'struct Pair { x: i32, y: i32 }; @group(0) @binding(0) var<storage, read_write> a: Pair; @group(0) @binding(1) var<storage, read_write> b: array<i32>; @compute @workgroup_size(1) fn main() { a.y = 42; b[0] = 17; }';
  const cases = [
    {name: 'success', source: normal, grid: 1, bindings: 2, status: 1, values: [42, 17]},
    {name: 'nonthrowing-validation', source: invalidBinding, grid: 1, bindings: 2, status: 2, values: [9, 8]},
    {name: 'device-dispatch-limit', source: normal, grid: overLimitGrid, bindings: 2, status: 2, values: [9, 8]},
    {name: 'no-readbacks', source: '@compute @workgroup_size(1) fn main() {}', grid: 1, bindings: 0, status: 1, values: []},
  ];
  const observed = [];
  try {
    for (const [id, control] of cases.entries()) {
      const waiter = new Int32Array(new SharedArrayBuffer(8));
      const errorBytes = new Uint8Array(new SharedArrayBuffer(4096));
      const bindings = Array.from({length: control.bindings}, (_, binding) => {
        const bytes = new Uint8Array(new SharedArrayBuffer(4));
        new DataView(bytes.buffer).setInt32(0, binding === 0 ? 9 : 8, true);
        return {binding, access: 'read_write', bytes};
      });
      worker.postMessage({type: 'dispatch', id, waiter, errorBytes,
        request: {source: control.source, entry: 'main', grid: control.grid, workgroupSize: 1, bindings}});
      const completion = await Atomics.waitAsync(waiter, 0, 0, 15000).value;
      if (completion === 'timed-out') throw new Error(`physical browser ${control.name} did not complete`);
      const status = Atomics.load(waiter, 0);
      const values = bindings.map(binding => new DataView(binding.bytes.buffer).getInt32(0, true));
      const detail = new TextDecoder().decode(errorBytes.subarray(0, Atomics.load(waiter, 1)));
      if (status !== control.status || JSON.stringify(values) !== JSON.stringify(control.values)) {
        throw new Error(`physical browser ${control.name}: ${JSON.stringify({status, values, detail})}`);
      }
      if (status === 2 && !detail) throw new Error('device refusal lost its diagnostic');
      observed.push({name: control.name, status, values, detail});
    }
    return observed;
  } finally {
    worker.terminate();
  }
}
