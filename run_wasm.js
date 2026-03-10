const fs = require('fs');
const net = require('net');
const dgram = require('dgram');
const dns = require('dns');
const os = require('os');
const path = require('path');
const { spawn } = require('child_process');
let UndiciWebSocket = null;
try {
  ({ WebSocket: UndiciWebSocket } = require('undici'));
} catch {
  UndiciWebSocket = null;
}
let WASI;
try {
  ({ WASI } = require('node:wasi'));
} catch {
  try {
    ({ WASI } = require('wasi'));
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err);
    throw new Error(
      `WASI module unavailable for Node ${process.version}; ` +
        "install Node >= 18 or set MOLT_NODE_BIN to a modern Node binary. " +
        `Original error: ${detail}`
    );
  }
}
const {
  Worker,
  MessageChannel,
  receiveMessageOnPort,
  isMainThread,
  parentPort,
  workerData,
} = require('worker_threads');

const IS_DB_WORKER = !isMainThread && workerData && workerData.kind === 'molt_db_host';
const IS_SOCKET_WORKER = !isMainThread && workerData && workerData.kind === 'molt_socket_host';

let wasmPath = null;
let wasmBuffer = null;
let linkedPath = null;
let linkedBuffer = null;
let runtimePath = null;
let runtimeBuffer = null;
let witSource = null;
let runtimeAssetsLoaded = false;
let wasmEnv = null;
let wasi = null;
let wasiImport = null;
let wasiExitCode = null;
let detectedWasmTableBase = null;
const callIndirectDebug = process.env.MOLT_WASM_CALL_INDIRECT_DEBUG === '1';
const traceExit = process.env.MOLT_WASM_TRACE_EXIT === '1';
const traceRun = process.env.MOLT_WASM_TRACE_RUN === '1';
const traceRunFile = process.env.MOLT_WASM_TRACE_FILE || null;
const trapOnExit = process.env.MOLT_WASM_TRAP_ON_EXIT === '1';
const traceOsClose = process.env.MOLT_WASM_TRACE_OS_CLOSE === '1';
const traceWasiIo = process.env.MOLT_WASM_TRACE_WASI_IO === '1';
const traceWasiIoStack = process.env.MOLT_WASM_TRACE_WASI_IO_STACK === '1';
const traceSocketHost = process.env.MOLT_WASM_TRACE_SOCKET_HOST === '1';
const installTableRefsEnabled = process.env.MOLT_WASM_INSTALL_TABLE_REFS === '1';
const verifyTableRefsEnabled = process.env.MOLT_WASM_VERIFY_TABLE_REFS === '1';
const formatTraceError = (err) => {
  if (err instanceof Error) {
    return err.stack || err.message || String(err);
  }
  if (typeof err === 'symbol') {
    return String(err);
  }
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
};
const traceMark = (message) => {
  if (!traceRunFile) return;
  try {
    fs.appendFileSync(traceRunFile, `${message}\n`);
  } catch {
    // Ignore tracing write errors to keep runtime behavior unchanged.
  }
};
const getWebSocketCtor = () => globalThis.WebSocket || UndiciWebSocket || null;

const ensureWasmLocaleEnv = (env) => {
  if (!env) return;
  if (
    env.MOLT_WASM_LOCALE_DECIMAL ||
    env.MOLT_WASM_LOCALE_THOUSANDS ||
    env.MOLT_WASM_LOCALE_GROUPING
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
  env.MOLT_WASM_LOCALE_DECIMAL = decimal;
  if (group) {
    env.MOLT_WASM_LOCALE_THOUSANDS = group;
    if (lastInteger) {
      env.MOLT_WASM_LOCALE_GROUPING = String(lastInteger.length);
    }
  }
};

const initWasmAssets = () => {
  const wasmArg = process.argv[2];
  const wasmEnvPath = process.env.MOLT_WASM_PATH;
  const explicitWasmPath = wasmArg || wasmEnvPath || null;
  const localWasm = path.join(__dirname, 'output.wasm');
  const tempWasm = path.join(os.tmpdir(), 'output.wasm');
  wasmPath =
    explicitWasmPath || (fs.existsSync(localWasm) ? localWasm : tempWasm);
  if (!wasmPath || !fs.existsSync(wasmPath)) {
    throw new Error('WASM path not found (arg, MOLT_WASM_PATH, ./output.wasm, or temp output.wasm)');
  }
  wasmBuffer = fs.readFileSync(wasmPath);
  const linkedEnvPath = process.env.MOLT_WASM_LINKED_PATH;
  const defaultLinkedPath = path.join(__dirname, 'output_linked.wasm');
  const explicitLinkedPath = (() => {
    if (!explicitWasmPath) return null;
    const parsed = path.parse(explicitWasmPath);
    return parsed.name.endsWith('_linked') ? explicitWasmPath : null;
  })();
  const siblingLinkedPath = (() => {
    if (!wasmPath) return null;
    const parsed = path.parse(wasmPath);
    const ext = parsed.ext || '.wasm';
    const candidate = path.join(parsed.dir, `${parsed.name}_linked${ext}`);
    return fs.existsSync(candidate) ? candidate : null;
  })();
  linkedPath =
    linkedEnvPath ||
    explicitLinkedPath ||
    siblingLinkedPath ||
    (!explicitWasmPath && fs.existsSync(defaultLinkedPath) ? defaultLinkedPath : null);
  linkedBuffer = null;
  if (linkedPath && linkedPath === wasmPath) {
    linkedBuffer = wasmBuffer;
  } else if (linkedPath && fs.existsSync(linkedPath)) {
    linkedBuffer = fs.readFileSync(linkedPath);
  }
  const tableBaseProbe = linkedBuffer || wasmBuffer;
  detectedWasmTableBase = extractWasmTableBase(tableBaseProbe);
  wasmEnv = { ...process.env };
  if (
    detectedWasmTableBase !== null &&
    !Object.prototype.hasOwnProperty.call(wasmEnv, 'MOLT_WASM_TABLE_BASE')
  ) {
    wasmEnv.MOLT_WASM_TABLE_BASE = String(detectedWasmTableBase);
  }
  ensureWasmLocaleEnv(wasmEnv);
  wasi = new WASI({
    version: 'preview1',
    env: wasmEnv,
    preopens: {
      '.': '.',
    },
  });
  wasiExitCode = null;
  wasiImport = { ...wasi.wasiImport };
  const originalProcExit = wasiImport.proc_exit;
  if (typeof originalProcExit === 'function') {
    wasiImport.proc_exit = (code) => {
      let exitCode = 1;
      try {
        exitCode = Number(code);
      } catch {
        exitCode = 1;
      }
      wasiExitCode = exitCode;
      traceMark(`proc_exit:${exitCode}`);
      if (traceExit) {
        const stack = new Error().stack;
        console.error(`[molt wasm] wasi proc_exit(${exitCode}) wasm=${wasmPath}`);
        if (stack) {
          console.error(stack);
        }
      }
      if (trapOnExit) {
        throw new Error(`WASI proc_exit(${exitCode})`);
      }
      return originalProcExit(code);
    };
  }
  if (traceWasiIo) {
    const originalFdClose = wasiImport.fd_close;
    if (typeof originalFdClose === 'function') {
      wasiImport.fd_close = (fd) => {
        const code = originalFdClose(fd);
        traceMark(`wasi_fd_close:fd=${Number(fd)} code=${Number(code)}`);
        if (traceWasiIoStack) {
          const stack = new Error().stack;
          if (stack) {
            traceMark(`wasi_fd_close_stack:${stack.replaceAll('\n', '\\n')}`);
          }
        }
        return code;
      };
    }
    const originalFdWrite = wasiImport.fd_write;
    if (typeof originalFdWrite === 'function') {
      wasiImport.fd_write = (fd, iovsPtr, iovsLen, nwrittenPtr) => {
        const code = originalFdWrite(fd, iovsPtr, iovsLen, nwrittenPtr);
        traceMark(`wasi_fd_write:fd=${Number(fd)} code=${Number(code)}`);
        if (traceWasiIoStack && Number(code) !== 0) {
          const stack = new Error().stack;
          if (stack) {
            traceMark(`wasi_fd_write_stack:${stack.replaceAll('\n', '\\n')}`);
          }
        }
        return code;
      };
    }
  }
};

const loadRuntimeAssets = () => {
  if (runtimeAssetsLoaded) {
    return;
  }
  runtimeAssetsLoaded = true;
  runtimePath =
    process.env.MOLT_RUNTIME_WASM || path.join(__dirname, 'wasm', 'molt_runtime.wasm');
  if (!fs.existsSync(runtimePath)) {
    runtimePath = null;
    runtimeBuffer = null;
    witSource = null;
    return;
  }
  runtimeBuffer = fs.readFileSync(runtimePath);
  const witPath = path.join(__dirname, 'wit', 'molt-runtime.wit');
  if (fs.existsSync(witPath)) {
    witSource = fs.readFileSync(witPath, 'utf8');
  }
};

let runtimeInstance = null;
let wasmMemory = null;
const traceImports = process.env.MOLT_WASM_TRACE === '1';
const isWasiExitSymbol = (err) => typeof err === 'symbol' && String(err) === 'Symbol(kExitCode)';
const runMainWithWasiExit = (fn) => {
  try {
    fn();
  } catch (err) {
    if (isWasiExitSymbol(err)) return;
    throw err;
  }
};

const setWasmMemory = (mem) => {
  if (mem) wasmMemory = mem;
};

const initializeWasiForInstance = (instance, memory) => {
  const exports = instance && instance.exports ? instance.exports : null;
  if (exports && typeof exports._initialize === 'function') {
    wasi.initialize(instance);
    return;
  }
  if (memory) {
    // Fallback for modules that expose memory but no explicit reactor init.
    wasi.initialize({ exports: { memory } });
  }
};

const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_BOOL = 0x0002000000000000n;
const TAG_MASK = 0x0007000000000000n;
const INT_MASK = (1n << 47n) - 1n;

const MAX_DB_FRAME_SIZE = 64 * 1024 * 1024;
const CANCEL_POLL_MS = 10;
const ERRNO = (os.constants && os.constants.errno) || {};
const errnoValue = (name, fallback) =>
  Number.isInteger(ERRNO[name]) ? ERRNO[name] : fallback;
const ENOSYS = errnoValue('ENOSYS', 38);
const EINVAL = errnoValue('EINVAL', 22);
const ENOMEM = errnoValue('ENOMEM', 12);
const EBADF = errnoValue('EBADF', 9);
const ENOENT = errnoValue('ENOENT', 2);
const EPIPE = errnoValue('EPIPE', 32);
const EWOULDBLOCK = errnoValue('EWOULDBLOCK', errnoValue('EAGAIN', 11));
const EINPROGRESS = errnoValue('EINPROGRESS', 115);
const ETIMEDOUT = errnoValue('ETIMEDOUT', 110);
const ENOTCONN = errnoValue('ENOTCONN', 107);
const ENOTSOCK = errnoValue('ENOTSOCK', 88);
const EAFNOSUPPORT = errnoValue('EAFNOSUPPORT', 97);
const EADDRINUSE = errnoValue('EADDRINUSE', 98);
const ECONNRESET = errnoValue('ECONNRESET', 104);
const ECONNREFUSED = errnoValue('ECONNREFUSED', 111);
const ENOPROTOOPT = errnoValue('ENOPROTOOPT', 92);
const EOPNOTSUPP = errnoValue('EOPNOTSUPP', errnoValue('ENOTSUP', 95));
const EISCONN = errnoValue('EISCONN', 106);
const IO_EVENT_READ = 1;
const IO_EVENT_WRITE = 1 << 1;
const IO_EVENT_ERROR = 1 << 2;
const PROCESS_STDIO_INHERIT = 0;
const PROCESS_STDIO_PIPE = 1;
const PROCESS_STDIO_DEVNULL = 2;
const PROCESS_STDIO_STDIN = 0;
const PROCESS_STDIO_STDOUT = 1;
const PROCESS_STDIO_STDERR = 2;
const WS_BUFFER_MAX = Number.parseInt(process.env.MOLT_WASM_WS_BUFFER_MAX || '1048576', 10);

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

const readUtf8 = (ptr, len) => readBytes(ptr, len).toString('utf8');

const writeU64 = (addr, value) => {
  const view = new DataView(wasmMemory.buffer);
  view.setBigUint64(addr, BigInt(value), true);
};

const writeU32 = (addr, value) => {
  const view = new DataView(wasmMemory.buffer);
  view.setUint32(addr, Number(value) >>> 0, true);
};

const writeI32 = (addr, value) => {
  const view = new DataView(wasmMemory.buffer);
  view.setInt32(addr, Number(value) | 0, true);
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

const dbHostWorkerMain = () => {
  if (!parentPort) return;
  const cmd = workerData && Array.isArray(workerData.cmd) ? workerData.cmd : null;
  let responsePort = null;
  let child = null;
  let buffer = Buffer.alloc(0);
  const pendingErrors = [];

  const notifyError = (message) => {
    if (responsePort) {
      responsePort.postMessage({ type: 'error', message });
    } else {
      pendingErrors.push(message);
    }
  };

  const handleFrame = (frame) => {
    let response;
    try {
      response = decodeWorkerFrame(frame);
    } catch (err) {
      notifyError(`worker response decode failed: ${err.message}`);
      return;
    }
    if (responsePort) {
      responsePort.postMessage({ type: 'response', response });
    }
  };

  const handleChunk = (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const size = buffer.readUInt32LE(0);
      if (size > MAX_DB_FRAME_SIZE) {
        notifyError(`worker frame too large: ${size}`);
        buffer = Buffer.alloc(0);
        return;
      }
      if (buffer.length < 4 + size) {
        return;
      }
      const frame = buffer.slice(4, 4 + size);
      buffer = buffer.slice(4 + size);
      handleFrame(frame);
    }
  };

  const dropChild = () => {
    child = null;
    buffer = Buffer.alloc(0);
  };

  const ensureChild = () => {
    if (child) return true;
    if (!cmd || cmd.length === 0) {
      notifyError('molt-worker not configured');
      return false;
    }
    try {
      child = spawn(cmd[0], cmd.slice(1), {
        stdio: ['pipe', 'pipe', 'inherit'],
        env: process.env,
      });
    } catch (err) {
      notifyError(`molt-worker spawn failed: ${err.message}`);
      return false;
    }
    if (!child.stdin || !child.stdout) {
      notifyError('molt-worker missing stdio pipes');
      dropChild();
      return false;
    }
    child.stdout.on('data', handleChunk);
    child.on('exit', (code, signal) => {
      const reason = code !== null ? `exit ${code}` : `signal ${signal}`;
      notifyError(`molt-worker ${reason}`);
      dropChild();
    });
    child.on('error', (err) => {
      notifyError(`molt-worker error: ${err.message}`);
      dropChild();
    });
    return true;
  };

  const sendRequest = (msg) => {
    if (!ensureChild()) return;
    const payload_b64 =
      msg.payload_b64 || (Buffer.isBuffer(msg.payload) ? msg.payload.toString('base64') : '');
    const message = {
      request_id: msg.requestId,
      entry: msg.entry,
      timeout_ms: msg.timeoutMs,
      codec: 'msgpack',
      payload_b64,
    };
    try {
      writeFrame(child.stdin, Buffer.from(JSON.stringify(message)));
    } catch (err) {
      notifyError(`db host send failed: ${err.message}`);
    }
  };

  const sendCancel = (targetId) => {
    if (!ensureChild()) return;
    const cancelPayload = Buffer.from(JSON.stringify({ request_id: targetId }));
    const message = {
      request_id: 0,
      entry: '__cancel__',
      timeout_ms: 0,
      codec: 'json',
      payload_b64: cancelPayload.toString('base64'),
    };
    try {
      writeFrame(child.stdin, Buffer.from(JSON.stringify(message)));
    } catch (err) {
      // Best effort.
    }
  };

  parentPort.on('message', (msg) => {
    if (!msg || typeof msg !== 'object') return;
    if (msg.type === 'init' && msg.port) {
      responsePort = msg.port;
      if (pendingErrors.length) {
        for (const message of pendingErrors.splice(0)) {
          responsePort.postMessage({ type: 'error', message });
        }
      }
      return;
    }
    if (msg.type === 'request') {
      sendRequest(msg);
      return;
    }
    if (msg.type === 'cancel') {
      sendCancel(msg.targetId);
    }
  });
};

if (IS_DB_WORKER) {
  dbHostWorkerMain();
}

class DbWorkerClient {
  constructor(cmd) {
    this.worker = new Worker(__filename, { workerData: { kind: 'molt_db_host', cmd } });
    const channel = new MessageChannel();
    this.port = channel.port1;
    this.pending = new Map();
    this.nextId = 1;
    this.lastCancelCheck = 0;
    this.dead = false;
    this.worker.postMessage({ type: 'init', port: channel.port2 }, [channel.port2]);
    this.worker.on('error', (err) => {
      this.dead = true;
      this._failAll(`molt-worker error: ${err.message}`);
    });
    this.worker.on('exit', (code, signal) => {
      const reason = code !== null ? `exit ${code}` : `signal ${signal}`;
      this.dead = true;
      this._failAll(`molt-worker ${reason}`);
    });
  }

  close() {
    if (this.dead && !this.worker) {
      return Promise.resolve();
    }
    this.dead = true;
    this._failAll('db host worker shutting down');
    if (this.port) {
      try {
        this.port.close();
      } catch (err) {
        // Best effort.
      }
      this.port = null;
    }
    const worker = this.worker;
    this.worker = null;
    if (!worker || typeof worker.terminate !== 'function') {
      return Promise.resolve();
    }
    try {
      return Promise.resolve(worker.terminate()).catch(() => {});
    } catch (err) {
      return Promise.resolve();
    }
  }

  send(entry, payload, timeoutMs, streamHandle, tokenId) {
    if (this.dead || !this.worker) {
      throw new Error('db host worker unavailable');
    }
    const requestId = this.nextId++;
    const pending = {
      requestId,
      streamHandle,
      tokenId: tokenId !== undefined && tokenId !== null ? BigInt(tokenId) : 0n,
      cancelSent: false,
    };
    this.pending.set(requestId, pending);
    const message = {
      type: 'request',
      requestId,
      entry,
      timeoutMs,
      payload_b64: Buffer.from(payload).toString('base64'),
    };
    try {
      this.worker.postMessage(message);
    } catch (err) {
      this.pending.delete(requestId);
      sendStreamError(streamHandle, `db host send failed: ${err.message}`);
    }
  }

  poll() {
    this._drainResponses();
    this._pollCancels();
  }

  _drainResponses() {
    while (true) {
      const result = receiveMessageOnPort(this.port);
      if (!result) break;
      const msg = result.message;
      if (!msg || typeof msg !== 'object') continue;
      if (msg.type === 'error') {
        this._failAll(msg.message || 'db host error');
        continue;
      }
      if (msg.type !== 'response') {
        continue;
      }
      const response = msg.response;
      if (!response) {
        continue;
      }
      const pending = this.pending.get(response.requestId);
      if (!pending) {
        continue;
      }
      this.pending.delete(response.requestId);
      this._deliverResponse(pending.streamHandle, response);
    }
  }

  _pollCancels() {
    if (!runtimeInstance || !runtimeInstance.exports.molt_cancel_token_is_cancelled) {
      return;
    }
    const now = Date.now();
    if (now - this.lastCancelCheck < CANCEL_POLL_MS) {
      return;
    }
    this.lastCancelCheck = now;
    for (const pending of this.pending.values()) {
      if (!pending.tokenId || pending.tokenId === 0n || pending.cancelSent) {
        continue;
      }
      let cancelled = false;
      try {
        const tokenBits = boxInt(pending.tokenId);
        const result = runtimeInstance.exports.molt_cancel_token_is_cancelled(tokenBits);
        if (typeof result === 'bigint' && isBoolBits(result) && unboxBool(result)) {
          cancelled = true;
        }
      } catch (err) {
        cancelled = true;
      }
      if (cancelled) {
        pending.cancelSent = true;
        this._sendCancel(pending.requestId);
      }
    }
  }

  _sendCancel(targetId) {
    try {
      this.worker.postMessage({ type: 'cancel', targetId });
    } catch (err) {
      // Best effort.
    }
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
      sendStreamError(pending.streamHandle, message);
    }
    this.pending.clear();
  }
}

