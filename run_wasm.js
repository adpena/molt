const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
const { WASI } = require('wasi');

const wasmArg = process.argv[2];
const wasmEnvPath = process.env.MOLT_WASM_PATH;
const localWasm = path.join(__dirname, 'output.wasm');
const tempWasm = path.join(os.tmpdir(), 'output.wasm');
const wasmPath =
  wasmArg || wasmEnvPath || (fs.existsSync(localWasm) ? localWasm : tempWasm);
const wasmBuffer = fs.readFileSync(wasmPath);
const linkedPath =
  process.env.MOLT_WASM_LINKED_PATH || path.join(__dirname, 'output_linked.wasm');
const linkedBuffer = fs.existsSync(linkedPath) ? fs.readFileSync(linkedPath) : null;
const runtimePath =
  process.env.MOLT_RUNTIME_WASM || path.join(__dirname, 'wasm', 'molt_runtime.wasm');
const runtimeBuffer = fs.readFileSync(runtimePath);
const witPath = path.join(__dirname, 'wit', 'molt-runtime.wit');
const witSource = fs.readFileSync(witPath, 'utf8');

const wasmEnv = { ...process.env };

const ensureWasmLocaleEnv = () => {
  if (
    wasmEnv.MOLT_WASM_LOCALE_DECIMAL ||
    wasmEnv.MOLT_WASM_LOCALE_THOUSANDS ||
    wasmEnv.MOLT_WASM_LOCALE_GROUPING
  ) {
    return;
  }
  let formatter = null;
  try {
    const locale =
      process.env.LC_ALL || process.env.LC_NUMERIC || process.env.LANG || undefined;
    formatter = new Intl.NumberFormat(locale);
  } catch (err) {
    return;
  }
  let decimal = '.';
  let group = '';
  let lastInteger = '';
  const parts = formatter.formatToParts(1234567.89);
  for (const part of parts) {
    if (part.type === 'decimal') {
      decimal = part.value;
    } else if (part.type === 'group') {
      group = part.value;
    } else if (part.type === 'integer') {
      lastInteger = part.value;
    }
  }
  wasmEnv.MOLT_WASM_LOCALE_DECIMAL = decimal;
  if (group) {
    wasmEnv.MOLT_WASM_LOCALE_THOUSANDS = group;
    if (lastInteger) {
      wasmEnv.MOLT_WASM_LOCALE_GROUPING = String(lastInteger.length);
    }
  }
};

ensureWasmLocaleEnv();

const wasi = new WASI({
  version: 'preview1',
  env: wasmEnv,
  preopens: {
    '.': '.',
  },
});

let runtimeInstance = null;
let wasmMemory = null;
const traceImports = process.env.MOLT_WASM_TRACE === '1';
const forceLegacy = process.env.MOLT_WASM_LEGACY === '1';

const setWasmMemory = (mem) => {
  if (mem) wasmMemory = mem;
};

const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_BOOL = 0x0002000000000000n;
const TAG_MASK = 0x0007000000000000n;
const INT_MASK = (1n << 47n) - 1n;

const MAX_DB_FRAME_SIZE = 64 * 1024 * 1024;
const CANCEL_POLL_MS = 10;

let wasmHeaderSize = null;

const getHeaderSize = () => {
  if (wasmHeaderSize === null) {
    if (runtimeInstance && runtimeInstance.exports.molt_header_size) {
      const raw = runtimeInstance.exports.molt_header_size();
      const size = typeof raw === 'bigint' ? Number(raw) : Number(BigInt(raw));
      wasmHeaderSize = Number.isFinite(size) && size > 0 ? size : 40;
    } else {
      wasmHeaderSize = 40;
    }
  }
  return wasmHeaderSize;
};

const boxInt = (value) => {
  let v = BigInt(value);
  if (v < 0n) {
    v = (1n << 47n) + v;
  }
  return QNAN | TAG_INT | (v & INT_MASK);
};

const isBoolBits = (bits) => (bits & (QNAN | TAG_MASK)) === (QNAN | TAG_BOOL);
const unboxBool = (bits) => (bits & 1n) === 1n;

const dbHostUnavailable = (outPtr) => {
  if (!wasmMemory) return 7;
  const addr = Number(outPtr);
  if (!Number.isFinite(addr) || addr === 0) return 2;
  const view = new DataView(wasmMemory.buffer);
  view.setBigUint64(addr, 0n, true);
  return 7;
};

