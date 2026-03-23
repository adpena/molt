// db-bridge.ts -- Bridge molt_db_*_host WASM imports to Durable Object SQLite
//
// The molt runtime calls molt_db_query_host / molt_db_exec_host with:
//   (req_ptr: u64, len: u64, out_ptr: u64, token: u64) -> i32
// where req_ptr points to a UTF-8 SQL string of `len` bytes in WASM memory,
// and out_ptr is where we write the result handle (8 bytes, little-endian).
//
// In the native host, results are streamed back asynchronously. In the DO
// environment we run synchronously against ctx.storage.sql and write
// the result directly into WASM memory as a JSON-encoded UTF-8 string.

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

// Error codes matching the Rust runtime conventions
const OK = 0;
const ERR_INVALID = 1;
const ERR_NULL_OUT = 2;
const ERR_UNAVAILABLE = 7;

/**
 * Allocator interface: the WASM module must export molt_alloc(size) -> ptr
 * so we can write variable-length results back into linear memory.
 */
interface WasmExports {
  memory: WebAssembly.Memory;
  molt_alloc?: (size: number) => number;
  molt_table_init?: () => void;
  molt_main?: () => void;
  _start?: () => void;
}

/**
 * State passed through the bridge so host functions can access DO storage
 * and WASM memory.
 */
export interface BridgeState {
  sql: SqlStorage;
  memory: WebAssembly.Memory | null;
  exports: WasmExports | null;
  /** Pending results keyed by a monotonic counter (written to out_ptr). */
  results: Map<number, Uint8Array>;
  nextResultId: number;
}

export function createBridgeState(sql: SqlStorage): BridgeState {
  return {
    sql,
    memory: null,
    exports: null,
    results: new Map(),
    nextResultId: 1,
  };
}

// ---------------------------------------------------------------------------
// Read a UTF-8 string from WASM memory
// ---------------------------------------------------------------------------
function readString(memory: WebAssembly.Memory, ptr: number, len: number): string {
  const bytes = new Uint8Array(memory.buffer, ptr, len);
  return DECODER.decode(bytes);
}

// ---------------------------------------------------------------------------
// Write a result id (u64 LE) at out_ptr in WASM memory
// ---------------------------------------------------------------------------
function writeResultId(memory: WebAssembly.Memory, outPtr: number, id: number): void {
  const view = new DataView(memory.buffer);
  // Write as u64 little-endian (two u32s)
  view.setUint32(outPtr, id, true);
  view.setUint32(outPtr + 4, 0, true);
}

// ---------------------------------------------------------------------------
// Run a SQL query and serialize rows to JSON bytes
// ---------------------------------------------------------------------------
function runQuery(sql: SqlStorage, query: string): Uint8Array {
  try {
    const cursor = sql.exec(query);
    const rows: Record<string, unknown>[] = [];
    for (const row of cursor) {
      rows.push({ ...row });
    }
    return ENCODER.encode(JSON.stringify({ ok: true, rows }));
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    return ENCODER.encode(JSON.stringify({ ok: false, error: msg }));
  }
}

// ---------------------------------------------------------------------------
// Run a SQL statement (INSERT/UPDATE/DELETE) and return affected info
// ---------------------------------------------------------------------------
function runStatement(sql: SqlStorage, statement: string): Uint8Array {
  try {
    const cursor = sql.exec(statement);
    // For write statements, rowsWritten is the relevant metric
    return ENCODER.encode(JSON.stringify({
      ok: true,
      rowsWritten: cursor.rowsWritten,
      rowsRead: cursor.rowsRead,
    }));
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err);
    return ENCODER.encode(JSON.stringify({ ok: false, error: msg }));
  }
}

// ---------------------------------------------------------------------------
// Host function implementations
// ---------------------------------------------------------------------------

