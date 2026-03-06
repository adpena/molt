const WASM_MAGIC = 0x6d736100;
const WASM_VERSION = 0x1;
const ENOSYS = 38;
const EINVAL = 22;
const ENOMEM = 12;
const EBADF = 9;
const EAFNOSUPPORT = 97;
const EPROTONOSUPPORT = 93;
const ENOPROTOOPT = 92;
const EOPNOTSUPP = 95;
const ENOTCONN = 107;
const ECONNREFUSED = 111;
const ECONNRESET = 104;
const EPIPE = 32;
const EINPROGRESS = 115;
const EALREADY = 114;
const EWOULDBLOCK = 11;
const ETIMEDOUT = 110;
const ENOENT = 2;
const IO_EVENT_READ = 1;
const IO_EVENT_WRITE = 1 << 1;
const IO_EVENT_ERROR = 1 << 2;
const WS_BUFFER_MAX = 1024 * 1024;
const AF_INET = 2;
const AF_INET6 = 10;
const AF_UNIX = 1;
const SOCK_STREAM = 1;
const SOCK_DGRAM = 2;
const SOL_SOCKET = 1;
const SO_REUSEADDR = 2;
const SO_ERROR = 4;
const SO_BROADCAST = 6;
const SO_SNDBUF = 7;
const SO_RCVBUF = 8;
const SO_KEEPALIVE = 9;
const SO_LINGER = 13;
const SO_REUSEPORT = 15;
const IPPROTO_TCP = 6;
const IPPROTO_UDP = 17;
const TCP_NODELAY = 1;
const MSG_PEEK = 2;

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
  const text = new TextDecoder('utf-8').decode(view.subarray(start, end));
  return { value: text, offset: end };
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

const parseWasmImports = (buffer) => {
  const view = new Uint8Array(buffer);
  const header = new DataView(view.buffer, view.byteOffset, view.byteLength);
  if (header.getUint32(0, true) !== WASM_MAGIC) {
    throw new Error('Invalid WASM header');
  }
  if (header.getUint32(4, true) !== WASM_VERSION) {
    throw new Error('Unsupported WASM version');
  }
  let offset = 8;
  const result = { funcImports: [], memory: null, table: null };
  while (offset < view.length) {
    const sectionId = view[offset++];
    const sizeRes = readVarUint(view, offset);
    const size = sizeRes.value;
    offset = sizeRes.offset;
    const end = offset + size;
    if (end > view.length) {
      throw new Error('Unexpected EOF while reading section');
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
        result.table = { min: limits.min, max: limits.max };
      } else if (kind === 2) {
        const limits = readLimits(view, inner);
        inner = limits.offset;
        result.memory = { min: limits.min, max: limits.max };
      } else {
        const skip = readVarUint(view, inner);
        inner = skip.offset;
      }
      count -= 1;
    }
    offset = end;
  }
  return result;
};

const mergeLimits = (left, right, label) => {
  if (!left) return right;
  if (!right) return left;
  const min = Math.max(left.min, right.min);
  let max = null;
  if (left.max !== null && right.max !== null) {
    max = Math.max(left.max, right.max);
  } else {
    max = left.max !== null ? left.max : right.max;
  }
  if (max !== null && min > max) {
    throw new Error(`Incompatible ${label} limits`);
  }
  return { min, max };
};

const makeMemory = (limits) => {
  if (!limits) return null;
  const descriptor = { initial: limits.min };
  if (limits.max !== null) descriptor.maximum = limits.max;
  return new WebAssembly.Memory(descriptor);
};

const makeTable = (limits) => {
  if (!limits) return null;
  const descriptor = { element: 'anyfunc', initial: limits.min };
  if (limits.max !== null) descriptor.maximum = limits.max;
  return new WebAssembly.Table(descriptor);
};

const UTF8_DECODER = new TextDecoder('utf-8');
const UTF8_ENCODER = new TextEncoder();

const stubI32 = () => -ENOSYS;
const stubI64 = () => -BigInt(ENOSYS);
const stubZero = () => 0;
const stubZeroI64 = () => 0n;

const readBytesFromMemory = (memory, ptr, len) => {
  if (!memory) return new Uint8Array(0);
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  const size = typeof len === 'bigint' ? Number(len) : Number(len >>> 0);
  if (!Number.isFinite(addr) || addr === 0 || size <= 0) return new Uint8Array(0);
  return new Uint8Array(memory.buffer, addr, size);
};

const readStringFromMemory = (memory, ptr, len) => {
  const bytes = readBytesFromMemory(memory, ptr, len);
  if (!bytes.length) return '';
  return UTF8_DECODER.decode(bytes);
};

const writeBytesToMemory = (memory, ptr, bytes) => {
  if (!memory) return false;
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  if (!Number.isFinite(addr) || addr === 0) return false;
  const view = new Uint8Array(memory.buffer, addr, bytes.length);
  view.set(bytes);
  return true;
};

const writeU32ToMemory = (memory, ptr, value) => {
  if (!memory) return false;
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  if (!Number.isFinite(addr) || addr === 0) return false;
  new DataView(memory.buffer).setUint32(addr, Number(value) >>> 0, true);
  return true;
};

const writeU64ToMemory = (memory, ptr, value) => {
  if (!memory) return false;
  const addr = typeof ptr === 'bigint' ? Number(ptr) : Number(ptr >>> 0);
  if (!Number.isFinite(addr) || addr === 0) return false;
  new DataView(memory.buffer).setBigUint64(addr, BigInt(value), true);
  return true;
};

const parseIPv4 = (text) => {
  if (typeof text !== 'string') return null;
  const parts = text.split('.');
  if (parts.length !== 4) return null;
  const out = new Uint8Array(4);
  for (let i = 0; i < 4; i += 1) {
    const val = Number(parts[i]);
    if (!Number.isFinite(val) || val < 0 || val > 255) return null;
    out[i] = val;
  }
  return out;
};