const readBytes = (ptr, len) => {
  if (!wasmMemory || len === 0) return Buffer.alloc(0);
  const addr = Number(ptr);
  if (!Number.isFinite(addr) || addr === 0) return Buffer.alloc(0);
  return Buffer.from(new Uint8Array(wasmMemory.buffer, addr, len));
};

const writeU64 = (addr, value) => {
  const view = new DataView(wasmMemory.buffer);
  view.setBigUint64(addr, BigInt(value), true);
};

const allocTempBytes = (bytes) => {
  if (!runtimeInstance) {
    throw new Error('molt_runtime not initialized');
  }
  const allocBits = runtimeInstance.exports.molt_alloc(BigInt(bytes.length));
  const ptr = runtimeInstance.exports.molt_handle_resolve(allocBits);
  if (!ptr || ptr === 0n) {
    throw new Error('molt_alloc failed');
  }
  const payloadPtr = ptr + BigInt(getHeaderSize());
  new Uint8Array(wasmMemory.buffer, Number(payloadPtr), bytes.length).set(bytes);
  return { allocBits, payloadPtr };
};

const sendStreamFrame = (streamHandle, bytes) => {
  if (!runtimeInstance || !wasmMemory) return false;
  const payload = bytes || Buffer.alloc(0);
  if (payload.length === 0) {
    const res = runtimeInstance.exports.molt_stream_send(
      streamHandle,
      0n,
      0n,
    );
    return res === 0n;
  }
  const temp = allocTempBytes(payload);
  try {
    const res = runtimeInstance.exports.molt_stream_send(
      streamHandle,
      temp.payloadPtr,
      BigInt(payload.length),
    );
    return res === 0n;
  } finally {
    runtimeInstance.exports.molt_dec_ref_obj(temp.allocBits);
  }
};

const encodeMsgpack = (value) => {
  const chunks = [];
  const push = (buf) => {
    chunks.push(Buffer.from(buf));
  };
  const encodeInt = (num) => {
    const n = typeof num === 'bigint' ? num : BigInt(num);
    if (n >= 0n) {
      if (n < 0x80n) {
        push([Number(n)]);
      } else if (n <= 0xffn) {
        push([0xcc, Number(n)]);
      } else if (n <= 0xffffn) {
        const buf = Buffer.alloc(3);
        buf[0] = 0xcd;
        buf.writeUInt16BE(Number(n), 1);
        push(buf);
      } else if (n <= 0xffffffffn) {
        const buf = Buffer.alloc(5);
        buf[0] = 0xce;
        buf.writeUInt32BE(Number(n), 1);
        push(buf);
      } else {
        const buf = Buffer.alloc(9);
        buf[0] = 0xcf;
        buf.writeBigUInt64BE(n, 1);
        push(buf);
      }
      return;
    }
    if (n >= -32n) {
      push([Number(0xe0n + (n + 32n))]);
    } else if (n >= -128n) {
      const buf = Buffer.alloc(2);
      buf[0] = 0xd0;
      buf.writeInt8(Number(n), 1);
      push(buf);
    } else if (n >= -32768n) {
      const buf = Buffer.alloc(3);
      buf[0] = 0xd1;
      buf.writeInt16BE(Number(n), 1);
      push(buf);
    } else if (n >= -2147483648n) {
      const buf = Buffer.alloc(5);
      buf[0] = 0xd2;
      buf.writeInt32BE(Number(n), 1);
      push(buf);
    } else {
      const buf = Buffer.alloc(9);
      buf[0] = 0xd3;
      buf.writeBigInt64BE(n, 1);
      push(buf);
    }
  };
  const encodeString = (str) => {
    const buf = Buffer.from(str, 'utf8');
    const len = buf.length;
    if (len < 32) {
      push([0xa0 | len]);
    } else if (len <= 0xff) {
      push([0xd9, len]);
    } else if (len <= 0xffff) {
      const header = Buffer.alloc(3);
      header[0] = 0xda;
      header.writeUInt16BE(len, 1);
      push(header);
    } else {
      const header = Buffer.alloc(5);
      header[0] = 0xdb;
      header.writeUInt32BE(len, 1);
      push(header);
    }
    push(buf);
  };
  const encodeBin = (buf) => {
    const len = buf.length;
    if (len <= 0xff) {
      push([0xc4, len]);
    } else if (len <= 0xffff) {
      const header = Buffer.alloc(3);
      header[0] = 0xc5;
      header.writeUInt16BE(len, 1);
      push(header);
    } else {
      const header = Buffer.alloc(5);
      header[0] = 0xc6;
      header.writeUInt32BE(len, 1);
      push(header);
    }
    push(buf);
  };
  const encodeArray = (arr) => {
    const len = arr.length;
    if (len < 16) {
      push([0x90 | len]);
    } else if (len <= 0xffff) {
      const header = Buffer.alloc(3);
      header[0] = 0xdc;
      header.writeUInt16BE(len, 1);
      push(header);
    } else {
      const header = Buffer.alloc(5);
      header[0] = 0xdd;
      header.writeUInt32BE(len, 1);
      push(header);
    }
    for (const item of arr) {
      encodeValue(item);
    }
  };
  const encodeMap = (entries) => {
    const len = entries.length;
    if (len < 16) {
      push([0x80 | len]);
    } else if (len <= 0xffff) {
      const header = Buffer.alloc(3);
      header[0] = 0xde;
      header.writeUInt16BE(len, 1);
      push(header);
    } else {
      const header = Buffer.alloc(5);
      header[0] = 0xdf;
      header.writeUInt32BE(len, 1);
      push(header);
    }
    for (const [key, val] of entries) {
      encodeString(String(key));
      encodeValue(val);
    }
  };
  const encodeValue = (val) => {
    if (val === null || val === undefined) {
      push([0xc0]);
      return;
    }
    if (typeof val === 'boolean') {
      push([val ? 0xc3 : 0xc2]);
      return;
    }
    if (typeof val === 'number') {
      if (Number.isInteger(val) && Number.isSafeInteger(val)) {
        encodeInt(BigInt(val));
      } else {
        const buf = Buffer.alloc(9);
        buf[0] = 0xcb;
        buf.writeDoubleBE(val, 1);
        push(buf);
      }
      return;
    }
    if (typeof val === 'bigint') {
      encodeInt(val);
      return;
    }
    if (typeof val === 'string') {
      encodeString(val);
      return;
    }
    if (Buffer.isBuffer(val) || val instanceof Uint8Array) {
      encodeBin(Buffer.from(val));
      return;
    }
    if (Array.isArray(val)) {
      encodeArray(val);
      return;
    }
    if (val instanceof Map) {
      encodeMap(Array.from(val.entries()));
      return;
    }
    if (typeof val === 'object') {
      encodeMap(Object.entries(val));
      return;
    }
    encodeString(String(val));
  };
  encodeValue(value);
  return Buffer.concat(chunks);
};

