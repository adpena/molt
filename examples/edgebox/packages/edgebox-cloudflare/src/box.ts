// box.ts -- EdgeBox Durable Object
//
// Each EdgeBox instance is keyed by a GitHub PR identifier (e.g. gh/owner/repo/pr/123).
// It owns a SQLite database (via DO storage) and invokes the precompiled Python
// WASM module shipped with this Worker to handle each request.

import boxModule from "../../../dist/box.wasm";

import {
  ProcExit,
  buildWasiShim,
  createBridgeState,
  createDbHostFunctions,
  createHostStubs,
} from "./db-bridge";

interface Env {
  BOX_SCHEMA_SQL: string;
}

export class EdgeBox implements DurableObject {
  private state: DurableObjectState;
  private env: Env;
  private schemaApplied = false;

  constructor(state: DurableObjectState, env: Env) {
    this.state = state;
    this.env = env;

    // Apply schema before any concurrent requests
    state.blockConcurrencyWhile(async () => {
      await this.applySchema();
    });
  }

  // -------------------------------------------------------------------------
  // Schema initialization
  // -------------------------------------------------------------------------

  private async applySchema(): Promise<void> {
    if (this.schemaApplied) return;

    const sql = this.state.storage.sql;
    const schema = this.env.BOX_SCHEMA_SQL;
    if (schema) {
      // Split on semicolons and run each statement
      const statements = schema.split(";").map((s) => s.trim()).filter((s) => s.length > 0);
      for (const stmt of statements) {
        sql.exec(stmt + ";");
      }
    }

    this.schemaApplied = true;
  }

  // -------------------------------------------------------------------------
  // Run the WASM module with a given request context
  // -------------------------------------------------------------------------

  private async runWasm(
    path: string,
    queryString: string,
    method: string,
    headers: Record<string, string>,
    body: string,
  ): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    const bridge = createBridgeState(this.state.storage.sql);

    const stdoutChunks: string[] = [];
    const stderrChunks: string[] = [];

    // WASI args: ["molt", path, queryString]
    const wasiArgs = ["molt", path, queryString];

    // Environment variables encode request metadata for the Python BoxRequest.from_env()
    const envVars = [
      "MOLT_TRUSTED=1",
      `EDGEBOX_METHOD=${method}`,
      `EDGEBOX_PATH=${path}`,
      `EDGEBOX_HEADERS=${JSON.stringify(headers)}`,
      ...(queryString ? [`QUERY_STRING=${queryString}`, `EDGEBOX_QUERY=${queryString}`] : []),
      ...(body ? [`EDGEBOX_BODY=${body}`] : []),
    ];

    const wasi = buildWasiShim(
      () => bridge.memory,
      wasiArgs,
      envVars,
      stdoutChunks,
      stderrChunks,
    );

    // Build the DB host functions that bridge to DO SQLite
    const dbFunctions = createDbHostFunctions(bridge);

    // Merge host stubs with live DB functions
    const hostStubs = createHostStubs();
    const hostEnv: WebAssembly.ModuleImports = {
      __indirect_function_table: new WebAssembly.Table({ initial: 8192, element: "anyfunc" }),
      ...hostStubs,
      // Override the DB stubs with live implementations
      molt_db_query_host: dbFunctions.molt_db_query_host,
      molt_db_exec_host: dbFunctions.molt_db_exec_host,
      molt_db_host_poll: dbFunctions.molt_db_host_poll,
    };

    const imports = {
      wasi_snapshot_preview1: wasi,
      env: hostEnv,
      molt_runtime: { molt_hash_drop(): bigint { return -1n; } },
    };

