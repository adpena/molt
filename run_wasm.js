const fs = require('fs');
const path = require('path');
const { WASI } = require('wasi');

const wasmBuffer = fs.readFileSync('output.wasm');
const linkedPath =
  process.env.MOLT_WASM_LINKED_PATH || path.join(__dirname, 'output_linked.wasm');
const linkedBuffer = fs.existsSync(linkedPath) ? fs.readFileSync(linkedPath) : null;
const runtimePath =
  process.env.MOLT_RUNTIME_WASM || path.join(__dirname, 'wasm', 'molt_runtime.wasm');
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
const traceImports = process.env.MOLT_WASM_TRACE === '1';
const forceLegacy = process.env.MOLT_WASM_LEGACY === '1';

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
  let memory = null;
  let table = null;
  const funcImports = [];
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
          funcImports.push({ module: moduleName, name: fieldName });
        } else if (kind === 1) {
          const elemType = bytes[offset++];
          let limits;
          [limits, offset] = readLimits(bytes, offset);
          if (moduleName === 'env' && fieldName === '__indirect_function_table') {
            table = { min: limits.min, max: limits.max, elemType };
          }
        } else if (kind === 2) {
          let limits;
          [limits, offset] = readLimits(bytes, offset);
          if (moduleName === 'env' && fieldName === 'memory') {
            memory = { min: limits.min, max: limits.max };
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
  return { memory, table, funcImports };
};

const outputImports = parseWasmImports(wasmBuffer);
const runtimeImportsDesc = parseWasmImports(runtimeBuffer);

const mergeLimits = (left, right, label) => {
  if (!left && !right) {
    return null;
  }
  const base = left || right;
  if (!left || !right) {
    return { min: base.min, max: base.max };
  }
  const min = Math.max(left.min, right.min);
  let max = null;
  if (left.max !== null && right.max !== null) {
    max = Math.min(left.max, right.max);
  } else {
    max = left.max !== null ? left.max : right.max;
  }
  if (max !== null && min > max) {
    throw new Error(`Incompatible ${label} limits: min ${min} > max ${max}`);
  }
  return { min, max };
};

const makeMemory = (limits) => {
  if (!limits) {
    return null;
  }
  const desc = { initial: limits.min };
  if (limits.max !== null) {
    desc.maximum = limits.max;
  }
  return new WebAssembly.Memory(desc);
};

const makeTable = (limits) => {
  if (!limits) {
    return null;
  }
  const desc = { initial: limits.min, element: 'anyfunc' };
  if (limits.max !== null) {
    desc.maximum = limits.max;
  }
  return new WebAssembly.Table(desc);
};

const canDirectLink =
  !forceLegacy && !traceImports && outputImports.memory && outputImports.table && runtimeImportsDesc.memory;

const buildRuntimeImportWrappers = () => {
  const funcSigs = new Map();
  for (const line of witSource.split('\n')) {
    const match = line.match(/^\s*([A-Za-z0-9_]+):\s*func\(([^)]*)\)\s*(?:->\s*([^;]+))?/);
    if (!match) {
      continue;
    }
    const name = match[1];
    const rawArgs = match[2].trim();
    const argTypes = rawArgs
      ? rawArgs.split(',').map((part) => part.split(':')[1].trim())
      : [];
    const retType = match[3] ? match[3].trim() : null;
    funcSigs.set(name, { argTypes, retType });
  }

  const expectsBigInt = (ty) =>
    ty === 'molt-object' || ty === 'molt-ptr' || ty === 'u64' || ty === 's64';
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

  const runtimeImports = {};
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
  return runtimeImports;
};

const buildRuntimeImportDirect = (runtimeInst) => {
  const runtimeImports = {};
  for (const entry of outputImports.funcImports) {
    if (entry.module !== 'molt_runtime') {
      continue;
    }
    const exportName = `molt_${entry.name}`;
    const fn = runtimeInst.exports[exportName];
    if (typeof fn !== 'function') {
      throw new Error(`molt_runtime.${entry.name} missing export ${exportName}`);
    }
    runtimeImports[entry.name] = fn;
  }
  return runtimeImports;
};

const runLegacy = async () => {
  const runtimeImports = buildRuntimeImportWrappers();
  const importObject = { molt_runtime: runtimeImports };
  let sharedMemory = null;
  let sharedTable = null;
  if (outputImports.memory && outputImports.table) {
    const memoryLimits = mergeLimits(outputImports.memory, runtimeImportsDesc.memory, 'memory');
    const tableLimits = mergeLimits(outputImports.table, runtimeImportsDesc.table, 'table');
    sharedMemory = makeMemory(memoryLimits);
    sharedTable = makeTable(tableLimits);
    importObject.env = {
      memory: sharedMemory,
      __indirect_function_table: sharedTable,
    };
  }

  const outputModule = await WebAssembly.instantiate(wasmBuffer, importObject);
  const { molt_main, molt_memory, molt_table, molt_call_indirect1 } = outputModule.instance.exports;
  const memory = sharedMemory || molt_memory;
  const table = sharedTable || molt_table;

  if (!memory || !table) {
    throw new Error('output.wasm missing molt_memory or molt_table export');
  }
  if (!molt_call_indirect1) {
    throw new Error('output.wasm missing molt_call_indirect1 export');
  }

  const runtimeImportsObj = {
    env: {
      memory,
      __indirect_function_table: table,
      molt_call_indirect1,
    },
    wasi_snapshot_preview1: wasi.wasiImport,
  };
  const runtimeModule = await WebAssembly.instantiate(runtimeBuffer, runtimeImportsObj);
  runtimeInstance = runtimeModule.instance;
  wasi.initialize({ exports: { memory } });
  molt_main();
};