const sendStreamHeader = (streamHandle, header) =>
  sendStreamFrame(streamHandle, encodeMsgpack(header));

const sendStreamError = (streamHandle, message) => {
  const header = {
    status: 'internal_error',
    codec: 'raw',
    error: message,
  };
  sendStreamHeader(streamHandle, header);
  runtimeInstance.exports.molt_stream_close(streamHandle);
};

const mapWorkerStatus = (status) => {
  switch (status) {
    case 'Ok':
      return 'ok';
    case 'InvalidInput':
      return 'invalid_input';
    case 'Busy':
      return 'busy';
    case 'Timeout':
      return 'timeout';
    case 'Cancelled':
      return 'cancelled';
    case 'InternalError':
      return 'internal_error';
    default:
      return 'internal_error';
  }
};

const decodeWorkerFrame = (frame) => {
  const message = JSON.parse(frame.toString('utf8'));
  const payload = message.payload_b64
    ? Buffer.from(message.payload_b64, 'base64')
    : Buffer.alloc(0);
  return {
    requestId: Number(message.request_id || 0),
    status: message.status || 'InternalError',
    codec: message.codec || 'raw',
    payload,
    error: message.error,
    metrics: message.metrics,
  };
};

const writeFrame = (stream, payload) => {
  const header = Buffer.alloc(4);
  header.writeUInt32LE(payload.length, 0);
  stream.write(header);
  stream.write(payload);
};

const findInPath = (name) => {
  const pathEnv = process.env.PATH || '';
  for (const dir of pathEnv.split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, name);
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
};

const resolveExportsPath = () => {
  const envPath = process.env.MOLT_WASM_DB_EXPORTS || process.env.MOLT_WORKER_EXPORTS;
  if (envPath && fs.existsSync(envPath)) {
    return envPath;
  }
  const packaged = path.join(__dirname, 'src', 'molt_accel', 'default_exports.json');
  if (fs.existsSync(packaged)) {
    return packaged;
  }
  const demo = path.join(__dirname, 'demo', 'molt_worker_app', 'molt_exports.json');
  if (fs.existsSync(demo)) {
    return demo;
  }
  return null;
};

