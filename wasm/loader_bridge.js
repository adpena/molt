(function installMoltWasmLoaderBridge(root, factory) {
  const api = factory();
  if (typeof module === 'object' && module && module.exports) {
    module.exports = api;
  }
  root.MoltWasmLoaderBridge = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, () => {
  const WASM_MAGIC = 0x6d736100;
  const WASM_VERSION = 0x1;
  const UTF8_DECODER = new TextDecoder('utf-8');
  const BIGINT_SIGNATURE_KINDS = new Set(['i64', 'u64', 's64', 'molt-object']);

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

  const readConstExprI32 = (view, offset) => {
    if (offset >= view.length) {
      throw new Error('Unexpected EOF while reading const expr');
    }
    let pos = offset;
    const opcode = view[pos++];
    let value = null;
    if (opcode === 0x41) {
      const res = readVarInt32(view, pos);
      value = res.value;
      pos = res.offset;
    } else if (opcode === 0x23 || opcode === 0xd2) {
      pos = readVarUint(view, pos).offset;
    } else if (opcode === 0xd0) {
      if (pos >= view.length) {
        throw new Error('Unexpected EOF while reading ref.null expr');
      }
      pos += 1;
    } else {
      throw new Error(`Unsupported const expr opcode ${opcode}`);
    }
    if (pos >= view.length || view[pos] !== 0x0b) {
      throw new Error('Malformed const expr');
    }
    return { value, offset: pos + 1 };
  };

  const skipVec = (view, offset, skipItem) => {
    let count;
    ({ value: count, offset } = readVarUint(view, offset));
    for (let idx = 0; idx < count; idx += 1) {
      offset = skipItem(view, offset);
    }
    return offset;
  };

  const inferWasmTableBaseFromExports = (buffer) => {
    try {
      const mod = new WebAssembly.Module(buffer);
      const refs = WebAssembly.Module.exports(mod)
        .map((entry) => entry.name)
        .filter((name) => name.startsWith('__molt_table_ref_'))
        .map((name) => Number.parseInt(name.slice('__molt_table_ref_'.length), 10))
        .filter((value) => Number.isFinite(value) && value > 0);
      return refs.length > 0 ? Math.min(...refs) : null;
    } catch {
      return null;
    }
  };

  const extractWasmTableBase = (buffer) => {
    if (!buffer) return null;
    const exportedBase = inferWasmTableBaseFromExports(buffer);
    try {
      const bytes = new Uint8Array(buffer);
      if (bytes.length < 8) {
        return null;
      }
      let offset = 8;
      let importFuncCount = 0;
      let tableInitFuncIndex = null;
      let codeBodies = null;
      const activeTableBases = [];
      while (offset < bytes.length) {
        const sectionId = bytes[offset++];
        const sizeRes = readVarUint(bytes, offset);
        const sectionSize = sizeRes.value;
        offset = sizeRes.offset;
        const sectionEnd = offset + sectionSize;
        if (sectionEnd > bytes.length) {
          return null;
        }
        if (sectionId === 2) {
          let count;
          ({ value: count, offset } = readVarUint(bytes, offset));
          for (let idx = 0; idx < count; idx += 1) {
            ({ offset } = readString(bytes, offset));
            ({ offset } = readString(bytes, offset));
            const kind = bytes[offset++];
            if (kind === 0) {
              importFuncCount += 1;
            }
            offset = skipImportDesc(bytes, offset, kind);
          }
        } else if (sectionId === 7) {
          let count;
          ({ value: count, offset } = readVarUint(bytes, offset));
          for (let idx = 0; idx < count; idx += 1) {
            let name;
            ({ value: name, offset } = readString(bytes, offset));
            if (offset >= bytes.length) {
              return null;
            }
            const kind = bytes[offset++];
            let index;
            ({ value: index, offset } = readVarUint(bytes, offset));
            if (kind === 0 && name === 'molt_table_init') {
              tableInitFuncIndex = index;
            }
          }
        } else if (sectionId === 9) {
          let count;
          ({ value: count, offset } = readVarUint(bytes, offset));
          for (let idx = 0; idx < count; idx += 1) {
            let flags;
            ({ value: flags, offset } = readVarUint(bytes, offset));
            const usesExpressions = (flags & 0x04) !== 0;
            const isActive = flags === 0 || flags === 2 || flags === 4 || flags === 6;
            if (flags === 2 || flags === 6) {
              ({ offset } = readVarUint(bytes, offset));
            }
            if (isActive) {
              const expr = readConstExprI32(bytes, offset);
              offset = expr.offset;
              if (flags === 2 || flags === 3) {
                offset += 1;
              } else if (flags === 6 || flags === 7 || flags === 5) {
                offset += 1;
              }
              if (
                Number.isFinite(expr.value) &&
                expr.value > 0 &&
                (!Number.isFinite(exportedBase) || expr.value >= exportedBase)
              ) {
                activeTableBases.push(expr.value);
              }
            } else if (flags === 1 || flags === 3 || flags === 5 || flags === 7) {
              offset += 1;
            } else {
              throw new Error(`Unsupported element segment flags ${flags}`);
            }
            offset = skipVec(
              bytes,
              offset,
              usesExpressions
                ? (view, pos) => readConstExprI32(view, pos).offset
                : (view, pos) => readVarUint(view, pos).offset,
            );
          }
        } else if (sectionId === 10) {
          let count;
          ({ value: count, offset } = readVarUint(bytes, offset));
          const bodies = new Array(count);
          for (let idx = 0; idx < count; idx += 1) {
            let bodySize;
            ({ value: bodySize, offset } = readVarUint(bytes, offset));
            const bodyStart = offset;
            const bodyEnd = bodyStart + bodySize;
            if (bodyEnd > bytes.length) {
              return null;
            }
            bodies[idx] = [bodyStart, bodyEnd];
            offset = bodyEnd;
          }
          codeBodies = bodies;
        } else {
          offset = sectionEnd;
        }
        if (offset !== sectionEnd && sectionId !== 10) {
          offset = sectionEnd;
        }
      }

      let tableInitBase = null;
      if (tableInitFuncIndex !== null && codeBodies) {
        const definedIndex = tableInitFuncIndex - importFuncCount;
        if (definedIndex >= 0 && definedIndex < codeBodies.length) {
          const [bodyStart, bodyEnd] = codeBodies[definedIndex];
          let pos = bodyStart;
          let localDeclCount;
          ({ value: localDeclCount, offset: pos } = readVarUint(bytes, pos));
          for (let idx = 0; idx < localDeclCount; idx += 1) {
            ({ offset: pos } = readVarUint(bytes, pos));
            if (pos >= bodyEnd) {
              break;
            }
            pos += 1;
          }
          if (pos < bodyEnd && bytes[pos] === 0x41) {
            pos += 1;
            tableInitBase = readVarInt32(bytes, pos).value;
          }
        }
      }
      if (Number.isFinite(tableInitBase) && tableInitBase > 0) {
        if (Number.isFinite(exportedBase) && exportedBase > 0 && tableInitBase < exportedBase) {
          return exportedBase;
        }
        return tableInitBase;
      }
      if (activeTableBases.length > 0) {
        const appActiveTableBases = activeTableBases.filter((base) => base > 1);
        if (appActiveTableBases.length > 0) {
          return Math.min(...appActiveTableBases);
        }
        return Math.min(...activeTableBases);
      }
      return exportedBase;
    } catch {
      return exportedBase;
    }
  };

  const decodeWasmValType = (byte) => {
    switch (byte) {
      case 0x7f:
        return 'i32';
      case 0x7e:
        return 'i64';
      case 0x7d:
        return 'f32';
      case 0x7c:
        return 'f64';
      case 0x6f:
        return 'externref';
      case 0x70:
        return 'funcref';
      default:
        throw new Error(`Unsupported wasm value type 0x${byte.toString(16)}`);
    }
  };

  const readWasmValTypeVec = (view, offset) => {
    const countRes = readVarUint(view, offset);
    let count = countRes.value;
    offset = countRes.offset;
    const out = [];
    while (count > 0) {
      if (offset >= view.length) {
        throw new Error('Unexpected EOF in valtype vec');
      }
      out.push(decodeWasmValType(view[offset++]));
      count -= 1;
    }
    return { values: out, offset };
  };

  const parseWasmExportFunctionSignatures = (buffer) => {
    if (!buffer) {
      return {};
    }
    const bytes = new Uint8Array(buffer);
    if (bytes.length < 8) {
      return {};
    }
    let offset = 8;
    const types = [];
    const funcTypeIndices = [];
    let importedFuncCount = 0;
    const exportFuncIndices = new Map();

    while (offset < bytes.length) {
      const sectionId = bytes[offset++];
      const sizeRes = readVarUint(bytes, offset);
      const sectionSize = sizeRes.value;
      offset = sizeRes.offset;
      const sectionEnd = offset + sectionSize;
      if (sectionEnd > bytes.length) {
        throw new Error('Malformed wasm section bounds');
      }
      if (sectionId === 1) {
        let count;
        ({ value: count, offset } = readVarUint(bytes, offset));
        for (let idx = 0; idx < count; idx += 1) {
          if (offset >= bytes.length || bytes[offset++] !== 0x60) {
            throw new Error('Unsupported wasm type form');
          }
          const params = readWasmValTypeVec(bytes, offset);
          offset = params.offset;
          const results = readWasmValTypeVec(bytes, offset);
          offset = results.offset;
          types.push({
            params: params.values,
            result: results.values.length ? results.values[0] : null,
          });
        }
      } else if (sectionId === 2) {
        let count;
        ({ value: count, offset } = readVarUint(bytes, offset));
        for (let idx = 0; idx < count; idx += 1) {
          ({ offset } = readString(bytes, offset));
          ({ offset } = readString(bytes, offset));
          if (offset >= bytes.length) {
            throw new Error('Unexpected EOF in import kind');
          }
          const kind = bytes[offset++];
          if (kind === 0) {
            importedFuncCount += 1;
          }
          offset = skipImportDesc(bytes, offset, kind);
        }
      } else if (sectionId === 3) {
        let count;
        ({ value: count, offset } = readVarUint(bytes, offset));
        for (let idx = 0; idx < count; idx += 1) {
          let typeIdx;
          ({ value: typeIdx, offset } = readVarUint(bytes, offset));
          funcTypeIndices.push(typeIdx);
        }
      } else if (sectionId === 7) {
        let count;
        ({ value: count, offset } = readVarUint(bytes, offset));
        for (let idx = 0; idx < count; idx += 1) {
          let name;
          ({ value: name, offset } = readString(bytes, offset));
          if (offset >= bytes.length) {
            throw new Error('Unexpected EOF in export kind');
          }
          const kind = bytes[offset++];
          let index;
          ({ value: index, offset } = readVarUint(bytes, offset));
          if (kind === 0) {
            exportFuncIndices.set(name, index);
          }
        }
      } else {
        offset = sectionEnd;
      }
    }

    const signatures = {};
    for (const [name, index] of exportFuncIndices.entries()) {
      if (index < importedFuncCount) {
        continue;
      }
      const definedIndex = index - importedFuncCount;
      const typeIndex = funcTypeIndices[definedIndex];
      const sig = types[typeIndex];
      if (sig) {
        signatures[name] = sig;
      }
    }
    return signatures;
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

  const parseWasmImports = (buffer, options = {}) => {
    const view = new Uint8Array(buffer);
    const header = new DataView(view.buffer, view.byteOffset, view.byteLength);
    if (view.length < 8 || header.getUint32(0, true) !== WASM_MAGIC) {
      throw new Error('Invalid WASM header');
    }
    if (header.getUint32(4, true) !== WASM_VERSION) {
      throw new Error('Unsupported WASM version');
    }
    let offset = 8;
    const result = { funcImports: [], tagImports: [], memory: null, table: null };
    const types = [];
    while (offset < view.length) {
      const sectionId = view[offset++];
      const sizeRes = readVarUint(view, offset);
      const size = sizeRes.value;
      offset = sizeRes.offset;
      const end = offset + size;
      if (end > view.length) {
        throw new Error('Unexpected EOF while reading section');
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
          result.funcImports.push({ module, name });
        } else if (kind === 1) {
          inner += 1;
          const limits = readLimits(view, inner);
          inner = limits.offset;
          if (importSelectorMatches(module, name, options.tableImport)) {
            result.table = { min: limits.min, max: limits.max };
          }
        } else if (kind === 2) {
          const limits = readLimits(view, inner);
          inner = limits.offset;
          if (importSelectorMatches(module, name, options.memoryImport)) {
            result.memory = { min: limits.min, max: limits.max };
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
          result.tagImports.push({
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
    return result;
  };

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

  const normalizeValueForKind = (value, kind) => {
    if (BIGINT_SIGNATURE_KINDS.has(kind)) {
      return normalizeI64BridgeValue(value, kind);
    }
    if (kind === 'i32' || kind === 'u32' || kind === 's32') {
      return typeof value === 'bigint' ? Number(value) : Number(value);
    }
    return value;
  };

  const normalizeImportResult = (value, resultKind) => {
    if (BIGINT_SIGNATURE_KINDS.has(resultKind)) {
      return normalizeI64BridgeValue(value, resultKind);
    }
    if (resultKind === 'i32' || resultKind === 'u32' || resultKind === 's32') {
      return typeof value === 'bigint' ? Number(value) : Number(value);
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
    const params = signature.params || signature.argTypes || null;
    if (!Array.isArray(params)) {
      return fn(...args);
    }
    const callArgs = args.map((value, index) =>
      normalizeValueForKind(value, params[index] || null));
    const out = fn(...callArgs);
    return normalizeImportResult(out, signature.result || signature.retType || null);
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

  const reservedRuntimeCallableForTableIndex = (
    idx,
    {
      sharedTableBase,
      reservedRuntimeCallableBase,
      reservedRuntimeCallableCount = null,
      reservedRuntimeCallables,
    },
  ) => {
    const count = reservedRuntimeCallableCount ?? reservedRuntimeCallables.length;
    if (!Number.isInteger(idx) || sharedTableBase === null || sharedTableBase === undefined) {
      return null;
    }
    const directStart = sharedTableBase + reservedRuntimeCallableBase;
    const trampolineStart = directStart + count;
    let offset = idx - directStart;
    let trampoline = false;
    if (offset < 0 || offset >= count) {
      offset = idx - trampolineStart;
      trampoline = true;
    }
    if (offset < 0 || offset >= count) {
      return null;
    }
    const spec = reservedRuntimeCallables.find((entry) => entry.index === offset);
    return spec ? { ...spec, trampoline } : null;
  };

  const reservedRuntimeCallablesFromManifest = (manifest) => {
    const entries = manifest?.abi?.browser_embed?.reserved_runtime_callables;
    if (!Array.isArray(entries)) {
      return null;
    }
    return entries.map((entry, idx) => {
      if (!entry || typeof entry !== 'object') {
        throw new Error(`reserved runtime callable manifest entry ${idx} must be an object`);
      }
      const index = Number(entry.index);
      const runtimeExport = entry.runtime_export;
      const importName = entry.import_name;
      const arity = Number(entry.arity);
      if (!Number.isInteger(index) || index < 0) {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid index`);
      }
      if (typeof runtimeExport !== 'string' || runtimeExport.length === 0) {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid runtime_export`);
      }
      if (typeof importName !== 'string' || importName.length === 0) {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid import_name`);
      }
      if (!Number.isInteger(arity) || arity < 0) {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid arity`);
      }
      return { index, runtimeExport, importName, arity };
    });
  };

  const remapLegacyRuntimeSharedTableIndex = (
    idx,
    {
      sharedTableBase,
      legacyTableBase,
      reservedRuntimeCallableBase,
      reservedRuntimeCallableCount,
    },
  ) => {
    if (
      !Number.isInteger(idx) ||
      sharedTableBase === null ||
      sharedTableBase === undefined ||
      sharedTableBase <= legacyTableBase
    ) {
      return idx;
    }
    const legacyStart = legacyTableBase + reservedRuntimeCallableBase;
    const legacyEnd = legacyStart + reservedRuntimeCallableCount * 2;
    if (idx >= legacyStart && idx < legacyEnd) {
      return idx - legacyTableBase + sharedTableBase;
    }
    return idx;
  };

  const readRuntimeCallargsVector = (memory, ptr, len) => {
    const count = Number(len);
    if (!Number.isInteger(count) || count < 0) {
      throw new Error(`reserved runtime trampoline arg count must be non-negative, got ${len}`);
    }
    const base = Number(ptr);
    if (!Number.isInteger(base) || base < 0) {
      throw new Error(`reserved runtime trampoline argv pointer must be non-negative, got ${ptr}`);
    }
    const view = new DataView(memory.buffer);
    const args = [];
    for (let idx = 0; idx < count; idx += 1) {
      args.push(view.getBigUint64(base + idx * 8, true));
    }
    return args;
  };

  const callReservedRuntimeCallable = ({
    runtimeExports,
    memory,
    entry,
    indirectName,
    args,
  }) => {
    const fn = runtimeExports ? runtimeExports[entry.runtimeExport] : null;
    if (typeof fn !== 'function') {
      throw new Error(`${indirectName} reserved runtime callable ${entry.runtimeExport} is not exported`);
    }
    let callArgs = args;
    if (entry.trampoline) {
      if (args.length !== 3) {
        throw new Error(
          `${indirectName} reserved runtime trampoline ${entry.runtimeExport} expects closure, argv, argc; got ${args.length} args`,
        );
      }
      const closureBits = normalizeI64BridgeValue(args[0], `${indirectName} closure`);
      if (closureBits !== 0n) {
        throw new Error(
          `${indirectName} reserved runtime trampoline ${entry.runtimeExport} does not accept closure bits ${closureBits}`,
        );
      }
      callArgs = readRuntimeCallargsVector(memory, args[1], args[2]);
    }
    if (callArgs.length !== entry.arity) {
      throw new Error(
        `${indirectName} reserved runtime callable ${entry.runtimeExport} arity mismatch: expected ${entry.arity}, got ${callArgs.length}`,
      );
    }
    return callWithWasmSignature(
      fn,
      { params: Array.from({ length: entry.arity }, () => 'i64'), result: 'i64' },
      callArgs,
    );
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

  return {
    callIndirectObjectSignature,
    callIsolateImportExport,
    callReservedRuntimeCallable,
    callRuntimeByteSpanOutImport,
    callRuntimeObjectArrayArgImport,
    callWithWasmSignature,
    extractWasmTableBase,
    installWasmTagImports,
    normalizeI64BridgeValue,
    normalizeImportResult,
    normalizeValueForKind,
    parseWasmExportFunctionSignatures,
    parseWasmImports,
    remapLegacyRuntimeSharedTableIndex,
    reservedRuntimeCallableForTableIndex,
    reservedRuntimeCallablesFromManifest,
    runtimeImportByteSpanOutNames,
    runtimeImportObjectArrayArgNames,
  };
});