const runDirectLink = async () => {
  const memoryLimits = mergeLimits(outputImports.memory, runtimeImportsDesc.memory, 'memory');
  const tableLimits = mergeLimits(outputImports.table, runtimeImportsDesc.table, 'table');
  const memory = makeMemory(memoryLimits);
  const table = makeTable(tableLimits);
  let callIndirect = null;

  const env = {
    memory,
    __indirect_function_table: table,
  };
  const needsCallIndirect = runtimeImportsDesc.funcImports.some(
    (entry) => entry.module === 'env' && entry.name === 'molt_call_indirect1'
  );
  if (needsCallIndirect) {
    env.molt_call_indirect1 = (funcIdx, arg0) => {
      if (!callIndirect) {
        throw new Error('molt_call_indirect1 used before output instantiation');
      }
      return callIndirect(funcIdx, arg0);
    };
  }

  const runtimeModule = await WebAssembly.instantiate(runtimeBuffer, {
    env,
    wasi_snapshot_preview1: wasi.wasiImport,
  });
  const runtimeInst = runtimeModule.instance;
  const outputImportsDirect = buildRuntimeImportDirect(runtimeInst);
  const outputModule = await WebAssembly.instantiate(wasmBuffer, {
    molt_runtime: outputImportsDirect,
    env: {
      memory,
      __indirect_function_table: table,
    },
  });

  runtimeInstance = runtimeInst;
  const { molt_main, molt_memory, molt_table, molt_call_indirect1 } =
    outputModule.instance.exports;
  callIndirect = molt_call_indirect1;
  if (!molt_memory || !molt_table) {
    throw new Error('output.wasm missing molt_memory or molt_table export');
  }
  wasi.initialize({ exports: { memory } });
  molt_main();
};

const runLinked = async () => {
  if (!linkedBuffer) {
    throw new Error(`Linked wasm not found at ${linkedPath}`);
  }
  const linkedImports = parseWasmImports(linkedBuffer);
  const hasRuntimeImports = linkedImports.funcImports.some(
    (entry) => entry.module === 'molt_runtime'
  );
  if (hasRuntimeImports) {
    throw new Error('Linked wasm still imports molt_runtime; link step incomplete');
  }
  const needsCallIndirect = linkedImports.funcImports.some(
    (entry) => entry.module === 'env' && entry.name === 'molt_call_indirect1'
  );

  const importObject = {
    wasi_snapshot_preview1: wasi.wasiImport,
  };
  if (linkedImports.memory || linkedImports.table) {
    const memory = makeMemory(linkedImports.memory);
    const table = makeTable(linkedImports.table);
    importObject.env = {};
    if (memory) {
      importObject.env.memory = memory;
    }
    if (table) {
      importObject.env.__indirect_function_table = table;
    }
  }
  if (needsCallIndirect) {
    if (!importObject.env) {
      importObject.env = {};
    }
    let callIndirect = null;
    importObject.env.molt_call_indirect1 = (funcIdx, arg0) => {
      if (!callIndirect) {
        throw new Error('molt_call_indirect1 used before linked instantiation');
      }
      return callIndirect(funcIdx, arg0);
    };
    importObject.__molt_call_indirect1 = (fn) => {
      callIndirect = fn;
    };
  }

  const linkedModule = await WebAssembly.instantiate(linkedBuffer, importObject);
  const { molt_main } = linkedModule.instance.exports;
  if (typeof molt_main !== 'function') {
    throw new Error('linked wasm missing molt_main export');
  }
  const linkedMemory =
    linkedModule.instance.exports.molt_memory ||
    linkedModule.instance.exports.memory ||
    (importObject.env && importObject.env.memory);
  if (linkedMemory) {
    wasi.initialize({ exports: { memory: linkedMemory } });
  }
  if (needsCallIndirect) {
    const linkedIndirect = linkedModule.instance.exports.molt_call_indirect1;
    if (typeof linkedIndirect !== 'function') {
      throw new Error('linked wasm missing molt_call_indirect1 export');
    }
    importObject.__molt_call_indirect1(linkedIndirect);
  }
  molt_main();
};

const useLinked = process.env.MOLT_WASM_LINKED === '1';
const runner = useLinked ? runLinked : canDirectLink ? runDirectLink : runLegacy;
runner().catch((err) => {
  console.error(err);
  process.exit(1);
});