const resolveWorkerCmd = () => {
  const envCmd = process.env.MOLT_WASM_DB_WORKER_CMD || process.env.MOLT_WORKER_CMD;
  if (envCmd) {
    return envCmd.split(/\s+/).filter(Boolean);
  }
  const workerBin = findInPath('molt-worker') || findInPath('molt_worker');
  if (!workerBin) {
    return null;
  }
  const exportsPath = resolveExportsPath();
  if (!exportsPath) {
    return null;
  }
  const cmd = [workerBin, '--stdio', '--exports', exportsPath];
  const compiled = process.env.MOLT_WASM_DB_COMPILED_EXPORTS;
  if (compiled) {
    cmd.push('--compiled-exports', compiled);
  }
  return cmd;
};

const resolveTimeoutMs = () => {
  const raw =
    process.env.MOLT_WASM_DB_TIMEOUT_MS ||
    process.env.MOLT_DB_QUERY_TIMEOUT_MS ||
    '';
  if (raw !== '') {
    const parsed = Number.parseInt(raw, 10);
    if (Number.isFinite(parsed) && parsed >= 0) {
      return parsed;
    }
  }
  return 250;
};

class DbWorkerClient {
  constructor(cmd) {
    this.proc = spawn(cmd[0], cmd.slice(1), {
      stdio: ['pipe', 'pipe', 'inherit'],
      env: process.env,
    });
    this.stdin = this.proc.stdin;
    this.stdout = this.proc.stdout;
    this.buffer = Buffer.alloc(0);
    this.pending = new Map();
    this.nextId = 1;
    this.proc.stdout.on('data', (chunk) => this._onData(chunk));
    this.proc.on('exit', (code, signal) => {
      const reason = code !== null ? `exit ${code}` : `signal ${signal}`;
      this._failAll(`molt-worker ${reason}`);
    });
    this.proc.on('error', (err) => {
      this._failAll(`molt-worker error: ${err.message}`);
    });
  }

  send(entry, payload, timeoutMs, streamHandle, tokenId) {
    const requestId = this.nextId++;
    const pending = {
      requestId,
      streamHandle,
      tokenId: tokenId !== undefined && tokenId !== null ? BigInt(tokenId) : null,
      cancelTimer: null,
      cancelSent: false,
    };
    this.pending.set(requestId, pending);
    const message = {
      request_id: requestId,
      entry,
      timeout_ms: timeoutMs,
      codec: 'msgpack',
      payload_b64: Buffer.from(payload).toString('base64'),
    };
    try {
      writeFrame(this.stdin, Buffer.from(JSON.stringify(message)));
    } catch (err) {
      this.pending.delete(requestId);
      sendStreamError(streamHandle, `db host send failed: ${err.message}`);
      return;
    }
    this._startCancelPoll(pending);
  }

  _startCancelPoll(pending) {
    if (!pending.tokenId || pending.tokenId === 0n) return;
    pending.cancelTimer = setInterval(() => {
      if (pending.cancelSent || !runtimeInstance) return;
      try {
        const tokenBits = boxInt(pending.tokenId);
        const cancelled = runtimeInstance.exports.molt_cancel_token_is_cancelled(tokenBits);
        if (typeof cancelled === 'bigint' && isBoolBits(cancelled) && unboxBool(cancelled)) {
          pending.cancelSent = true;
          clearInterval(pending.cancelTimer);
          this._sendCancel(pending.requestId);
        }
      } catch (err) {
        pending.cancelSent = true;
        clearInterval(pending.cancelTimer);
        this._sendCancel(pending.requestId);
      }
    }, CANCEL_POLL_MS);
  }

  _sendCancel(targetId) {
    const cancelPayload = Buffer.from(JSON.stringify({ request_id: targetId }));
    const message = {
      request_id: this.nextId++,
      entry: '__cancel__',
      timeout_ms: 0,
      codec: 'json',
      payload_b64: cancelPayload.toString('base64'),
    };
    try {
      writeFrame(this.stdin, Buffer.from(JSON.stringify(message)));
    } catch (err) {
      // Best effort.
    }
  }

