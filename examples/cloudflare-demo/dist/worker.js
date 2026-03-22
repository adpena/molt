// Molt Python on Cloudflare Workers
// Compiled Python -> WASM, running at the edge

import wasmModule from "./worker_linked.wasm";

// MCP JSON-RPC response helper
function mcpResponse(id, result) {
    return new Response(JSON.stringify({ jsonrpc: '2.0', id, result }), {
        headers: {
            'content-type': 'application/json',
            'access-control-allow-origin': '*',
            'access-control-allow-methods': 'POST, OPTIONS',
            'access-control-allow-headers': 'content-type',
        },
    });
}

function mcpError(id, code, message) {
    return new Response(JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } }), {
        status: code === -32600 ? 400 : code === -32601 ? 404 : 500,
        headers: {
            'content-type': 'application/json',
            'access-control-allow-origin': '*',
        },
    });
}

const MCP_TOOLS = [
    {
        name: 'query_sql',
        description: 'Execute SQL query against pre-loaded city and language datasets',
        inputSchema: {
            type: 'object',
            properties: {
                sql: { type: 'string', description: 'SQL query to execute' }
            },
            required: ['sql']
        }
    },
    {
        name: 'fibonacci',
        description: 'Compute the Nth Fibonacci number',
        inputSchema: {
            type: 'object',
            properties: {
                n: { type: 'integer', description: 'Which Fibonacci number (1-10000)' }
            },
            required: ['n']
        }
    },
    {
        name: 'generate_names',
        description: 'Generate fictional names using microGPT (4192-parameter model)',
        inputSchema: {
            type: 'object',
            properties: {
                count: { type: 'integer', description: 'Number of names (1-20)', default: 5 }
            }
        }
    },
    {
        name: 'primes',
        description: 'Find all prime numbers up to N',
        inputSchema: {
            type: 'object',
            properties: {
                n: { type: 'integer', description: 'Upper bound (max 50000)' }
            },
            required: ['n']
        }
    },
    {
        name: 'hash_text',
        description: 'Compute cryptographic hashes (MD5, SHA1, SHA256, SHA512) of text',
        inputSchema: {
            type: 'object',
            properties: {
                text: { type: 'string', description: 'Text to hash' }
            },
            required: ['text']
        }
    }
];

// Map MCP tool call to WASM route + query string
function mcpToolRoute(toolName, args) {
    if (toolName === 'query_sql')       return ['/sql', 'q=' + encodeURIComponent(args.sql || '')];
    if (toolName === 'fibonacci')       return ['/fib/' + (args.n || 10), ''];
    if (toolName === 'generate_names')  return ['/generate/' + (args.count || 5), ''];
    if (toolName === 'primes')          return ['/primes/' + (args.n || 100), ''];
    if (toolName === 'hash_text')       return ['/hash', 'msg=' + encodeURIComponent(args.text || '')];
    return null;
}