export function createDbHostFunctions(state: BridgeState) {
  return {
    /**
     * molt_db_query_host(req_ptr, len, out_ptr, token) -> i32
     * Read SQL from WASM memory, run as a query, store result for polling.
     */
    molt_db_query_host(reqPtr: number, len: number, outPtr: number, _token: number): number {
      if (!state.memory) return ERR_UNAVAILABLE;
      if (outPtr === 0) return ERR_NULL_OUT;
      if (reqPtr === 0 && len !== 0) return ERR_INVALID;

      const query = len > 0 ? readString(state.memory, reqPtr, len) : "";
      const resultBytes = runQuery(state.sql, query);

      const id = state.nextResultId++;
      state.results.set(id, resultBytes);
      writeResultId(state.memory, outPtr, id);
      return OK;
    },

    /**
     * molt_db_exec_host(req_ptr, len, out_ptr, token) -> i32
     * Read SQL from WASM memory, run as a statement, store result.
     */
    molt_db_exec_host(reqPtr: number, len: number, outPtr: number, _token: number): number {
      if (!state.memory) return ERR_UNAVAILABLE;
      if (outPtr === 0) return ERR_NULL_OUT;
      if (reqPtr === 0 && len !== 0) return ERR_INVALID;

      const statement = len > 0 ? readString(state.memory, reqPtr, len) : "";
      const resultBytes = runStatement(state.sql, statement);

      const id = state.nextResultId++;
      state.results.set(id, resultBytes);
      writeResultId(state.memory, outPtr, id);
      return OK;
    },

    /**
     * molt_db_host_poll() -> i32
     * In the DO environment all queries are synchronous, so poll always
     * returns 0 (nothing pending / all complete).
     */
    molt_db_host_poll(): number {
      return 0;
    },
  };
}

// ---------------------------------------------------------------------------
// Full WASI shim for running molt WASM inside a Durable Object
// Mirrors the pattern from examples/cloudflare-demo/dist/worker.js
// ---------------------------------------------------------------------------

class ProcExit extends Error {
  code: number;
  constructor(code: number) {
    super(`proc_exit(${code})`);
    this.code = code;
  }
}

export { ProcExit };

export function buildWasiShim(
  getMemory: () => WebAssembly.Memory | null,
  wasiArgs: string[],
  envVars: string[],
  stdoutChunks: string[],
  stderrChunks: string[],
) {
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  const argsEncoded = wasiArgs.map((a) => encoder.encode(a + "\0"));
  const argsTotalSize = argsEncoded.reduce((s, a) => s + a.length, 0);
  const envEncoded = envVars.map((e) => encoder.encode(e + "\0"));
  const envTotalSize = envEncoded.reduce((s, e) => s + e.length, 0);

  return {
    fd_write(fd: number, iovs: number, iovsLen: number, nwritten: number): number {
      const mem = getMemory();
      if ((fd === 1 || fd === 2) && mem) {
        const view = new DataView(mem.buffer);
        let totalWritten = 0;
        for (let i = 0; i < iovsLen; i++) {
          const ptr = view.getUint32(iovs + i * 8, true);
          const len = view.getUint32(iovs + i * 8 + 4, true);
          const bytes = new Uint8Array(mem.buffer, ptr, len);
          const text = decoder.decode(bytes, { stream: true });
          if (fd === 1) stdoutChunks.push(text);
          else stderrChunks.push(text);
          totalWritten += len;
        }
        view.setUint32(nwritten, totalWritten, true);
      }
      return 0;
    },
    fd_read(): number { return 0; },
    fd_close(): number { return 0; },
    fd_seek(): number { return 0; },
    fd_prestat_get(): number { return 8; },
    fd_prestat_dir_name(): number { return 8; },
    fd_fdstat_get(fd: number, statPtr: number): number {
      const mem = getMemory();
      if (mem) {
        const view = new DataView(mem.buffer);
        const filetype = fd <= 2 ? 2 : 4;
        view.setUint8(statPtr, filetype);
        view.setUint16(statPtr + 2, 0, true);
        view.setBigUint64(statPtr + 8, 0xFFFFFFFFFFFFFFFFn, true);
        view.setBigUint64(statPtr + 16, 0xFFFFFFFFFFFFFFFFn, true);
      }
      return 0;
    },
    fd_tell(): number { return 0; },
    fd_filestat_get(_fd: number, bufPtr: number): number {
      const mem = getMemory();
      if (mem) {
        new Uint8Array(mem.buffer, bufPtr, 64).fill(0);
      }
      return 0;
    },
    fd_filestat_set_size(): number { return 0; },
    fd_readdir(): number { return 0; },
    environ_sizes_get(countPtr: number, sizePtr: number): number {
      const mem = getMemory();
      if (mem) {
        const view = new DataView(mem.buffer);
        view.setUint32(countPtr, envVars.length, true);
        view.setUint32(sizePtr, envTotalSize, true);
      }
      return 0;
    },
    environ_get(environPtr: number, environBufPtr: number): number {
      const mem = getMemory();
      if (mem) {
        const view = new DataView(mem.buffer);
        let bufOffset = environBufPtr;
        for (let i = 0; i < envEncoded.length; i++) {
          view.setUint32(environPtr + i * 4, bufOffset, true);
          new Uint8Array(mem.buffer, bufOffset, envEncoded[i].length).set(envEncoded[i]);
          bufOffset += envEncoded[i].length;
        }
      }
      return 0;
    },
    args_sizes_get(countPtr: number, sizePtr: number): number {
      const mem = getMemory();
      if (mem) {
        const view = new DataView(mem.buffer);
        view.setUint32(countPtr, wasiArgs.length, true);
        view.setUint32(sizePtr, argsTotalSize, true);
      }
      return 0;
    },
    args_get(argvPtr: number, argvBufPtr: number): number {
      const mem = getMemory();
      if (mem) {
        const view = new DataView(mem.buffer);
        let bufOffset = argvBufPtr;
        for (let i = 0; i < argsEncoded.length; i++) {
          view.setUint32(argvPtr + i * 4, bufOffset, true);
          new Uint8Array(mem.buffer, bufOffset, argsEncoded[i].length).set(argsEncoded[i]);
          bufOffset += argsEncoded[i].length;
        }
      }
      return 0;
    },
    clock_time_get(_id: number, _precision: bigint, outPtr: number): number {
      const mem = getMemory();
      if (mem) {
        new DataView(mem.buffer).setBigUint64(outPtr, BigInt(Date.now()) * 1000000n, true);
      }
      return 0;
    },
    random_get(ptr: number, len: number): number {
      const mem = getMemory();
      if (mem) {
        crypto.getRandomValues(new Uint8Array(mem.buffer, ptr, len));
      }
      return 0;
    },
    proc_exit(code: number): void { throw new ProcExit(code); },
    sched_yield(): number { return 0; },
    poll_oneoff(): number { return 0; },
    path_open(): number { return 44; },
    path_filestat_get(): number { return 44; },
    path_rename(): number { return 44; },
    path_readlink(): number { return 44; },
    path_unlink_file(): number { return 44; },
    path_create_directory(): number { return 44; },
    path_remove_directory(): number { return 44; },
  };
}