    let exitCode = 0;
    try {
      const instance = await WebAssembly.instantiate(boxModule, imports);
      const exports = instance.exports as unknown as {
        memory: WebAssembly.Memory;
        molt_main?: () => void;
        molt_main_exit_status?: () => bigint;
        _start?: () => void;
      };

      bridge.memory = exports.memory;
      bridge.exports = exports;

      if (exports.molt_main) {
        exports.molt_main();
        // The runtime reports an exception the entry left pending (CPython's
        // PyErr_Print, through sys.excepthook into stderr) and returns the
        // program's exit status, as the native main stub does.
        if (!exports.molt_main_exit_status) {
          throw new Error("box missing molt_main_exit_status export");
        }
        exitCode = Number(exports.molt_main_exit_status());
      } else if (exports._start) exports._start();
    } catch (err) {
      if (err instanceof ProcExit) {
        exitCode = err.code;
      } else {
        throw err;
      }
    }

    return {
      stdout: stdoutChunks.join(""),
      stderr: stderrChunks.join(""),
      exitCode,
    };
  }

  // -------------------------------------------------------------------------
  // HTTP fetch handler
  // -------------------------------------------------------------------------

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;
    const queryString = url.search ? url.search.slice(1) : "";

    // Collect headers into a plain object for env serialization
    const headers: Record<string, string> = {};
    request.headers.forEach((value, key) => {
      headers[key] = value;
    });

    // Read body for POST/PUT/PATCH
    let body = "";
    if (request.body && !["GET", "HEAD"].includes(request.method)) {
      body = await request.text();
    }

    try {
      const result = await this.runWasm(path, queryString, request.method, headers, body);

      if (result.exitCode !== 0 && !result.stdout) {
        return new Response(
          JSON.stringify({ error: "box exited with code " + result.exitCode, stderr: result.stderr }),
          { status: 500, headers: { "content-type": "application/json" } },
        );
      }

      // Detect content type from output
      const trimmed = result.stdout.trimStart();
      const contentType =
        trimmed.startsWith("<!DOCTYPE html>") || trimmed.startsWith("<html")
          ? "text/html; charset=utf-8"
          : trimmed.startsWith("{") || trimmed.startsWith("[")
            ? "application/json"
            : "text/plain; charset=utf-8";

      const responseHeaders: Record<string, string> = {
        "content-type": contentType,
        "x-edgebox-runtime": "molt-wasm-do",
      };
      if (result.stderr) {
        responseHeaders["x-edgebox-stderr"] = encodeURIComponent(result.stderr.slice(0, 512));
      }

      return new Response(result.stdout, {
        status: result.exitCode === 0 ? 200 : 400,
        headers: responseHeaders,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      console.error("EdgeBox runtime error:", msg);
      return new Response(
        JSON.stringify({ error: "internal error", detail: msg }),
        { status: 500, headers: { "content-type": "application/json" } },
      );
    }
  }

  // -------------------------------------------------------------------------
  // Alarm handler -- invoked by the DO runtime on a schedule
  // -------------------------------------------------------------------------

  async alarm(): Promise<void> {
    // Read the pending alarm name from box_meta
    const sql = this.state.storage.sql;
    let alarmName = "tick";
    try {
      const cursor = sql.exec("SELECT value FROM box_meta WHERE key = 'pending_alarm'");
      for (const row of cursor) {
        alarmName = (row as Record<string, unknown>).value as string;
      }
    } catch {
      // No pending alarm configured, use default
    }

    try {
      await this.runWasm(`/alarm/${alarmName}`, "", "POST", {}, "");
    } catch (err) {
      console.error("EdgeBox alarm error:", err instanceof Error ? err.message : String(err));
    }
  }

  // -------------------------------------------------------------------------
  // WebSocket handlers -- for terminal / streaming use cases
  // -------------------------------------------------------------------------

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const text = typeof message === "string" ? message : new TextDecoder().decode(message);

    try {
      const result = await this.runWasm("/ws/message", "", "POST", {}, text);
      if (result.stdout) {
        ws.send(result.stdout);
      }
    } catch (err) {
      ws.send(JSON.stringify({ error: err instanceof Error ? err.message : String(err) }));
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string, _wasClean: boolean): Promise<void> {
    try {
      await this.runWasm("/ws/close", `code=${code}&reason=${encodeURIComponent(reason)}`, "POST", {}, "");
    } catch {
      // Best-effort cleanup
    }
  }
}
