(function installMoltWasmLoaderBridge(root, factory) {
  const executionAbi =
    root.MoltExecutionAbiGenerated ||
    (typeof module === 'object' && module && module.exports
      ? require('./execution_abi_generated.js')
      : null);
  const api = factory(executionAbi);
  if (typeof module === 'object' && module && module.exports) {
    module.exports = api;
  }
  root.MoltWasmLoaderBridge = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, (executionAbi) => {
  if (!executionAbi) {
    throw new Error('generated execution ABI authority is unavailable');
  }
  const WASM_MAGIC = 0x6d736100;
  const WASM_VERSION = 0x1;
  const UTF8_DECODER = new TextDecoder('utf-8');
  const executionContextAbi = executionAbi.execution_context_abi;
  if (!executionContextAbi || typeof executionContextAbi.suffix !== 'string') {
    throw new Error('generated attached execution ABI authority is unavailable');
  }
  const compiledExecutionSymbol = logical => logical + executionContextAbi.suffix;
  // Heap RC, invocation handoff, exception bytes and execution-context headers
  // all address runtime objects through compiled memory zero. Byte-copy host
  // bridges do not establish that Python execution invariant.
  const requireSharedExecutionMemory = (runtimeMemory, appMemory) => {
    if (!(runtimeMemory instanceof WebAssembly.Memory) || appMemory !== runtimeMemory) {
      throw new Error('attached compiled execution requires shared app/runtime linear memory');
    }
  };
  const requireSharedExecutionMemoryImport = module => {
    const memories = WebAssembly.Module.imports(module).filter(entry => entry.kind === 'memory');
    if (memories.length !== 1 || memories[0].module !== 'env' || memories[0].name !== 'memory') {
      throw new Error('attached split application must import the shared env.memory');
    }
  };

  // Detached status observes the exception byte without initializing or
  // entering Python execution; attached compiled readers have another ABI.
  // A missing/wrong-shaped export is never evidence of successful execution.
  const runtimeExceptionPending = (instance) => {
    const pending = instance?.exports?.molt_exception_pending_detached;
    if (typeof pending !== 'function' || pending.length !== 0) {
      throw new Error('missing or malformed molt_exception_pending_detached startup status export');
    }
    const status = pending();
    if (status !== 0n && status !== 1n) {
      throw new Error('malformed molt_exception_pending status: expected i64 raw 0 or 1');
    }
    return status === 1n;
  };

  const requireAppExecutionAbi = (module, generated) => {
    const schema = generated?.execution_context_abi;
    if (!schema || typeof schema.section !== 'string' || typeof schema.suffix !== 'string' ||
        typeof schema.fingerprint !== 'string' || !/^(?:0|[1-9][0-9]*)$/.test(schema.fingerprint)) {
      throw new Error('missing or malformed generated attached execution ABI');
    }
    for (const key of ['fingerprint', 'section', 'suffix', 'thread_record_offset']) {
      if (schema[key] !== executionContextAbi[key]) {
        throw new Error('attached execution ABI mismatch between manifest and generated host');
      }
    }
    const required = BigInt(schema.fingerprint);
    if (required < 0n || required > (1n << 64n) - 1n) {
      throw new Error('malformed attached execution ABI fingerprint');
    }
    const sections = WebAssembly.Module.customSections(module, schema.section);
    if (sections.length !== 1 || sections[0].byteLength !== 8) {
      throw new Error('application missing exact attached execution ABI section');
    }
    const app = new DataView(sections[0]).getBigUint64(0, true);
    if (app !== required) throw new Error('attached execution ABI mismatch between app and host');
    return required;
  };
  const requireAttachedExecutionAbi = (module, runtimeInstance, generated) => {
    const required = requireAppExecutionAbi(module, generated);
    const getter = runtimeInstance?.exports?.molt_attached_context_abi_fingerprint;
    if (typeof getter !== 'function' || getter.length !== 0) {
      throw new Error('runtime missing attached execution ABI fingerprint export');
    }
    const offered = getter();
    if (typeof offered !== 'bigint' || BigInt.asUintN(64, offered) !== required) {
      throw new Error('attached execution ABI mismatch between app, runtime, and host');
    }
    return generated.execution_context_abi;
  };

  const readVarUint = (view, offset) => {
    let result = 0;
    let shift = 0;
    let pos = offset;
    while (true) {
      if (pos >= view.length) {
        throw new Error('Unexpected EOF while reading varuint');
      }
      const byte = view[pos++];
      if (shift === 28 && (byte & 0xf0) !== 0) {
        throw new Error('Overflow while reading varuint32');
      }
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

  const readVarInt32 = (view, offset) => {
    let result = 0;
    let shift = 0;
    let byte = 0;
    let pos = offset;
    while (true) {
      if (pos >= view.length) {
        throw new Error('Unexpected EOF while reading varint');
      }
      byte = view[pos++];
      result |= (byte & 0x7f) << shift;
      shift += 7;
      if ((byte & 0x80) === 0) {
        break;
      }
    }
    if (shift < 32 && (byte & 0x40) !== 0) {
      result |= ~0 << shift;
    }
    return { value: result | 0, offset: pos };
  };

  const skipImportDesc = (view, offset, kind) => {
    let pos = offset;
    if (kind === 0) {
      return readVarUint(view, pos).offset;
    }
    if (kind === 1) {
      if (pos >= view.length) throw new Error('Unexpected EOF in table import');
      pos += 1;
      return readLimits(view, pos).offset;
    }
    if (kind === 2) {
      return readLimits(view, pos).offset;
    }
    if (kind === 3) {
      if (pos + 2 > view.length) throw new Error('Unexpected EOF in global import');
      return pos + 2;
    }
    if (kind === 4) {
      if (pos >= view.length) throw new Error('Unexpected EOF in tag import');
      pos = readVarUint(view, pos).offset;
      return readVarUint(view, pos).offset;
    }
    throw new Error(`Unknown import kind ${kind}`);
  };

  const decodeWasmValType = (encoded) => {
    const view = encoded instanceof Uint8Array ? encoded : new Uint8Array(encoded);
    if (view.length === 0) {
      throw new Error('Empty wasm value type encoding');
    }
    switch (view[0]) {
      case 0x7f:
        return 'i32';
      case 0x7e:
        return 'i64';
      case 0x7d:
        return 'f32';
      case 0x7c:
        return 'f64';
      case 0x7b:
        return 'v128';
      case 0x6f:
        return 'externref';
      case 0x70:
        return 'funcref';
      default:
        if (view[0] === 0x63 || view[0] === 0x64) {
          const heapType = readVarInt32(view, 1);
          if (heapType.offset !== view.length) {
            throw new Error('Trailing bytes in typed-reference value type');
          }
          const qualifier = view[0] === 0x63 ? 'ref null' : 'ref';
          return `(${qualifier} ${heapType.value})`;
        }
        throw new Error(`Unsupported wasm value type 0x${view[0].toString(16)}`);
    }
  };

  const readWasmValType = (view, offset) => {
    if (offset >= view.length) {
      throw new Error('Unexpected EOF in wasm value type');
    }
    const start = offset;
    const form = view[offset++];
    if (form === 0x63 || form === 0x64) {
      offset = readVarInt32(view, offset).offset;
    }
    const encoded = view.subarray(start, offset);
    return { value: decodeWasmValType(encoded), encoded, offset };
  };

  const readWasmValTypeVec = (view, offset) => {
    const countRes = readVarUint(view, offset);
    let count = countRes.value;
    offset = countRes.offset;
    const out = [];
    while (count > 0) {
      const valueType = readWasmValType(view, offset);
      out.push(valueType.value);
      offset = valueType.offset;
      count -= 1;
    }
    return { values: out, offset };
  };

  const importSelectorMatches = (module, name, selector) => {
    if (!selector) {
      return true;
    }
    if (selector.module !== undefined && selector.module !== module) {
      return false;
    }
    return selector.name === undefined || selector.name === name;
  };

  const parseWasmMetadata = (buffer, options = {}) => {
    const view = new Uint8Array(buffer);
    const header = new DataView(view.buffer, view.byteOffset, view.byteLength);
    if (view.length < 8 || header.getUint32(0, true) !== WASM_MAGIC) {
      throw new Error('Invalid WASM header');
    }
    if (header.getUint32(4, true) !== WASM_VERSION) {
      throw new Error('Unsupported WASM version');
    }
    let offset = 8;
    const imports = { funcImports: [], tagImports: [], memory: null, table: null };
    const types = [];
    const funcTypeIndices = [];
    let importedFuncCount = 0;
    const exportFuncIndices = new Map();
    const includeExportFunctionSignatures = options.exportFunctionSignatures !== false;
    while (offset < view.length) {
      const sectionId = view[offset++];
      const sizeRes = readVarUint(view, offset);
      const size = sizeRes.value;
      offset = sizeRes.offset;
      const end = offset + size;
      if (end > view.length) {
        throw new Error('Unexpected EOF while reading section');
      }
      if (sectionId === 0) {
        offset = end;
        continue;
      }
      if (sectionId === 1) {
        let inner = offset;
        const countRes = readVarUint(view, inner);
        let count = countRes.value;
        inner = countRes.offset;
        while (count > 0) {
          if (inner >= end || view[inner++] !== 0x60) {
            throw new Error('Unsupported wasm type form');
          }
          const params = readWasmValTypeVec(view, inner);
          inner = params.offset;
          const results = readWasmValTypeVec(view, inner);
          inner = results.offset;
          types.push({ params: params.values, results: results.values });
          count -= 1;
        }
        offset = end;
        continue;
      }
      if (sectionId === 3) {
        if (!includeExportFunctionSignatures) {
          offset = end;
          continue;
        }
        let inner = offset;
        const countRes = readVarUint(view, inner);
        let count = countRes.value;
        inner = countRes.offset;
        while (count > 0) {
          const typeRes = readVarUint(view, inner);
          funcTypeIndices.push(typeRes.value);
          inner = typeRes.offset;
          count -= 1;
        }
        offset = end;
        continue;
      }
      if (sectionId === 7) {
        if (!includeExportFunctionSignatures) {
          offset = end;
          continue;
        }
        let inner = offset;
        const countRes = readVarUint(view, inner);
        let count = countRes.value;
        inner = countRes.offset;
        while (count > 0) {
          const nameRes = readString(view, inner);
          inner = nameRes.offset;
          if (inner >= end) {
            throw new Error('Unexpected EOF in export kind');
          }
          const kind = view[inner++];
          const indexRes = readVarUint(view, inner);
          inner = indexRes.offset;
          if (kind === 0) {
            exportFuncIndices.set(nameRes.value, indexRes.value);
          }
          count -= 1;
        }
        offset = end;
        continue;
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
          const typeRes = readVarUint(view, inner);
          inner = typeRes.offset;
          importedFuncCount += 1;
          imports.funcImports.push({ module, name });
        } else if (kind === 1) {
          inner += 1;
          const limits = readLimits(view, inner);
          inner = limits.offset;
          if (importSelectorMatches(module, name, options.tableImport)) {
            imports.table = { min: limits.min, max: limits.max };
          }
        } else if (kind === 2) {
          const limits = readLimits(view, inner);
          inner = limits.offset;
          if (importSelectorMatches(module, name, options.memoryImport)) {
            imports.memory = { min: limits.min, max: limits.max };
          }
        } else if (kind === 3) {
          if (inner + 2 > view.length) {
            throw new Error('Unexpected EOF in global import');
          }
          inner += 2;
        } else if (kind === 4) {
          if (inner >= view.length) {
            throw new Error('Unexpected EOF in tag import');
          }
          const attrRes = readVarUint(view, inner);
          const attribute = attrRes.value;
          inner = attrRes.offset;
          const typeRes = readVarUint(view, inner);
          const typeIndex = typeRes.value;
          inner = typeRes.offset;
          const type = types[typeIndex];
          if (!type) {
            throw new Error(`Tag import ${module}.${name} references unknown type index ${typeIndex}`);
          }
          imports.tagImports.push({
            module,
            name,
            attribute,
            typeIndex,
            parameters: type.params,
            results: type.results,
          });
        } else {
          throw new Error(`Unknown import kind ${kind}`);
        }
        count -= 1;
      }
      offset = end;
    }
    const exportFunctionSignatures = {};
    for (const [name, index] of exportFuncIndices.entries()) {
      if (index < importedFuncCount) {
        continue;
      }
      const typeIndex = funcTypeIndices[index - importedFuncCount];
      const type = types[typeIndex];
      if (type) {
        exportFunctionSignatures[name] = {
          params: type.params,
          result: type.results.length ? type.results[0] : null,
        };
      }
    }
    return { imports, exportFunctionSignatures };
  };

  // A split application is a wasm32 side module (`wasm-ld -shared`, the
  // tool-conventions DynamicLinking ABI). The runtime module is the main
  // module: it owns linear memory, the function table and the shadow stack.
  // The host places the side module after the runtime has started: it grows
  // memory for the module's data and the table for its functions, resolves
  // its GOT entries against the runtime's exports, and runs its data
  // relocations before any application code.
  const DYLINK_SECTION = 'dylink.0';
  const DYLINK_MEM_INFO = 1;
  const DYLINK_NEEDED = 2;
  const WASM_PAGE_BYTES = 65536;
  const readSideModuleLayout = (module) => {
    const sections = WebAssembly.Module.customSections(module, DYLINK_SECTION);
    if (sections.length !== 1) {
      throw new Error(`split application must carry exactly one ${DYLINK_SECTION} section`);
    }
    const view = new Uint8Array(sections[0]);
    let layout = null;
    let offset = 0;
    while (offset < view.length) {
      const kind = view[offset++];
      const size = readVarUint(view, offset);
      const end = size.offset + size.value;
      if (end > view.length) throw new Error(`truncated ${DYLINK_SECTION} subsection ${kind}`);
      if (kind === DYLINK_MEM_INFO) {
        let cursor = size.offset;
        const fields = [];
        for (let index = 0; index < 4; index++) {
          const field = readVarUint(view, cursor);
          fields.push(field.value);
          cursor = field.offset;
        }
        layout = {
          memorySize: fields[0], memoryAlign: fields[1],
          tableSize: fields[2], tableAlign: fields[3],
        };
      } else if (kind === DYLINK_NEEDED) {
        if (readVarUint(view, size.offset).value !== 0) {
          throw new Error('split application must not need other shared libraries');
        }
      }
      offset = end;
    }
    if (layout === null) throw new Error(`${DYLINK_SECTION} has no memory info`);
    return layout;
  };

  const alignUp = (value, alignment) => Math.ceil(value / alignment) * alignment;

  // Returns the side module's import namespaces and the start step that must
  // run once, right after instantiation.
  const placeSideModule = ({ module, runtimeExports, memory, table }) => {
    const layout = readSideModuleLayout(module);
    const memoryAlign = 2 ** layout.memoryAlign;
    let memoryBase = 0;
    if (layout.memorySize !== 0) {
      // Pages the host grows are never handed out by the runtime allocator,
      // which only uses the pages its own memory.grow returns.
      const previousPages = memory.grow(
        Math.ceil((layout.memorySize + memoryAlign) / WASM_PAGE_BYTES));
      memoryBase = alignUp(previousPages * WASM_PAGE_BYTES, memoryAlign);
    }
    const tableAlign = 2 ** layout.tableAlign;
    const tableBase = alignUp(table.length, tableAlign);
    if (tableBase + layout.tableSize > table.length) {
      table.grow(tableBase + layout.tableSize - table.length);
    }
    const stackPointer = runtimeExports.__stack_pointer;
    if (!(stackPointer instanceof WebAssembly.Global)) {
      throw new Error('runtime module does not export its __stack_pointer');
    }
    let tableSlots = null;
    const tableSlotOf = (fn) => {
      if (tableSlots === null) {
        tableSlots = new Map();
        for (let slot = 0; slot < table.length; slot++) {
          const entry = table.get(slot);
          if (entry !== null && !tableSlots.has(entry)) tableSlots.set(entry, slot);
        }
      }
      let slot = tableSlots.get(fn);
      if (slot === undefined) {
        slot = table.grow(1);
        table.set(slot, fn);
        tableSlots.set(fn, slot);
      }
      return slot;
    };
    const memoryGot = Object.create(null);
    const functionGot = Object.create(null);
    for (const entry of WebAssembly.Module.imports(module)) {
      if (entry.module === 'GOT.mem') {
        const address = runtimeExports[entry.name];
        if (!(address instanceof WebAssembly.Global)) {
          throw new Error(`runtime module does not export data symbol ${entry.name}`);
        }
        memoryGot[entry.name] = new WebAssembly.Global({ value: 'i32', mutable: true }, address.value);
      } else if (entry.module === 'GOT.func') {
        const fn = runtimeExports[entry.name];
        if (typeof fn !== 'function') {
          throw new Error(`runtime module does not export function ${entry.name}`);
        }
        functionGot[entry.name] = new WebAssembly.Global({ value: 'i32', mutable: true }, tableSlotOf(fn));
      }
    }
    return {
      env: {
        __memory_base: new WebAssembly.Global({ value: 'i32', mutable: false }, memoryBase),
        __table_base: new WebAssembly.Global({ value: 'i32', mutable: false }, tableBase),
        __stack_pointer: stackPointer,
      },
      'GOT.mem': memoryGot,
      'GOT.func': functionGot,
      start: (instance) => {
        const relocate = instance.exports.__wasm_apply_data_relocs;
        if (typeof relocate === 'function') relocate();
        const constructors = instance.exports.__wasm_call_ctors;
        if (typeof constructors === 'function') constructors();
      },
    };
  };

  // Whether the module bytes carry a dylink.0 section: one pass over the
  // section headers, without compiling the module.
  const bufferIsSideModule = (buffer) => {
    const view = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    let offset = 8;
    while (offset < view.length) {
      const id = view[offset++];
      const size = readVarUint(view, offset);
      const end = size.offset + size.value;
      if (id === 0) {
        const name = readVarUint(view, size.offset);
        const bytes = view.subarray(name.offset, name.offset + name.value);
        if (UTF8_DECODER.decode(bytes) === DYLINK_SECTION) return true;
      }
      offset = end;
    }
    return false;
  };

  // Whether `module` (a compiled WebAssembly.Module) is a wasm32 side module.
  const isSideModule = (module) =>
    WebAssembly.Module.customSections(module, DYLINK_SECTION).length !== 0;

  // The `molt_runtime` namespace of a side module: each import is the
  // runtime export of the same physical name, bound directly (a wasm-to-wasm
  // call, no host adapter).
  const sideModuleRuntimeImports = (module, runtimeExports) => {
    const imports = Object.create(null);
    const missing = [];
    for (const entry of WebAssembly.Module.imports(module)) {
      if (entry.module !== 'molt_runtime') continue;
      const fn = runtimeExports[entry.name];
      if (typeof fn !== 'function') missing.push(entry.name);
      else imports[entry.name] = fn;
    }
    if (missing.length) {
      throw new Error(`molt_runtime missing side-module imports: ${missing.sort().join(', ')}`);
    }
    return imports;
  };

  const parseWasmImports = (buffer, options = {}) =>
    parseWasmMetadata(buffer, options).imports;

  const parseWasmExportFunctionSignatures = (buffer) =>
    parseWasmMetadata(buffer).exportFunctionSignatures;

  const makeWasmTagImport = (entry) => {
    if (typeof WebAssembly.Tag !== 'function') {
      throw new Error(
        `WASM tag import ${entry.module}.${entry.name} requires WebAssembly.Tag host support`,
      );
    }
    const results = Array.isArray(entry.results) ? entry.results : [];
    if (results.length !== 0) {
      throw new Error(
        `WASM tag import ${entry.module}.${entry.name} has unsupported result arity ${results.length}`,
      );
    }
    const parameters = Array.isArray(entry.parameters) ? entry.parameters : [];
    return new WebAssembly.Tag({ parameters });
  };

  const installWasmTagImports = (importObject, imports) => {
    for (const entry of imports.tagImports || []) {
      if (!importObject[entry.module]) {
        importObject[entry.module] = {};
      }
      const moduleImports = importObject[entry.module];
      const existing = moduleImports[entry.name];
      if (
        existing !== undefined &&
        !(typeof WebAssembly.Tag === 'function' && existing instanceof WebAssembly.Tag)
      ) {
        throw new Error(
          `WASM tag import ${entry.module}.${entry.name} conflicts with existing non-tag import`,
        );
      }
      moduleImports[entry.name] = existing || makeWasmTagImport(entry);
    }
    return importObject;
  };

  const makeManifestLinkImportTrap = (entry, primitiveClass) => {
    const qualified = `${entry.module}.${entry.name}`;
    return () => {
      throw new Error(
        `WASM link import ${qualified} (${primitiveClass}) was called at runtime; ` +
          'the symbol is manifest-approved only as an external native link import',
      );
    };
  };

  const installManifestLinkImportTraps = (importObject, imports, linkImportAbi) => {
    const primitiveClasses = linkImportAbi?.primitive_classes || {};
    const symbolKinds = linkImportAbi?.symbol_kinds || {};
    for (const entry of imports.funcImports || []) {
      if (entry.module !== 'env') {
        continue;
      }
      const primitiveClass = primitiveClasses[entry.name];
      if (!primitiveClass) {
        continue;
      }
      const symbolKind = symbolKinds[entry.name] || 'function';
      if (symbolKind !== 'function') {
        continue;
      }
      if (!importObject[entry.module]) {
        importObject[entry.module] = {};
      }
      const moduleImports = importObject[entry.module];
      const existing = moduleImports[entry.name];
      if (existing === undefined) {
        moduleImports[entry.name] = makeManifestLinkImportTrap(entry, primitiveClass);
        continue;
      }
      if (typeof existing !== 'function') {
        throw new Error(
          `WASM link import ${entry.module}.${entry.name} (${primitiveClass}) ` +
            `conflicts with existing non-function import`,
        );
      }
    }
    return importObject;
  };

  const normalizeI64BridgeValue = (value, label) => {
    if (value === undefined || value === null) {
      return 0n;
    }
    if (typeof value === 'bigint') {
      return value;
    }
    if (typeof value !== 'number' || !Number.isFinite(value) || !Number.isInteger(value)) {
      throw new TypeError(`Expected integer for ${label}, got ${value}`);
    }
    return BigInt.asUintN(64, BigInt(value));
  };

  const callIsolateImportExport = (fn, args) => {
    if (args.length !== 2) {
      throw new TypeError(`molt_isolate_import expects context and one i64 handle, got ${args.length}`);
    }
    const context = normalizeI64BridgeValue(args[0], 'molt_isolate_import context');
    const handle = normalizeI64BridgeValue(args[1], 'molt_isolate_import handle');
    return normalizeI64BridgeValue(fn(context, handle), 'molt_isolate_import result');
  };

  const requireMoltDeploymentManifest = manifest => {
    if (!Number.isInteger(executionAbi.execution_manifest_version) || executionAbi.execution_manifest_version <= 0) {
      throw new Error('Molt host deployment ABI projection is missing; rebuild host assets');
    }
    if (!manifest || typeof manifest !== 'object' || manifest.version !== executionAbi.execution_manifest_version) {
      throw new Error(`Molt deployment manifest version must be ${executionAbi.execution_manifest_version}; rebuild this deployment with the current Molt compiler`);
    }
    const launch = manifest.launch;
    if (!launch || typeof launch !== 'object' || Array.isArray(launch) ||
        Object.keys(launch).length !== 1 || !Object.hasOwn(launch, 'environment')) {
      throw new Error('WASM deployment launch requires exactly environment');
    }
    const environment = launch.environment;
    if (!environment || typeof environment !== 'object' || Array.isArray(environment)) {
      throw new Error('WASM deployment launch.environment must be a string map');
    }
    for (const [name, value] of Object.entries(environment)) {
      if (!name || name.includes('=') || name.includes('\0')) {
        throw new Error('WASM deployment launch environment name is invalid');
      }
      if (typeof value !== 'string' || value.includes('\0')) {
        throw new Error('WASM deployment launch environment value is invalid');
      }
    }
    if (!Array.isArray(executionAbi.execution_manifest_required_environment) ||
        executionAbi.execution_manifest_required_environment.length === 0 ||
        executionAbi.execution_manifest_required_environment.some(name => !Object.hasOwn(environment, name))) {
      throw new Error('WASM deployment launch environment lacks required policy entries');
    }
    return manifest;
  };

  // Match native launch semantics: emitted policy keys override the host's
  // values; optional omitted keys and unrelated environment remain ambient.
  // A null prototype keeps every transported name a data entry.
  const deploymentLaunchEnvironment = (manifest, ambient = {}) => {
    requireMoltDeploymentManifest(manifest);
    const result = Object.create(null);
    for (const [name, value] of Object.entries(ambient)) result[name] = value;
    for (const [name, value] of Object.entries(manifest.launch.environment)) result[name] = value;
    return result;
  };

  const manifestModuleUrl = (manifestUrl, descriptor, label) => {
    if (!descriptor || typeof descriptor.path !== 'string' || descriptor.path.length === 0) {
      throw new Error(`Molt deployment manifest missing modules.${label}.path`);
    }
    try { return new URL(descriptor.path, manifestUrl).href; }
    catch { throw new Error(`Molt deployment manifest has invalid modules.${label}.path`); }
  };

  const verifyManifestModuleBytes = async (
    bytes,
    descriptor,
    label,
    requestedUrl,
    cryptoAuthority = globalThis.crypto,
    manifestUrl = null,
    moduleUrl = manifestUrl || requestedUrl,
  ) => {
    if (
      !descriptor ||
      typeof descriptor !== 'object' ||
      typeof descriptor.path !== 'string' ||
      descriptor.path.length === 0 ||
      !Number.isSafeInteger(descriptor.size) ||
      descriptor.size < 0 ||
      typeof descriptor.sha256 !== 'string' ||
      !/^[0-9a-f]{64}$/.test(descriptor.sha256)
    ) {
      throw new Error(`${label} manifest module descriptor is invalid`);
    }
    let requestedHref;
    let manifestHref;
    try {
      requestedHref = new URL(requestedUrl, moduleUrl).href;
      manifestHref = manifestUrl
        ? new URL(descriptor.path, manifestUrl).href
        : new URL(descriptor.path, requestedHref).href;
    } catch {
      throw new Error(`${label} requested module URL is invalid`);
    }
    if (requestedHref !== manifestHref) {
      throw new Error(
        `${label} path mismatch: manifest=${manifestHref} requested=${requestedHref}`,
      );
    }
    if (!(bytes instanceof ArrayBuffer)) {
      throw new Error(`${label} fetch did not return an ArrayBuffer`);
    }
    if (bytes.byteLength !== descriptor.size) {
      throw new Error(
        `${label} size mismatch: manifest=${descriptor.size} fetched=${bytes.byteLength}`,
      );
    }
    const subtle = cryptoAuthority?.subtle;
    if (!subtle || typeof subtle.digest !== 'function') {
      throw new Error(`${label} integrity verification requires WebCrypto SHA-256`);
    }
    let digest;
    try {
      digest = await subtle.digest('SHA-256', bytes);
    } catch (error) {
      throw new Error(`${label} SHA-256 verification failed`, { cause: error });
    }
    const actual = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, '0'),
    ).join('');
    if (actual !== descriptor.sha256) {
      throw new Error(
        `${label} SHA-256 mismatch: manifest=${descriptor.sha256} fetched=${actual}`,
      );
    }
  };
  return {
    requireMoltDeploymentManifest,
    deploymentLaunchEnvironment,
    manifestModuleUrl,
    verifyManifestModuleBytes,
    compiledExecutionSymbol,
    requireSharedExecutionMemory,
    requireSharedExecutionMemoryImport,
    callIsolateImportExport,
    installManifestLinkImportTraps,
    installWasmTagImports,
    normalizeI64BridgeValue,
    parseWasmMetadata,
    parseWasmExportFunctionSignatures,
    parseWasmImports,
    bufferIsSideModule,
    isSideModule,
    placeSideModule,
    sideModuleRuntimeImports,
    runtimeExceptionPending,
    requireAttachedExecutionAbi,
    requireAppExecutionAbi,
  };
});