// Run the WASM module with a given route path and query string, return stdout text.
async function runWasm(urlPath, queryString) {
    const stdoutChunks = [];
    const stderrChunks = [];
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    let wasmMemory = null;

    const wasiArgs = ["molt", urlPath, queryString];
    const argsEncoded = wasiArgs.map(a => encoder.encode(a + "\0"));
    const argsTotalSize = argsEncoded.reduce((s, a) => s + a.length, 0);

    const envVars = [
        "MOLT_TRUSTED=1",
        ...(queryString ? [`QUERY_STRING=${queryString}`] : []),
    ];
    const envEncoded = envVars.map(e => encoder.encode(e + "\0"));
    const envTotalSize = envEncoded.reduce((s, e) => s + e.length, 0);

    const wasi = {
        fd_write(fd, iovs, iovsLen, nwritten) {
            if ((fd === 1 || fd === 2) && wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                let totalWritten = 0;
                for (let i = 0; i < iovsLen; i++) {
                    const ptr = view.getUint32(iovs + i * 8, true);
                    const len = view.getUint32(iovs + i * 8 + 4, true);
                    const bytes = new Uint8Array(wasmMemory.buffer, ptr, len);
                    const text = decoder.decode(bytes, { stream: true });
                    if (fd === 1) stdoutChunks.push(text);
                    else stderrChunks.push(text);
                    totalWritten += len;
                }
                view.setUint32(nwritten, totalWritten, true);
            }
            return 0;
        },
        fd_read() { return 0; },
        fd_close() { return 0; },
        fd_seek() { return 0; },
        fd_prestat_get() { return 8; },
        fd_prestat_dir_name() { return 8; },
        fd_fdstat_get(fd, statPtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                const filetype = (fd <= 2) ? 2 : 4;
                view.setUint8(statPtr, filetype);
                view.setUint16(statPtr + 2, 0, true);
                view.setBigUint64(statPtr + 8, 0xFFFFFFFFFFFFFFFFn, true);
                view.setBigUint64(statPtr + 16, 0xFFFFFFFFFFFFFFFFn, true);
            }
            return 0;
        },
        fd_tell() { return 0; },
        fd_filestat_get(fd, bufPtr) {
            if (wasmMemory) {
                const bytes = new Uint8Array(wasmMemory.buffer, bufPtr, 64);
                bytes.fill(0);
            }
            return 0;
        },
        fd_filestat_set_size() { return 0; },
        fd_readdir() { return 0; },
        environ_sizes_get(countPtr, sizePtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                view.setUint32(countPtr, envVars.length, true);
                view.setUint32(sizePtr, envTotalSize, true);
            }
            return 0;
        },
        environ_get(environPtr, environBufPtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                let bufOffset = environBufPtr;
                for (let i = 0; i < envEncoded.length; i++) {
                    view.setUint32(environPtr + i * 4, bufOffset, true);
                    const bytes = new Uint8Array(wasmMemory.buffer, bufOffset, envEncoded[i].length);
                    bytes.set(envEncoded[i]);
                    bufOffset += envEncoded[i].length;
                }
            }
            return 0;
        },
        args_sizes_get(countPtr, sizePtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                view.setUint32(countPtr, wasiArgs.length, true);
                view.setUint32(sizePtr, argsTotalSize, true);
            }
            return 0;
        },
        args_get(argvPtr, argvBufPtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                let bufOffset = argvBufPtr;
                for (let i = 0; i < argsEncoded.length; i++) {
                    view.setUint32(argvPtr + i * 4, bufOffset, true);
                    const bytes = new Uint8Array(wasmMemory.buffer, bufOffset, argsEncoded[i].length);
                    bytes.set(argsEncoded[i]);
                    bufOffset += argsEncoded[i].length;
                }
            }
            return 0;
        },
        clock_time_get(id, precision, outPtr) {
            if (wasmMemory) {
                const view = new DataView(wasmMemory.buffer);
                view.setBigUint64(outPtr, BigInt(Date.now()) * 1000000n, true);
            }
            return 0;
        },
        random_get(ptr, len) {
            if (wasmMemory) {
                const bytes = new Uint8Array(wasmMemory.buffer, ptr, len);
                crypto.getRandomValues(bytes);
            }
            return 0;
        },
        proc_exit(code) { throw new ProcExit(code); },
        sched_yield() { return 0; },
        poll_oneoff() { return 0; },
        path_open() { return 44; },
        path_filestat_get() { return 44; },
        path_rename() { return 44; },
        path_readlink() { return 44; },
        path_unlink_file() { return 44; },
        path_create_directory() { return 44; },
        path_remove_directory() { return 44; },
    };

    const hostEnv = {
        __indirect_function_table: new WebAssembly.Table({ initial: 8192, element: "anyfunc" }),
        molt_time_timezone_host()  { return 0n; },
        molt_time_local_offset_host() { return 0n; },
        molt_getpid_host()         { return 1n; },
        molt_socket_clone_host()   { return -1n; },
        molt_socket_detach_host()  { return -1n; },
        molt_socket_accept_host()  { return -1n; },
        molt_socket_new_host()     { return -1n; },
        molt_time_tzname_host()    { return -1; },
        molt_process_write_host()  { return -1; },
        molt_process_close_stdin_host() { return -1; },
        molt_socket_wait_host()    { return -1; },
        molt_db_exec_host()        { return -1; },
        molt_db_query_host()       { return -1; },
        molt_ws_recv_host()        { return -1; },
        molt_ws_send_host()        { return -1; },
        molt_ws_close_host()       { return -1; },
        molt_socket_poll_host()    { return 0; },
        molt_ws_poll_host()        { return 0; },
        molt_process_terminate_host() { return -1; },
        molt_os_close_host()       { return 0; },
        molt_process_kill_host()   { return -1; },
        molt_process_wait_host()   { return -1; },
        molt_process_spawn_host()  { return -1; },
        molt_process_stdio_host()  { return -1; },
        molt_socket_bind_host()    { return -1; },
        molt_socket_close_host()   { return 0; },
        molt_socket_connect_host() { return -1; },
        molt_socket_connect_ex_host() { return -1; },
        molt_socket_getaddrinfo_host() { return -1; },
        molt_socket_gethostname_host() { return -1; },
        molt_socket_getpeername_host() { return -1; },
        molt_socket_getservbyname_host() { return -1; },
        molt_socket_getservbyport_host() { return -1; },
        molt_socket_getsockname_host() { return -1; },
        molt_socket_getsockopt_host() { return -1; },
        molt_socket_has_ipv6_host() { return 0; },
        molt_socket_listen_host()  { return -1; },
        molt_socket_recv_host()    { return -1; },
        molt_socket_recvfrom_host() { return -1; },
        molt_socket_recvmsg_host() { return -1; },
        molt_socket_send_host()    { return -1; },
        molt_socket_sendmsg_host() { return -1; },
        molt_socket_sendto_host()  { return -1; },
        molt_socket_setsockopt_host() { return -1; },
        molt_socket_shutdown_host() { return -1; },
        molt_socket_socketpair_host() { return -1; },
        molt_db_host_poll()        { return 0; },
        molt_process_host_poll()   { return 0; },
        molt_ws_connect_host()     { return -1; },
    };

    const imports = {
        wasi_snapshot_preview1: wasi,
        env: hostEnv,
        molt_runtime: { molt_hash_drop() { return -1n; } },
    };

    try {
        const instance = await WebAssembly.instantiate(wasmModule, imports);
        wasmMemory = instance.exports.memory;
        if (instance.exports.molt_table_init) instance.exports.molt_table_init();
        if (instance.exports.molt_main) instance.exports.molt_main();
        else if (instance.exports._start) instance.exports._start();
    } catch (err) {
        if (!(err instanceof ProcExit)) throw err;
        // ProcExit is normal termination — stdout is already captured
    }

    return stdoutChunks.join("");
}