const parseIPv6 = (text) => {
  if (typeof text !== 'string') return null;
  let zone = '';
  let base = text;
  const zoneIndex = text.indexOf('%');
  if (zoneIndex >= 0) {
    base = text.slice(0, zoneIndex);
    zone = text.slice(zoneIndex + 1);
  }
  if (!base) {
    return { bytes: new Uint8Array(16), scopeId: zone ? Number.parseInt(zone, 10) || 0 : 0 };
  }
  const parts = base.split('::');
  if (parts.length > 2) return null;
  const head = parts[0] ? parts[0].split(':').filter(Boolean) : [];
  const tail = parts[1] ? parts[1].split(':').filter(Boolean) : [];
  if (tail.length && tail[tail.length - 1].includes('.')) {
    const v4 = parseIPv4(tail[tail.length - 1]);
    if (!v4) return null;
    tail.pop();
    tail.push(((v4[0] << 8) | v4[1]).toString(16));
    tail.push(((v4[2] << 8) | v4[3]).toString(16));
  } else if (head.length && head[head.length - 1].includes('.')) {
    const v4 = parseIPv4(head[head.length - 1]);
    if (!v4) return null;
    head.pop();
    head.push(((v4[0] << 8) | v4[1]).toString(16));
    head.push(((v4[2] << 8) | v4[3]).toString(16));
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
  return { bytes, scopeId: zone ? Number.parseInt(zone, 10) || 0 : 0 };
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

const decodeSockaddr = (bytes) => {
  if (!bytes || bytes.length < 4) {
    throw new Error('invalid sockaddr');
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const family = view.getUint16(0, true);
  const port = view.getUint16(2, true);
  if (family === AF_INET) {
    if (bytes.length < 8) throw new Error('invalid IPv4 sockaddr');
    const host = `${bytes[4]}.${bytes[5]}.${bytes[6]}.${bytes[7]}`;
    return { family, host, port };
  }
  if (family === AF_INET6) {
    if (bytes.length < 28) throw new Error('invalid IPv6 sockaddr');
    const flowinfo = view.getUint32(4, true);
    const scopeId = view.getUint32(8, true);
    const host = ipv6ToString(bytes.subarray(12, 28));
    return { family, host, port, flowinfo, scopeId };
  }
  if (family === AF_UNIX) {
    throw new Error('AF_UNIX unsupported');
  }
  throw new Error('unsupported address family');
};

const encodeSockaddr = (addr) => {
  if (!addr) return new Uint8Array(0);
  const family = addr.family || 0;
  const port = addr.port || 0;
  if (family === AF_INET) {
    const bytes = new Uint8Array(8);
    const view = new DataView(bytes.buffer);
    view.setUint16(0, AF_INET, true);
    view.setUint16(2, port, true);
    const ip = parseIPv4(addr.host || addr.address || '0.0.0.0');
    if (!ip) {
      throw new Error('invalid IPv4 address');
    }
    bytes.set(ip, 4);
    return bytes;
  }
  if (family === AF_INET6) {
    const bytes = new Uint8Array(28);
    const view = new DataView(bytes.buffer);
    view.setUint16(0, AF_INET6, true);
    view.setUint16(2, port, true);
    view.setUint32(4, addr.flowinfo || 0, true);
    view.setUint32(8, addr.scopeId || addr.scopeid || 0, true);
    const parsed = parseIPv6(addr.host || addr.address || '::');
    if (!parsed) {
      throw new Error('invalid IPv6 address');
    }
    bytes.set(parsed.bytes, 12);
    return bytes;
  }
  throw new Error('unsupported address family');
};

const QNAN = 0x7ff8000000000000n;
const TAG_INT = 0x0001000000000000n;
const TAG_BOOL = 0x0002000000000000n;
const TAG_MASK = 0x0007000000000000n;
const INT_MASK = (1n << 47n) - 1n;
const CANCEL_POLL_MS = 10;

const boxInt = (value) => {
  let v = BigInt(value);
  if (v < 0n) {
    v = (1n << 47n) + v;
  }
  return QNAN | TAG_INT | (v & INT_MASK);
};

const isBoolBits = (bits) => (bits & (QNAN | TAG_MASK)) === (QNAN | TAG_BOOL);
const unboxBool = (bits) => (bits & 1n) === 1n;

const base64FromBytes = (bytes) => {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    const slice = bytes.subarray(i, i + chunk);
    binary += String.fromCharCode(...slice);
  }
  return btoa(binary);
};

const bytesFromBase64 = (text) => {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
};

const encodeMsgpack = (value) => {
  const chunks = [];
  const push = (buf) => chunks.push(Uint8Array.from(buf));
  const pushBuf = (buf) => chunks.push(buf);
  const encodeInt = (num) => {
    const n = typeof num === 'bigint' ? num : BigInt(num);
    if (n >= 0n) {
      if (n < 0x80n) {
        push([Number(n)]);
      } else if (n <= 0xffn) {
        push([0xcc, Number(n)]);
      } else if (n <= 0xffffn) {
        const buf = new Uint8Array(3);
        buf[0] = 0xcd;
        new DataView(buf.buffer).setUint16(1, Number(n), false);
        pushBuf(buf);
      } else if (n <= 0xffffffffn) {
        const buf = new Uint8Array(5);
        buf[0] = 0xce;
        new DataView(buf.buffer).setUint32(1, Number(n), false);
        pushBuf(buf);
      } else {
        const buf = new Uint8Array(9);
        buf[0] = 0xcf;
        new DataView(buf.buffer).setBigUint64(1, n, false);
        pushBuf(buf);
      }
      return;
    }
    if (n >= -32n) {
      push([Number(0xe0n + (n + 32n))]);
    } else if (n >= -128n) {
      const buf = new Uint8Array(2);
      buf[0] = 0xd0;
      new DataView(buf.buffer).setInt8(1, Number(n));
      pushBuf(buf);
    } else if (n >= -32768n) {
      const buf = new Uint8Array(3);
      buf[0] = 0xd1;
      new DataView(buf.buffer).setInt16(1, Number(n), false);
      pushBuf(buf);
    } else if (n >= -2147483648n) {
      const buf = new Uint8Array(5);
      buf[0] = 0xd2;
      new DataView(buf.buffer).setInt32(1, Number(n), false);
      pushBuf(buf);
    } else {
      const buf = new Uint8Array(9);
      buf[0] = 0xd3;
      new DataView(buf.buffer).setBigInt64(1, n, false);
      pushBuf(buf);
    }
  };
  const encodeString = (text) => {
    const bytes = new TextEncoder().encode(text);
    const len = bytes.length;
    if (len < 32) {
      push([0xa0 | len]);
    } else if (len <= 0xff) {
      push([0xd9, len]);
    } else if (len <= 0xffff) {
      const buf = new Uint8Array(3);
      buf[0] = 0xda;
      new DataView(buf.buffer).setUint16(1, len, false);
      pushBuf(buf);
    } else {
      const buf = new Uint8Array(5);
      buf[0] = 0xdb;
      new DataView(buf.buffer).setUint32(1, len, false);
      pushBuf(buf);
    }
    pushBuf(bytes);
  };
  const encodeBin = (bytes) => {
    const len = bytes.length;
    if (len <= 0xff) {
      push([0xc4, len]);
    } else if (len <= 0xffff) {
      const buf = new Uint8Array(3);
      buf[0] = 0xc5;
      new DataView(buf.buffer).setUint16(1, len, false);
      pushBuf(buf);
    } else {
      const buf = new Uint8Array(5);
      buf[0] = 0xc6;
      new DataView(buf.buffer).setUint32(1, len, false);
      pushBuf(buf);
    }
    pushBuf(bytes);
  };
  const encodeArray = (arr) => {
    const len = arr.length;
    if (len < 16) {
      push([0x90 | len]);
    } else if (len <= 0xffff) {
      const buf = new Uint8Array(3);
      buf[0] = 0xdc;
      new DataView(buf.buffer).setUint16(1, len, false);
      pushBuf(buf);
    } else {
      const buf = new Uint8Array(5);
      buf[0] = 0xdd;
      new DataView(buf.buffer).setUint32(1, len, false);
      pushBuf(buf);
    }
    for (const item of arr) encodeValue(item);
  };
  const encodeMap = (entries) => {
    const len = entries.length;
    if (len < 16) {
      push([0x80 | len]);
    } else if (len <= 0xffff) {
      const buf = new Uint8Array(3);
      buf[0] = 0xde;
      new DataView(buf.buffer).setUint16(1, len, false);
      pushBuf(buf);
    } else {
      const buf = new Uint8Array(5);
      buf[0] = 0xdf;
      new DataView(buf.buffer).setUint32(1, len, false);
      pushBuf(buf);
    }
    for (const [key, val] of entries) {
      encodeValue(key);
      encodeValue(val);
    }
  };
  const encodeValue = (val) => {
    if (val === null || val === undefined) {
      push([0xc0]);
      return;
    }
    if (val === false) {
      push([0xc2]);
      return;
    }
    if (val === true) {
      push([0xc3]);
      return;
    }
    if (typeof val === 'number') {
      if (Number.isInteger(val)) {
        encodeInt(val);
      } else {
        const buf = new Uint8Array(9);
        buf[0] = 0xcb;
        new DataView(buf.buffer).setFloat64(1, val, false);
        pushBuf(buf);
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
    if (val instanceof Uint8Array) {
      encodeBin(val);
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
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
};

const buildEnv = (memory, table, callIndirect, logFn, overrides) => {
  const env = {
    molt_db_query_host: stubI32,
    molt_db_exec_host: stubI32,
    molt_db_host_poll: stubZero,
    molt_getpid_host: stubZeroI64,
    molt_socket_new_host: stubI64,
    molt_socket_close_host: stubI32,
    molt_socket_clone_host: stubI64,
    molt_socket_bind_host: stubI32,
    molt_socket_listen_host: stubI32,
    molt_socket_accept_host: stubI64,
    molt_socket_connect_host: stubI32,
    molt_socket_connect_ex_host: stubI32,
    molt_socket_recv_host: stubI32,
    molt_socket_send_host: stubI32,
    molt_socket_sendto_host: stubI32,
    molt_socket_recvfrom_host: stubI32,
    molt_socket_shutdown_host: stubI32,
    molt_socket_getsockname_host: stubI32,
    molt_socket_getpeername_host: stubI32,
    molt_socket_setsockopt_host: stubI32,
    molt_socket_getsockopt_host: stubI32,
    molt_socket_detach_host: stubI64,
    molt_socket_socketpair_host: stubI32,
    molt_socket_getaddrinfo_host: stubI32,
    molt_socket_gethostname_host: stubI32,
    molt_socket_getservbyname_host: stubI32,
    molt_socket_getservbyport_host: stubI32,
    molt_socket_poll_host: () => -ENOSYS,
    molt_socket_wait_host: () => -ENOSYS,
    molt_socket_has_ipv6_host: stubZero,
    molt_ws_connect_host: stubI32,
    molt_ws_send_host: stubI32,
    molt_ws_recv_host: stubI32,
    molt_ws_close_host: stubI32,
    molt_process_spawn_host: stubI32,
    molt_process_wait_host: stubI32,
    molt_process_kill_host: stubI32,
    molt_process_terminate_host: stubI32,
    molt_process_write_host: stubI32,
    molt_process_close_stdin_host: stubI32,
    molt_process_stdio_host: stubI32,
    molt_process_host_poll: stubZero,
    molt_log_host: (level, ptr, len) => {
      if (!memory || !logFn) return;
      const view = new Uint8Array(memory.buffer, ptr >>> 0, len >>> 0);
      const msg = new TextDecoder('utf-8').decode(view);
      logFn(level, msg);
    },
  };
  if (overrides && typeof overrides === 'object') {
    for (const [name, fn] of Object.entries(overrides)) {
      env[name] = fn;
    }
  }
  if (memory) env.memory = memory;
  if (table) env.__indirect_function_table = table;
  if (callIndirect) {
    for (const [name, fn] of Object.entries(callIndirect)) {
      env[name] = fn;
    }
  }
  return env;
};

const buildRuntimeImports = (outputImports, runtimeInstance) => {
  const imports = {};
  for (const entry of outputImports.funcImports) {
    if (entry.module !== 'molt_runtime') continue;
    const exportName = `molt_${entry.name}`;
    const fn = runtimeInstance.exports[exportName];
    if (typeof fn !== 'function') {
      throw new Error(`molt_runtime missing export ${exportName}`);
    }
    imports[entry.name] = fn;
  }
  return imports;
};

const createBrowserDbHost = (state, options) => {
  const opts = options && typeof options === 'object' ? options : {};
  const pending = new Map();
  const responses = [];
  let nextId = 1;
  let lastCancelCheck = 0;
  let headerSize = null;

  const getRuntime = () => state.runtimeInstance;
  const getMemory = () => state.memory;

  const getHeaderSize = () => {
    if (headerSize !== null) return headerSize;
    const runtime = getRuntime();
    if (runtime && typeof runtime.exports.molt_header_size === 'function') {
      const raw = runtime.exports.molt_header_size();
      const size = typeof raw === 'bigint' ? Number(raw) : Number(raw);
      headerSize = Number.isFinite(size) && size > 0 ? size : 40;
      return headerSize;
    }
    headerSize = 40;
    return headerSize;
  };

  const allocTempBytes = (bytes) => {
    const runtime = getRuntime();
    const memory = getMemory();
    if (!runtime || !memory) {
      throw new Error('runtime not initialized');
    }
    const allocBits = runtime.exports.molt_alloc(BigInt(bytes.length));
    const ptr = runtime.exports.molt_handle_resolve(allocBits);
    if (!ptr || ptr === 0n) {
      throw new Error('molt_alloc failed');
    }
    const payloadPtr = ptr + BigInt(getHeaderSize());
    new Uint8Array(memory.buffer, Number(payloadPtr), bytes.length).set(bytes);
    return { allocBits, payloadPtr };
  };

  const sendStreamFrame = (streamHandle, bytes) => {
    const runtime = getRuntime();
    if (!runtime) return false;
    const payload = bytes || new Uint8Array(0);
    if (payload.length === 0) {
      const res = runtime.exports.molt_stream_send(streamHandle, 0n, 0n);
      return res === 0n;
    }
    const temp = allocTempBytes(payload);
    try {
      const res = runtime.exports.molt_stream_send(
        streamHandle,
        temp.payloadPtr,
        BigInt(payload.length),
      );
      return res === 0n;
    } finally {
      runtime.exports.molt_dec_ref_obj(temp.allocBits);
    }
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
    const runtime = getRuntime();
    if (runtime) {
      runtime.exports.molt_stream_close(streamHandle);
    }
  };

  const mapStatus = (status) => {
    switch (status) {
      case 'Ok':
      case 'ok':
        return 'ok';
      case 'InvalidInput':
      case 'invalid_input':
        return 'invalid_input';
      case 'Busy':
      case 'busy':
        return 'busy';
      case 'Timeout':
      case 'timeout':
        return 'timeout';
      case 'Cancelled':
      case 'cancelled':
        return 'cancelled';
      default:
        return 'internal_error';
    }
  };

  const deliverResponse = (streamHandle, response) => {
    const runtime = getRuntime();
    if (!runtime) return;
    const status = mapStatus(response.status);
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
      runtime.exports.molt_stream_close(streamHandle);
      return;
    }
    if (header.codec === 'arrow_ipc') {
      sendStreamHeader(streamHandle, header);
      if (response.payload && response.payload.length > 0) {
        sendStreamFrame(streamHandle, response.payload);
      }
    } else {
      header.payload = response.payload || new Uint8Array(0);
      sendStreamHeader(streamHandle, header);
    }
    runtime.exports.molt_stream_close(streamHandle);
  };

  const queueResponse = (requestId, response) => {
    responses.push({ requestId, response });
  };

  const handleFetch = async (entryName, streamHandle, payload, tokenId) => {
    const endpoint = opts.dbEndpoint;
    const controller = new AbortController();
    const requestId = nextId++;
    pending.set(requestId, { streamHandle, tokenId, controller });
    try {
      const body = JSON.stringify({
        entry: entryName,
        payload_b64: base64FromBytes(payload),
        token_id: tokenId ? Number(tokenId) : 0,
        request_id: requestId,
      });
      const res = await fetch(endpoint, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body,
        signal: controller.signal,
      });
      if (!res.ok) {
        queueResponse(requestId, {
          status: 'InternalError',
          error: `db host http ${res.status}`,
          codec: 'raw',
          payload: new Uint8Array(0),
        });
      } else {
        const json = await res.json();
        const payloadBytes = json.payload_b64
          ? bytesFromBase64(json.payload_b64)
          : new Uint8Array(0);
        queueResponse(requestId, {
          status: json.status || 'InternalError',
          codec: json.codec || 'raw',
          payload: payloadBytes,
          error: json.error,
          metrics: json.metrics,
        });
      }
    } catch (err) {
      queueResponse(requestId, {
        status: 'InternalError',
        codec: 'raw',
        payload: new Uint8Array(0),
        error: err && err.message ? err.message : 'db host error',
      });
    }
  };

  const handleAdapter = async (entryName, streamHandle, payload, tokenId) => {
    const adapter = opts.dbAdapter;
    const requestId = nextId++;
    const controller = new AbortController();
    pending.set(requestId, { streamHandle, tokenId, controller });
    try {
      const response = await adapter({
        entry: entryName,
        payload,
        tokenId,
        signal: controller.signal,
      });
      if (response instanceof Uint8Array) {
        queueResponse(requestId, {
          status: 'Ok',
          codec: 'raw',
          payload: response,
        });
      } else {
        let payloadBytes = response && response.payload ? response.payload : new Uint8Array(0);
        if (payloadBytes instanceof ArrayBuffer) {
          payloadBytes = new Uint8Array(payloadBytes);
        }
        if (!(payloadBytes instanceof Uint8Array)) {
          payloadBytes = new Uint8Array(0);
        }
        queueResponse(requestId, {
          status: response.status || 'InternalError',
          codec: response.codec || 'raw',
          payload: payloadBytes,
          error: response.error,
          metrics: response.metrics,
        });
      }
    } catch (err) {
      queueResponse(requestId, {
        status: 'InternalError',
        codec: 'raw',
        payload: new Uint8Array(0),
        error: err && err.message ? err.message : 'db host error',
      });
    }
  };

  const dispatchDbHost = (entryName, reqPtr, reqLen, outPtr, tokenId) => {
    const runtime = getRuntime();
    const memory = getMemory();
    if (!runtime || !memory) return -ENOSYS;
    if (!opts.dbAdapter && !opts.dbEndpoint) return -ENOSYS;
    const outAddr =
      typeof outPtr === 'bigint' ? Number(outPtr) : Number(outPtr >>> 0);
    if (!Number.isFinite(outAddr) || outAddr === 0) return 2;
    const len = typeof reqLen === 'bigint' ? Number(reqLen) : Number(reqLen >>> 0);
    const reqAddr =
      typeof reqPtr === 'bigint' ? Number(reqPtr) : Number(reqPtr >>> 0);
    if ((!Number.isFinite(reqAddr) || reqAddr === 0) && len !== 0) return 1;
    const payload =
      len > 0 ? new Uint8Array(memory.buffer, reqAddr, len) : new Uint8Array(0);
    const streamHandle = runtime.exports.molt_stream_new(0n);
    if (!streamHandle || streamHandle === 0n) {
      return 7;
    }
    new DataView(memory.buffer).setBigUint64(outAddr, streamHandle, true);
    const tokenValue = tokenId !== undefined && tokenId !== null ? BigInt(tokenId) : 0n;
    if (opts.dbAdapter) {
      handleAdapter(entryName, streamHandle, payload, tokenValue);
    } else {
      handleFetch(entryName, streamHandle, payload, tokenValue);
    }
    return 0;
  };

  const dbQueryHost = (reqPtr, reqLen, outPtr, tokenId) =>
    dispatchDbHost('db_query', reqPtr, reqLen, outPtr, tokenId);

  const dbExecHost = (reqPtr, reqLen, outPtr, tokenId) =>
    dispatchDbHost('db_exec', reqPtr, reqLen, outPtr, tokenId);

  const dbHostPoll = () => {
    const runtime = getRuntime();
    if (!runtime) return 0;
    while (responses.length) {
      const { requestId, response } = responses.shift();
      const pendingEntry = pending.get(requestId);
      if (!pendingEntry) continue;
      pending.delete(requestId);
      deliverResponse(pendingEntry.streamHandle, response);
    }
    if (!runtime.exports.molt_cancel_token_is_cancelled) {
      return 0;
    }
    const now =
      typeof performance !== 'undefined' && performance.now ? performance.now() : Date.now();
    if (now - lastCancelCheck < CANCEL_POLL_MS) {
      return 0;
    }
    lastCancelCheck = now;
    for (const [requestId, entry] of pending.entries()) {
      if (!entry.tokenId || entry.tokenId === 0n) continue;
      try {
        const tokenBits = boxInt(entry.tokenId);
        const result = runtime.exports.molt_cancel_token_is_cancelled(tokenBits);
        if (typeof result === 'bigint' && isBoolBits(result) && unboxBool(result)) {
          entry.controller.abort();
          queueResponse(requestId, {
            status: 'Cancelled',
            codec: 'raw',
            payload: new Uint8Array(0),
            error: 'cancelled',
          });
        }
      } catch (err) {
        entry.controller.abort();
        queueResponse(requestId, {
          status: 'InternalError',
          codec: 'raw',
          payload: new Uint8Array(0),
          error: err && err.message ? err.message : 'cancel error',
        });
      }
    }
    return 0;
  };

  return { dbQueryHost, dbExecHost, dbHostPoll, sendStreamError };
};

export const createBrowserSocketHost = (state, options) => {
  const opts = options && typeof options === 'object' ? options : {};
  const sockets = new Map();
  const detached = new Map();
  const hostToSynthetic = new Map();
  const syntheticToHost = new Map();
  let nextHandle = 1;
  let nextSynthetic = 1;

  const canBlock =
    typeof SharedArrayBuffer !== 'undefined' &&
    typeof Atomics !== 'undefined' &&
    typeof Atomics.wait === 'function' &&
    typeof document === 'undefined';

  const socketFactory =
    typeof opts.socketFactory === 'function'
      ? opts.socketFactory
      : typeof WebSocket !== 'undefined'
        ? (url, protocols) => new WebSocket(url, protocols)
        : null;

  const socketProtocols = Array.isArray(opts.socketProtocols) ? opts.socketProtocols : undefined;

  const resolveSocketUrl = (addr) => {
    if (typeof opts.socketUrlResolver === 'function') {
      return opts.socketUrlResolver(addr);
    }
    let scheme = opts.socketScheme;
    if (!scheme) {
      if (typeof location !== 'undefined' && location.protocol === 'https:') {
        scheme = 'wss';
      } else {
        scheme = 'ws';
      }
    }
    const host =
      addr.family === AF_INET6 && addr.host && !addr.host.startsWith('[')
        ? `[${addr.host}]`
        : addr.host;
    const port = addr.port || 0;
    return `${scheme}://${host}:${port}`;
  };

  const allocHandle = () => {
    let handle = nextHandle;
    while (sockets.has(handle) || detached.has(handle)) {
      handle += 1;
    }
    nextHandle = handle + 1;
    return handle;
  };

  const ensureSynthetic = (host) => {
    if (hostToSynthetic.has(host)) {
      return hostToSynthetic.get(host);
    }
    const idx = nextSynthetic;
    nextSynthetic += 1;
    const a = (idx >> 16) & 0xff;
    const b = (idx >> 8) & 0xff;
    const c = idx & 0xff;
    const ip = `240.${a}.${b}.${c}`;
    hostToSynthetic.set(host, ip);
    syntheticToHost.set(ip, host);
    return ip;
  };

  const findHostForAddress = (addr) => {
    if (!addr || !addr.host) return addr;
    const mapped = syntheticToHost.get(addr.host);
    if (mapped) {
      return { ...addr, host: mapped };
    }
    return addr;
  };

  const makeCore = (meta) => ({
    handle: meta.handle,
    family: meta.family,
    sockType: meta.sockType,
    proto: meta.proto,
    state: 'new',
    ws: null,
    recvQueue: [],
    recvOffset: 0,
    lastError: 0,
    refCount: 1,
    peerAddr: null,
    localAddr: null,
    sockopts: new Map(),
    waiter: canBlock ? new Int32Array(new SharedArrayBuffer(4)) : null,
  });

  const notifyWaiter = (core) => {
    if (!core || !core.waiter) return;
    Atomics.store(core.waiter, 0, 1);
    Atomics.notify(core.waiter, 0, 1);
  };

  const enqueueData = (core, bytes) => {
    if (!core) return;
    if (bytes && bytes.length) {
      core.recvQueue.push(bytes);
      notifyWaiter(core);
    }
  };

  const markError = (core, errno) => {
    if (!core) return;
    if (core.state !== 'closed') {
      core.state = 'error';
      core.lastError = errno || ECONNRESET;
    }
    notifyWaiter(core);
  };

  const markClosed = (core) => {
    if (!core) return;
    if (core.state !== 'error') {
      core.state = 'closed';
    }
    notifyWaiter(core);
  };

  const computeReady = (core, events) => {
    let mask = 0;
    if (!core) return 0;
    if (core.state === 'error') {
      mask |= IO_EVENT_ERROR;
    }
    if (events & IO_EVENT_READ) {
      if (core.recvQueue.length > 0 || core.state === 'closed' || core.state === 'error') {
        mask |= IO_EVENT_READ;
      }
    }
    if (events & IO_EVENT_WRITE) {
      if (core.state === 'open' || core.state === 'error' || core.state === 'closed') {
        mask |= IO_EVENT_WRITE;
      }
    }
    if (events & IO_EVENT_ERROR) {
      if (core.state === 'error') {
        mask |= IO_EVENT_ERROR;
      }
    }
    return mask;
  };

  const waitForReady = (core, events, timeoutMs) => {
    if (!core) return -EBADF;
    const initial = computeReady(core, events);
    if (initial !== 0) return 0;
    if (timeoutMs === 0) return -EWOULDBLOCK;
    if (!canBlock || !core.waiter) {
      return timeoutMs > 0 ? -ETIMEDOUT : -EWOULDBLOCK;
    }
    const deadline =
      typeof timeoutMs === 'number' && timeoutMs >= 0 ? Date.now() + timeoutMs : null;
    while (true) {
      Atomics.store(core.waiter, 0, 0);
      const remaining = deadline ? Math.max(0, deadline - Date.now()) : undefined;
      const res =
        remaining === undefined
          ? Atomics.wait(core.waiter, 0, 0)
          : Atomics.wait(core.waiter, 0, 0, remaining);
      const ready = computeReady(core, events);
      if (ready !== 0) return 0;
      if (deadline && Date.now() >= deadline) return -ETIMEDOUT;
      if (res === 'timed-out') return -ETIMEDOUT;
    }
  };

  const startConnect = (core, addr) => {
    if (!socketFactory) {
      markError(core, ENOSYS);
      return -ENOSYS;
    }
    const resolved = findHostForAddress(addr);
    const url = resolveSocketUrl(resolved);
    let ws;
    try {
      ws = socketFactory(url, socketProtocols);
    } catch (err) {
      markError(core, ECONNREFUSED);
      return -ECONNREFUSED;
    }
    core.peerAddr = addr;
    core.ws = ws;
    core.state = 'connecting';
    try {
      ws.binaryType = 'arraybuffer';
    } catch (err) {
      // Ignore if not supported.
    }
    const handleMessage = (event) => {
      const data = event && event.data !== undefined ? event.data : event;
      if (data instanceof ArrayBuffer) {
        enqueueData(core, new Uint8Array(data));
        return;
      }
      if (data instanceof Uint8Array) {
        enqueueData(core, data);
        return;
      }
      if (typeof Blob !== 'undefined' && data instanceof Blob) {
        data
          .arrayBuffer()
          .then((buffer) => enqueueData(core, new Uint8Array(buffer)))
          .catch(() => markError(core, ECONNRESET));
        return;
      }
      if (typeof data === 'string') {
        enqueueData(core, UTF8_ENCODER.encode(data));
      }
    };
    const handleOpen = () => {
      core.state = 'open';
      notifyWaiter(core);
    };
    const handleError = () => {
      markError(core, ECONNREFUSED);
    };
    const handleClose = () => {
      markClosed(core);
    };
    if (ws.addEventListener) {
      ws.addEventListener('open', handleOpen);
      ws.addEventListener('message', handleMessage);
      ws.addEventListener('error', handleError);
      ws.addEventListener('close', handleClose);
    } else {
      ws.onopen = handleOpen;
      ws.onmessage = handleMessage;
      ws.onerror = handleError;
      ws.onclose = handleClose;
    }
    return -EINPROGRESS;
  };

  const socketHostNew = (family, sockType, proto, fileno) => {
    const fileVal = typeof fileno === 'bigint' ? Number(fileno) : Number(fileno);
    if (Number.isFinite(fileVal) && fileVal >= 0 && detached.has(fileVal)) {
      const core = detached.get(fileVal);
      detached.delete(fileVal);
      sockets.set(fileVal, core);
      core.refCount += 1;
      return BigInt(fileVal);
    }
    if (family !== AF_INET && family !== AF_INET6) {
      return BigInt(-EAFNOSUPPORT);
    }
    if (sockType !== SOCK_STREAM) {
      return BigInt(-EPROTONOSUPPORT);
    }
    const handle = allocHandle();
    const core = makeCore({ handle, family, sockType, proto });
    sockets.set(handle, core);
    return BigInt(handle);
  };

  const socketHostClose = (handle) => {
    const key = Number(handle);
    const core = sockets.get(key);
    if (!core) return -EBADF;
    sockets.delete(key);
    core.refCount -= 1;
    if (core.refCount <= 0) {
      if (core.ws && core.state !== 'closed') {
        try {
          core.ws.close();
        } catch (err) {
          // Ignore close failures.
        }
      }
      core.state = 'closed';
    }
    return 0;
  };

  const socketHostClone = (handle) => {
    const key = Number(handle);
    const core = sockets.get(key);
    if (!core) return BigInt(-EBADF);
    const newHandle = allocHandle();
    sockets.set(newHandle, core);
    core.refCount += 1;
    return BigInt(newHandle);
  };

  const socketHostBind = (_handle, _addrPtr, _addrLen) => -EOPNOTSUPP;
  const socketHostListen = (_handle, _backlog) => -EOPNOTSUPP;
  const socketHostAccept = (_handle, _addrPtr, _addrCap, outLenPtr) => {
    const memory = state.memory;
    if (memory && outLenPtr) {
      writeU32ToMemory(memory, outLenPtr, 0);
    }
    return -BigInt(EOPNOTSUPP);
  };

  const socketHostConnect = (handle, addrPtr, addrLen) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    if (core.state === 'open') return 0;
    if (core.state === 'connecting') return -EINPROGRESS;
    if (core.state === 'error') return -core.lastError || -ECONNREFUSED;
    if (core.state === 'closed') return -ECONNRESET;
    const memory = state.memory;
    const bytes = readBytesFromMemory(memory, addrPtr, addrLen);
    let addr;
    try {
      addr = decodeSockaddr(bytes);
    } catch (err) {
      return -EINVAL;
    }
    return startConnect(core, addr);
  };

  const socketHostConnectEx = (handle) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    if (core.state === 'open') return 0;
    if (core.state === 'connecting') return -EINPROGRESS;
    if (core.state === 'error') return -core.lastError || -ECONNREFUSED;
    if (core.state === 'closed') return -ECONNRESET;
    return -EINPROGRESS;
  };

  const socketHostRecv = (handle, bufPtr, bufLen, flags) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const memory = state.memory;
    const want = typeof bufLen === 'bigint' ? Number(bufLen) : Number(bufLen);
    if (!memory || want <= 0) return 0;
    if (core.recvQueue.length === 0) {
      if (core.state === 'closed') return 0;
      if (core.state === 'error') return -core.lastError || -ECONNRESET;
      return -EWOULDBLOCK;
    }
    const peek = (flags & MSG_PEEK) !== 0;
    const chunk = core.recvQueue[0];
    const offset = core.recvOffset;
    const available = chunk.length - offset;
    const count = Math.min(want, available);
    const slice = chunk.subarray(offset, offset + count);
    if (!writeBytesToMemory(memory, bufPtr, slice)) return -EINVAL;
    if (!peek) {
      if (count === available) {
        core.recvQueue.shift();
        core.recvOffset = 0;
      } else {
        core.recvOffset += count;
      }
    }
    return count;
  };

  const socketHostSend = (handle, bufPtr, bufLen, _flags) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    if (core.state !== 'open' || !core.ws) return -ENOTCONN;
    const memory = state.memory;
    const payload = readBytesFromMemory(memory, bufPtr, bufLen);
    try {
      core.ws.send(payload);
      return payload.length;
    } catch (err) {
      markError(core, EPIPE);
      return -EPIPE;
    }
  };

  const socketHostSendTo = () => -EOPNOTSUPP;

  const socketHostRecvFrom = (_handle, _bufPtr, _bufLen, _flags, _addrPtr, _addrCap, outLenPtr) => {
    const memory = state.memory;
    if (memory && outLenPtr) writeU32ToMemory(memory, outLenPtr, 0);
    return -EOPNOTSUPP;
  };

  const socketHostShutdown = (handle, _how) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    if (core.ws) {
      try {
        core.ws.close();
      } catch (err) {
        // Ignore close failures.
      }
    }
    markClosed(core);
    return 0;
  };

  const socketHostGetsockname = (handle, addrPtr, addrCap, outLenPtr) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const addr = core.localAddr || { family: core.family, host: '0.0.0.0', port: 0 };
    let encoded;
    try {
      encoded = encodeSockaddr(addr);
    } catch (err) {
      return -EINVAL;
    }
    if (encoded.length > addrCap) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, encoded.length);
      return -ENOMEM;
    }
    if (!writeBytesToMemory(memory, addrPtr, encoded)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, encoded.length);
    return 0;
  };

  const socketHostGetpeername = (handle, addrPtr, addrCap, outLenPtr) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    if (!core.peerAddr) return -ENOTCONN;
    let encoded;
    try {
      encoded = encodeSockaddr(core.peerAddr);
    } catch (err) {
      return -EINVAL;
    }
    if (encoded.length > addrCap) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, encoded.length);
      return -ENOMEM;
    }
    if (!writeBytesToMemory(memory, addrPtr, encoded)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, encoded.length);
    return 0;
  };

  const socketHostSetsockopt = (handle, level, optname, valPtr, valLen) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const value = readBytesFromMemory(memory, valPtr, valLen);
    const key = `${level}:${optname}`;
    core.sockopts.set(key, new Uint8Array(value));
    return 0;
  };

  const socketHostGetsockopt = (handle, level, optname, valPtr, valLen, outLenPtr) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    if (level === SOL_SOCKET && optname === SO_ERROR) {
      const view = new DataView(new ArrayBuffer(4));
      view.setInt32(0, core.lastError || 0, true);
      const bytes = new Uint8Array(view.buffer);
      if (bytes.length > valLen) {
        if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
        return -ENOMEM;
      }
      if (!writeBytesToMemory(memory, valPtr, bytes)) return -EINVAL;
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
      return 0;
    }
    const key = `${level}:${optname}`;
    const stored = core.sockopts.get(key);
    if (!stored) {
      return -ENOPROTOOPT;
    }
    if (stored.length > valLen) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, stored.length);
      return -ENOMEM;
    }
    if (!writeBytesToMemory(memory, valPtr, stored)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, stored.length);
    return 0;
  };

  const socketHostDetach = (handle) => {
    const key = Number(handle);
    const core = sockets.get(key);
    if (!core) return BigInt(-EBADF);
    sockets.delete(key);
    detached.set(key, core);
    return BigInt(key);
  };

  const socketHostSocketpair = () => -ENOSYS;

  const socketHostGetaddrinfo = (
    hostPtr,
    hostLen,
    servPtr,
    servLen,
    family,
    sockType,
    proto,
    _flags,
    outPtr,
    outCap,
    outLenPtr,
  ) => {
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const host = hostLen ? readStringFromMemory(memory, hostPtr, hostLen) : '';
    const service = servLen ? readStringFromMemory(memory, servPtr, servLen) : '';
    let port = 0;
    if (service) {
      const parsed = Number.parseInt(service, 10);
      if (Number.isFinite(parsed)) {
        port = parsed;
      } else if (service === 'http' || service === 'ws') {
        port = 80;
      } else if (service === 'https' || service === 'wss') {
        port = 443;
      } else {
        return -ENOENT;
      }
    }
    let addrFamily = family;
    let addrHost = host;
    let encodedAddr = null;
    if (!addrHost) {
      addrHost = family === AF_INET6 ? '::' : '0.0.0.0';
    }
    let v4 = parseIPv4(addrHost);
    if (v4) {
      addrFamily = addrFamily === 0 ? AF_INET : addrFamily;
      if (addrFamily !== AF_INET) return -EAFNOSUPPORT;
      encodedAddr = encodeSockaddr({ family: AF_INET, host: addrHost, port });
    } else {
      const v6 = parseIPv6(addrHost);
      if (v6) {
        addrFamily = addrFamily === 0 ? AF_INET6 : addrFamily;
        if (addrFamily !== AF_INET6) return -EAFNOSUPPORT;
        encodedAddr = encodeSockaddr({
          family: AF_INET6,
          host: addrHost,
          port,
          flowinfo: 0,
          scopeId: v6.scopeId || 0,
        });
      } else {
        const synthetic = ensureSynthetic(addrHost);
        addrFamily = addrFamily === 0 ? AF_INET : addrFamily;
        if (addrFamily !== AF_INET) return -EAFNOSUPPORT;
        encodedAddr = encodeSockaddr({ family: AF_INET, host: synthetic, port });
      }
    }
    if (!encodedAddr) return -EINVAL;
    const chunks = [];
    const pushU32 = (val) => {
      const buf = new Uint8Array(4);
      new DataView(buf.buffer).setUint32(0, val >>> 0, true);
      chunks.push(buf);
    };
    const pushI32 = (val) => {
      const buf = new Uint8Array(4);
      new DataView(buf.buffer).setInt32(0, val | 0, true);
      chunks.push(buf);
    };
    pushU32(1);
    pushI32(addrFamily || AF_INET);
    pushI32(sockType || SOCK_STREAM);
    pushI32(proto || 0);
    pushU32(0);
    pushU32(encodedAddr.length);
    chunks.push(encodedAddr);
    const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    if (total > outCap) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, total);
      return -ENOMEM;
    }
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    if (!writeBytesToMemory(memory, outPtr, out)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, total);
    return 0;
  };

  const socketHostGethostname = (bufPtr, bufCap, outLenPtr) => {
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const bytes = UTF8_ENCODER.encode('browser');
    if (bytes.length > bufCap) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
      return -ENOMEM;
    }
    if (!writeBytesToMemory(memory, bufPtr, bytes)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
    return 0;
  };

  const socketHostGetservbyname = (namePtr, nameLen, _protoPtr, _protoLen) => {
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const name = readStringFromMemory(memory, namePtr, nameLen).toLowerCase();
    if (name === 'http' || name === 'ws') return 80;
    if (name === 'https' || name === 'wss') return 443;
    return -ENOENT;
  };

  const socketHostGetservbyport = (port, _protoPtr, _protoLen, bufPtr, bufCap, outLenPtr) => {
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const portNum = typeof port === 'bigint' ? Number(port) : Number(port);
    let name = '';
    if (portNum === 80) name = 'http';
    if (portNum === 443) name = 'https';
    if (!name) return -ENOENT;
    const bytes = UTF8_ENCODER.encode(name);
    if (bytes.length > bufCap) {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
      return -ENOMEM;
    }
    if (!writeBytesToMemory(memory, bufPtr, bytes)) return -EINVAL;
    if (outLenPtr) writeU32ToMemory(memory, outLenPtr, bytes.length);
    return 0;
  };

  const socketHostPoll = (handle, events) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const mask = computeReady(core, events);
    return mask;
  };

  const socketHostWait = (handle, events, timeoutMs) => {
    const core = sockets.get(Number(handle));
    if (!core) return -EBADF;
    const timeout = typeof timeoutMs === 'bigint' ? Number(timeoutMs) : Number(timeoutMs);
    return waitForReady(core, events, timeout);
  };

  const socketHasIpv6Host = () => 1;

  return {
    socketHostNew,
    socketHostClose,
    socketHostClone,
    socketHostBind,
    socketHostListen,
    socketHostAccept,
    socketHostConnect,
    socketHostConnectEx,
    socketHostRecv,
    socketHostSend,
    socketHostSendTo,
    socketHostRecvFrom,
    socketHostShutdown,
    socketHostGetsockname,
    socketHostGetpeername,
    socketHostSetsockopt,
    socketHostGetsockopt,
    socketHostDetach,
    socketHostSocketpair,
    socketHostGetaddrinfo,
    socketHostGethostname,
    socketHostGetservbyname,
    socketHostGetservbyport,
    socketHostPoll,
    socketHostWait,
    socketHasIpv6Host,
  };
};