// ---------------------------------------------------------------------------
// Stub host env functions for unsupported molt imports
// Matches the signatures from examples/cloudflare-demo/dist/worker.js
// ---------------------------------------------------------------------------
export function createHostStubs() {
  return {
    molt_time_timezone_host(): bigint { return 0n; },
    molt_time_local_offset_host(): bigint { return 0n; },
    molt_getpid_host(): bigint { return 1n; },
    molt_socket_clone_host(): bigint { return -1n; },
    molt_socket_detach_host(): bigint { return -1n; },
    molt_socket_accept_host(): bigint { return -1n; },
    molt_socket_new_host(): bigint { return -1n; },
    molt_time_tzname_host(): number { return -1; },
    molt_process_write_host(): number { return -1; },
    molt_process_close_stdin_host(): number { return -1; },
    molt_socket_wait_host(): number { return -1; },
    molt_ws_recv_host(): number { return -1; },
    molt_ws_send_host(): number { return -1; },
    molt_ws_close_host(): number { return -1; },
    molt_socket_poll_host(): number { return 0; },
    molt_ws_poll_host(): number { return 0; },
    molt_process_terminate_host(): number { return -1; },
    molt_os_close_host(): number { return 0; },
    molt_process_kill_host(): number { return -1; },
    molt_process_wait_host(): number { return -1; },
    molt_process_spawn_host(): number { return -1; },
    molt_process_stdio_host(): number { return -1; },
    molt_socket_bind_host(): number { return -1; },
    molt_socket_close_host(): number { return 0; },
    molt_socket_connect_host(): number { return -1; },
    molt_socket_connect_ex_host(): number { return -1; },
    molt_socket_getaddrinfo_host(): number { return -1; },
    molt_socket_gethostname_host(): number { return -1; },
    molt_socket_getpeername_host(): number { return -1; },
    molt_socket_getservbyname_host(): number { return -1; },
    molt_socket_getservbyport_host(): number { return -1; },
    molt_socket_getsockname_host(): number { return -1; },
    molt_socket_getsockopt_host(): number { return -1; },
    molt_socket_has_ipv6_host(): number { return 0; },
    molt_socket_listen_host(): number { return -1; },
    molt_socket_recv_host(): number { return -1; },
    molt_socket_recvfrom_host(): number { return -1; },
    molt_socket_recvmsg_host(): number { return -1; },
    molt_socket_send_host(): number { return -1; },
    molt_socket_sendmsg_host(): number { return -1; },
    molt_socket_sendto_host(): number { return -1; },
    molt_socket_setsockopt_host(): number { return -1; },
    molt_socket_shutdown_host(): number { return -1; },
    molt_socket_socketpair_host(): number { return -1; },
    molt_db_host_poll(): number { return 0; },
    molt_process_host_poll(): number { return 0; },
    molt_ws_connect_host(): number { return -1; },
  };
}
