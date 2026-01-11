const fs = require('fs');
const path = require('path');
const { WASI } = require('wasi');

const wasmBuffer = fs.readFileSync('output.wasm');
const runtimePath = process.env.MOLT_RUNTIME_WASM || path.join(__dirname, 'wasm', 'molt_runtime.wasm');
const runtimeBuffer = fs.readFileSync(runtimePath);
const witPath = path.join(__dirname, 'wit', 'molt-runtime.wit');
const witSource = fs.readFileSync(witPath, 'utf8');

const wasi = new WASI({
  version: 'preview1',
  env: process.env,
  preopens: {
    '.': '.',
  },
});

let runtimeInstance = null;
const runtimeImports = {};
const funcSigs = new Map();
const traceImports = process.env.MOLT_WASM_TRACE === '1';

for (const line of witSource.split('\n')) {
  const match = line.match(/^\s*([A-Za-z0-9_]+):\s*func\(([^)]*)\)\s*(?:->\s*([^;]+))?/);
  if (!match) {
    continue;
  }
  const name = match[1];
  const rawArgs = match[2].trim();
  const argTypes = rawArgs
    ? rawArgs.split(',').map(part => part.split(':')[1].trim())
    : [];
  const retType = match[3] ? match[3].trim() : null;
  funcSigs.set(name, { argTypes, retType });
}

const expectsBigInt = (ty) => ty === 'molt-object' || ty === 'molt-ptr' || ty === 'u64' || ty === 's64';
const toBigInt = (value, ty) => {
  if (typeof value === 'bigint') {
    return value;
  }
  if (typeof value !== 'number' || !Number.isFinite(value) || !Number.isInteger(value)) {
    throw new TypeError(`Expected integer for ${ty}, got ${value}`);
  }
  return BigInt.asUintN(64, BigInt(value));
};
const normalizeArg = (arg, ty) => {
  if (!ty) {
    return arg;
  }
  if (expectsBigInt(ty)) {
    return toBigInt(arg, ty);
  }
  if (typeof arg === 'bigint') {
    return Number(arg);
  }
  return arg;
};
const normalizeReturn = (value, ty) => {
  if (!ty) {
    return value;
  }
  if (expectsBigInt(ty)) {
    return toBigInt(value, ty);
  }
  if (typeof value === 'bigint') {
    return Number(value);
  }
  return value;
};

for (const [name, sig] of funcSigs) {
  runtimeImports[name] = (...args) => {
    if (!runtimeInstance) {
      throw new Error('molt_runtime not initialized');
    }
    const exportName = `molt_${name}`;
    const fn = runtimeInstance.exports[exportName];
    if (typeof fn !== 'function') {
      throw new Error(`molt_runtime.${name} missing export ${exportName}`);
    }
    const converted = args.map((arg, idx) => normalizeArg(arg, sig.argTypes[idx]));
    if (traceImports) {
      console.error(`molt_runtime.${name}`, sig.argTypes, converted);
    }
    const result = fn(...converted);
    return normalizeReturn(result, sig.retType);
  };
}

const importObject = {
  molt_runtime: runtimeImports,
};

WebAssembly.instantiate(wasmBuffer, importObject)
  .then(wasmModule => {
    const { molt_main, molt_memory, molt_table, molt_call_indirect1 } = wasmModule.instance.exports;
    if (!molt_memory || !molt_table) {
      throw new Error('output.wasm missing molt_memory or molt_table export');
    }
    if (!molt_call_indirect1) {
      throw new Error('output.wasm missing molt_call_indirect1 export');
    }

    const runtimeImports = {
      env: {
        memory: molt_memory,
        __indirect_function_table: molt_table,
        molt_call_indirect1,
      },
      wasi_snapshot_preview1: wasi.wasiImport,
    };
    return WebAssembly.instantiate(runtimeBuffer, runtimeImports).then(runtimeModule => {
      runtimeInstance = runtimeModule.instance;
      wasi.initialize({ exports: { memory: molt_memory } });
      molt_main();
    });
  })
  .catch(err => {
    console.error(err);
    process.exit(1);
  });