let dbWorkerClient = null;

const getDbWorkerClient = () => {
  if (dbWorkerClient && !dbWorkerClient.dead) return dbWorkerClient;
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
const dbHostPoll = () => {
  if (!dbWorkerClient) return 0;
  try {
    dbWorkerClient.poll();
  } catch (err) {
    dbWorkerClient._failAll(`db host poll failed: ${err.message}`);
  }
  return 0;
};

const SOCKET_SAB_HEADER_SIZE = 32;
const SOCKET_SAB_DATA_OFFSET = SOCKET_SAB_HEADER_SIZE;
const SOCKET_RPC_TIMEOUT_MS = 30000;
const AF_INET = 2;
const AF_INET6 = 10;
const AF_UNIX = 1;
const SOCK_STREAM = 1;
const SOCK_DGRAM = 2;
const SOL_SOCKET = 1;
const SO_SNDBUF = 7;
const SO_RCVBUF = 8;
const SO_BROADCAST = 6;
const SO_LINGER = 13;
const SO_REUSEADDR = 2;
const SO_REUSEPORT = 15;
const SO_ERROR = 4;
const SO_KEEPALIVE = 9;
const IPPROTO_TCP = 6;
const IPPROTO_UDP = 17;
const TCP_NODELAY = 1;
const SHUT_RD = 0;
const SHUT_WR = 1;
const MSG_PEEK = 2;
const MSG_TRUNC = 0x20;
const I64_MIN = -(1n << 63n);

const writeBytes = (ptr, bytes) => {
  if (!wasmMemory) return false;
  const addr = Number(ptr);
  if (!Number.isFinite(addr) || addr === 0) return false;
  new Uint8Array(wasmMemory.buffer, addr, bytes.length).set(bytes);
  return true;
};

const ipv6ToString = (bytes) => {
  const parts = [];
  for (let i = 0; i < 16; i += 2) {
    parts.push(((bytes[i] << 8) | bytes[i + 1]).toString(16));
  }
  let bestStart = -1;
  let bestLen = 0;
  let curStart = -1;
  let curLen = 0;
  for (let i = 0; i <= parts.length; i += 1) {
    if (i < parts.length && parts[i] === '0') {
      if (curStart === -1) curStart = i;
      curLen += 1;
    } else {
      if (curLen > bestLen) {
        bestLen = curLen;
        bestStart = curStart;
      }
      curStart = -1;
      curLen = 0;
    }
  }
  if (bestLen > 1) {
    parts.splice(bestStart, bestLen, '');
    if (bestStart === 0) parts.unshift('');
    if (bestStart + bestLen === 8) parts.push('');
  }
  return parts.join(':').replace(/:{3,}/, '::');
};

const parseIPv4 = (text) => {
  const parts = text.split('.');
  if (parts.length !== 4) return null;
  const bytes = new Uint8Array(4);
  for (let i = 0; i < 4; i += 1) {
    const val = Number.parseInt(parts[i], 10);
    if (!Number.isFinite(val) || val < 0 || val > 255) return null;
    bytes[i] = val;
  }
  return bytes;
};

const parseIPv6 = (text) => {
  const zoneIndex = text.indexOf('%');
  let zone = null;
  if (zoneIndex >= 0) {
    zone = text.slice(zoneIndex + 1);
    text = text.slice(0, zoneIndex);
  }
  if (text === '::') {
    return { bytes: new Uint8Array(16), scopeId: zone ? Number.parseInt(zone, 10) || 0 : 0 };
  }
  const parts = text.split('::');
  if (parts.length > 2) return null;
  const head = parts[0] ? parts[0].split(':').filter(Boolean) : [];
  const tail = parts[1] ? parts[1].split(':').filter(Boolean) : [];
  let hasV4 = false;
  if (tail.length && tail[tail.length - 1].includes('.')) {
    const v4 = parseIPv4(tail[tail.length - 1]);
    if (!v4) return null;
    tail.pop();
    tail.push(((v4[0] << 8) | v4[1]).toString(16));
    tail.push(((v4[2] << 8) | v4[3]).toString(16));
    hasV4 = true;
  } else if (head.length && head[head.length - 1].includes('.')) {
    const v4 = parseIPv4(head[head.length - 1]);
    if (!v4) return null;
    head.pop();
    head.push(((v4[0] << 8) | v4[1]).toString(16));
    head.push(((v4[2] << 8) | v4[3]).toString(16));
    hasV4 = true;
  }
  const total = head.length + tail.length;
  const missing = 8 - total;
  if (missing < 0) return null;
  const groups = [...head, ...Array(missing).fill('0'), ...tail];
  if (groups.length !== 8) return null;
  const bytes = new Uint8Array(16);
  for (let i = 0; i < 8; i += 1) {
    const val = Number.parseInt(groups[i], 16);
    if (!Number.isFinite(val) || val < 0 || val > 0xffff) return null;
    bytes[i * 2] = (val >> 8) & 0xff;
    bytes[i * 2 + 1] = val & 0xff;
  }
  return { bytes, scopeId: zone ? Number.parseInt(zone, 10) || 0 : 0, hasV4 };
};

const decodeSockaddr = (value) => {
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(value || []);
  if (!bytes || bytes.length < 4) {
    throw new Error('invalid sockaddr');
  }
  const family = bytes.readUInt16LE(0);
  const port = bytes.readUInt16LE(2);
  if (family === AF_INET) {
    if (bytes.length < 8) {
      throw new Error('invalid IPv4 sockaddr');
    }
    const host = `${bytes[4]}.${bytes[5]}.${bytes[6]}.${bytes[7]}`;
    return { family, host, port };
  }
  if (family === AF_INET6) {
    if (bytes.length < 28) {
      throw new Error('invalid IPv6 sockaddr');
    }
    const flowinfo = bytes.readUInt32LE(4);
    const scopeId = bytes.readUInt32LE(8);
    const host = ipv6ToString(bytes.subarray(12, 28));
    return { family, host, port, flowinfo, scopeId };
  }
  if (family === AF_UNIX) {
    throw new Error('AF_UNIX unsupported');
  }
  throw new Error('unsupported address family');
};

const encodeSockaddr = (addr) => {
  if (!addr) return Buffer.alloc(0);
  const family = addr.family || 0;
  const port = addr.port || 0;
  if (family === AF_INET) {
    const bytes = Buffer.alloc(8);
    bytes.writeUInt16LE(AF_INET, 0);
    bytes.writeUInt16LE(port, 2);
    const ip = parseIPv4(addr.host || addr.address || '0.0.0.0');
    if (!ip) {
      throw new Error('invalid IPv4 address');
    }
    bytes.set(ip, 4);
    return bytes;
  }
  if (family === AF_INET6) {
    const bytes = Buffer.alloc(28);
    bytes.writeUInt16LE(AF_INET6, 0);
    bytes.writeUInt16LE(port, 2);
    bytes.writeUInt32LE(addr.flowinfo || 0, 4);
    bytes.writeUInt32LE(addr.scopeId || addr.scopeid || 0, 8);
    const parsed = parseIPv6(addr.host || addr.address || '::');
    if (!parsed) {
      throw new Error('invalid IPv6 address');
    }
    bytes.set(parsed.bytes, 12);
    return bytes;
  }
  throw new Error('unsupported address family');
};

const EMPTY_ANCILLARY_PAYLOAD = (() => {
  const out = Buffer.alloc(4);
  out.writeUInt32LE(0, 0);
  return out;
})();

const decodeAncillaryPayload = (payload) => {
  const bytes = Buffer.isBuffer(payload) ? payload : Buffer.from(payload || []);
  if (bytes.length < 4) {
    throw new Error('ancillary payload too short');
  }
  const count = bytes.readUInt32LE(0);
  let offset = 4;
  const out = [];
  for (let i = 0; i < count; i += 1) {
    if (offset + 12 > bytes.length) {
      throw new Error('ancillary payload truncated');
    }
    const level = bytes.readInt32LE(offset);
    offset += 4;
    const kind = bytes.readInt32LE(offset);
    offset += 4;
    const dataLen = bytes.readUInt32LE(offset);
    offset += 4;
    const end = offset + dataLen;
    if (end > bytes.length) {
      throw new Error('ancillary payload truncated');
    }
    out.push({ level, kind, data: bytes.subarray(offset, end) });
    offset = end;
  }
  if (offset !== bytes.length) {
    throw new Error('ancillary payload trailing bytes');
  }
  return out;
};

const encodeAncillaryPayload = (items) => {
  const list = Array.isArray(items) ? items : [];
  let total = 4;
  for (const item of list) {
    const data = Buffer.isBuffer(item.data) ? item.data : Buffer.from(item.data || []);
    total += 12 + data.length;
  }
  const out = Buffer.alloc(total);
  out.writeUInt32LE(list.length, 0);
  let offset = 4;
  for (const item of list) {
    const data = Buffer.isBuffer(item.data) ? item.data : Buffer.from(item.data || []);
    out.writeInt32LE(item.level | 0, offset);
    offset += 4;
    out.writeInt32LE(item.kind | 0, offset);
    offset += 4;
    out.writeUInt32LE(data.length, offset);
    offset += 4;
    data.copy(out, offset);
    offset += data.length;
  }
  return out;
};

const encodeRecvmsgExtra = (addrBytes, ancillaryPayload) => {
  const addr = Buffer.isBuffer(addrBytes) ? addrBytes : Buffer.from(addrBytes || []);
  const anc = Buffer.isBuffer(ancillaryPayload)
    ? ancillaryPayload
    : Buffer.from(ancillaryPayload || []);
  const out = Buffer.alloc(8 + addr.length + anc.length);
  out.writeUInt32LE(addr.length, 0);
  addr.copy(out, 4);
  out.writeUInt32LE(anc.length, 4 + addr.length);
  anc.copy(out, 8 + addr.length);
  return out;
};

const decodeRecvmsgExtra = (extra) => {
  const bytes = Buffer.isBuffer(extra) ? extra : Buffer.from(extra || []);
  if (bytes.length < 8) {
    throw new Error('recvmsg payload too short');
  }
  const addrLen = bytes.readUInt32LE(0);
  const addrStart = 4;
  const addrEnd = addrStart + addrLen;
  if (addrEnd + 4 > bytes.length) {
    throw new Error('recvmsg payload truncated');
  }
  const ancLen = bytes.readUInt32LE(addrEnd);
  const ancStart = addrEnd + 4;
  const ancEnd = ancStart + ancLen;
  if (ancEnd > bytes.length) {
    throw new Error('recvmsg payload truncated');
  }
  if (ancEnd !== bytes.length) {
    throw new Error('recvmsg payload trailing bytes');
  }
  return {
    addr: bytes.subarray(addrStart, addrEnd),
    ancillary: bytes.subarray(ancStart, ancEnd),
  };
};

const mapSocketError = (err) => {
  if (!err) return EINVAL;
  if (typeof err.errno === 'number') {
    return Math.abs(err.errno) || EINVAL;
  }
  if (err.code && ERRNO[err.code]) {
    return ERRNO[err.code];
  }
  switch (err.code) {
    case 'EAGAIN':
    case 'EWOULDBLOCK':
      return EWOULDBLOCK;
    case 'EINPROGRESS':
      return EINPROGRESS;
    case 'ECONNRESET':
      return ECONNRESET;
    case 'ECONNREFUSED':
      return ECONNREFUSED;
    case 'ETIMEDOUT':
      return ETIMEDOUT;
    case 'ENOTCONN':
      return ENOTCONN;
    case 'EISCONN':
      return EISCONN;
    case 'ENOTSUP':
    case 'EOPNOTSUPP':
      return EOPNOTSUPP;
    case 'EPIPE':
      return EPIPE;
    default:
      return EINVAL;
  }
};

const writeHandleParts = (view, index, value) => {
  const v = BigInt(value);
  const lo = Number(v & 0xffffffffn);
  const hi = Number((v >> 32n) & 0xffffffffn);
  view[index] = lo | 0;
  view[index + 1] = hi | 0;
};

const readHandleParts = (view, index) => {
  const lo = BigInt(view[index] >>> 0);
  const hi = BigInt(view[index + 1] >>> 0);
  let value = (hi << 32n) | lo;
  if (value & 0x8000000000000000n) {
    value -= 1n << 64n;
  }
  return value;
};

const socketWorkerMain = () => {
  if (!parentPort) return;
  let nextHandle = 1n;
  const sockets = new Map();
  const detached = new Map();
  let serviceCache = null;

  const allocHandle = (entry) => {
    const handle = nextHandle;
    nextHandle += 1n;
    sockets.set(handle.toString(), entry);
    return handle;
  };

  const getEntry = (handle) => sockets.get(handle.toString()) || null;

  const removeEntry = (handle) => {
    const key = handle.toString();
    const entry = sockets.get(key);
    if (!entry) return null;
    sockets.delete(key);
    return entry;
  };

  const removeDetached = (handle) => {
    const key = handle.toString();
    const entry = detached.get(key);
    if (!entry) return null;
    detached.delete(key);
    return entry;
  };

  const loadServiceCache = () => {
    if (serviceCache) return serviceCache;
    const byName = new Map();
    const byPort = new Map();
    const paths = [];
    if (process.platform === 'win32') {
      const root = process.env.SystemRoot || 'C:\\\\Windows';
      paths.push(path.join(root, 'System32', 'drivers', 'etc', 'services'));
    } else {
      paths.push('/etc/services');
    }
    for (const filePath of paths) {
      try {
        const raw = fs.readFileSync(filePath, 'utf8');
        for (const line of raw.split(/\r?\n/)) {
          const trimmed = line.trim();
          if (!trimmed || trimmed.startsWith('#')) continue;
          const parts = trimmed.split(/\s+/);
          if (parts.length < 2) continue;
          const name = parts[0].toLowerCase();
          const portProto = parts[1].split('/');
          if (portProto.length !== 2) continue;
          const port = Number.parseInt(portProto[0], 10);
          if (!Number.isFinite(port)) continue;
          const proto = portProto[1].toLowerCase();
          const aliases = parts.slice(2).map((alias) => alias.toLowerCase());
          const allNames = [name, ...aliases];
          for (const svc of allNames) {
            if (!byName.has(svc)) byName.set(svc, new Map());
            const protoMap = byName.get(svc);
            if (!protoMap.has(proto)) {
              protoMap.set(proto, port);
            }
            if (!protoMap.has('')) {
              protoMap.set('', port);
            }
          }
          const portKey = `${port}`;
          if (!byPort.has(portKey)) byPort.set(portKey, new Map());
          const portMap = byPort.get(portKey);
          if (!portMap.has(proto)) {
            portMap.set(proto, name);
          }
          if (!portMap.has('')) {
            portMap.set('', name);
          }
        }
      } catch (err) {
        // Best-effort only.
      }
    }
    serviceCache = { byName, byPort };
    return serviceCache;
  };

  const lookupServicePort = (name, proto) => {
    const cache = loadServiceCache();
    const key = name.toLowerCase();
    const protoKey = (proto || '').toLowerCase();
    const entry = cache.byName.get(key);
    if (!entry) return null;
    if (protoKey && entry.has(protoKey)) return entry.get(protoKey);
    if (entry.has('')) return entry.get('');
    return null;
  };

  const lookupServiceName = (port, proto) => {
    const cache = loadServiceCache();
    const portKey = `${port}`;
    const protoKey = (proto || '').toLowerCase();
    const entry = cache.byPort.get(portKey);
    if (!entry) return null;
    if (protoKey && entry.has(protoKey)) return entry.get(protoKey);
    if (entry.has('')) return entry.get('');
    return null;
  };

  const notifyWaiters = (entry) => {
    if (!entry.waiters.length) return;
    const readyMask = pollReady(entry, IO_EVENT_READ | IO_EVENT_WRITE | IO_EVENT_ERROR);
    if (readyMask === 0) return;
    const waiters = entry.waiters.splice(0);
    for (const waiter of waiters) {
      if (waiter.timer) clearTimeout(waiter.timer);
      waiter.resolve(readyMask);
    }
  };

  const drainWaiters = (entry, readyMask = IO_EVENT_ERROR) => {
    if (!entry.waiters.length) return;
    const waiters = entry.waiters.splice(0);
    for (const waiter of waiters) {
      if (waiter.timer) clearTimeout(waiter.timer);
      waiter.resolve(readyMask);
    }
  };

  const pollReady = (entry, events) => {
    let ready = 0;
    if (entry.error) {
      ready |= IO_EVENT_ERROR;
    }
    if ((events & IO_EVENT_READ) !== 0) {
      if (entry.kind === 'server') {
        if (entry.acceptQueue.length) ready |= IO_EVENT_READ;
      } else if (entry.kind === 'udp') {
        if (entry.recvQueue.length || entry.ended) ready |= IO_EVENT_READ;
      } else if (entry.readQueue.length || entry.ended) {
        ready |= IO_EVENT_READ;
      }
    }
    if ((events & IO_EVENT_WRITE) !== 0) {
      if (entry.kind === 'udp') {
        if (!entry.closed) ready |= IO_EVENT_WRITE;
      } else if (entry.kind === 'tcp') {
        if (entry.connected && !entry.backpressure) ready |= IO_EVENT_WRITE;
        if (entry.connectError) ready |= IO_EVENT_ERROR;
      } else if (entry.kind === 'server' && entry.listening) {
        ready |= IO_EVENT_WRITE;
      }
    }
    return ready;
  };

  const attachTcpHandlers = (entry) => {
    const socket = entry.socket;
    socket.on('data', (chunk) => {
      if (entry.closed) return;
      entry.readQueue.push(Buffer.from(chunk));
      notifyWaiters(entry);
    });
    socket.on('end', () => {
      entry.ended = true;
      notifyWaiters(entry);
    });
    socket.on('close', () => {
      entry.closed = true;
      notifyWaiters(entry);
    });
    socket.on('error', (err) => {
      entry.error = mapSocketError(err);
      entry.connectError = entry.error;
      notifyWaiters(entry);
    });
    socket.on('connect', () => {
      entry.connected = true;
      entry.connectPending = false;
      notifyWaiters(entry);
    });
    socket.on('drain', () => {
      entry.backpressure = false;
      notifyWaiters(entry);
    });
  };

  const attachUdpHandlers = (entry) => {
    const socket = entry.socket;
    socket.on('message', (msg, rinfo) => {
      entry.recvQueue.push({
        data: Buffer.from(msg),
        addr: {
          family: rinfo.family === 'IPv6' ? AF_INET6 : AF_INET,
          host: rinfo.address,
          port: rinfo.port,
          scopeId: rinfo.scopeid || 0,
        },
      });
      notifyWaiters(entry);
    });
    socket.on('listening', () => {
      entry.listening = true;
      notifyWaiters(entry);
    });
    socket.on('close', () => {
      entry.closed = true;
      notifyWaiters(entry);
    });
    socket.on('error', (err) => {
      entry.error = mapSocketError(err);
      notifyWaiters(entry);
    });
  };

  const attachServerHandlers = (entry) => {
    const server = entry.server;
    server.on('connection', (sock) => {
      const child = {
        kind: 'tcp',
        socket: sock,
        refCount: 1,
        readQueue: [],
        recvQueue: [],
        acceptQueue: [],
        waiters: [],
        ended: false,
        closed: false,
        error: null,
        backpressure: false,
        connected: true,
        connectPending: false,
        connectError: null,
        listening: true,
        ancillarySegments: [],
        ancillaryPeer: null,
      };
      attachTcpHandlers(child);
      const handle = allocHandle(child);
      let addrBytes = Buffer.alloc(0);
      try {
        addrBytes = encodeSockaddr({
          family: sock.remoteFamily === 'IPv6' ? AF_INET6 : AF_INET,
          host: sock.remoteAddress,
          port: sock.remotePort || 0,
          scopeId: sock.remoteScopeid || 0,
        });
      } catch (err) {
        addrBytes = Buffer.alloc(0);
      }
      entry.acceptQueue.push({ handle, addr: addrBytes });
      notifyWaiters(entry);
    });
    server.on('listening', () => {
      entry.listening = true;
      notifyWaiters(entry);
    });
    server.on('close', () => {
      entry.closed = true;
      notifyWaiters(entry);
    });
    server.on('error', (err) => {
      entry.error = mapSocketError(err);
      notifyWaiters(entry);
    });
  };

  const readFromQueue = (queue, size, peek) => {
    if (!queue.length) return Buffer.alloc(0);
    let remaining = size;
    const chunks = [];
    if (peek) {
      for (const chunk of queue) {
        if (remaining <= 0) break;
        if (chunk.length <= remaining) {
          chunks.push(chunk);
          remaining -= chunk.length;
        } else {
          chunks.push(chunk.subarray(0, remaining));
          remaining = 0;
        }
      }
      return Buffer.concat(chunks);
    }
    const newQueue = [];
    for (const chunk of queue) {
      if (remaining <= 0) {
        newQueue.push(chunk);
        continue;
      }
      if (chunk.length <= remaining) {
        chunks.push(chunk);
        remaining -= chunk.length;
      } else {
        chunks.push(chunk.subarray(0, remaining));
        newQueue.push(chunk.subarray(remaining));
        remaining = 0;
      }
    }
    queue.splice(0, queue.length, ...newQueue);
    return Buffer.concat(chunks);
  };

  const cloneAncillaryItems = (items) =>
    (Array.isArray(items) ? items : []).map((item) => ({
      level: item.level | 0,
      kind: item.kind | 0,
      data: Buffer.isBuffer(item.data) ? Buffer.from(item.data) : Buffer.from(item.data || []),
    }));

  const appendStreamAncillarySegment = (entry, bytes, items) => {
    const n = Number(bytes);
    if (!Number.isFinite(n) || n <= 0) return false;
    if (!Array.isArray(entry.ancillarySegments)) {
      entry.ancillarySegments = [];
    }
    const normalized = cloneAncillaryItems(items);
    const last = entry.ancillarySegments[entry.ancillarySegments.length - 1];
    if (
      last &&
      !last.delivered &&
      last.items.length === 0 &&
      normalized.length === 0
    ) {
      last.remaining += n;
      return true;
    }
    entry.ancillarySegments.push({
      remaining: n,
      items: normalized,
      delivered: false,
    });
    return true;
  };

  const recordPeerStreamWrite = (entry, bytes, ancillaryItems) => {
    const normalized = cloneAncillaryItems(ancillaryItems);
    if (normalized.length > 0) {
      if (!Number.isFinite(bytes) || Number(bytes) <= 0) {
        return false;
      }
      if (!entry.ancillaryPeer || entry.ancillaryPeer.closed) {
        return false;
      }
    }
    const peer = entry.ancillaryPeer;
    if (!peer || peer.closed) return true;
    return appendStreamAncillarySegment(peer, bytes, normalized);
  };

  const canTransportPeerAncillary = (entry, bytes, ancillaryItems) => {
    const normalized = cloneAncillaryItems(ancillaryItems);
    if (normalized.length === 0) return true;
    if (!Number.isFinite(bytes) || Number(bytes) <= 0) return false;
    return !!(entry.ancillaryPeer && !entry.ancillaryPeer.closed);
  };

  const consumeStreamAncillary = (entry, consumedBytes, collect) => {
    let remaining = Math.trunc(Number(consumedBytes));
    if (!Number.isFinite(remaining) || remaining <= 0) return [];
    if (!Array.isArray(entry.ancillarySegments) || entry.ancillarySegments.length === 0) {
      return [];
    }
    const out = [];
    while (remaining > 0 && entry.ancillarySegments.length) {
      const segment = entry.ancillarySegments[0];
      const step = Math.min(remaining, segment.remaining);
      if (step > 0 && segment.items.length > 0 && !segment.delivered) {
        if (collect) out.push(...cloneAncillaryItems(segment.items));
        segment.delivered = true;
      }
      segment.remaining -= step;
      remaining -= step;
      if (segment.remaining <= 0) {
        entry.ancillarySegments.shift();
      }
    }
    return out;
  };

  const previewStreamAncillary = (entry, consumedBytes) => {
    let remaining = Math.trunc(Number(consumedBytes));
    if (!Number.isFinite(remaining) || remaining <= 0) return [];
    if (!Array.isArray(entry.ancillarySegments) || entry.ancillarySegments.length === 0) {
      return [];
    }
    const out = [];
    for (const segment of entry.ancillarySegments) {
      if (remaining <= 0) break;
      const step = Math.min(remaining, segment.remaining);
      if (step > 0 && segment.items.length > 0 && !segment.delivered) {
        out.push(...cloneAncillaryItems(segment.items));
      }
      remaining -= step;
    }
    return out;
  };

  const awaitReady = (entry, events, timeoutMs) =>
    new Promise((resolve) => {
      const ready = pollReady(entry, events);
      if (ready) {
        resolve(ready);
        return;
      }
      const waiter = { events, resolve, timer: null };
      if (timeoutMs >= 0) {
        waiter.timer = setTimeout(() => {
          const idx = entry.waiters.indexOf(waiter);
          if (idx >= 0) entry.waiters.splice(idx, 1);
          resolve(0);
        }, timeoutMs);
      }
      entry.waiters.push(waiter);
    });

  const handleSocketOp = async (op, request, bufferView) => {
    switch (op) {
      case 'new': {
        const { family, sockType, proto, fileno } = request;
        const maybeHandle = typeof fileno === 'number' ? fileno : -1;
        if (maybeHandle >= 0) {
          const existing = removeDetached(BigInt(maybeHandle));
          if (existing) {
            sockets.set(String(maybeHandle), existing);
            return { status: 0, handle1: BigInt(maybeHandle) };
          }
          return { status: -EBADF };
        }
        if (family !== AF_INET && family !== AF_INET6) {
          return { status: -EAFNOSUPPORT };
        }
        if (sockType !== SOCK_STREAM && sockType !== SOCK_DGRAM) {
          return { status: -EAFNOSUPPORT };
        }
        if (sockType === SOCK_DGRAM) {
          const type = family === AF_INET6 ? 'udp6' : 'udp4';
          const socket = dgram.createSocket({ type });
          const entry = {
            kind: 'udp',
            socket,
            refCount: 1,
            recvQueue: [],
            readQueue: [],
            acceptQueue: [],
            waiters: [],
            ended: false,
            closed: false,
            error: null,
            listening: false,
            connected: false,
            connectPending: false,
            connectError: null,
            backpressure: false,
            sockopts: new Map(),
            reuseAddr: false,
            reusePort: false,
            broadcast: false,
            recvBuf: null,
            sendBuf: null,
            ancillarySegments: [],
            ancillaryPeer: null,
          };
          attachUdpHandlers(entry);
          const handle = allocHandle(entry);
          return { status: 0, handle1: handle };
        }
        let socket;
        try {
          if (fileno !== undefined && fileno !== null && fileno >= 0) {
            socket = new net.Socket({ fd: fileno, readable: true, writable: true });
          } else {
            socket = new net.Socket({ allowHalfOpen: true });
          }
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
        const entry = {
          kind: 'tcp',
          socket,
          refCount: 1,
          readQueue: [],
          recvQueue: [],
          acceptQueue: [],
          waiters: [],
          ended: false,
          closed: false,
          error: null,
          backpressure: false,
          connected: false,
          connectPending: false,
          connectError: null,
          listening: false,
          bindAddr: null,
          noDelay: false,
          keepAlive: false,
          sockopts: new Map(),
          reuseAddr: false,
          reusePort: false,
          broadcast: false,
          recvBuf: null,
          sendBuf: null,
          ancillarySegments: [],
          ancillaryPeer: null,
        };
        attachTcpHandlers(entry);
        const handle = allocHandle(entry);
        return { status: 0, handle1: handle };
      }
      case 'close': {
        const { handle } = request;
        const entry = removeEntry(handle);
        if (!entry) return { status: -EBADF };
        entry.refCount -= 1;
        if (entry.refCount > 0) return { status: 0 };
        entry.closed = true;
        entry.ended = true;
        drainWaiters(entry, IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE);
        if (entry.kind === 'server') {
          entry.server.close();
        } else if (entry.socket) {
          entry.socket.destroy();
        }
        if (entry.ancillaryPeer && entry.ancillaryPeer.ancillaryPeer === entry) {
          entry.ancillaryPeer.ancillaryPeer = null;
        }
        entry.ancillaryPeer = null;
        entry.ancillarySegments = [];
        return { status: 0 };
      }
      case 'clone': {
        const { handle } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        entry.refCount += 1;
        const dupHandle = allocHandle(entry);
        return { status: 0, handle1: dupHandle };
      }
      case 'bind': {
        const { handle, addr } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (traceSocketHost) {
          console.error(
            `[socket-host] bind handle=${handle} kind=${entry.kind} addr_len=${addr ? addr.length : 0} addr_hex=${Buffer.from(addr || []).toString('hex')}`,
          );
        }
        let decoded;
        try {
          decoded = decodeSockaddr(addr);
          if (traceSocketHost) {
            console.error(
              `[socket-host] bind decoded family=${decoded.family} host=${decoded.host} port=${decoded.port}`,
            );
          }
        } catch (err) {
          if (traceSocketHost) {
            const msg = err instanceof Error ? err.message : String(err);
            console.error(`[socket-host] bind decode failed: ${msg}`);
          }
          return { status: -EAFNOSUPPORT };
        }
        if (entry.kind === 'udp') {
          try {
            await new Promise((resolve, reject) => {
              entry.socket.once('error', reject);
              entry.socket.bind(decoded.port, decoded.host, () => {
                entry.socket.removeListener('error', reject);
                resolve();
              });
            });
            return { status: 0 };
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        }
        entry.bindAddr = decoded;
        return { status: 0 };
      }
      case 'listen': {
        const { handle, backlog } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (traceSocketHost) {
          console.error(
            `[socket-host] listen handle=${handle} kind=${entry.kind} backlog=${backlog}`,
          );
        }
        if (entry.kind === 'udp') {
          return { status: -EINVAL };
        }
        if (entry.kind !== 'server') {
          const server = net.createServer({ allowHalfOpen: true });
          if (entry.socket) {
            try {
              entry.socket.destroy();
            } catch (err) {
              // Best-effort cleanup.
            }
            entry.socket = null;
          }
          entry.kind = 'server';
          entry.server = server;
          if (!entry.sockopts) entry.sockopts = new Map();
          attachServerHandlers(entry);
        }
        const addr = entry.bindAddr || { host: '0.0.0.0', port: 0, family: AF_INET };
        try {
          await new Promise((resolve, reject) => {
            entry.server.once('error', reject);
            entry.server.listen(
              {
                port: addr.port,
                host: addr.host,
                backlog: backlog || 128,
              },
              () => {
                entry.server.removeListener('error', reject);
                resolve();
              },
            );
          });
          return { status: 0 };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'accept': {
        const { handle } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.kind !== 'server') return { status: -ENOTSOCK };
        if (!entry.acceptQueue.length) {
          return { status: -EWOULDBLOCK };
        }
        const next = entry.acceptQueue.shift();
        return { status: 0, handle1: next.handle, data: next.addr };
      }
      case 'connect': {
        const { handle, addr } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        let decoded;
        try {
          decoded = decodeSockaddr(addr);
        } catch (err) {
          return { status: -EAFNOSUPPORT };
        }
        if (entry.kind === 'udp') {
          try {
            entry.socket.connect(decoded.port, decoded.host);
            entry.connected = true;
            return { status: 0 };
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        }
        if (entry.connected) return { status: 0 };
        if (entry.connectPending) return { status: -EINPROGRESS };
        entry.connectPending = true;
        entry.connectError = null;
        try {
          entry.socket.connect({
            host: decoded.host,
            port: decoded.port,
            localAddress: entry.bindAddr ? entry.bindAddr.host : undefined,
            localPort: entry.bindAddr ? entry.bindAddr.port : undefined,
          });
        } catch (err) {
          entry.connectPending = false;
          entry.connectError = mapSocketError(err);
          return { status: -entry.connectError };
        }
        return { status: -EINPROGRESS };
      }
      case 'connect_ex': {
        const { handle } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.connectError) return { status: -entry.connectError };
        if (entry.connectPending) return { status: -EINPROGRESS };
        if (entry.connected) return { status: 0 };
        return { status: -ENOTCONN };
      }
      case 'recv': {
        const { handle, size, flags } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const peek = (flags & MSG_PEEK) !== 0;
        if (entry.kind === 'udp') {
          if (!entry.recvQueue.length) {
            return entry.closed ? { status: 0 } : { status: -EWOULDBLOCK };
          }
          const packet = entry.recvQueue[0];
          const data = packet.data.subarray(0, size);
          if (!peek) {
            if (data.length === packet.data.length) {
              entry.recvQueue.shift();
            } else {
              entry.recvQueue[0] = {
                data: packet.data.subarray(data.length),
                addr: packet.addr,
              };
            }
          }
          return { status: data.length, data };
        }
        if (!entry.readQueue.length) {
          if (entry.ended) return { status: 0 };
          return { status: -EWOULDBLOCK };
        }
        const data = readFromQueue(entry.readQueue, size, peek);
        if (!peek && data.length > 0) {
          consumeStreamAncillary(entry, data.length, false);
        }
        return { status: data.length, data };
      }
      case 'send': {
        const { handle, payload, flags } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.kind === 'udp') {
          if (!entry.connected) return { status: -ENOTCONN };
          try {
            entry.socket.send(payload);
            return { status: payload.length };
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        }
        if (entry.backpressure) return { status: -EWOULDBLOCK };
        try {
          const ok = entry.socket.write(payload);
          if (!ok) entry.backpressure = true;
          if (payload.length > 0) {
            recordPeerStreamWrite(entry, payload.length, []);
          }
          return { status: payload.length };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'sendto': {
        const { handle, payload, addr } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        let decoded;
        try {
          decoded = decodeSockaddr(addr);
        } catch (err) {
          return { status: -EAFNOSUPPORT };
        }
        if (entry.kind !== 'udp') {
          return { status: -EINVAL };
        }
        try {
          await new Promise((resolve, reject) => {
            entry.socket.send(payload, decoded.port, decoded.host, (err) => {
              if (err) reject(err);
              else resolve();
            });
          });
          return { status: payload.length };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'sendmsg': {
        const { handle, payload, flags, addr, ancillary } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        let ancillaryItems;
        try {
          ancillaryItems = decodeAncillaryPayload(ancillary);
        } catch {
          return { status: -EINVAL };
        }
        if (entry.kind === 'udp') {
          if (ancillaryItems.length > 0) {
            return { status: -EOPNOTSUPP };
          }
          if (addr && addr.length) {
            let decoded;
            try {
              decoded = decodeSockaddr(addr);
            } catch {
              return { status: -EAFNOSUPPORT };
            }
            try {
              await new Promise((resolve, reject) => {
                entry.socket.send(payload, decoded.port, decoded.host, (err) => {
                  if (err) reject(err);
                  else resolve();
                });
              });
              return { status: payload.length };
            } catch (err) {
              return { status: -mapSocketError(err) };
            }
          }
          if (!entry.connected) return { status: -ENOTCONN };
          try {
            entry.socket.send(payload);
            return { status: payload.length };
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        }
        if (addr && addr.length) {
          return { status: -EISCONN };
        }
        if (ancillaryItems.length > 0 && !canTransportPeerAncillary(entry, payload.length, ancillaryItems)) {
          return { status: -EOPNOTSUPP };
        }
        if (entry.backpressure) return { status: -EWOULDBLOCK };
        try {
          const ok = entry.socket.write(payload);
          if (!ok) entry.backpressure = true;
          if (payload.length > 0) {
            recordPeerStreamWrite(entry, payload.length, ancillaryItems);
          }
          if ((flags & MSG_PEEK) !== 0) {
            // MSG_PEEK is ignored for sendmsg on this host bridge.
          }
          return { status: payload.length };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'recvfrom': {
        const { handle, size, flags } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const peek = (flags & MSG_PEEK) !== 0;
        if (entry.kind === 'udp') {
          if (!entry.recvQueue.length) {
            return entry.closed ? { status: 0 } : { status: -EWOULDBLOCK };
          }
          const packet = entry.recvQueue[0];
          const data = packet.data.subarray(0, size);
          if (!peek) {
            if (data.length === packet.data.length) {
              entry.recvQueue.shift();
            } else {
              entry.recvQueue[0] = {
                data: packet.data.subarray(data.length),
                addr: packet.addr,
              };
            }
          }
          let addrBytes = Buffer.alloc(0);
          try {
            addrBytes = encodeSockaddr(packet.addr);
          } catch (err) {
            addrBytes = Buffer.alloc(0);
          }
          return { status: data.length, data, extra: addrBytes };
        }
        if (!entry.readQueue.length) {
          if (entry.ended) return { status: 0, extra: Buffer.alloc(0) };
          return { status: -EWOULDBLOCK };
        }
        const data = readFromQueue(entry.readQueue, size, peek);
        if (!peek && data.length > 0) {
          consumeStreamAncillary(entry, data.length, false);
        }
        let addrBytes = Buffer.alloc(0);
        try {
          addrBytes = encodeSockaddr({
            family: entry.socket.remoteFamily === 'IPv6' ? AF_INET6 : AF_INET,
            host: entry.socket.remoteAddress,
            port: entry.socket.remotePort || 0,
            scopeId: entry.socket.remoteScopeid || 0,
          });
        } catch (err) {
          addrBytes = Buffer.alloc(0);
        }
        return { status: data.length, data, extra: addrBytes };
      }
      case 'recvmsg': {
        const { handle, size, flags } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const peek = (flags & MSG_PEEK) !== 0;
        if (entry.kind === 'udp') {
          const ancillaryPayload = EMPTY_ANCILLARY_PAYLOAD;
          if (!entry.recvQueue.length) {
            if (entry.closed) {
              return {
                status: 0,
                data: Buffer.alloc(0),
                extra: encodeRecvmsgExtra(Buffer.alloc(0), ancillaryPayload),
                handle1: 0n,
              };
            }
            return { status: -EWOULDBLOCK };
          }
          const packet = entry.recvQueue[0];
          const data = packet.data.subarray(0, size);
          let msgFlags = 0;
          if (packet.data.length > size) {
            msgFlags |= MSG_TRUNC;
          }
          if (!peek) {
            if (data.length === packet.data.length) {
              entry.recvQueue.shift();
            } else {
              entry.recvQueue[0] = {
                data: packet.data.subarray(data.length),
                addr: packet.addr,
              };
            }
          }
          let addrBytes = Buffer.alloc(0);
          try {
            addrBytes = encodeSockaddr(packet.addr);
          } catch {
            addrBytes = Buffer.alloc(0);
          }
          return {
            status: data.length,
            data,
            extra: encodeRecvmsgExtra(addrBytes, ancillaryPayload),
            handle1: BigInt(msgFlags),
          };
        }
        if (!entry.readQueue.length) {
          if (entry.ended) {
            return {
              status: 0,
              data: Buffer.alloc(0),
              extra: encodeRecvmsgExtra(Buffer.alloc(0), EMPTY_ANCILLARY_PAYLOAD),
              handle1: 0n,
            };
          }
          return { status: -EWOULDBLOCK };
        }
        const data = readFromQueue(entry.readQueue, size, peek);
        const ancillaryItems = peek
          ? previewStreamAncillary(entry, data.length)
          : consumeStreamAncillary(entry, data.length, true);
        const ancillaryPayload =
          ancillaryItems.length > 0
            ? encodeAncillaryPayload(ancillaryItems)
            : EMPTY_ANCILLARY_PAYLOAD;
        let addrBytes = Buffer.alloc(0);
        try {
          addrBytes = encodeSockaddr({
            family: entry.socket.remoteFamily === 'IPv6' ? AF_INET6 : AF_INET,
            host: entry.socket.remoteAddress,
            port: entry.socket.remotePort || 0,
            scopeId: entry.socket.remoteScopeid || 0,
          });
        } catch {
          addrBytes = Buffer.alloc(0);
        }
        return {
          status: data.length,
          data,
          extra: encodeRecvmsgExtra(addrBytes, ancillaryPayload),
          handle1: 0n,
        };
      }
      case 'shutdown': {
        const { handle, how } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.kind === 'udp') return { status: 0 };
        try {
          if (how === SHUT_RD) {
            entry.socket.pause();
          } else if (how === SHUT_WR) {
            entry.socket.end();
          } else {
            entry.socket.destroy();
          }
          return { status: 0 };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'getsockname': {
        const { handle } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        try {
          let info = null;
          if (entry.kind === 'server') {
            info = entry.server.address();
          } else if (entry.kind === 'udp') {
            info = entry.socket.address();
          } else if (entry.bindAddr) {
            // TCP sockets can be explicitly bound before listen/connect. Node's
            // net.Socket.address() reports 0.0.0.0:0 in that state, so preserve
            // the requested bind tuple for CPython-compatible getsockname().
            info = {
              family: entry.bindAddr.family === AF_INET6 ? 'IPv6' : 'IPv4',
              address: entry.bindAddr.host,
              port: entry.bindAddr.port,
              scopeid: entry.bindAddr.scopeId || 0,
            };
          } else {
            info = entry.socket.address();
          }
          if (!info) return { status: -ENOTCONN };
          const addr = {
            family: info.family === 'IPv6' || info.family === 6 ? AF_INET6 : AF_INET,
            host: info.address,
            port: info.port,
            scopeId: info.scopeid || 0,
          };
          return { status: 0, data: encodeSockaddr(addr) };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'getpeername': {
        const { handle } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.kind === 'udp') {
          if (!entry.connected || !entry.socket.remoteAddress) {
            return { status: -ENOTCONN };
          }
          try {
            const info = entry.socket.remoteAddress();
            const addr = {
              family: info.family === 'IPv6' ? AF_INET6 : AF_INET,
              host: info.address,
              port: info.port,
              scopeId: info.scopeid || 0,
            };
            return { status: 0, data: encodeSockaddr(addr) };
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        }
        if (!entry.socket.remoteAddress) {
          return { status: -ENOTCONN };
        }
        try {
          const addr = {
            family: entry.socket.remoteFamily === 'IPv6' ? AF_INET6 : AF_INET,
            host: entry.socket.remoteAddress,
            port: entry.socket.remotePort || 0,
            scopeId: entry.socket.remoteScopeid || 0,
          };
          return { status: 0, data: encodeSockaddr(addr) };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'setsockopt': {
        const { handle, level, optname, value } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const raw = Buffer.from(value || []);
        if (!entry.sockopts) entry.sockopts = new Map();
        entry.sockopts.set(`${level}:${optname}`, raw);
        const val = raw.length >= 4 ? raw.readInt32LE(0) : 0;
        if (level === SOL_SOCKET && optname === SO_KEEPALIVE && entry.kind === 'tcp') {
          entry.socket.setKeepAlive(val !== 0);
          entry.keepAlive = val !== 0;
        } else if (level === IPPROTO_TCP && optname === TCP_NODELAY && entry.kind === 'tcp') {
          entry.socket.setNoDelay(val !== 0);
          entry.noDelay = val !== 0;
        } else if (level === SOL_SOCKET && optname === SO_REUSEADDR) {
          entry.reuseAddr = val !== 0;
        } else if (level === SOL_SOCKET && optname === SO_REUSEPORT) {
          entry.reusePort = val !== 0;
        } else if (level === SOL_SOCKET && optname === SO_BROADCAST && entry.kind === 'udp') {
          entry.socket.setBroadcast(val !== 0);
          entry.broadcast = val !== 0;
        } else if (level === SOL_SOCKET && optname === SO_RCVBUF && entry.kind === 'udp') {
          try {
            entry.socket.setRecvBufferSize(val);
            entry.recvBuf = entry.socket.getRecvBufferSize();
            const buf = Buffer.alloc(4);
            buf.writeInt32LE(entry.recvBuf, 0);
            entry.sockopts.set(`${level}:${optname}`, buf);
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        } else if (level === SOL_SOCKET && optname === SO_SNDBUF && entry.kind === 'udp') {
          try {
            entry.socket.setSendBufferSize(val);
            entry.sendBuf = entry.socket.getSendBufferSize();
            const buf = Buffer.alloc(4);
            buf.writeInt32LE(entry.sendBuf, 0);
            entry.sockopts.set(`${level}:${optname}`, buf);
          } catch (err) {
            return { status: -mapSocketError(err) };
          }
        } else if (level === SOL_SOCKET && optname === SO_LINGER) {
          // Stored in sockopts as raw bytes for parity; Node does not expose linger.
        }
        return { status: 0 };
      }
      case 'getsockopt': {
        const { handle, level, optname } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        if (entry.sockopts) {
          const stored = entry.sockopts.get(`${level}:${optname}`);
          if (stored) {
            return { status: 0, data: Buffer.from(stored) };
          }
        }
        let val = 0;
        if (level === SOL_SOCKET && optname === SO_ERROR) {
          val = entry.connectError || entry.error || 0;
        } else if (level === SOL_SOCKET && optname === SO_KEEPALIVE && entry.kind === 'tcp') {
          val = entry.keepAlive ? 1 : 0;
        } else if (level === IPPROTO_TCP && optname === TCP_NODELAY && entry.kind === 'tcp') {
          val = entry.noDelay ? 1 : 0;
        } else if (level === SOL_SOCKET && optname === SO_REUSEADDR) {
          val = entry.reuseAddr ? 1 : 0;
        } else if (level === SOL_SOCKET && optname === SO_REUSEPORT) {
          val = entry.reusePort ? 1 : 0;
        } else if (level === SOL_SOCKET && optname === SO_BROADCAST && entry.kind === 'udp') {
          val = entry.broadcast ? 1 : 0;
        } else if (level === SOL_SOCKET && optname === SO_RCVBUF && entry.kind === 'udp') {
          val = entry.recvBuf !== null ? entry.recvBuf : entry.socket.getRecvBufferSize();
        } else if (level === SOL_SOCKET && optname === SO_SNDBUF && entry.kind === 'udp') {
          val = entry.sendBuf !== null ? entry.sendBuf : entry.socket.getSendBufferSize();
        } else {
          return { status: -ENOPROTOOPT };
        }
        const buf = Buffer.alloc(4);
        buf.writeInt32LE(val, 0);
        return { status: 0, data: buf };
      }
      case 'detach': {
        const { handle } = request;
        const entry = removeEntry(handle);
        if (!entry) return { status: -EBADF };
        entry.refCount -= 1;
        const detachedHandle = BigInt(handle);
        entry.refCount += 1;
        detached.set(detachedHandle.toString(), entry);
        return { status: 0, handle1: detachedHandle };
      }
      case 'close_detached': {
        const { handle } = request;
        const entry = removeDetached(handle);
        if (!entry) return { status: -EBADF };
        entry.refCount -= 1;
        if (entry.refCount > 0) return { status: 0 };
        entry.closed = true;
        entry.ended = true;
        drainWaiters(entry, IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE);
        if (entry.kind === 'server') {
          entry.server.close();
        } else if (entry.socket) {
          entry.socket.destroy();
        }
        if (entry.ancillaryPeer && entry.ancillaryPeer.ancillaryPeer === entry) {
          entry.ancillaryPeer.ancillaryPeer = null;
        }
        entry.ancillaryPeer = null;
        entry.ancillarySegments = [];
        return { status: 0 };
      }
      case 'socketpair': {
        const { family, sockType } = request;
        if (family !== AF_INET && family !== AF_INET6) {
          return { status: -EAFNOSUPPORT };
        }
        if (sockType !== SOCK_STREAM) {
          return { status: -EAFNOSUPPORT };
        }
        const host = family === AF_INET6 ? '::1' : '127.0.0.1';
        const server = net.createServer({ allowHalfOpen: true });
        try {
          await new Promise((resolve, reject) => {
            server.once('error', reject);
            server.listen({ host, port: 0 }, () => {
              server.removeListener('error', reject);
              resolve();
            });
          });
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
        const address = server.address();
        if (!address) {
          server.close();
          return { status: -EINVAL };
        }
        const clientSock = new net.Socket({ allowHalfOpen: true });
        const clientEntry = {
          kind: 'tcp',
          socket: clientSock,
          refCount: 1,
          readQueue: [],
          recvQueue: [],
          acceptQueue: [],
          waiters: [],
          ended: false,
          closed: false,
          error: null,
          backpressure: false,
          connected: false,
          connectPending: true,
          connectError: null,
          listening: false,
          noDelay: false,
          keepAlive: false,
          sockopts: new Map(),
          reuseAddr: false,
          reusePort: false,
          broadcast: false,
          recvBuf: null,
          sendBuf: null,
          ancillarySegments: [],
          ancillaryPeer: null,
        };
        attachTcpHandlers(clientEntry);
        const leftHandle = allocHandle(clientEntry);
        const acceptedPromise = new Promise((resolve, reject) => {
          server.once('connection', resolve);
          server.once('error', reject);
        });
        clientSock.connect({ host: address.address, port: address.port });
        let serverSock;
        try {
          serverSock = await acceptedPromise;
        } catch (err) {
          server.close();
          return { status: -mapSocketError(err) };
        }
        server.close();
        const serverEntry = {
          kind: 'tcp',
          socket: serverSock,
          refCount: 1,
          readQueue: [],
          recvQueue: [],
          acceptQueue: [],
          waiters: [],
          ended: false,
          closed: false,
          error: null,
          backpressure: false,
          connected: true,
          connectPending: false,
          connectError: null,
          listening: false,
          noDelay: false,
          keepAlive: false,
          sockopts: new Map(),
          reuseAddr: false,
          reusePort: false,
          broadcast: false,
          recvBuf: null,
          sendBuf: null,
          ancillarySegments: [],
          ancillaryPeer: null,
        };
        attachTcpHandlers(serverEntry);
        const rightHandle = allocHandle(serverEntry);
        clientEntry.ancillaryPeer = serverEntry;
        serverEntry.ancillaryPeer = clientEntry;
        return { status: 0, handle1: leftHandle, handle2: rightHandle };
      }
      case 'getaddrinfo': {
        const {
          hostBytes,
          serviceBytes,
          family,
          sockType,
          proto,
          flags,
        } = request;
        const host = hostBytes && hostBytes.length ? hostBytes.toString('utf8') : null;
        const service = serviceBytes && serviceBytes.length ? serviceBytes.toString('utf8') : null;
        let port = 0;
        if (service) {
          if (/^\d+$/.test(service)) {
            port = Number.parseInt(service, 10);
          } else {
            let protoName = '';
            if (proto === IPPROTO_TCP) protoName = 'tcp';
            if (proto === 17) protoName = 'udp';
            if (!protoName) {
              if (sockType === SOCK_DGRAM) protoName = 'udp';
              if (sockType === SOCK_STREAM) protoName = 'tcp';
            }
            const svcPort = lookupServicePort(service, protoName);
            if (svcPort === null || svcPort === undefined) {
              return { status: -ENOENT };
            }
            port = svcPort;
          }
        }
        try {
          let entries = [];
          if (host && net.isIP(host)) {
            entries = [
              {
                address: host,
                family: net.isIP(host),
              },
            ];
          } else {
            entries = await dns.promises.lookup(host || '0.0.0.0', {
              all: true,
              family: family === AF_INET6 ? 6 : family === AF_INET ? 4 : 0,
            });
          }
          const payload = [];
          const count = Buffer.alloc(4);
          count.writeUInt32LE(entries.length, 0);
          payload.push(count);
          for (const entry of entries) {
            const fam = entry.family === 6 ? AF_INET6 : AF_INET;
            const canon = Buffer.alloc(0);
            const addrBytes = encodeSockaddr({
              family: fam,
              host: entry.address,
              port,
            });
            const header = Buffer.alloc(12);
            header.writeInt32LE(fam, 0);
            header.writeInt32LE(sockType, 4);
            header.writeInt32LE(proto, 8);
            const canonLen = Buffer.alloc(4);
            canonLen.writeUInt32LE(canon.length, 0);
            const addrLen = Buffer.alloc(4);
            addrLen.writeUInt32LE(addrBytes.length, 0);
            payload.push(header, canonLen, canon, addrLen, addrBytes);
          }
          return { status: 0, data: Buffer.concat(payload) };
        } catch (err) {
          return { status: -ENOENT };
        }
      }
      case 'gethostname': {
        try {
          const name = os.hostname();
          return { status: 0, data: Buffer.from(name, 'utf8') };
        } catch (err) {
          return { status: -mapSocketError(err) };
        }
      }
      case 'getservbyname': {
        const { nameBytes, protoBytes } = request;
        const name = nameBytes ? nameBytes.toString('utf8') : '';
        if (!name) return { status: -EINVAL };
        const proto = protoBytes && protoBytes.length ? protoBytes.toString('utf8') : '';
        const port = lookupServicePort(name, proto);
        if (port === null || port === undefined) {
          return { status: -ENOENT };
        }
        return { status: port };
      }
      case 'getservbyport': {
        const { port, protoBytes } = request;
        const proto = protoBytes && protoBytes.length ? protoBytes.toString('utf8') : '';
        const name = lookupServiceName(port, proto);
        if (!name) {
          return { status: -ENOENT };
        }
        return { status: 0, data: Buffer.from(name, 'utf8') };
      }
      case 'poll': {
        const { handle, events } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const mask = pollReady(entry, events);
        return { status: mask };
      }
      case 'wait': {
        const { handle, events, timeoutMs } = request;
        const entry = getEntry(handle);
        if (!entry) return { status: -EBADF };
        const ready = await awaitReady(entry, events, timeoutMs);
        if (ready === 0) {
          return { status: -ETIMEDOUT };
        }
        return { status: 0 };
      }
      case 'has_ipv6': {
        try {
          const test = net.createServer();
          await new Promise((resolve, reject) => {
            test.once('error', reject);
            test.listen({ host: '::1', port: 0 }, () => {
              test.close(() => resolve());
            });
          });
          return { status: 1 };
        } catch (err) {
          return { status: 0 };
        }
      }
      default:
        return { status: -ENOSYS };
    }
  };

  parentPort.on('message', async (msg) => {
    if (!msg || typeof msg !== 'object') return;
    if (msg.type !== 'request' || !msg.sab) return;
    const sab = msg.sab;
    const header = new Int32Array(sab, 0, SOCKET_SAB_HEADER_SIZE / 4);
    const dataView = new Uint8Array(sab, SOCKET_SAB_DATA_OFFSET);
    const respond = (result) => {
      const status = result && typeof result.status === 'number' ? result.status : -EINVAL;
      header[1] = status | 0;
      let dataTotalLen = 0;
      let dataCopyLen = 0;
      if (result && result.data) {
        const bytes = Buffer.from(result.data);
        dataTotalLen = bytes.length;
        dataCopyLen = Math.min(bytes.length, dataView.length);
        dataView.set(bytes.subarray(0, dataCopyLen), 0);
      }
      let extraTotalLen = 0;
      if (result && result.extra) {
        const bytes = Buffer.from(result.extra);
        extraTotalLen = bytes.length;
        const offset = dataCopyLen;
        const extraCopyLen = Math.min(bytes.length, dataView.length - offset);
        dataView.set(bytes.subarray(0, extraCopyLen), offset);
      }
      header[2] = dataTotalLen;
      header[7] = extraTotalLen;
      if (result && result.handle1 !== undefined && result.handle1 !== null) {
        writeHandleParts(header, 3, result.handle1);
      } else {
        writeHandleParts(header, 3, 0n);
      }
      if (result && result.handle2 !== undefined && result.handle2 !== null) {
        writeHandleParts(header, 5, result.handle2);
      }
      Atomics.store(header, 0, 1);
      Atomics.notify(header, 0, 1);
    };
    try {
      const result = await handleSocketOp(msg.op, msg.request || {}, dataView);
      respond(result || { status: 0 });
    } catch (err) {
      respond({ status: -mapSocketError(err) });
    }
  });
};

if (IS_SOCKET_WORKER) {
  socketWorkerMain();
}

class SocketWorkerClient {
  constructor() {
    this.worker = new Worker(__filename, { workerData: { kind: 'molt_socket_host' } });
    this.dead = false;
    this.worker.on('error', () => {
      this.dead = true;
    });
    this.worker.on('exit', () => {
      this.dead = true;
    });
  }

  close() {
    if (this.dead && !this.worker) {
      return Promise.resolve();
    }
    this.dead = true;
    const worker = this.worker;
    this.worker = null;
    if (!worker || typeof worker.terminate !== 'function') {
      return Promise.resolve();
    }
    try {
      return Promise.resolve(worker.terminate()).catch(() => {});
    } catch (err) {
      return Promise.resolve();
    }
  }

  call(op, request, dataCap, timeoutMs) {
    if (this.dead) {
      throw new Error('socket host worker unavailable');
    }
    const cap = dataCap || 0;
    const sab = new SharedArrayBuffer(SOCKET_SAB_HEADER_SIZE + cap);
    const header = new Int32Array(sab, 0, SOCKET_SAB_HEADER_SIZE / 4);
    this.worker.postMessage({
      type: 'request',
      op,
      request,
      sab,
    });
    let waitTimeout;
    if (typeof timeoutMs === 'number') {
      waitTimeout = timeoutMs < 0 ? undefined : timeoutMs + 1000;
    } else {
      waitTimeout = SOCKET_RPC_TIMEOUT_MS;
    }
    const waitRes = Atomics.wait(header, 0, 0, waitTimeout);
    if (waitRes === 'timed-out') {
      throw new Error(`socket host ${op} timed out`);
    }
    const status = header[1] | 0;
    const dataLen = header[2] >>> 0;
    const extraLen = header[7] >>> 0;
    const dataView = new Uint8Array(sab, SOCKET_SAB_DATA_OFFSET);
    const dataCopyLen = Math.min(dataLen, dataView.length);
    const data = dataCopyLen ? Buffer.from(dataView.subarray(0, dataCopyLen)) : Buffer.alloc(0);
    const extraCopyLen = Math.min(extraLen, dataView.length - dataCopyLen);
    const extra = extraCopyLen
      ? Buffer.from(dataView.subarray(dataCopyLen, dataCopyLen + extraCopyLen))
      : Buffer.alloc(0);
    const handle1 = readHandleParts(header, 3);
    const handle2 = readHandleParts(header, 5);
    return { status, data, extra, dataLen, extraLen, handle1, handle2 };
  }
}

let socketWorkerClient = null;

const getSocketWorkerClient = () => {
  if (socketWorkerClient && !socketWorkerClient.dead) return socketWorkerClient;
  socketWorkerClient = new SocketWorkerClient();
  return socketWorkerClient;
};

const socketCall = (op, request, dataCap, timeoutMs) => {
  try {
    const client = getSocketWorkerClient();
    return client.call(op, request, dataCap, timeoutMs);
  } catch (err) {
    return {
      status: -ENOSYS,
      data: Buffer.alloc(0),
      extra: Buffer.alloc(0),
      dataLen: 0,
      extraLen: 0,
      handle1: 0n,
      handle2: 0n,
    };
  }
};

const shutdownHostWorkers = async () => {
  const socketClient = socketWorkerClient;
  socketWorkerClient = null;
  const dbClient = dbWorkerClient;
  dbWorkerClient = null;
  const tasks = [];
  if (socketClient) tasks.push(socketClient.close());
  if (dbClient) tasks.push(dbClient.close());
  if (tasks.length) {
    await Promise.all(tasks);
  }
};

const socketHostNew = (family, sockType, proto, fileno) => {
  const raw = typeof fileno === 'bigint' ? Number(fileno) : fileno;
  const res = socketCall('new', { family, sockType, proto, fileno: raw }, 0);
  if (res.status < 0) return BigInt(res.status);
  return res.handle1;
};

const socketHostClose = (handle) => {
  const res = socketCall('close', { handle: Number(handle) }, 0);
  return res.status;
};

const socketHostClone = (handle) => {
  const res = socketCall('clone', { handle: Number(handle) }, 0);
  if (res.status < 0) return BigInt(res.status);
  return res.handle1;
};

const socketHostBind = (handle, addrPtr, addrLen) => {
  if (!wasmMemory) return -ENOSYS;
  const addr = readBytes(addrPtr, addrLen);
  if (traceSocketHost) {
    console.error(
      `[socket-host-main] bind handle=${handle} addr_len=${addrLen} addr_hex=${Buffer.from(addr || []).toString('hex')}`,
    );
  }
  const res = socketCall('bind', { handle: Number(handle), addr }, 0);
  if (traceSocketHost) {
    console.error(`[socket-host-main] bind status=${res.status}`);
  }
  return res.status;
};

const socketHostListen = (handle, backlog) => {
  const res = socketCall('listen', { handle: Number(handle), backlog }, 0);
  return res.status;
};

const socketHostAccept = (handle, addrPtr, addrCap, outLenPtr) => {
  if (!wasmMemory) return -BigInt(ENOSYS);
  const res = socketCall('accept', { handle: Number(handle) }, addrCap);
  if (res.status < 0) {
    if (outLenPtr) writeU32(Number(outLenPtr), 0);
    return BigInt(res.status);
  }
  const addrLen = res.dataLen || 0;
  if (addrLen > addrCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
    return -BigInt(ENOMEM);
  }
  if (!writeBytes(addrPtr, res.data)) {
    return -BigInt(EINVAL);
  }
  if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
  return res.handle1;
};

const socketHostConnect = (handle, addrPtr, addrLen) => {
  if (!wasmMemory) return -ENOSYS;
  const addr = readBytes(addrPtr, addrLen);
  const res = socketCall('connect', { handle: Number(handle), addr }, 0);
  return res.status;
};

const socketHostConnectEx = (handle) => {
  const res = socketCall('connect_ex', { handle: Number(handle) }, 0);
  return res.status;
};

const socketHostRecv = (handle, bufPtr, bufLen, flags) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('recv', { handle: Number(handle), size: bufLen, flags }, bufLen);
  if (res.status >= 0) {
    if (!writeBytes(bufPtr, res.data)) return -EINVAL;
  }
  return res.status;
};

const socketHostSend = (handle, bufPtr, bufLen, flags) => {
  if (!wasmMemory) return -ENOSYS;
  const payload = readBytes(bufPtr, bufLen);
  const res = socketCall('send', { handle: Number(handle), payload, flags }, 0);
  return res.status;
};

const socketHostSendTo = (handle, bufPtr, bufLen, flags, addrPtr, addrLen) => {
  if (!wasmMemory) return -ENOSYS;
  const payload = readBytes(bufPtr, bufLen);
  const addr = readBytes(addrPtr, addrLen);
  const res = socketCall('sendto', { handle: Number(handle), payload, flags, addr }, 0);
  return res.status;
};

const socketHostSendMsg = (
  handle,
  bufPtr,
  bufLen,
  flags,
  addrPtr,
  addrLen,
  ancPtr,
  ancLen,
) => {
  if (!wasmMemory) return -ENOSYS;
  const payload = readBytes(bufPtr, bufLen);
  const addr = addrLen ? readBytes(addrPtr, addrLen) : Buffer.alloc(0);
  const ancillary = ancLen ? readBytes(ancPtr, ancLen) : EMPTY_ANCILLARY_PAYLOAD;
  const res = socketCall(
    'sendmsg',
    { handle: Number(handle), payload, flags, addr, ancillary },
    0,
  );
  return res.status;
};

const socketHostRecvFrom = (handle, bufPtr, bufLen, flags, addrPtr, addrCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall(
    'recvfrom',
    { handle: Number(handle), size: bufLen, flags },
    bufLen + addrCap,
  );
  if (res.status >= 0) {
    if (!writeBytes(bufPtr, res.data)) return -EINVAL;
    const addrLen = res.extraLen || 0;
    if (addrLen > addrCap) {
      if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
      return -ENOMEM;
    }
    if (!writeBytes(addrPtr, res.extra)) return -EINVAL;
    if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
  }
  return res.status;
};

const socketHostRecvMsg = (
  handle,
  bufPtr,
  bufLen,
  flags,
  addrPtr,
  addrCap,
  outAddrLenPtr,
  ancPtr,
  ancCap,
  outAncLenPtr,
  outMsgFlagsPtr,
) => {
  if (!wasmMemory) return -ENOSYS;
  const cap =
    Number(bufLen || 0) +
    Number(addrCap || 0) +
    Number(ancCap || 0) +
    32;
  const res = socketCall(
    'recvmsg',
    { handle: Number(handle), size: Number(bufLen), flags },
    cap,
  );
  if (res.status < 0) return res.status;
  if (!writeBytes(bufPtr, res.data)) return -EINVAL;
  let decoded;
  try {
    decoded = decodeRecvmsgExtra(res.extra);
  } catch {
    return -EINVAL;
  }
  const addrLen = decoded.addr.length;
  const ancLen = decoded.ancillary.length;
  if (outAddrLenPtr) writeU32(Number(outAddrLenPtr), addrLen);
  if (outAncLenPtr) writeU32(Number(outAncLenPtr), ancLen);
  if (addrLen > Number(addrCap) || ancLen > Number(ancCap)) {
    return -ENOMEM;
  }
  if (addrLen > 0 && !writeBytes(addrPtr, decoded.addr)) return -EINVAL;
  if (ancLen > 0 && !writeBytes(ancPtr, decoded.ancillary)) return -EINVAL;
  if (outMsgFlagsPtr) {
    writeI32(Number(outMsgFlagsPtr), Number(res.handle1) | 0);
  }
  return res.status;
};

const socketHostShutdown = (handle, how) => {
  const res = socketCall('shutdown', { handle: Number(handle), how }, 0);
  return res.status;
};

const socketHostGetsockname = (handle, addrPtr, addrCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('getsockname', { handle: Number(handle) }, addrCap);
  if (res.status < 0) return res.status;
  const addrLen = res.dataLen || 0;
  if (addrLen > addrCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
    return -ENOMEM;
  }
  if (!writeBytes(addrPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
  return 0;
};

const socketHostGetpeername = (handle, addrPtr, addrCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('getpeername', { handle: Number(handle) }, addrCap);
  if (res.status < 0) return res.status;
  const addrLen = res.dataLen || 0;
  if (addrLen > addrCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
    return -ENOMEM;
  }
  if (!writeBytes(addrPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), addrLen);
  return 0;
};

const socketHostSetsockopt = (handle, level, optname, valPtr, valLen) => {
  if (!wasmMemory) return -ENOSYS;
  const value = readBytes(valPtr, valLen);
  const res = socketCall('setsockopt', { handle: Number(handle), level, optname, value }, 0);
  return res.status;
};

const socketHostGetsockopt = (handle, level, optname, valPtr, valLen, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('getsockopt', { handle: Number(handle), level, optname }, valLen);
  if (res.status < 0) return res.status;
  const dataLen = res.dataLen || 0;
  if (dataLen > valLen) {
    if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
    return -ENOMEM;
  }
  if (!writeBytes(valPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
  return 0;
};

const socketHostDetach = (handle) => {
  const res = socketCall('detach', { handle: Number(handle) }, 0);
  if (res.status < 0) return BigInt(res.status);
  return res.handle1;
};

const osCloseHost = (fd) => {
  const fdNum = typeof fd === 'bigint' ? Number(fd) : Number(fd);
  if (traceOsClose) {
    traceMark(`os_close:${fdNum}`);
  }
  if (!Number.isFinite(fdNum)) return -EINVAL;
  const res = socketCall('close_detached', { handle: fdNum }, 0);
  if (res.status === 0) return 0;
  if (res.status !== -EBADF) return res.status;
  try {
    fs.closeSync(fdNum);
    return 0;
  } catch (err) {
    return -mapSocketError(err);
  }
};

const toFiniteUnixSeconds = (raw) => {
  const value = typeof raw === 'bigint' ? Number(raw) : Number(raw);
  if (!Number.isFinite(value)) return null;
  return value;
};

const tzNameForDate = (date) => {
  try {
    const parts = new Intl.DateTimeFormat(undefined, { timeZoneName: 'short' }).formatToParts(date);
    const tzPart = parts.find((part) => part.type === 'timeZoneName');
    if (tzPart && tzPart.value) return tzPart.value;
  } catch {
    // fall through
  }
  return 'UTC';
};

const tzProfileForYear = (year) => {
  const jan = new Date(year, 0, 1, 12, 0, 0);
  const jul = new Date(year, 6, 1, 12, 0, 0);
  const janOffset = jan.getTimezoneOffset();
  const julOffset = jul.getTimezoneOffset();
  const stdDate = janOffset >= julOffset ? jan : jul;
  const dstDate = janOffset >= julOffset ? jul : jan;
  const stdName = tzNameForDate(stdDate);
  const dstName = janOffset === julOffset ? stdName : tzNameForDate(dstDate);
  return {
    stdOffsetSeconds: Math.trunc(Math.max(janOffset, julOffset) * 60),
    stdName,
    dstName,
  };
};

const timeTimezoneHost = () => {
  const profile = tzProfileForYear(new Date().getFullYear());
  return BigInt(profile.stdOffsetSeconds);
};

const timeLocalOffsetHost = (secsRaw) => {
  const secs = toFiniteUnixSeconds(secsRaw);
  if (secs === null) return I64_MIN;
  const date = new Date(secs * 1000);
  if (!Number.isFinite(date.getTime())) return I64_MIN;
  return BigInt(Math.trunc(date.getTimezoneOffset() * 60));
};

const timeTznameHost = (whichRaw, bufPtr, bufCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  if (!outLenPtr) return -EINVAL;
  const which = typeof whichRaw === 'bigint' ? Number(whichRaw) : Number(whichRaw);
  if (!Number.isFinite(which) || (which !== 0 && which !== 1)) return -EINVAL;
  const profile = tzProfileForYear(new Date().getFullYear());
  const label = which === 0 ? profile.stdName : profile.dstName;
  const data = Buffer.from(label, 'utf8');
  writeU32(Number(outLenPtr), data.length);
  const cap = Number(bufCap);
  if (!Number.isFinite(cap) || cap < 0) return -EINVAL;
  if (data.length > cap) return -ENOMEM;
  if (data.length > 0 && !writeBytes(bufPtr, data)) return -EINVAL;
  return 0;
};

const socketHostSocketpair = (family, sockType, proto, outLeftPtr, outRightPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('socketpair', { family, sockType, proto }, 0);
  if (res.status < 0) return res.status;
  writeU64(Number(outLeftPtr), res.handle1);
  writeU64(Number(outRightPtr), res.handle2);
  return 0;
};

const socketHostGetaddrinfo = (
  hostPtr,
  hostLen,
  servPtr,
  servLen,
  family,
  sockType,
  proto,
  flags,
  outPtr,
  outCap,
  outLenPtr,
) => {
  if (!wasmMemory) return -ENOSYS;
  const hostBytes = hostLen ? readBytes(hostPtr, hostLen) : Buffer.alloc(0);
  const serviceBytes = servLen ? readBytes(servPtr, servLen) : Buffer.alloc(0);
  const res = socketCall(
    'getaddrinfo',
    { hostBytes, serviceBytes, family, sockType, proto, flags },
    outCap,
  );
  if (res.status < 0) return res.status;
  const dataLen = res.dataLen || 0;
  if (dataLen > outCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
    return -ENOMEM;
  }
  if (!writeBytes(outPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
  return 0;
};

const socketHostGethostname = (bufPtr, bufCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const res = socketCall('gethostname', {}, bufCap);
  if (res.status < 0) return res.status;
  const dataLen = res.dataLen || 0;
  if (dataLen > bufCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
    return -ENOMEM;
  }
  if (!writeBytes(bufPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
  return 0;
};

const socketHostGetservbyname = (namePtr, nameLen, protoPtr, protoLen) => {
  if (!wasmMemory) return -ENOSYS;
  const nameBytes = readBytes(namePtr, nameLen);
  const protoBytes = protoLen ? readBytes(protoPtr, protoLen) : Buffer.alloc(0);
  const res = socketCall('getservbyname', { nameBytes, protoBytes }, 0);
  return res.status;
};

const socketHostGetservbyport = (port, protoPtr, protoLen, bufPtr, bufCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const protoBytes = protoLen ? readBytes(protoPtr, protoLen) : Buffer.alloc(0);
  const res = socketCall('getservbyport', { port, protoBytes }, bufCap);
  if (res.status < 0) return res.status;
  const dataLen = res.dataLen || 0;
  if (dataLen > bufCap) {
    if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
    return -ENOMEM;
  }
  if (!writeBytes(bufPtr, res.data)) return -EINVAL;
  if (outLenPtr) writeU32(Number(outLenPtr), dataLen);
  return 0;
};

const socketHostPoll = (handle, events) => {
  const res = socketCall('poll', { handle: Number(handle), events }, 0);
  return res.status;
};

const socketHostWait = (handle, events, timeoutMs) => {
  const res = socketCall('wait', { handle: Number(handle), events, timeoutMs }, 0, timeoutMs);
  return res.status;
};

const socketHasIpv6Host = () => {
  const res = socketCall('has_ipv6', {}, 0);
  return res.status;
};

const wsHandles = new Map();
let nextWsHandle = 1;

const allocWsHandle = () => {
  let handle = nextWsHandle;
  while (wsHandles.has(handle)) {
    handle += 1;
  }
  nextWsHandle = handle + 1;
  return handle;
};

const wsEntryForHandle = (handle) => wsHandles.get(Number(handle)) || null;

const wsNormalizeData = (data) => {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  if (Buffer.isBuffer(data)) {
    return new Uint8Array(data);
  }
  if (typeof data === 'string') {
    return new Uint8Array(Buffer.from(data, 'utf8'));
  }
  return new Uint8Array(0);
};

const wsAttachHandlers = (entry) => {
  const ws = entry.ws;
  const handleMessage = (event) => {
    const payload = event && event.data !== undefined ? event.data : event;
    const bytes = wsNormalizeData(payload);
    if (bytes.length) {
      entry.queue.push(bytes);
    }
  };
  const handleOpen = () => {
    entry.state = 'open';
  };
  const handleError = () => {
    entry.state = 'error';
    entry.error = ECONNRESET;
  };
  const handleClose = () => {
    entry.state = 'closed';
  };
  if (typeof ws.addEventListener === 'function') {
    ws.addEventListener('open', handleOpen);
    ws.addEventListener('message', handleMessage);
    ws.addEventListener('error', handleError);
    ws.addEventListener('close', handleClose);
  } else if (typeof ws.on === 'function') {
    ws.on('open', handleOpen);
    ws.on('message', handleMessage);
    ws.on('error', handleError);
    ws.on('close', handleClose);
  } else {
    ws.onopen = handleOpen;
    ws.onmessage = handleMessage;
    ws.onerror = handleError;
    ws.onclose = handleClose;
  }
};

const wsHostConnect = (urlPtr, urlLen, outHandlePtr) => {
  if (!wasmMemory) return -ENOSYS;
  if (!outHandlePtr) return -EINVAL;
  const ctor = getWebSocketCtor();
  if (!ctor) return -ENOSYS;
  const url = readUtf8(urlPtr, Number(urlLen));
  if (!url) return -EINVAL;
  let ws;
  try {
    ws = new ctor(url);
  } catch (err) {
    return -ECONNREFUSED;
  }
  const handle = allocWsHandle();
  const entry = { handle, ws, state: 'connecting', queue: [], error: 0 };
  wsHandles.set(handle, entry);
  try {
    ws.binaryType = 'arraybuffer';
  } catch {
    // ignore
  }
  wsAttachHandlers(entry);
  writeU64(Number(outHandlePtr), BigInt(handle));
  return 0;
};

const wsHostPoll = (handle, events) => {
  const entry = wsEntryForHandle(handle);
  if (!entry) return -EBADF;
  if (entry.state === 'error') return -(entry.error || ECONNRESET);
  if (entry.state === 'closed') {
    return IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
  }
  let ready = 0;
  if ((events & IO_EVENT_READ) !== 0) {
    if (entry.queue.length) ready |= IO_EVENT_READ;
  }
  if ((events & IO_EVENT_WRITE) !== 0) {
    if (entry.state === 'open') {
      const buffered = entry.ws && typeof entry.ws.bufferedAmount === 'number' ? entry.ws.bufferedAmount : 0;
      if (buffered <= WS_BUFFER_MAX) ready |= IO_EVENT_WRITE;
    }
  }
  return ready;
};

const wsHostSend = (handle, dataPtr, len) => {
  const entry = wsEntryForHandle(handle);
  if (!entry) return -EBADF;
  if (entry.state === 'error') return -(entry.error || ECONNRESET);
  if (entry.state !== 'open') return -EWOULDBLOCK;
  const buffered = entry.ws && typeof entry.ws.bufferedAmount === 'number' ? entry.ws.bufferedAmount : 0;
  if (buffered > WS_BUFFER_MAX) return -EWOULDBLOCK;
  const payload = readBytes(dataPtr, Number(len));
  try {
    entry.ws.send(payload);
    return 0;
  } catch (err) {
    entry.state = 'error';
    entry.error = EPIPE;
    return -EPIPE;
  }
};

const wsHostRecv = (handle, bufPtr, bufCap, outLenPtr) => {
  if (!wasmMemory) return -ENOSYS;
  const entry = wsEntryForHandle(handle);
  if (!entry) return -EBADF;
  const cap = Number(bufCap);
  if (entry.queue.length) {
    const payload = entry.queue.shift();
    const size = payload.length;
    if (outLenPtr) writeU32(Number(outLenPtr), size);
    if (size > cap) return -ENOMEM;
    if (!writeBytes(bufPtr, Buffer.from(payload))) return -EINVAL;
    return 0;
  }
  if (entry.state === 'closed') {
    if (outLenPtr) writeU32(Number(outLenPtr), 0);
    return 0;
  }
  if (entry.state === 'error') {
    return -(entry.error || ECONNRESET);
  }
  return -EWOULDBLOCK;
};

const wsHostClose = (handle) => {
  const entry = wsEntryForHandle(handle);
  if (!entry) return -EBADF;
  wsHandles.delete(Number(handle));
  try {
    if (entry.ws && entry.state !== 'closed') {
      entry.ws.close();
    }
  } catch {
    // ignore
  }
  entry.state = 'closed';
  return 0;
};

const processHandles = new Map();
let nextProcessHandle = 1;

const allocProcessHandle = () => {
  let handle = nextProcessHandle;
  while (processHandles.has(handle)) {
    handle += 1;
  }
  nextProcessHandle = handle + 1;
  return handle;
};

const readU32LE = (buf, offset) => {
  if (offset + 4 > buf.length) {
    throw new Error('unexpected EOF');
  }
  return buf.readUInt32LE(offset);
};

const decodeStringList = (buf) => {
  let offset = 0;
  const count = readU32LE(buf, offset);
  offset += 4;
  const out = [];
  for (let i = 0; i < count; i += 1) {
    const len = readU32LE(buf, offset);
    offset += 4;
    const end = offset + len;
    if (end > buf.length) throw new Error('unexpected EOF');
    out.push(buf.slice(offset, end).toString('utf8'));
    offset = end;
  }
  return out;
};

const decodeEnv = (buf) => {
  if (!buf || buf.length === 0) return { mode: 0, entries: [] };
  let offset = 0;
  const mode = buf.readUInt8(offset);
  offset += 1;
  const count = readU32LE(buf, offset);
  offset += 4;
  const entries = [];
  for (let i = 0; i < count; i += 1) {
    const keyLen = readU32LE(buf, offset);
    offset += 4;
    const keyEnd = offset + keyLen;
    if (keyEnd > buf.length) throw new Error('unexpected EOF');
    const key = buf.slice(offset, keyEnd).toString('utf8');
    offset = keyEnd;
    const valLen = readU32LE(buf, offset);
    offset += 4;
    const valEnd = offset + valLen;
    if (valEnd > buf.length) throw new Error('unexpected EOF');
    const value = buf.slice(offset, valEnd).toString('utf8');
    offset = valEnd;
    entries.push([key, value]);
  }
  return { mode, entries };
};

const processStdioMode = (mode) => {
  if (mode === PROCESS_STDIO_PIPE) return 'pipe';
  if (mode === PROCESS_STDIO_DEVNULL) return 'ignore';
  return 'inherit';
};

const processEntryForHandle = (handle) => processHandles.get(Number(handle)) || null;

const flushProcessQueue = (entry, which) => {
  if (!runtimeInstance) return;
  const queue = which === 'stdout' ? entry.pendingOut : entry.pendingErr;
  const streamBits = which === 'stdout' ? entry.stdoutStream : entry.stderrStream;
  if (!streamBits) return;
  while (queue.length) {
    const chunk = queue[0];
    const ok = sendStreamFrame(streamBits, chunk);
    if (!ok) {
      return;
    }
    queue.shift();
  }
};

const closeProcessStream = (streamBits) => {
  if (!runtimeInstance || !streamBits) return;
  try {
    runtimeInstance.exports.molt_stream_close(streamBits);
  } catch {
    // ignore
  }
};

const notifyProcessExit = (entry, exitCode) => {
  if (!runtimeInstance) return;
  const notify = runtimeInstance.exports.molt_process_host_notify;
  if (typeof notify === 'function') {
    try {
      notify(BigInt(entry.handle), exitCode | 0);
    } catch {
      // ignore
    }
  }
};

const processHostSpawn = (
  argsPtr,
  argsLen,
  envPtr,
  envLen,
  cwdPtr,
  cwdLen,
  stdinMode,
  stdoutMode,
  stderrMode,
  outHandlePtr,
) => {
  if (!wasmMemory) return -ENOSYS;
  if (!outHandlePtr) return -EINVAL;
  let args;
  try {
    args = decodeStringList(readBytes(argsPtr, argsLen));
  } catch {
    return -EINVAL;
  }
  if (!args.length) return -EINVAL;
  let env = null;
  if (envPtr && envLen) {
    try {
      const decoded = decodeEnv(readBytes(envPtr, envLen));
      if (decoded.mode === 1) {
        env = {};
      } else if (decoded.mode === 2) {
        env = { ...process.env };
      }
      if (env !== null) {
        for (const [key, value] of decoded.entries) {
          env[key] = value;
        }
      }
    } catch {
      return -EINVAL;
    }
  }
  const cwd = cwdPtr && cwdLen ? readUtf8(cwdPtr, cwdLen) : undefined;
  const stdio = [
    processStdioMode(stdinMode),
    processStdioMode(stdoutMode),
    processStdioMode(stderrMode),
  ];
  let child;
  try {
    child = spawn(args[0], args.slice(1), { env: env || undefined, cwd, stdio });
  } catch {
    return -ENOENT;
  }
  const handle = Number.isFinite(child.pid) ? child.pid : allocProcessHandle();
  const entry = {
    handle,
    child,
    stdin: child.stdin || null,
    stdout: child.stdout || null,
    stderr: child.stderr || null,
    stdoutStream: 0n,
    stderrStream: 0n,
    pendingOut: [],
    pendingErr: [],
    exitCode: null,
  };
  processHandles.set(handle, entry);

  if (entry.stdout) {
    entry.stdout.on('data', (chunk) => {
      if (!chunk || !chunk.length) return;
      if (entry.stdoutStream) {
        if (!sendStreamFrame(entry.stdoutStream, Buffer.from(chunk))) {
          entry.pendingOut.push(Buffer.from(chunk));
        }
      } else {
        entry.pendingOut.push(Buffer.from(chunk));
      }
    });
    entry.stdout.on('end', () => {
      flushProcessQueue(entry, 'stdout');
      closeProcessStream(entry.stdoutStream);
    });
  }
  if (entry.stderr) {
    entry.stderr.on('data', (chunk) => {
      if (!chunk || !chunk.length) return;
      if (entry.stderrStream) {
        if (!sendStreamFrame(entry.stderrStream, Buffer.from(chunk))) {
          entry.pendingErr.push(Buffer.from(chunk));
        }
      } else {
        entry.pendingErr.push(Buffer.from(chunk));
      }
    });
    entry.stderr.on('end', () => {
      flushProcessQueue(entry, 'stderr');
      closeProcessStream(entry.stderrStream);
    });
  }
  child.on('exit', (code, signal) => {
    let exitCode = typeof code === 'number' ? code : null;
    if (exitCode === null && signal) {
      const sig = os.constants.signals && os.constants.signals[signal];
      exitCode = Number.isFinite(sig) ? -sig : -1;
    }
    if (exitCode === null) exitCode = -1;
    entry.exitCode = exitCode;
    notifyProcessExit(entry, exitCode);
    flushProcessQueue(entry, 'stdout');
    flushProcessQueue(entry, 'stderr');
    closeProcessStream(entry.stdoutStream);
    closeProcessStream(entry.stderrStream);
  });
  child.on('error', () => {
    entry.exitCode = -1;
    notifyProcessExit(entry, -1);
  });
  writeU64(Number(outHandlePtr), BigInt(handle));
  return 0;
};

const processHostWait = (handle, _timeoutMs, outCodePtr) => {
  if (!wasmMemory) return -ENOSYS;
  const entry = processEntryForHandle(handle);
  if (!entry) return -EBADF;
  if (entry.exitCode === null || entry.exitCode === undefined) {
    return -EWOULDBLOCK;
  }
  if (outCodePtr) writeI32(Number(outCodePtr), entry.exitCode | 0);
  return 0;
};

const processHostKill = (handle) => {
  const entry = processEntryForHandle(handle);
  if (!entry) return -EBADF;
  try {
    entry.child.kill('SIGKILL');
    return 0;
  } catch {
    return -EINVAL;
  }
};

const processHostTerminate = (handle) => {
  const entry = processEntryForHandle(handle);
  if (!entry) return -EBADF;
  try {
    entry.child.kill('SIGTERM');
    return 0;
  } catch {
    return -EINVAL;
  }
};

const processHostWrite = (handle, dataPtr, len) => {
  const entry = processEntryForHandle(handle);
  if (!entry) return -EBADF;
  if (!entry.stdin || entry.stdin.destroyed) return -EPIPE;
  const payload = readBytes(dataPtr, Number(len));
  if (!payload.length) return 0;
  try {
    const ok = entry.stdin.write(payload);
    return ok ? 0 : -EWOULDBLOCK;
  } catch {
    return -EPIPE;
  }
};

const processHostCloseStdin = (handle) => {
  const entry = processEntryForHandle(handle);
  if (!entry) return -EBADF;
  if (!entry.stdin) return 0;
  try {
    entry.stdin.end();
    return 0;
  } catch {
    return -EINVAL;
  }
};

const processHostStdio = (handle, which, outStreamPtr) => {
  if (!runtimeInstance || !wasmMemory) return -ENOSYS;
  const entry = processEntryForHandle(handle);
  if (!entry || !outStreamPtr) return -EBADF;
  if (which === PROCESS_STDIO_STDOUT) {
    if (!entry.stdoutStream) {
      const streamBits = runtimeInstance.exports.molt_stream_new(0n);
      entry.stdoutStream = streamBits;
    }
    writeU64(Number(outStreamPtr), BigInt(entry.stdoutStream));
    flushProcessQueue(entry, 'stdout');
    return 0;
  }
  if (which === PROCESS_STDIO_STDERR) {
    if (!entry.stderrStream) {
      const streamBits = runtimeInstance.exports.molt_stream_new(0n);
      entry.stderrStream = streamBits;
    }
    writeU64(Number(outStreamPtr), BigInt(entry.stderrStream));
    flushProcessQueue(entry, 'stderr');
    return 0;
  }
  return -EINVAL;
};

const processHostPoll = () => {
  for (const entry of processHandles.values()) {
    flushProcessQueue(entry, 'stdout');
    flushProcessQueue(entry, 'stderr');
  }
  return 0;
};

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

const readVarInt32 = (bytes, offset) => {
  let result = 0;
  let shift = 0;
  let pos = offset;
  let byte = 0;
  while (true) {
    if (pos >= bytes.length) {
      throw new Error('Unexpected EOF while reading varint');
    }
    byte = bytes[pos++];
    result |= (byte & 0x7f) << shift;
    shift += 7;
    if ((byte & 0x80) === 0) {
      break;
    }
  }
  if (shift < 32 && (byte & 0x40) !== 0) {
    result |= ~0 << shift;
  }
  return [result | 0, pos];
};

const skipImportDesc = (bytes, offset, kind) => {
  if (kind === 0) {
    const [, next] = readVarUint(bytes, offset);
    return next;
  }
  if (kind === 1) {
    if (offset >= bytes.length) throw new Error('Unexpected EOF in table import');
    offset += 1;
    const [, next] = readLimits(bytes, offset);
    return next;
  }
  if (kind === 2) {
    const [, next] = readLimits(bytes, offset);
    return next;
  }
  if (kind === 3) {
    if (offset + 2 > bytes.length) throw new Error('Unexpected EOF in global import');
    return offset + 2;
  }
  if (kind === 4) {
    if (offset >= bytes.length) throw new Error('Unexpected EOF in tag import');
    offset += 1;
    const [, next] = readVarUint(bytes, offset);
    return next;
  }
  throw new Error(`Unknown import kind ${kind}`);
};

const extractWasmTableBase = (buffer) => {
  if (!buffer) return null;
  try {
    const bytes = new Uint8Array(buffer);
    if (bytes.length < 8) {
      return null;
    }
    let offset = 8;
    let importFuncCount = 0;
    let tableInitFuncIndex = null;
    let codeBodies = null;
    while (offset < bytes.length) {
      const sectionId = bytes[offset++];
      const [sectionSize, sizePos] = readVarUint(bytes, offset);
      offset = sizePos;
      const sectionEnd = offset + sectionSize;
      if (sectionEnd > bytes.length) {
        return null;
      }
      if (sectionId === 2) {
        let count;
        [count, offset] = readVarUint(bytes, offset);
        for (let i = 0; i < count; i += 1) {
          [, offset] = readString(bytes, offset);
          [, offset] = readString(bytes, offset);
          const kind = bytes[offset++];
          if (kind === 0) {
            importFuncCount += 1;
          }
          offset = skipImportDesc(bytes, offset, kind);
        }
      } else if (sectionId === 7) {
        let count;
        [count, offset] = readVarUint(bytes, offset);
        for (let i = 0; i < count; i += 1) {
          let name;
          [name, offset] = readString(bytes, offset);
          if (offset >= bytes.length) {
            return null;
          }
          const kind = bytes[offset++];
          let index;
          [index, offset] = readVarUint(bytes, offset);
          if (kind === 0 && name === 'molt_table_init') {
            tableInitFuncIndex = index;
          }
        }
      } else if (sectionId === 10) {
        let count;
        [count, offset] = readVarUint(bytes, offset);
        const bodies = new Array(count);
        for (let i = 0; i < count; i += 1) {
          let bodySize;
          [bodySize, offset] = readVarUint(bytes, offset);
          const bodyStart = offset;
          const bodyEnd = bodyStart + bodySize;
          if (bodyEnd > bytes.length) {
            return null;
          }
          bodies[i] = [bodyStart, bodyEnd];
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

    if (tableInitFuncIndex === null || !codeBodies) {
      return null;
    }
    const definedIndex = tableInitFuncIndex - importFuncCount;
    if (definedIndex < 0 || definedIndex >= codeBodies.length) {
      return null;
    }
    const [bodyStart, bodyEnd] = codeBodies[definedIndex];
    let pos = bodyStart;
    let localDeclCount;
    [localDeclCount, pos] = readVarUint(bytes, pos);
    for (let i = 0; i < localDeclCount; i += 1) {
      [, pos] = readVarUint(bytes, pos);
      if (pos >= bodyEnd) {
        return null;
      }
      pos += 1;
    }
    if (pos >= bodyEnd || bytes[pos] !== 0x41) {
      return null;
    }
    pos += 1;
    const [tableBase] = readVarInt32(bytes, pos);
    if (!Number.isFinite(tableBase) || tableBase <= 0) {
      return null;
    }
    return tableBase;
  } catch {
    return null;
  }
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

let outputImports = null;
let runtimeImportsDesc = null;
let inputHasRuntimeImports = false;
let runtimeCallIndirectNames = [];
let canDirectLink = false;

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

const parseWitFunctions = (source) => {
  const funcSigs = new Map();
  let buffer = '';
  for (const rawLine of source.split('\n')) {
    const line = rawLine.trim();
    if (!buffer) {
      if (!/^[A-Za-z0-9_]+:\s*func\(/.test(line)) {
        continue;
      }
      buffer = line;
    } else {
      buffer = `${buffer} ${line}`;
    }
    if (!buffer.includes(';')) {
      continue;
    }
    const match = buffer.match(
      /^\s*([A-Za-z0-9_]+):\s*func\((.*)\)\s*(?:->\s*([^;]+))?;/
    );
    if (match) {
      const name = match[1];
      const rawArgs = match[2].trim();
      const argTypes = rawArgs
        ? rawArgs
            .split(',')
            .map((part) => part.split(':')[1]?.trim())
            .filter(Boolean)
        : [];
      const retType = match[3] ? match[3].trim() : null;
      funcSigs.set(name, { argTypes, retType });
    }
    buffer = '';
  }
  return funcSigs;
};

const buildRuntimeImportWrappers = () => {
  if (!witSource) {
    throw new Error(
      'molt runtime WIT metadata is unavailable; disable MOLT_WASM_TRACE=1 or provide wit/molt-runtime.wit',
    );
  }
  const funcSigs = parseWitFunctions(witSource);

  const expectsBigInt = (ty) => ty === 'molt-object' || ty === 'u64' || ty === 's64';
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
  const traceStrings = process.env.MOLT_WASM_TRACE_STRINGS === '1';
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
      if (traceStrings && name === 'string_from_bytes') {
        const ptr = Number(converted[0] ?? 0);
        const len = Number(converted[1] ?? 0);
        const outPtr = Number(converted[2] ?? 0);
        const raw = readBytes(ptr, Math.min(len, 64));
        const preview = raw.length ? raw.toString('utf8') : '';
        let outBits = null;
        if (wasmMemory && outPtr) {
          const view = new DataView(wasmMemory.buffer);
          outBits = view.getBigUint64(outPtr, true);
        }
        console.error(
          `[molt wasm] string_from_bytes ptr=${ptr} len=${len} out=${outPtr} ret=${result} bits=${outBits} preview=${preview}`
        );
      }
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

const installTableRefs = (instance, table, label) => {
  if (!instance || !table) {
    return;
  }
  const refs = [];
  for (const [name, value] of Object.entries(instance.exports)) {
    const match = /^__molt_table_ref_(\d+)$/.exec(name);
    if (!match || typeof value !== 'function') {
      continue;
    }
    refs.push({ index: Number(match[1]), fn: value });
  }
  if (refs.length === 0) {
    return;
  }
  refs.sort((a, b) => a.index - b.index);
  const maxIndex = refs[refs.length - 1].index;
  if (maxIndex >= table.length) {
    table.grow(maxIndex + 1 - table.length);
  }
  for (const ref of refs) {
    table.set(ref.index, ref.fn);
  }
  if (traceRun) {
    console.error(`[molt wasm] installed ${refs.length} ${label} table refs`);
  }
};

const verifyTableRefs = (instance, table, label) => {
  if (!instance || !table) {
    return;
  }
  let refs = 0;
  let mismatches = 0;
  for (const [name, value] of Object.entries(instance.exports)) {
    const match = /^__molt_table_ref_(\d+)$/.exec(name);
    if (!match || typeof value !== 'function') {
      continue;
    }
    refs += 1;
    const idx = Number(match[1]);
    const entry = table.get(idx);
    if (entry !== value) {
      mismatches += 1;
      if (traceRun && mismatches <= 16) {
        const entryName = entry && entry.name ? entry.name : 'unknown';
        const valueName = value && value.name ? value.name : 'unknown';
        console.error(
          `[molt wasm] table-ref mismatch ${label} idx=${idx} expected=${valueName} actual=${entryName}`
        );
      }
    }
  }
  if (traceRun || verifyTableRefsEnabled) {
    console.error(
      `[molt wasm] table-ref verify ${label}: refs=${refs} mismatches=${mismatches}`
    );
  }
};

const runDirectLink = async () => {
  if (!runtimeBuffer) {
    throw new Error(
      'molt runtime wasm is required for direct-link mode; set MOLT_RUNTIME_WASM or use linked execution.',
    );
  }
  if (!runtimeImportsDesc) {
    throw new Error('molt runtime import metadata missing for direct-link mode');
  }
  if (!canDirectLink) {
    throw new Error(
      'WASM output is missing shared memory/table imports; rebuild with the updated toolchain or use a linked artifact.'
    );
  }
  const memoryLimits = mergeLimits(outputImports.memory, runtimeImportsDesc.memory, 'memory');
  const tableLimits = mergeLimits(outputImports.table, runtimeImportsDesc.table, 'table');
  const memory = makeMemory(memoryLimits);
  const table = makeTable(tableLimits);
  const callIndirectFns = {};
  setWasmMemory(memory);

  const env = {
    memory,
    __indirect_function_table: table,
    molt_db_query_host: dbQueryHost,
    molt_db_exec_host: dbExecHost,
    molt_db_host_poll: dbHostPoll,
    molt_getpid_host: () =>
      BigInt(typeof process !== 'undefined' && process.pid ? process.pid : 0),
    molt_time_timezone_host: timeTimezoneHost,
    molt_time_local_offset_host: timeLocalOffsetHost,
    molt_time_tzname_host: timeTznameHost,
    molt_os_close_host: osCloseHost,
    molt_socket_new_host: socketHostNew,
    molt_socket_close_host: socketHostClose,
    molt_socket_clone_host: socketHostClone,
    molt_socket_bind_host: socketHostBind,
    molt_socket_listen_host: socketHostListen,
    molt_socket_accept_host: socketHostAccept,
    molt_socket_connect_host: socketHostConnect,
    molt_socket_connect_ex_host: socketHostConnectEx,
    molt_socket_recv_host: socketHostRecv,
    molt_socket_send_host: socketHostSend,
    molt_socket_sendto_host: socketHostSendTo,
    molt_socket_sendmsg_host: socketHostSendMsg,
    molt_socket_recvfrom_host: socketHostRecvFrom,
    molt_socket_recvmsg_host: socketHostRecvMsg,
    molt_socket_shutdown_host: socketHostShutdown,
    molt_socket_getsockname_host: socketHostGetsockname,
    molt_socket_getpeername_host: socketHostGetpeername,
    molt_socket_setsockopt_host: socketHostSetsockopt,
    molt_socket_getsockopt_host: socketHostGetsockopt,
    molt_socket_detach_host: socketHostDetach,
    molt_socket_socketpair_host: socketHostSocketpair,
    molt_socket_getaddrinfo_host: socketHostGetaddrinfo,
    molt_socket_gethostname_host: socketHostGethostname,
    molt_socket_getservbyname_host: socketHostGetservbyname,
    molt_socket_getservbyport_host: socketHostGetservbyport,
    molt_socket_poll_host: socketHostPoll,
    molt_socket_wait_host: socketHostWait,
    molt_socket_has_ipv6_host: socketHasIpv6Host,
    molt_ws_connect_host: wsHostConnect,
    molt_ws_poll_host: wsHostPoll,
    molt_ws_send_host: wsHostSend,
    molt_ws_recv_host: wsHostRecv,
    molt_ws_close_host: wsHostClose,
    molt_process_spawn_host: processHostSpawn,
    molt_process_wait_host: processHostWait,
    molt_process_kill_host: processHostKill,
    molt_process_terminate_host: processHostTerminate,
    molt_process_write_host: processHostWrite,
    molt_process_close_stdin_host: processHostCloseStdin,
    molt_process_stdio_host: processHostStdio,
    molt_process_host_poll: processHostPoll,
  };
  for (const name of runtimeCallIndirectNames) {
    env[name] = (...args) => {
      if (callIndirectDebug) {
        const rawIdx = args[0];
        const idx = typeof rawIdx === 'bigint' ? Number(rawIdx) : Number(rawIdx);
        const entry = table ? table.get(idx) : null;
        const state = entry ? 'set' : 'null';
        const entryName = entry && entry.name ? entry.name : 'unknown';
        const entryLen = entry && typeof entry.length === 'number' ? entry.length : 'unknown';
        const envGet =
          runtimeInstance && runtimeInstance.exports ? runtimeInstance.exports.molt_env_get : null;
        const isEnvGet = entry && envGet ? entry === envGet : false;
        console.error(
          `[molt wasm] ${name} idx=${idx} entry=${state} name=${entryName} len=${entryLen} env_get=${isEnvGet}`
        );
      }
      const fn = callIndirectFns[name];
      if (!fn) {
        throw new Error(`${name} used before output instantiation`);
      }
      return fn(...args);
    };
  }

  const runtimeModule = await WebAssembly.instantiate(runtimeBuffer, {
    env,
    wasi_snapshot_preview1: wasiImport,
  });
  const runtimeInst = runtimeModule.instance;
  runtimeInstance = runtimeInst;
  if (detectedWasmTableBase !== null) {
    const setTableBase = runtimeInst.exports.molt_set_wasm_table_base;
    if (typeof setTableBase === 'function') {
      setTableBase(BigInt(detectedWasmTableBase));
    }
  }
  if (installTableRefsEnabled) {
    installTableRefs(runtimeInst, table, 'runtime');
  }
  const outputImportsDirect = traceImports
    ? buildRuntimeImportWrappers()
    : buildRuntimeImportDirect(runtimeInst);
  const outputModule = await WebAssembly.instantiate(wasmBuffer, {
    molt_runtime: outputImportsDirect,
    env: {
      memory,
      __indirect_function_table: table,
    },
  });

  const { molt_main, molt_memory, molt_table, molt_table_init } =
    outputModule.instance.exports;
  if (typeof molt_table_init === 'function' && process.env.MOLT_WASM_SKIP_TABLE_INIT !== '1') {
    molt_table_init();
  }
  for (const name of runtimeCallIndirectNames) {
    const fn = outputModule.instance.exports[name];
    if (typeof fn !== 'function') {
      throw new Error(`${wasmPath} missing ${name} export`);
    }
    callIndirectFns[name] = fn;
  }
  if (installTableRefsEnabled) {
    installTableRefs(outputModule.instance, table, 'output');
  }
  if (!molt_memory || !molt_table) {
    throw new Error(`${wasmPath} missing molt_memory or molt_table export`);
  }
  if (process.env.MOLT_WASM_CALL_INDIRECT_SMOKE === '1') {
    if (typeof outputModule.instance.exports.molt_call_indirect2 !== 'function') {
      throw new Error('molt_call_indirect2 export missing for smoke test');
    }
    const res = outputModule.instance.exports.molt_call_indirect2(298n, 0n, 0n);
    console.error(`[molt wasm] call_indirect2 smoke result=${res}`);
  }
  initializeWasiForInstance(runtimeInst, memory);
  runMainWithWasiExit(() => {
    molt_main();
  });
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
  const linkedCallIndirectNames = linkedImports.funcImports
    .filter((entry) => entry.module === 'env' && entry.name.startsWith('molt_call_indirect'))
    .map((entry) => entry.name);
  if (linkedCallIndirectNames.length) {
    throw new Error(
      `Linked wasm still imports ${linkedCallIndirectNames.join(
        ', '
      )}; JS call_indirect stubs removed.`,
    );
  }

  const importObject = {
    wasi_snapshot_preview1: wasiImport,
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
  importObject.env.molt_db_host_poll = dbHostPoll;
  importObject.env.molt_getpid_host = () =>
    BigInt(typeof process !== 'undefined' && process.pid ? process.pid : 0);
  importObject.env.molt_time_timezone_host = timeTimezoneHost;
  importObject.env.molt_time_local_offset_host = timeLocalOffsetHost;
  importObject.env.molt_time_tzname_host = timeTznameHost;
  importObject.env.molt_os_close_host = osCloseHost;
  importObject.env.molt_socket_new_host = socketHostNew;
  importObject.env.molt_socket_close_host = socketHostClose;
  importObject.env.molt_socket_clone_host = socketHostClone;
  importObject.env.molt_socket_bind_host = socketHostBind;
  importObject.env.molt_socket_listen_host = socketHostListen;
  importObject.env.molt_socket_accept_host = socketHostAccept;
  importObject.env.molt_socket_connect_host = socketHostConnect;
  importObject.env.molt_socket_connect_ex_host = socketHostConnectEx;
  importObject.env.molt_socket_recv_host = socketHostRecv;
  importObject.env.molt_socket_send_host = socketHostSend;
  importObject.env.molt_socket_sendto_host = socketHostSendTo;
  importObject.env.molt_socket_sendmsg_host = socketHostSendMsg;
  importObject.env.molt_socket_recvfrom_host = socketHostRecvFrom;
  importObject.env.molt_socket_recvmsg_host = socketHostRecvMsg;
  importObject.env.molt_socket_shutdown_host = socketHostShutdown;
  importObject.env.molt_socket_getsockname_host = socketHostGetsockname;
  importObject.env.molt_socket_getpeername_host = socketHostGetpeername;
  importObject.env.molt_socket_setsockopt_host = socketHostSetsockopt;
  importObject.env.molt_socket_getsockopt_host = socketHostGetsockopt;
  importObject.env.molt_socket_detach_host = socketHostDetach;
  importObject.env.molt_socket_socketpair_host = socketHostSocketpair;
  importObject.env.molt_socket_getaddrinfo_host = socketHostGetaddrinfo;
  importObject.env.molt_socket_gethostname_host = socketHostGethostname;
  importObject.env.molt_socket_getservbyname_host = socketHostGetservbyname;
  importObject.env.molt_socket_getservbyport_host = socketHostGetservbyport;
  importObject.env.molt_socket_poll_host = socketHostPoll;
  importObject.env.molt_socket_wait_host = socketHostWait;
  importObject.env.molt_socket_has_ipv6_host = socketHasIpv6Host;
  importObject.env.molt_ws_connect_host = wsHostConnect;
  importObject.env.molt_ws_poll_host = wsHostPoll;
  importObject.env.molt_ws_send_host = wsHostSend;
  importObject.env.molt_ws_recv_host = wsHostRecv;
  importObject.env.molt_ws_close_host = wsHostClose;
  importObject.env.molt_process_spawn_host = processHostSpawn;
  importObject.env.molt_process_wait_host = processHostWait;
  importObject.env.molt_process_kill_host = processHostKill;
  importObject.env.molt_process_terminate_host = processHostTerminate;
  importObject.env.molt_process_write_host = processHostWrite;
  importObject.env.molt_process_close_stdin_host = processHostCloseStdin;
  importObject.env.molt_process_stdio_host = processHostStdio;
  importObject.env.molt_process_host_poll = processHostPoll;

  const linkedModule = await WebAssembly.instantiate(linkedBuffer, importObject);
  const { molt_main, molt_table_init } = linkedModule.instance.exports;
  if (typeof molt_main !== 'function') {
    throw new Error('linked wasm missing molt_main export');
  }
  if (detectedWasmTableBase !== null) {
    const setTableBase = linkedModule.instance.exports.molt_set_wasm_table_base;
    if (typeof setTableBase === 'function') {
      setTableBase(BigInt(detectedWasmTableBase));
    }
  }
  const linkedTable =
    linkedModule.instance.exports.molt_table ||
    (importObject.env && importObject.env.__indirect_function_table) ||
    null;
  if (typeof molt_table_init === 'function') {
    molt_table_init();
  }
  // Linked artifacts can still carry table-relocation edge cases on some wasm-ld
  // versions. Opt-in reinstall helps debug signature-mismatch traps.
  if (installTableRefsEnabled) {
    installTableRefs(linkedModule.instance, linkedTable, 'linked');
  }
  if (verifyTableRefsEnabled) {
    verifyTableRefs(linkedModule.instance, linkedTable, 'linked');
  }
  const linkedMemory =
    linkedModule.instance.exports.molt_memory ||
    linkedModule.instance.exports.memory ||
    (importObject.env && importObject.env.memory);
  if (linkedMemory) {
    initializeWasiForInstance(linkedModule.instance, linkedMemory);
    setWasmMemory(linkedMemory);
  }
  runMainWithWasiExit(() => {
    molt_main();
  });
};

const runMain = async () => {
  initWasmAssets();
  traceMark('runMain:init');
  outputImports = parseWasmImports(wasmBuffer);
  inputHasRuntimeImports = outputImports.funcImports.some(
    (entry) => entry.module === 'molt_runtime'
  );
  if (traceRun) {
    console.error(
      `[molt wasm] runMain wasm=${wasmPath} linked=${linkedPath || 'none'} imports_runtime=${inputHasRuntimeImports}`
    );
  }
  if (!inputHasRuntimeImports && !linkedBuffer) {
    linkedPath = wasmPath;
    linkedBuffer = wasmBuffer;
  }

  const preferLinkedEnv = process.env.MOLT_WASM_PREFER_LINKED;
  const preferLinked =
    preferLinkedEnv === undefined ||
    !['0', 'false', 'no', 'off'].includes(preferLinkedEnv.toLowerCase());
  const forceLinked = process.env.MOLT_WASM_LINKED === '1';
  const directLinkEnv = process.env.MOLT_WASM_DIRECT_LINK;
  const directLinkRequestedByEnv =
    directLinkEnv !== undefined &&
    ['1', 'true', 'yes', 'on'].includes(directLinkEnv.toLowerCase());
  const directLinkRequestedByLegacyPrefer = !forceLinked && !preferLinked;
  const directLinkRequested = directLinkRequestedByEnv || directLinkRequestedByLegacyPrefer;
  const useLinked = forceLinked || !directLinkRequested;
  if (useLinked && inputHasRuntimeImports && !linkedBuffer) {
    throw new Error(
      'Linked wasm required for Molt runtime outputs. Rebuild with --linked or set MOLT_WASM_LINK=1 to emit output_linked.wasm.'
    );
  }
  runtimeImportsDesc = null;
  runtimeCallIndirectNames = [];
  canDirectLink = false;
  if (!useLinked) {
    loadRuntimeAssets();
    if (!runtimeBuffer) {
      const expected =
        process.env.MOLT_RUNTIME_WASM || path.join(__dirname, 'wasm', 'molt_runtime.wasm');
      throw new Error(
        `Direct-link mode requires runtime wasm at ${expected}; provide MOLT_RUNTIME_WASM or use linked execution.`,
      );
    }
    runtimeImportsDesc = parseWasmImports(runtimeBuffer);
    runtimeCallIndirectNames = runtimeImportsDesc.funcImports
      .filter((entry) => entry.module === 'env' && entry.name.startsWith('molt_call_indirect'))
      .map((entry) => entry.name);
    canDirectLink = outputImports.memory && outputImports.table && runtimeImportsDesc.memory;
    if (!canDirectLink) {
      throw new Error(
        'Direct-link mode is unavailable for this wasm artifact. Use linked output or rebuild with shared env.memory/env.__indirect_function_table imports.'
      );
    }
  }
  // Safety-first policy: default to linked execution. Direct-linking is opt-in
  // and only allowed when the artifact advertises the shared memory/table ABI.
  const runner = useLinked ? runLinked : runDirectLink;
  traceMark(`runMain:runner:${useLinked ? 'linked' : 'direct'}`);
  if (traceRun) {
    console.error(`[molt wasm] runner=${useLinked ? 'linked' : 'direct'}`);
  }
  try {
    await runner();
    traceMark('runMain:runner_completed');
    if (traceRun) {
      console.error('[molt wasm] runner completed');
    }
    if (wasiExitCode !== null && wasiExitCode !== 0) {
      traceMark(`runMain:exit_wasi:${wasiExitCode}`);
      if (traceRun) {
        console.error(`[molt wasm] exiting with wasi code ${wasiExitCode}`);
      }
      return wasiExitCode;
    }
    return 0;
  } finally {
    await shutdownHostWorkers();
  }
};

if (!IS_DB_WORKER && !IS_SOCKET_WORKER) {
  runMain()
    .then((exitCode) => {
      if (exitCode !== 0) {
        process.exit(exitCode);
      }
    })
    .catch((err) => {
      traceMark('runMain:catch');
      traceMark(`runMain:catch_err:${formatTraceError(err).replaceAll('\n', '\\n')}`);
      if (traceRun) {
        console.error('[molt wasm] runMain rejected');
      }
      if (isWasiExitSymbol(err)) {
        traceMark(`runMain:catch_wasi_symbol:${wasiExitCode === null ? 0 : wasiExitCode}`);
        if (traceRun) {
          console.error(
            `[molt wasm] caught wasi exit symbol, code=${wasiExitCode === null ? 0 : wasiExitCode}`
          );
        }
        process.exit(wasiExitCode === null ? 0 : wasiExitCode);
        return;
      }
      traceMark('runMain:catch_non_wasi');
      console.error(err);
      traceMark('runMain:exit_1');
      process.exit(1);
    });
}
