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
      pos = readVarInt32(view, pos).offset;
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

  const extractWasmTableBase = (buffer) => {
    if (!buffer) return null;
    try {
      const bytes = new Uint8Array(buffer);
      if (bytes.length < 8) {
        return null;
      }
      let offset = 8;
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
        if (sectionId === 9) {
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
              if (Number.isFinite(expr.value) && expr.value > 0) {
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
        } else {
          offset = sectionEnd;
        }
        if (offset !== sectionEnd && sectionId !== 10) {
          offset = sectionEnd;
        }
      }

      if (activeTableBases.length > 0) {
        const appActiveTableBases = activeTableBases.filter((base) => base > 1);
        if (appActiveTableBases.length > 0) {
          return Math.min(...appActiveTableBases);
        }
        return Math.min(...activeTableBases);
      }
      return null;
    } catch {
      return null;
    }
  };

  const wasmTableBaseFromManifest = (manifest) => {
    const raw = manifest?.wasm_table_base;
    if (raw === null || raw === undefined) {
      return null;
    }
    const value = Number(raw);
    if (!Number.isInteger(value) || value < 0) {
      throw new Error(`manifest.wasm_table_base must be a non-negative integer, got ${raw}`);
    }
    return value;
  };

  const resolveWasmTableBase = ({ manifest = null, extracted = null }) => {
    const manifestBase = wasmTableBaseFromManifest(manifest);
    if (manifestBase === null) {
      return extracted;
    }
    if (extracted !== null && extracted !== undefined && extracted < manifestBase) {
      throw new Error(
        `manifest wasm_table_base ${manifestBase} is above binary table base ${extracted}`,
      );
    }
    return manifestBase;
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

  const planReservedRuntimeDispatch = ({
    dispatchIdx,
    sharedTableBase,
    reservedRuntimeCallableBase,
    reservedRuntimeCallableCount = null,
    reservedRuntimeCallables,
  }) => {
    const reservedRuntimeCallable = reservedRuntimeCallableForTableIndex(
      dispatchIdx,
      {
        sharedTableBase,
        reservedRuntimeCallableBase,
        reservedRuntimeCallableCount,
        reservedRuntimeCallables,
      },
    );
    if (
      reservedRuntimeCallable &&
      !reservedRuntimeCallable.trampoline &&
      reservedRuntimeCallable.dispatch === 'trampoline'
    ) {
      throw new Error(
        `reserved runtime callable ${reservedRuntimeCallable.runtimeExport} at idx=${dispatchIdx} is trampoline-only`,
      );
    }
    return {
      reservedRuntimeCallable,
      dispatchReservedRuntimeCallable: Boolean(reservedRuntimeCallable),
    };
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
      const dispatch = entry.dispatch === undefined ? 'direct' : entry.dispatch;
      const trampolineAbi = entry.trampoline_abi === undefined ? 'unpack_args' : entry.trampoline_abi;
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
      if (dispatch !== 'direct' && dispatch !== 'trampoline') {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid dispatch`);
      }
      if (trampolineAbi !== 'unpack_args' && trampolineAbi !== 'call_frame') {
        throw new Error(`reserved runtime callable manifest entry ${idx} has invalid trampoline_abi`);
      }
      return { index, runtimeExport, importName, arity, dispatch, trampolineAbi };
    });
  };

  const remapDefaultAppRuntimeSharedTableIndex = (
    idx,
    {
      sharedTableBase,
      defaultAppTableBase,
      reservedRuntimeCallableBase,
      reservedRuntimeCallableCount,
      rawIndexHasInstalledEntry = null,
    },
  ) => {
    if (
      !Number.isInteger(idx) ||
      sharedTableBase === null ||
      sharedTableBase === undefined ||
      sharedTableBase <= defaultAppTableBase
    ) {
      return idx;
    }
    const defaultStart = defaultAppTableBase + reservedRuntimeCallableBase;
    const defaultEnd = defaultStart + reservedRuntimeCallableCount * 2;
    if (idx >= defaultStart && idx < defaultEnd) {
      // A default-base reserved-callable reference is a *bare* index (no installed
      // table entry) baked by an app-layout runtime module, to be relocated
      // into the live shared-table reserved region. App-local function pointers
      // legitimately occupy this same low index window (below the shared-table
      // base), so an index that already resolves to an installed funcref is a
      // genuine indirect call — NOT a default-base reserved reference — and must be
      // dispatched directly. The index range alone is ambiguous because both
      // kinds share [defaultStart, defaultEnd); table occupancy disambiguates.
      // Only relocate unpopulated (bare) references.
      if (
        typeof rawIndexHasInstalledEntry === 'function' &&
        rawIndexHasInstalledEntry(idx)
      ) {
        return idx;
      }
      return idx - defaultAppTableBase + sharedTableBase;
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
    describeArgs = null,
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
      if (entry.trampolineAbi === 'call_frame') {
        callArgs = args;
      } else {
        const closureBits = normalizeI64BridgeValue(args[0], `${indirectName} closure`);
        if (closureBits !== 0n) {
          throw new Error(
            `${indirectName} reserved runtime trampoline ${entry.runtimeExport} does not accept closure bits ${closureBits}`,
          );
        }
        callArgs = readRuntimeCallargsVector(memory, args[1], args[2]);
      }
    }
    if (
      typeof process !== 'undefined' &&
      process &&
      process.env &&
      process.env.MOLT_WASM_CALL_INDIRECT_DEBUG === '1'
    ) {
      const printableArgs = callArgs.map((arg) => String(arg)).join(',');
      const described =
        typeof describeArgs === 'function' ? describeArgs(entry, callArgs) || '' : '';
      console.error(
        `[molt wasm] reserved-runtime ${indirectName} ${entry.runtimeExport}` +
          ` trampoline=${entry.trampoline ? 'yes' : 'no'} argc=${callArgs.length}` +
          ` args=[${printableArgs}]${described}`,
      );
    }
    // Reserved runtime callables are reached through the generic
    // `molt_call_indirectN` fixed-arity lane, whose N is chosen by the
    // *caller's* positional-argument count, not by the callable's true C
    // signature. A type `__new__` slot, for instance, forwards `(cls, *args)`,
    // so `molt_types_capsule_new(cls) -> u64` (declared arity 1) can be invoked
    // as `capsule(cls, x, None, None)` and arrive here with 4 operands. The
    // callable reads only its declared leading params and ignores the surplus —
    // exactly as the native C ABI silently drops extra positional arguments.
    // WASM cannot invoke a 1-param function with 4 operands (a call_indirect
    // type-mismatch trap), so this host bridge is the single point that
    // reconciles the two conventions: forward exactly `entry.arity` leading
    // args. Under-supply (fewer operands than the declared arity) remains a hard
    // error because the missing arguments cannot be fabricated.
    if (callArgs.length < entry.arity) {
      throw new Error(
        `${indirectName} reserved runtime callable ${entry.runtimeExport} arity mismatch: expected ${entry.arity}, got ${callArgs.length}`,
      );
    }
    const forwardedArgs =
      callArgs.length === entry.arity ? callArgs : callArgs.slice(0, entry.arity);
    return callWithWasmSignature(
      fn,
      { params: Array.from({ length: entry.arity }, () => 'i64'), result: 'i64' },
      forwardedArgs,
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
    CALLABLE_TABLE_SECTION_NAME,
    callableTableFromModule,
    callableTableSignature,
    callIndirectObjectSignature,
    callIsolateImportExport,
    callReservedRuntimeCallable,
    callRuntimeByteSpanOutImport,
    callRuntimeObjectArrayArgImport,
    callWithWasmSignature,
    extractWasmTableBase,
    installManifestLinkImportTraps,
    installWasmTagImports,
    normalizeI64BridgeValue,
    normalizeImportResult,
    normalizeValueForKind,
    parseWasmMetadata,
    parseWasmExportFunctionSignatures,
    parseWasmImports,
    requireWasmCallableTable,
    planReservedRuntimeDispatch,
    remapDefaultAppRuntimeSharedTableIndex,
    resolveWasmTableBase,
    reservedRuntimeCallablesFromManifest,
    runtimeImportByteSpanOutNames,
    runtimeImportObjectArrayArgNames,
    verifyCallableTableEntries,
    verifyCallableTableManifestSummary,
  };
});