  _onData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (this.buffer.length >= 4) {
      const size = this.buffer.readUInt32LE(0);
      if (size > MAX_DB_FRAME_SIZE) {
        this._failAll(`worker frame too large: ${size}`);
        return;
      }
      if (this.buffer.length < 4 + size) {
        return;
      }
      const frame = this.buffer.slice(4, 4 + size);
      this.buffer = this.buffer.slice(4 + size);
      this._handleFrame(frame);
    }
  }

  _handleFrame(frame) {
    let response;
    try {
      response = decodeWorkerFrame(frame);
    } catch (err) {
      this._failAll(`worker response decode failed: ${err.message}`);
      return;
    }
    const pending = this.pending.get(response.requestId);
    if (!pending) {
      return;
    }
    this.pending.delete(response.requestId);
    if (pending.cancelTimer) {
      clearInterval(pending.cancelTimer);
    }
    this._deliverResponse(pending.streamHandle, response);
  }

  _deliverResponse(streamHandle, response) {
    const status = mapWorkerStatus(response.status);
    const header = {
      status,
      codec: response.codec || 'raw',
    };
    if (response.metrics && typeof response.metrics === 'object') {
      header.metrics = response.metrics;
    }
    if (status !== 'ok') {
      header.error = response.error || response.status || 'internal error';
      sendStreamHeader(streamHandle, header);
      runtimeInstance.exports.molt_stream_close(streamHandle);
      return;
    }
    if (header.codec === 'arrow_ipc') {
      sendStreamHeader(streamHandle, header);
      if (response.payload && response.payload.length > 0) {
        sendStreamFrame(streamHandle, response.payload);
      }
    } else {
      header.payload = response.payload || Buffer.alloc(0);
      sendStreamHeader(streamHandle, header);
    }
    runtimeInstance.exports.molt_stream_close(streamHandle);
  }

  _failAll(message) {
    for (const pending of this.pending.values()) {
      if (pending.cancelTimer) {
        clearInterval(pending.cancelTimer);
      }
      sendStreamError(pending.streamHandle, message);
    }
    this.pending.clear();
  }
}

let dbWorkerClient = null;

const getDbWorkerClient = () => {
  if (dbWorkerClient) return dbWorkerClient;
  const cmd = resolveWorkerCmd();
  if (!cmd) {
    throw new Error(
      'molt-worker not found; set MOLT_WASM_DB_WORKER_CMD or MOLT_WORKER_CMD',
    );
  }
  dbWorkerClient = new DbWorkerClient(cmd);
  return dbWorkerClient;
};

const handleDbHost = (entry, reqPtr, reqLen, outPtr, tokenId) => {
  if (!wasmMemory || !runtimeInstance) {
    return dbHostUnavailable(outPtr);
  }
  const outAddr = Number(outPtr);
  if (!Number.isFinite(outAddr) || outAddr === 0) {
    return 2;
  }
  const len = Number(reqLen);
  const reqAddr = Number(reqPtr);
  if ((!Number.isFinite(reqAddr) || reqAddr === 0) && len !== 0) {
    return 1;
  }
  let client;
  try {
    client = getDbWorkerClient();
  } catch (err) {
    return dbHostUnavailable(outPtr);
  }
  const streamHandle = runtimeInstance.exports.molt_stream_new(0n);
  if (!streamHandle || streamHandle === 0n) {
    return dbHostUnavailable(outPtr);
  }
  writeU64(outAddr, streamHandle);
  const payload = len > 0 ? readBytes(reqPtr, len) : Buffer.alloc(0);
  const timeoutMs = resolveTimeoutMs();
  try {
    client.send(entry, payload, timeoutMs, streamHandle, tokenId);
  } catch (err) {
    sendStreamError(streamHandle, `db host error: ${err.message}`);
  }
  return 0;
};

const dbQueryHost = (reqPtr, reqLen, outPtr, tokenId) =>
  handleDbHost('db_query', reqPtr, reqLen, outPtr, tokenId);
const dbExecHost = (reqPtr, reqLen, outPtr, tokenId) =>
  handleDbHost('db_exec', reqPtr, reqLen, outPtr, tokenId);

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
    throw new Error(`${wasmPath} missing molt_memory or molt_table export`);
  }
  if (!molt_call_indirect1) {
    throw new Error(`${wasmPath} missing molt_call_indirect1 export`);
  }
  setWasmMemory(memory);

  const runtimeImportsObj = {
    env: {
      memory,
      __indirect_function_table: table,
      molt_call_indirect1,
      molt_db_query_host: dbQueryHost,
      molt_db_exec_host: dbExecHost,
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
  setWasmMemory(memory);

  const env = {
    memory,
    __indirect_function_table: table,
    molt_db_query_host: dbQueryHost,
    molt_db_exec_host: dbExecHost,
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
    throw new Error(`${wasmPath} missing molt_memory or molt_table export`);
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
  if (!importObject.env) {
    importObject.env = {};
  }
  importObject.env.molt_db_query_host = dbQueryHost;
  importObject.env.molt_db_exec_host = dbExecHost;
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
    setWasmMemory(linkedMemory);
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