export default {
    async fetch(request, env, ctx) {
        // CORS preflight for MCP endpoint
        if (request.method === "OPTIONS") {
            return new Response(null, {
                status: 204,
                headers: {
                    'access-control-allow-origin': '*',
                    'access-control-allow-methods': 'GET, HEAD, POST, OPTIONS',
                    'access-control-allow-headers': 'content-type',
                    'access-control-max-age': '86400',
                },
            });
        }

        // MCP JSON-RPC endpoint
        if (request.method === "POST") {
            const url = new URL(request.url);
            if (url.pathname !== "/mcp") {
                return new Response("Not found\n", {
                    status: 404,
                    headers: { "content-type": "text/plain; charset=utf-8" },
                });
            }

            const ct = (request.headers.get("content-type") || "").toLowerCase();
            if (!ct.includes("application/json")) {
                return mcpError(null, -32600, "Content-Type must be application/json");
            }

            let body;
            try { body = await request.json(); } catch {
                return mcpError(null, -32700, "Parse error");
            }

            const method = body.method;
            const id = body.id !== undefined ? body.id : null;

            if (method === "initialize") {
                return mcpResponse(id, {
                    protocolVersion: '2024-11-05',
                    capabilities: { tools: {} },
                    serverInfo: { name: 'molt-edge', version: '0.1.0' }
                });
            }

            if (method === "notifications/initialized") {
                // Client ack — no response required for notifications
                return new Response(null, { status: 204 });
            }

            if (method === "tools/list") {
                return mcpResponse(id, { tools: MCP_TOOLS });
            }

            if (method === "tools/call") {
                const toolName = (body.params || {}).name;
                const args = (body.params || {}).arguments || {};
                const mapping = mcpToolRoute(toolName, args);

                if (!mapping) {
                    return mcpError(id, -32602, "Unknown tool: " + toolName);
                }

                const [routePath, queryString] = mapping;

                // Run the WASM module with the mapped route
                const output = await runWasm(routePath, queryString);

                return mcpResponse(id, {
                    content: [{ type: 'text', text: output }]
                });
            }

            return mcpError(id, -32601, "Method not found: " + method);
        }

        // Only allow GET and HEAD for non-MCP routes
        if (request.method !== "GET" && request.method !== "HEAD") {
            return new Response("Method not allowed\n", {
                status: 405,
                headers: { "content-type": "text/plain; charset=utf-8", "allow": "GET, HEAD, POST" },
            });
        }

        const startTime = Date.now();
        const stdoutChunks = [];
        const stderrChunks = [];
        const decoder = new TextDecoder();
        const encoder = new TextEncoder();
        let wasmMemory = null;
        const hostCalls = [];

        // Parse request URL for routing
        const url = new URL(request.url);
        const urlPath = url.pathname;
        const queryString = url.search ? url.search.slice(1) : ""; // remove leading '?'

        // Derive route name for response header
        const routeParts = urlPath.replace(/^\//, "").split("/");
        const route = routeParts[0] || "index";

        // Prepare WASI args: ["molt", urlPath, queryString]
        const wasiArgs = ["molt", urlPath, queryString];
        const argsEncoded = wasiArgs.map(a => encoder.encode(a + "\0"));
        const argsTotalSize = argsEncoded.reduce((s, a) => s + a.length, 0);

        // Prepare WASI environ
        // MOLT_TRUSTED=1 enables os/sys stdlib modules (capability gate).
        // QUERY_STRING passes the URL query to the Python app.
        const envVars = [
            "MOLT_TRUSTED=1",
            ...(queryString ? [`QUERY_STRING=${queryString}`] : []),
        ];
        const envEncoded = envVars.map(e => encoder.encode(e + "\0"));
        const envTotalSize = envEncoded.reduce((s, e) => s + e.length, 0);

        try {
            // Minimal WASI shim
            const wasi = {
                fd_write(fd, iovs, iovsLen, nwritten) {
                    if ((fd === 1 || fd === 2) && wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        let totalWritten = 0;
                        for (let i = 0; i < iovsLen; i++) {
                            const ptr = view.getUint32(iovs + i * 8, true);
                            const len = view.getUint32(iovs + i * 8 + 4, true);
                            const bytes = new Uint8Array(wasmMemory.buffer, ptr, len);
                            const text = decoder.decode(bytes, { stream: true });
                            if (fd === 1) stdoutChunks.push(text);
                            else stderrChunks.push(text);
                            totalWritten += len;
                        }
                        view.setUint32(nwritten, totalWritten, true);
                    }
                    return 0;
                },
                fd_read() { return 0; },
                fd_close() { return 0; },
                fd_seek(fd, offsetLo, offsetHi, whence, newOffsetPtr) { return 0; },
                fd_prestat_get() { return 8; },   // EBADF - no preopens
                fd_prestat_dir_name() { return 8; },
                fd_fdstat_get(fd, statPtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        // fd 0/1/2 are character devices; others are regular files
                        const filetype = (fd <= 2) ? 2 : 4;
                        view.setUint8(statPtr, filetype);
                        view.setUint16(statPtr + 2, 0, true);
                        // rights_base and rights_inheriting = all bits
                        view.setBigUint64(statPtr + 8, 0xFFFFFFFFFFFFFFFFn, true);
                        view.setBigUint64(statPtr + 16, 0xFFFFFFFFFFFFFFFFn, true);
                    }
                    return 0;
                },
                fd_tell() { return 0; },
                fd_filestat_get(fd, bufPtr) {
                    if (wasmMemory) {
                        // Zero out the filestat struct (64 bytes)
                        const bytes = new Uint8Array(wasmMemory.buffer, bufPtr, 64);
                        bytes.fill(0);
                    }
                    return 0;
                },
                fd_filestat_set_size() { return 0; },
                fd_readdir() { return 0; },
                environ_sizes_get(countPtr, sizePtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        view.setUint32(countPtr, envVars.length, true);
                        view.setUint32(sizePtr, envTotalSize, true);
                    }
                    return 0;
                },
                environ_get(environPtr, environBufPtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        let bufOffset = environBufPtr;
                        for (let i = 0; i < envEncoded.length; i++) {
                            // Write pointer to this env string
                            view.setUint32(environPtr + i * 4, bufOffset, true);
                            // Write the string data
                            const bytes = new Uint8Array(wasmMemory.buffer, bufOffset, envEncoded[i].length);
                            bytes.set(envEncoded[i]);
                            bufOffset += envEncoded[i].length;
                        }
                    }
                    return 0;
                },
                args_sizes_get(countPtr, sizePtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        view.setUint32(countPtr, wasiArgs.length, true);
                        view.setUint32(sizePtr, argsTotalSize, true);
                    }
                    return 0;
                },
                args_get(argvPtr, argvBufPtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        let bufOffset = argvBufPtr;
                        for (let i = 0; i < argsEncoded.length; i++) {
                            // Write pointer to this arg string
                            view.setUint32(argvPtr + i * 4, bufOffset, true);
                            // Write the string data
                            const bytes = new Uint8Array(wasmMemory.buffer, bufOffset, argsEncoded[i].length);
                            bytes.set(argsEncoded[i]);
                            bufOffset += argsEncoded[i].length;
                        }
                    }
                    return 0;
                },
                clock_time_get(id, precision, outPtr) {
                    if (wasmMemory) {
                        const view = new DataView(wasmMemory.buffer);
                        view.setBigUint64(outPtr, BigInt(Date.now()) * 1000000n, true);
                    }
                    return 0;
                },
                random_get(ptr, len) {
                    if (wasmMemory) {
                        const bytes = new Uint8Array(wasmMemory.buffer, ptr, len);
                        crypto.getRandomValues(bytes);
                    }
                    return 0;
                },
                proc_exit(code) {
                    throw new ProcExit(code);
                },
                sched_yield() { return 0; },
                poll_oneoff() { return 0; },
                path_open() { return 44; },          // ENOENT
                path_filestat_get() { return 44; },
                path_rename() { return 44; },
                path_readlink() { return 44; },
                path_unlink_file() { return 44; },
                path_create_directory() { return 44; },
                path_remove_directory() { return 44; },
            };

            // Host function stubs for env imports.
            // These are I/O operations (socket, DB, process, websocket, etc.)
            // that are not available in the Cloudflare Workers environment.
            // Each must return the correct WASM type (i32 vs i64).
            //
            // For i32-returning functions: return -1 (error/unsupported)
            // For i64-returning functions: return -1n (error/unsupported)
            // Exceptions: _poll and _has_ functions return 0/0n (nothing ready/false)

            const hostEnv = {
                __indirect_function_table: new WebAssembly.Table({ initial: 8192, element: "anyfunc" }),

                // --- Returns i64 ---
                molt_time_timezone_host()  { hostCalls.push("timezone"); return 0n; },
                molt_time_local_offset_host(ts) { hostCalls.push("local_offset"); return 0n; },
                molt_getpid_host()         { hostCalls.push("getpid"); return 1n; },
                molt_socket_clone_host(h)  { return -1n; },
                molt_socket_detach_host(h) { return -1n; },
                molt_socket_accept_host(h, addrPtr, addrLen, flagsPtr) { return -1n; },
                molt_socket_new_host(domain, socktype, protocol, handle) { return -1n; },

                // --- Returns i32 ---
                molt_time_tzname_host(buf, bufLen, isDst, outLen) { return -1; },
                molt_process_write_host(h, ptr, len)   { return -1; },
                molt_process_close_stdin_host(h)       { return -1; },
                molt_socket_wait_host(h, ptr, timeout)  { return -1; },
                molt_db_exec_host(a, b, c, d)          { return -1; },
                molt_db_query_host(a, b, c, d)         { return -1; },
                molt_ws_recv_host(h, buf, len, flags)  { return -1; },
                molt_ws_send_host(h, ptr, len)         { return -1; },
                molt_ws_close_host(h)                  { return -1; },
                molt_socket_poll_host(h, events)       { return 0; },  // nothing ready
                molt_ws_poll_host(h, events)           { return 0; },  // nothing ready
                molt_process_terminate_host(h)         { return -1; },
                molt_os_close_host(h)                  { return 0; },  // OK - close is idempotent
                molt_process_kill_host(h)              { return -1; },
                molt_process_wait_host(h, status, flags) { return -1; },
                molt_process_spawn_host(a,b,c,d,e,f,g,h,i,j) { return -1; },
                molt_process_stdio_host(h, ptr, len)   { return -1; },
                molt_socket_bind_host(h, ptr, len)     { return -1; },
                molt_socket_close_host(h)              { return 0; },  // OK - close is idempotent
                molt_socket_connect_host(h, ptr, len)  { return -1; },
                molt_socket_connect_ex_host(h)         { return -1; },
                molt_socket_getaddrinfo_host(a,b,c,d,e,f,g,h,i,j,k) { return -1; },
                molt_socket_gethostname_host(buf, len, outLen) { return -1; },
                molt_socket_getpeername_host(h, buf, len, outLen) { return -1; },
                molt_socket_getservbyname_host(a,b,c,d) { return -1; },
                molt_socket_getservbyport_host(a,b,c,d,e,f) { return -1; },
                molt_socket_getsockname_host(h, buf, len, outLen) { return -1; },
                molt_socket_getsockopt_host(h, level, opt, val, valLen, outLen) { return -1; },
                molt_socket_has_ipv6_host()            { return 0; },  // no IPv6
                molt_socket_listen_host(h, backlog)    { return -1; },
                molt_socket_recv_host(h, buf, len, flags) { return -1; },
                molt_socket_recvfrom_host(h, buf, len, flags, addr, addrLen, outLen) { return -1; },
                molt_socket_recvmsg_host(h, a,b,c,d,e,f,g,i,j,k) { return -1; },
                molt_socket_send_host(h, buf, len, flags) { return -1; },
                molt_socket_sendmsg_host(h, a,b,c,d,e,f,g) { return -1; },
                molt_socket_sendto_host(h, buf, len, flags, addr, addrLen) { return -1; },
                molt_socket_setsockopt_host(h, level, opt, val, valLen) { return -1; },
                molt_socket_shutdown_host(h, how)      { return -1; },
                molt_socket_socketpair_host(a,b,c,d,e) { return -1; },
                molt_db_host_poll()                    { return 0; },  // nothing ready
                molt_process_host_poll()               { return 0; },  // nothing ready
                molt_ws_connect_host(ptr, handle, len) { return -1; },
            };

            // Stubs for runtime functions that may be missing from micro-profile builds
            const runtimeStubs = {
                molt_hash_drop() { return -1n; },
            };

            const imports = {
                wasi_snapshot_preview1: wasi,
                env: hostEnv,
                molt_runtime: runtimeStubs,
            };

            // Instantiate
            const instance = await WebAssembly.instantiate(wasmModule, imports);

            // Get memory from exports
            wasmMemory = instance.exports.memory;

            // Initialize table from the module's own table init function
            if (instance.exports.molt_table_init) {
                instance.exports.molt_table_init();
            }

            // Call entry point
            if (instance.exports.molt_main) {
                instance.exports.molt_main();
            } else if (instance.exports._start) {
                instance.exports._start();
            }

            const elapsed = Date.now() - startTime;
            const output = stdoutChunks.join("");

            // Detect HTML output and set content-type accordingly
            const trimmed = output.trimStart();
            const contentType = (trimmed.startsWith("<!DOCTYPE html>") || trimmed.startsWith("<html"))
                ? "text/html; charset=utf-8"
                : "text/plain; charset=utf-8";

            return new Response(output || JSON.stringify({
                status: "ok", runtime: "molt-wasm", elapsed_ms: elapsed
            }, null, 2), {
                headers: {
                    "content-type": contentType,
                    "x-molt-runtime": "wasm",
                    "x-content-type-options": "nosniff",
                    "x-molt-elapsed-ms": String(elapsed),
                    "x-molt-route": route,
                },
            });
        } catch (err) {
            const elapsed = Date.now() - startTime;
            const output = stdoutChunks.join("");
            const trimmedErr = output.trimStart();
            const errContentType = (trimmedErr.startsWith("<!DOCTYPE html>") || trimmedErr.startsWith("<html"))
                ? "text/html; charset=utf-8"
                : "text/plain; charset=utf-8";
            const hdrs = {
                "content-type": errContentType,
                "x-molt-runtime": "wasm",
                "x-content-type-options": "nosniff",
                "x-molt-elapsed-ms": String(elapsed),
                "x-molt-route": route,
            };

            // Clean program exit (sys.exit(0) or normal termination)
            if (err instanceof ProcExit && err.code === 0) {
                return new Response(output || "OK\n", { headers: hdrs });
            }

            // Non-zero exit (bad input, unhandled exception in Python)
            if (err instanceof ProcExit) {
                return new Response(output || "Error\n", {
                    status: 400,
                    headers: hdrs,
                });
            }

            // Runtime crash — log details internally, never expose to client
            console.error("molt runtime error:", err.message);
            console.error("stack:", err.stack);
            if (stderrChunks.length) console.error("stderr:", stderrChunks.join(""));
            return new Response("Internal server error\n", {
                status: 500,
                headers: {
                    "content-type": "text/plain; charset=utf-8",
                    "x-molt-runtime": "wasm",
                    "x-content-type-options": "nosniff",
                    "x-molt-elapsed-ms": String(elapsed),
                },
            });
        }
    }
};

class ProcExit extends Error {
    constructor(code) {
        super(`proc_exit(${code})`);
        this.code = code;
    }
}
