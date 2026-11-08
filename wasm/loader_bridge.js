(function installMoltWasmLoaderBridge(root, factory) {
  const callableTableAbi =
    root.MoltCallableTableAbiGenerated ||
    (typeof module === 'object' && module && module.exports
      ? require('./callable_table_abi_generated.js')
      : null);
  const api = factory(callableTableAbi);
  if (typeof module === 'object' && module && module.exports) {
    module.exports = api;
  }
  root.MoltWasmLoaderBridge = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, (callableTableAbi) => {
  if (!callableTableAbi) {
    throw new Error('generated callable-table ABI authority is unavailable');
  }
  const WASM_MAGIC = 0x6d736100;
  const WASM_VERSION = 0x1;
  const CALLABLE_TABLE_SECTION_NAME = callableTableAbi.section_name;
  const CALLABLE_TABLE_SECTION_VERSION = callableTableAbi.version;
  const CALLABLE_TABLE_ACTIVE_ELEMENT_ROLE = callableTableAbi.active_element_role;
  const CALLABLE_TABLE_VALUE_TYPE_FORMAT = callableTableAbi.value_type_format;
  const UTF8_DECODER = new TextDecoder('utf-8');

  // Resolve the exact generated identities once, before application startup.
  // Adaptation belongs to each host's memory/representation bridge, not to
  // export selection. Successful binding allocates only the required table;
  // complete missing-export diagnostics allocate only on refusal.
  const bindRuntimeFunctionImports = (entries, runtimeExports, exportForImport, adapt) => {
    const imports = Object.create(null);
    let missing = null;
    for (const entry of entries) {
      if (entry.module !== 'molt_runtime') continue;
      const exportName = exportForImport(entry.name);
      const fn = exportName ? runtimeExports[exportName] : null;
      if (typeof fn !== 'function') {
        if (missing === null) missing = new Set();
        missing.add(`${entry.name} -> ${exportName || '<unmapped>'}`);
        continue;
      }
      imports[entry.name] = adapt(entry, fn, exportName);
    }
    if (missing !== null) {
      throw new Error(`molt_runtime missing required exports: ${[...missing].sort().join(', ')}`);
    }
    return imports;
  };

  // Canonical runtime ABI: molt_exception_pending is () -> u64 raw 0/1.
  // A missing/wrong-shaped export is never evidence of successful execution.
  const runtimeExceptionPending = (instance) => {
    const pending = instance?.exports?.molt_exception_pending;
    if (typeof pending !== 'function' || pending.length !== 0) {
      throw new Error('missing or malformed molt_exception_pending startup status export');
    }
    const status = pending();
    if (status !== 0n && status !== 1n) {
      throw new Error('malformed molt_exception_pending status: expected i64 raw 0 or 1');
    }
    return status === 1n;
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

  const readWasmValTypeVec = (view, offset, lengthPrefixed = false) => {
    const countRes = readVarUint(view, offset);
    let count = countRes.value;
    offset = countRes.offset;
    const out = [];
    while (count > 0) {
      if (lengthPrefixed) {
        const byteCountRes = readVarUint(view, offset);
        const byteCount = byteCountRes.value;
        offset = byteCountRes.offset;
        const end = offset + byteCount;
        if (byteCount === 0 || end > view.length) {
          throw new Error('Unexpected EOF in length-prefixed valtype');
        }
        const valueType = readWasmValType(view.subarray(offset, end), 0);
        if (valueType.offset !== byteCount) {
          throw new Error('Malformed length-prefixed valtype');
        }
        out.push(valueType.value);
        offset = end;
      } else {
        const valueType = readWasmValType(view, offset);
        out.push(valueType.value);
        offset = valueType.offset;
      }
      count -= 1;
    }
    return { values: out, offset };
  };

  const parseCallableTableSectionPayload = (payload, label = 'wasm') => {
    const view = payload instanceof Uint8Array ? payload : new Uint8Array(payload);
    let offset = 0;
    const versionRes = readVarUint(view, offset);
    const version = versionRes.value;
    offset = versionRes.offset;
    if (version !== CALLABLE_TABLE_SECTION_VERSION) {
      throw new Error(
        `${label} ${CALLABLE_TABLE_SECTION_NAME} has unsupported version ${version}`,
      );
    }
    const valueTypeFormatRes = readVarUint(view, offset);
    const valueTypeFormat = valueTypeFormatRes.value;
    offset = valueTypeFormatRes.offset;
    if (valueTypeFormat !== CALLABLE_TABLE_VALUE_TYPE_FORMAT) {
      throw new Error(
        `${label} ${CALLABLE_TABLE_SECTION_NAME} has unsupported value-type format ` +
          `${valueTypeFormat}`,
      );
    }

    const typeCountRes = readVarUint(view, offset);
    const typeCount = typeCountRes.value;
    offset = typeCountRes.offset;
    const types = new Map();
    let previousTypeIndex = -1;
    for (let index = 0; index < typeCount; index += 1) {
      const typeIndexRes = readVarUint(view, offset);
      const typeIndex = typeIndexRes.value;
      offset = typeIndexRes.offset;
      if (typeIndex <= previousTypeIndex) {
        throw new Error(
          `${label} ${CALLABLE_TABLE_SECTION_NAME} type indices are not strictly ordered`,
        );
      }
      previousTypeIndex = typeIndex;
      const params = readWasmValTypeVec(view, offset, true);
      offset = params.offset;
      const results = readWasmValTypeVec(view, offset, true);
      offset = results.offset;
      types.set(typeIndex, {
        typeIndex,
        params: params.values,
        results: results.values,
        result: results.values.length === 0 ? 'nil' : results.values.join(', '),
      });
    }

    const entryCountRes = readVarUint(view, offset);
    const entryCount = entryCountRes.value;
    offset = entryCountRes.offset;
    const entries = [];
    const bySlot = new Map();
    let slot = 0;
    for (let index = 0; index < entryCount; index += 1) {
      const slotDeltaRes = readVarUint(view, offset);
      const slotDelta = slotDeltaRes.value;
      offset = slotDeltaRes.offset;
      if (index > 0 && slotDelta === 0) {
        throw new Error(
          `${label} ${CALLABLE_TABLE_SECTION_NAME} contains duplicate table slot ${slot}`,
        );
      }
      slot = index === 0 ? slotDelta : slot + slotDelta;
      if (slot > 0xffffffff) {
        throw new Error(`${label} ${CALLABLE_TABLE_SECTION_NAME} slot exceeds u32`);
      }
      const functionIndexRes = readVarUint(view, offset);
      const functionIndex = functionIndexRes.value;
      offset = functionIndexRes.offset;
      const typeIndexRes = readVarUint(view, offset);
      const typeIndex = typeIndexRes.value;
      offset = typeIndexRes.offset;
      const roleRes = readVarUint(view, offset);
      const role = roleRes.value;
      offset = roleRes.offset;
      if (role !== CALLABLE_TABLE_ACTIVE_ELEMENT_ROLE) {
        throw new Error(
          `${label} ${CALLABLE_TABLE_SECTION_NAME} slot ${slot} has unknown role ${role}`,
        );
      }
      const signature = types.get(typeIndex);
      if (!signature) {
        throw new Error(
          `${label} ${CALLABLE_TABLE_SECTION_NAME} slot ${slot} references missing type ${typeIndex}`,
        );
      }
      const entry = { slot, functionIndex, typeIndex, role, signature };
      entries.push(entry);
      bySlot.set(slot, entry);
    }
    if (offset !== view.length) {
      throw new Error(
        `${label} ${CALLABLE_TABLE_SECTION_NAME} has ${view.length - offset} trailing byte(s)`,
      );
    }
    return { version, types, entries, bySlot };
  };

  const callableTableFromModule = (module, label = 'wasm') => {
    const sections = WebAssembly.Module.customSections(module, CALLABLE_TABLE_SECTION_NAME);
    if (sections.length !== 1) {
      throw new Error(
        `${label} must contain exactly one ${CALLABLE_TABLE_SECTION_NAME} section; ` +
          `found ${sections.length}`,
      );
    }
    return parseCallableTableSectionPayload(new Uint8Array(sections[0]), label);
  };

  const callableTableSignature = (attestation, slot) => {
    const index = Number(slot);
    if (!Number.isInteger(index) || index < 0) {
      return null;
    }
    return attestation?.bySlot?.get(index)?.signature || null;
  };

  const verifyCallableTableEntries = (attestation, table, label = 'wasm') => {
    if (!attestation) {
      throw new Error(`${label} callable-table verification requires an attestation`);
    }
    if (attestation.entries.length === 0) {
      return;
    }
    if (!table) {
      throw new Error(`${label} callable-table verification requires a table`);
    }
    for (const entry of attestation.entries) {
      if (entry.slot >= table.length) {
        throw new Error(
          `${label} callable-table slot ${entry.slot} exceeds table length ${table.length}`,
        );
      }
      if (typeof table.get(entry.slot) !== 'function') {
        throw new Error(`${label} callable-table slot ${entry.slot} is not initialized`);
      }
    }
  };

  const verifyCallableTableManifestSummary = (attestation, summary, label = 'wasm') => {
    if (!summary || typeof summary !== 'object') {
      throw new Error(`${label} manifest is missing callable-table summary`);
    }
    const expected = {
      section: CALLABLE_TABLE_SECTION_NAME,
      version: attestation.version,
      entry_count: attestation.entries.length,
      first_slot: attestation.entries.length ? attestation.entries[0].slot : null,
      last_slot: attestation.entries.length
        ? attestation.entries[attestation.entries.length - 1].slot
        : null,
    };
    for (const [name, value] of Object.entries(expected)) {
      if (summary[name] !== value) {
        throw new Error(
          `${label} callable-table manifest ${name}=${String(summary[name])} ` +
            `does not match binary ${String(value)}`,
        );
      }
    }
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
    let callableTable = null;
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
        const nameRes = readString(view, offset);
        if (nameRes.value === CALLABLE_TABLE_SECTION_NAME) {
          if (callableTable !== null) {
            throw new Error(`WASM contains duplicate ${CALLABLE_TABLE_SECTION_NAME} sections`);
          }
          callableTable = parseCallableTableSectionPayload(
            view.subarray(nameRes.offset, end),
            options.label || 'wasm',
          );
        }
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
    return { imports, exportFunctionSignatures, callableTable };
  };

  const requireWasmCallableTable = (buffer, label = 'wasm') => {
    const callableTable = parseWasmMetadata(buffer, {
      exportFunctionSignatures: false,
      label,
    }).callableTable;
    if (callableTable === null) {
      throw new Error(`${label} is missing required ${CALLABLE_TABLE_SECTION_NAME} section`);
    }
    return callableTable;
  };

  // Hosts read an artifact's callable-table attestation only for the
  // signatures of its own entries. No slot is reserved for runtime callables,
  // so no table base is published to the runtime (HF-699 K3). Compiled-module
  // consumers (Workers import Modules) read the same attestation.
  const readCallableTableAttestation = ({ buffer = null, module = null, label = 'wasm' }) => {
    try {
      if (module !== null) {
        const sections = WebAssembly.Module.customSections(module, CALLABLE_TABLE_SECTION_NAME);
        if (sections.length === 0) {
          throw new Error('missing callable-table attestation');
        }
        return callableTableFromModule(module, label);
      }
      const callableTable = parseWasmMetadata(buffer, {
        exportFunctionSignatures: false, label,
      }).callableTable;
      if (callableTable === null) throw new Error('missing callable-table attestation');
      return callableTable;
    } catch (error) {
      const missing = error.message === 'missing callable-table attestation';
      throw new Error(`${missing ? 'MOLT_WASM_TABLE_ATTESTATION_MISSING' : 'MOLT_WASM_TABLE_ATTESTATION_MALFORMED'}: ${label}: ${error.message}`);
    }
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

  // Kinds are WebAssembly value types: an i64 crosses as a BigInt and an i32
  // as a Number.
  const normalizeValueForKind = (value, kind) => {
    if (kind === 'i64') {
      return normalizeI64BridgeValue(value, kind);
    }
    if (kind === 'i32') {
      return Number(value);
    }
    return value;
  };


  const callIsolateImportExport = (fn, args) => {
    if (args.length !== 1) {
      throw new TypeError(`molt_isolate_import expects one i64 handle, got ${args.length}`);
    }
    const handle = normalizeI64BridgeValue(args[0], 'molt_isolate_import handle');
    return normalizeI64BridgeValue(fn(handle), 'molt_isolate_import result');
  };

  const callWithWasmSignature = (fn, signature, args) => {
    if (!signature) {
      return fn(...args);
    }
    const { params } = signature;
    if (!Array.isArray(params)) {
      return fn(...args);
    }
    const callArgs = args.map((value, index) =>
      normalizeValueForKind(value, params[index] || null));
    const out = fn(...callArgs);
    return normalizeValueForKind(out, signature.result || null);
  };

  const callIndirectObjectSignature = (name, { includeIndex = false } = {}) => {
    const match = /^molt_call_indirect(\d+)$/.exec(name);
    if (!match) {
      return null;
    }
    const arity = Number(match[1]);
    if (!Number.isInteger(arity) || arity < 0) {
      return null;
    }
    return {
      params: Array.from({ length: arity + (includeIndex ? 1 : 0) }, () => 'i64'),
      result: 'i64',
    };
  };

  const runtimeImportByteSpanOutNames = new Set([
    'string_from_bytes',
    'molt_string_from_bytes',
    'bytes_from_bytes',
    'molt_bytes_from_bytes',
  ]);

  const runtimeImportObjectArrayArgNames = new Set([
    'call_func_dispatch',
    'molt_call_func_dispatch',
  ]);

  const copyBytes = (bytes) => {
    const source = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const out = new Uint8Array(source.length);
    out.set(source);
    return out;
  };

  const objectArrayByteLength = (countRaw, name) => {
    const count = Number(countRaw);
    if (!Number.isInteger(count) || count < 0) {
      throw new Error(`runtime import ${name} received invalid object count ${String(countRaw)}`);
    }
    const byteLength = count * 8;
    if (!Number.isSafeInteger(byteLength)) {
      throw new Error(`runtime import ${name} object array is too large: ${String(countRaw)}`);
    }
    return byteLength;
  };

  const normalizeRuntimeObjectArrayArgImportArgs = (args, name) => {
    const normalized = [...args];
    for (let idx = 0; idx < Math.min(4, normalized.length); idx += 1) {
      normalized[idx] = normalizeI64BridgeValue(normalized[idx], `${name} arg${idx}`);
    }
    return normalized;
  };

  const callRuntimeByteSpanOutImport = ({
    runtime,
    runtimeMemory,
    appMemory,
    fn,
    args,
    name,
    readBytesFromMemory,
    allocRuntimeTempBytes,
    freeRuntimeTempBytes,
    writeU64ToMemory,
  }) => {
    if (!runtime || !runtimeMemory || !appMemory || appMemory === runtimeMemory) {
      return fn(...args);
    }
    const [ptrRaw, lenRaw, outRaw] = args;
    const len = Number(lenRaw);
    if (!Number.isFinite(len) || len < 0) {
      throw new Error(`runtime import ${name} received invalid byte length ${String(lenRaw)}`);
    }
    const payload = copyBytes(readBytesFromMemory(appMemory, ptrRaw, len));
    const tempBytes = allocRuntimeTempBytes(runtime, runtimeMemory, payload);
    const tempOut = allocRuntimeTempBytes(runtime, runtimeMemory, new Uint8Array(8));
    try {
      const result = fn(
        Number(tempBytes.payloadPtr),
        BigInt(payload.length),
        Number(tempOut.payloadPtr),
      );
      const bits = new DataView(runtimeMemory.buffer).getBigUint64(Number(tempOut.payloadPtr), true);
      writeU64ToMemory(appMemory, outRaw, bits, name);
      return result;
    } finally {
      freeRuntimeTempBytes(runtime, tempBytes);
      freeRuntimeTempBytes(runtime, tempOut);
    }
  };

  const callRuntimeObjectArrayArgImport = ({
    runtime,
    runtimeMemory,
    appMemory,
    fn,
    args,
    name,
    readBytesFromMemory,
    allocRuntimeTempBytes,
    freeRuntimeTempBytes,
  }) => {
    const dispatchArgs = normalizeRuntimeObjectArrayArgImportArgs(args, name);
    if (!runtime || !runtimeMemory || !appMemory || appMemory === runtimeMemory) {
      return fn(...dispatchArgs);
    }
    const byteLength = objectArrayByteLength(dispatchArgs[2] ?? 0, name);
    if (byteLength === 0) {
      return fn(...dispatchArgs);
    }
    const ptr = Number(dispatchArgs[1]);
    if (!Number.isInteger(ptr) || ptr <= 0) {
      throw new Error(`runtime import ${name} received invalid object array pointer ${String(dispatchArgs[1])}`);
    }
    const payload = copyBytes(readBytesFromMemory(appMemory, dispatchArgs[1], byteLength));
    if (payload.length !== byteLength) {
      throw new Error(`runtime import ${name} could not read ${byteLength} object-array bytes`);
    }
    const tempArgs = allocRuntimeTempBytes(runtime, runtimeMemory, payload);
    try {
      const bridgedArgs = [...dispatchArgs];
      bridgedArgs[1] = tempArgs.payloadPtr;
      return fn(...bridgedArgs);
    } finally {
      freeRuntimeTempBytes(runtime, tempArgs);
    }
  };

  const requireMoltDeploymentManifest = manifest => {
    if (!Number.isInteger(callableTableAbi.execution_manifest_version) || callableTableAbi.execution_manifest_version <= 0) {
      throw new Error('Molt host deployment ABI projection is missing; rebuild host assets');
    }
    if (!manifest || typeof manifest !== 'object' || manifest.version !== callableTableAbi.execution_manifest_version) {
      throw new Error(`Molt deployment manifest version must be ${callableTableAbi.execution_manifest_version}; rebuild this deployment with the current Molt compiler`);
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
    if (!Array.isArray(callableTableAbi.execution_manifest_required_environment) ||
        callableTableAbi.execution_manifest_required_environment.length === 0 ||
        callableTableAbi.execution_manifest_required_environment.some(name => !Object.hasOwn(environment, name))) {
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
    CALLABLE_TABLE_SECTION_NAME,
    bindRuntimeFunctionImports,
    callableTableFromModule,
    callableTableSignature,
    callIndirectObjectSignature,
    callIsolateImportExport,
    callRuntimeByteSpanOutImport,
    callRuntimeObjectArrayArgImport,
    callWithWasmSignature,
    installManifestLinkImportTraps,
    installWasmTagImports,
    normalizeI64BridgeValue,
    normalizeValueForKind,
    parseWasmMetadata,
    parseWasmExportFunctionSignatures,
    parseWasmImports,
    readCallableTableAttestation,
    requireWasmCallableTable,
    runtimeExceptionPending,
    runtimeImportByteSpanOutNames,
    runtimeImportObjectArrayArgNames,
    verifyCallableTableEntries,
    verifyCallableTableManifestSummary,
  };
});