export const createBrowserWebSocketHost = (state, options) => {
  const opts = options && typeof options === 'object' ? options : {};
  const sockets = new Map();
  let nextHandle = 1;
  const wsFactory =
    typeof opts.websocketFactory === 'function'
      ? opts.websocketFactory
      : typeof WebSocket !== 'undefined'
        ? (url) => new WebSocket(url)
        : null;
  const bufferedMax =
    typeof opts.wsBufferedMax === 'number' && opts.wsBufferedMax > 0
      ? opts.wsBufferedMax
      : WS_BUFFER_MAX;

  const allocHandle = () => {
    let handle = nextHandle;
    while (sockets.has(handle)) {
      handle += 1;
    }
    nextHandle = handle + 1;
    return handle;
  };

  const attachHandlers = (entry) => {
    const ws = entry.ws;
    const handleMessage = (event) => {
      const payload = event && event.data !== undefined ? event.data : event;
      if (payload instanceof ArrayBuffer) {
        entry.queue.push(new Uint8Array(payload));
        return;
      }
      if (payload instanceof Uint8Array) {
        entry.queue.push(payload);
        return;
      }
      if (typeof Blob !== 'undefined' && payload instanceof Blob) {
        payload
          .arrayBuffer()
          .then((buffer) => entry.queue.push(new Uint8Array(buffer)))
          .catch(() => {
            entry.state = 'error';
            entry.error = ECONNRESET;
          });
        return;
      }
      if (typeof payload === 'string') {
        entry.queue.push(UTF8_ENCODER.encode(payload));
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
    if (ws.addEventListener) {
      ws.addEventListener('open', handleOpen);
      ws.addEventListener('message', handleMessage);
      ws.addEventListener('error', handleError);
      ws.addEventListener('close', handleClose);
    } else {
      ws.onopen = handleOpen;
      ws.onmessage = handleMessage;
      ws.onerror = handleError;
      ws.onclose = handleClose;
    }
  };

  const wsConnectHost = (urlPtr, urlLen, outHandlePtr) => {
    const memory = state.memory;
    if (!memory || !outHandlePtr) return -ENOSYS;
    if (!wsFactory) return -ENOSYS;
    const url = readStringFromMemory(memory, urlPtr, urlLen);
    if (!url) return -EINVAL;
    let ws;
    try {
      ws = wsFactory(url);
    } catch (err) {
      return -ECONNREFUSED;
    }
    const handle = allocHandle();
    const entry = { handle, ws, state: 'connecting', queue: [], error: 0 };
    sockets.set(handle, entry);
    try {
      ws.binaryType = 'arraybuffer';
    } catch (err) {
      // ignore
    }
    attachHandlers(entry);
    writeU64ToMemory(memory, outHandlePtr, BigInt(handle));
    return 0;
  };

  const wsSendHost = (handle, dataPtr, len) => {
    const entry = sockets.get(Number(handle));
    if (!entry) return -EBADF;
    if (entry.state === 'error') return -(entry.error || ECONNRESET);
    if (entry.state !== 'open') return -EWOULDBLOCK;
    const buffered =
      entry.ws && typeof entry.ws.bufferedAmount === 'number' ? entry.ws.bufferedAmount : 0;
    if (buffered > bufferedMax) return -EWOULDBLOCK;
    const payload = readBytesFromMemory(state.memory, dataPtr, len);
    try {
      entry.ws.send(payload);
      return 0;
    } catch (err) {
      entry.state = 'error';
      entry.error = EPIPE;
      return -EPIPE;
    }
  };

  const wsRecvHost = (handle, bufPtr, bufCap, outLenPtr) => {
    const entry = sockets.get(Number(handle));
    if (!entry) return -EBADF;
    const memory = state.memory;
    if (!memory) return -ENOSYS;
    const cap = typeof bufCap === 'bigint' ? Number(bufCap) : Number(bufCap);
    if (entry.queue.length) {
      const payload = entry.queue.shift();
      const size = payload.length;
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, size);
      if (size > cap) return -ENOMEM;
      if (!writeBytesToMemory(memory, bufPtr, payload)) return -EINVAL;
      return 0;
    }
    if (entry.state === 'closed') {
      if (outLenPtr) writeU32ToMemory(memory, outLenPtr, 0);
      return 0;
    }
    if (entry.state === 'error') {
      return -(entry.error || ECONNRESET);
    }
    return -EWOULDBLOCK;
  };

  const wsCloseHost = (handle) => {
    const entry = sockets.get(Number(handle));
    if (!entry) return -EBADF;
    sockets.delete(Number(handle));
    try {
      if (entry.ws && entry.state !== 'closed') {
        entry.ws.close();
      }
    } catch (err) {
      // ignore
    }
    entry.state = 'closed';
    return 0;
  };

  return { wsConnectHost, wsSendHost, wsRecvHost, wsCloseHost };
};

const buildWasiStub = () => ({
  proc_exit: (code) => {
    throw new Error(`WASM proc_exit ${code}`);
  },
  fd_write: stubI32,
  fd_read: stubI32,
  fd_close: stubI32,
  fd_seek: stubI32,
  fd_fdstat_get: stubI32,
  fd_fdstat_set_flags: stubI32,
  path_open: stubI32,
  path_filestat_get: stubI32,
  random_get: stubI32,
  clock_time_get: stubI32,
});

const tryFetch = async (url) => {
  try {
    const res = await fetch(url);
    if (!res.ok) return null;
    return await res.arrayBuffer();
  } catch (err) {
    return null;
  }
};

export const loadMoltWasm = async (options = {}) => {
  const wasmUrl = options.wasmUrl || './output.wasm';
  const linkedUrl = options.linkedUrl || './output_linked.wasm';
  const runtimeUrl = options.runtimeUrl || './molt_runtime.wasm';
  const preferLinked = options.preferLinked !== false;
  const logFn = options.log || null;
  const state = { runtimeInstance: null, memory: null };
  const dbHost = createBrowserDbHost(state, {
    dbEndpoint: options.dbEndpoint,
    dbAdapter: options.dbAdapter,
  });
  const socketHost = createBrowserSocketHost(state, {
    socketFactory: options.socketFactory,
    socketProtocols: options.socketProtocols,
    socketScheme: options.socketScheme,
    socketUrlResolver: options.socketUrlResolver,
  });
  const wsHost = createBrowserWebSocketHost(state, {
    websocketFactory: options.websocketFactory,
    wsBufferedMax: options.wsBufferedMax,
  });
  const overrides = {
    molt_db_query_host: dbHost.dbQueryHost,
    molt_db_exec_host: dbHost.dbExecHost,
    molt_db_host_poll: dbHost.dbHostPoll,
    molt_socket_new_host: socketHost.socketHostNew,
    molt_socket_close_host: socketHost.socketHostClose,
    molt_socket_clone_host: socketHost.socketHostClone,
    molt_socket_bind_host: socketHost.socketHostBind,
    molt_socket_listen_host: socketHost.socketHostListen,
    molt_socket_accept_host: socketHost.socketHostAccept,
    molt_socket_connect_host: socketHost.socketHostConnect,
    molt_socket_connect_ex_host: socketHost.socketHostConnectEx,
    molt_socket_recv_host: socketHost.socketHostRecv,
    molt_socket_send_host: socketHost.socketHostSend,
    molt_socket_sendto_host: socketHost.socketHostSendTo,
    molt_socket_recvfrom_host: socketHost.socketHostRecvFrom,
    molt_socket_shutdown_host: socketHost.socketHostShutdown,
    molt_socket_getsockname_host: socketHost.socketHostGetsockname,
    molt_socket_getpeername_host: socketHost.socketHostGetpeername,
    molt_socket_setsockopt_host: socketHost.socketHostSetsockopt,
    molt_socket_getsockopt_host: socketHost.socketHostGetsockopt,
    molt_socket_detach_host: socketHost.socketHostDetach,
    molt_socket_socketpair_host: socketHost.socketHostSocketpair,
    molt_socket_getaddrinfo_host: socketHost.socketHostGetaddrinfo,
    molt_socket_gethostname_host: socketHost.socketHostGethostname,
    molt_socket_getservbyname_host: socketHost.socketHostGetservbyname,
    molt_socket_getservbyport_host: socketHost.socketHostGetservbyport,
    molt_socket_poll_host: socketHost.socketHostPoll,
    molt_socket_wait_host: socketHost.socketHostWait,
    molt_socket_has_ipv6_host: socketHost.socketHasIpv6Host,
    molt_ws_connect_host: wsHost.wsConnectHost,
    molt_ws_send_host: wsHost.wsSendHost,
    molt_ws_recv_host: wsHost.wsRecvHost,
    molt_ws_close_host: wsHost.wsCloseHost,
  };

  let linkedBytes = null;
  if (preferLinked) {
    linkedBytes = await tryFetch(linkedUrl);
    if (linkedBytes) {
      const imports = parseWasmImports(linkedBytes);
      const hasRuntime = imports.funcImports.some((imp) => imp.module === 'molt_runtime');
      const hasCallIndirect = imports.funcImports.some(
        (imp) => imp.module === 'env' && imp.name.startsWith('molt_call_indirect'),
      );
      if (hasRuntime || hasCallIndirect) {
        linkedBytes = null;
      }
    }
  }

  if (linkedBytes) {
    const linkedImports = parseWasmImports(linkedBytes);
    const memory = makeMemory(linkedImports.memory);
    const table = makeTable(linkedImports.table);
    state.memory = memory;
    const env = buildEnv(memory, table, null, logFn, overrides);
    const importObject = { env, wasi_snapshot_preview1: buildWasiStub() };
    const result = await WebAssembly.instantiate(linkedBytes, importObject);
    const instance = result.instance;
    const memoryExport =
      instance.exports.molt_memory || instance.exports.memory || env.memory || null;
    state.runtimeInstance = instance;
    state.memory = memoryExport || memory || env.memory || null;
    return {
      instance,
      memory: memoryExport || memory || env.memory || null,
      table: instance.exports.molt_table || env.__indirect_function_table || null,
      linked: true,
      run: () => {
        if (typeof instance.exports.molt_main !== 'function') {
          throw new Error('molt_main export missing');
        }
        instance.exports.molt_main();
      },
    };
  }

  const wasmBytes = await tryFetch(wasmUrl);
  if (!wasmBytes) {
    throw new Error(`Failed to load wasm at ${wasmUrl}`);
  }
  const runtimeBytes = await tryFetch(runtimeUrl);
  if (!runtimeBytes) {
    throw new Error(`Failed to load runtime wasm at ${runtimeUrl}`);
  }
  const outputImports = parseWasmImports(wasmBytes);
  const runtimeImports = parseWasmImports(runtimeBytes);
  if (!outputImports.memory || !runtimeImports.memory) {
    throw new Error('Direct-link wasm requires shared memory imports');
  }
  const memory = makeMemory(mergeLimits(outputImports.memory, runtimeImports.memory, 'memory'));
  const table = makeTable(mergeLimits(outputImports.table, runtimeImports.table, 'table'));
  state.memory = memory;
  const callIndirectFns = {};
  const callIndirectNames = runtimeImports.funcImports
    .filter((imp) => imp.module === 'env' && imp.name.startsWith('molt_call_indirect'))
    .map((imp) => imp.name);
  const callIndirect = {};
  for (const name of callIndirectNames) {
    callIndirect[name] = (...args) => {
      const fn = callIndirectFns[name];
      if (!fn) {
        throw new Error(`${name} called before output instantiation`);
      }
      return fn(...args);
    };
  }
  const env = buildEnv(memory, table, callIndirect, logFn, overrides);
  const runtimeModule = await WebAssembly.instantiate(runtimeBytes, {
    env,
    wasi_snapshot_preview1: buildWasiStub(),
  });
  const runtimeInstance = runtimeModule.instance;
  state.runtimeInstance = runtimeInstance;
  const runtimeImportsObj = buildRuntimeImports(outputImports, runtimeInstance);
  const outputModule = await WebAssembly.instantiate(wasmBytes, {
    molt_runtime: runtimeImportsObj,
    env: {
      memory,
      __indirect_function_table: table,
    },
  });
  for (const name of callIndirectNames) {
    const fn = outputModule.instance.exports[name];
    if (typeof fn !== 'function') {
      throw new Error(`WASM output missing export ${name}`);
    }
    callIndirectFns[name] = fn;
  }
  return {
    instance: outputModule.instance,
    memory,
    table,
    linked: false,
    run: () => {
      if (typeof outputModule.instance.exports.molt_main !== 'function') {
        throw new Error('molt_main export missing');
      }
      outputModule.instance.exports.molt_main();
    },
  };
};
