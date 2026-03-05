(function () {
  "use strict";

  const root = window;

  function nowMs() {
    if (root.performance && typeof root.performance.now === "function") {
      return root.performance.now();
    }
    return Date.now();
  }

  function clampLimit(limit, fallback) {
    const parsed = Number(limit);
    if (!Number.isFinite(parsed) || parsed <= 0) {
      return fallback;
    }
    return Math.max(Math.floor(parsed), 1);
  }

  function defaultClassifyEventTone(eventName) {
    const name = String(eventName || "").toLowerCase();
    if (
      name.includes("failed") ||
      name.includes("error") ||
      name.includes("cancel") ||
      name.includes("timeout")
    ) {
      return "danger";
    }
    if (name.includes("token") || name.includes("rate") || name.includes("usage")) {
      return "info";
    }
    if (name.includes("retry") || name.includes("input_required")) {
      return "warn";
    }
    if (name.includes("complete") || name.includes("started")) {
      return "ok";
    }
    return "warn";
  }

  function defaultClassifyTraceStatus(status) {
    const norm = String(status || "").toLowerCase();
    if (norm.includes("run")) return "status-running";
    if (norm.includes("retry")) return "status-retrying";
    if (norm.includes("block") || norm.includes("fail")) return "status-blocked";
    return "";
  }

  function defaultCompactRecentEvents(rows, limit) {
    const source = Array.isArray(rows) ? rows : [];
    const cap = clampLimit(limit, 80);
    return source.slice(0, cap).map((row) => {
      const item = row && typeof row === "object" ? row : {};
      return {
        event: String(item.event || ""),
        message: String(item.message || ""),
        detail: String(item.detail || ""),
        at: String(item.at || ""),
        tone: defaultClassifyEventTone(item.event || ""),
      };
    });
  }

  const profileState = {
    generated_at: new Date().toISOString(),
    mode: "js_fallback",
    wasm_status: "not_loaded",
    wasm_url: "",
    last_error: null,
    latencies_ms: {
      classify_event_tone: [],
      classify_trace_status: [],
      compact_recent_events: [],
      wasm_load: [],
    },
    counters: {
      classify_event_tone_calls: 0,
      classify_trace_status_calls: 0,
      compact_recent_events_calls: 0,
      wasm_load_attempts: 0,
      wasm_load_successes: 0,
      wasm_load_failures: 0,
    },
  };

  function observeLatency(name, elapsedMs) {
    const list = profileState.latencies_ms[name];
    if (!Array.isArray(list)) return;
    list.push(Math.max(Number(elapsedMs) || 0, 0));
    if (list.length > 256) {
      list.splice(0, list.length - 256);
    }
    root.__MOLT_SYMPHONY_KERNEL_PROFILE__ = profileState;
  }

  function wrapProfiled(name, fn, counterName) {
    return function (...args) {
      const started = nowMs();
      try {
        return fn(...args);
      } finally {
        profileState.counters[counterName] =
          Number(profileState.counters[counterName] || 0) + 1;
        observeLatency(name, nowMs() - started);
      }
    };
  }

  function stats(values) {
    if (!Array.isArray(values) || values.length === 0) {
      return { count: 0, avg_ms: 0, p95_ms: 0, max_ms: 0 };
    }
    const count = values.length;
    const ordered = values.slice().sort((a, b) => a - b);
    const total = ordered.reduce((sum, value) => sum + value, 0);
    const p95 = ordered[Math.max(Math.ceil(count * 0.95) - 1, 0)];
    return {
      count,
      avg_ms: Number((total / count).toFixed(3)),
      p95_ms: Number((p95 || 0).toFixed(3)),
      max_ms: Number((ordered[count - 1] || 0).toFixed(3)),
    };
  }

  function getProfileSnapshot() {
    return {
      generated_at: new Date().toISOString(),
      mode: profileState.mode,
      wasm_status: profileState.wasm_status,
      wasm_url: profileState.wasm_url,
      last_error: profileState.last_error,
      counters: { ...profileState.counters },
      timings: {
        classify_event_tone: stats(profileState.latencies_ms.classify_event_tone),
        classify_trace_status: stats(profileState.latencies_ms.classify_trace_status),
        compact_recent_events: stats(profileState.latencies_ms.compact_recent_events),
        wasm_load: stats(profileState.latencies_ms.wasm_load),
      },
    };
  }

  const existing = root.__MOLT_SYMPHONY_KERNEL__;
  const kernel =
    existing && typeof existing === "object" ? existing : Object.create(null);

  if (typeof kernel.classifyEventTone !== "function") {
    kernel.classifyEventTone = defaultClassifyEventTone;
  }
  if (typeof kernel.classifyTraceStatus !== "function") {
    kernel.classifyTraceStatus = defaultClassifyTraceStatus;
  }
  if (typeof kernel.compactRecentEvents !== "function") {
    kernel.compactRecentEvents = defaultCompactRecentEvents;
  }

  kernel.classifyEventTone = wrapProfiled(
    "classify_event_tone",
    kernel.classifyEventTone,
    "classify_event_tone_calls"
  );
  kernel.classifyTraceStatus = wrapProfiled(
    "classify_trace_status",
    kernel.classifyTraceStatus,
    "classify_trace_status_calls"
  );
  kernel.compactRecentEvents = wrapProfiled(
    "compact_recent_events",
    kernel.compactRecentEvents,
    "compact_recent_events_calls"
  );
  kernel.getProfileSnapshot = getProfileSnapshot;
  kernel.timeit = function timeit(label, fn) {
    const started = nowMs();
    let threw = false;
    try {
      return fn();
    } catch (_err) {
      threw = true;
      throw _err;
    } finally {
      const elapsed = nowMs() - started;
      observeLatency("compact_recent_events", elapsed);
      if (threw) {
        profileState.last_error = `timeit:${String(label || "unknown")}:threw`;
      }
    }
  };

  root.__MOLT_SYMPHONY_KERNEL__ = kernel;
  root.__MOLT_SYMPHONY_KERNEL_PROFILE__ = profileState;

  function installWasmAdapter(adapted) {
    if (!adapted || typeof adapted !== "object") return false;
    let changed = false;
    if (typeof adapted.classifyEventTone === "function") {
      kernel.classifyEventTone = wrapProfiled(
        "classify_event_tone",
        adapted.classifyEventTone,
        "classify_event_tone_calls"
      );
      changed = true;
    }
    if (typeof adapted.classifyTraceStatus === "function") {
      kernel.classifyTraceStatus = wrapProfiled(
        "classify_trace_status",
        adapted.classifyTraceStatus,
        "classify_trace_status_calls"
      );
      changed = true;
    }
    if (typeof adapted.compactRecentEvents === "function") {
      kernel.compactRecentEvents = wrapProfiled(
        "compact_recent_events",
        adapted.compactRecentEvents,
        "compact_recent_events_calls"
      );
      changed = true;
    }
    if (changed) {
      profileState.mode = "wasm_adapter";
      profileState.wasm_status = "ready";
      root.dispatchEvent(
        new CustomEvent("molt:symphony:kernel-ready", {
          detail: getProfileSnapshot(),
        })
      );
    }
    return changed;
  }

  async function loadWasmKernel() {
    const wasmUrl = String(
      root.__MOLT_SYMPHONY_KERNEL_WASM_URL__ || "/dashboard-kernel.wasm"
    );
    profileState.wasm_url = wasmUrl;
    profileState.wasm_status = "loading";
    profileState.counters.wasm_load_attempts += 1;
    const started = nowMs();
    try {
      if (!(root.WebAssembly && typeof root.WebAssembly.instantiate === "function")) {
        throw new Error("WebAssembly runtime unavailable");
      }
      const response = await fetch(wasmUrl, { cache: "no-cache" });
      if (!response.ok) {
        throw new Error(`WASM fetch failed: ${response.status}`);
      }
      const imports =
        root.__MOLT_SYMPHONY_KERNEL_IMPORTS__ &&
        typeof root.__MOLT_SYMPHONY_KERNEL_IMPORTS__ === "object"
          ? root.__MOLT_SYMPHONY_KERNEL_IMPORTS__
          : {};
      const bytes = await response.arrayBuffer();
      const moduleResult = await root.WebAssembly.instantiate(bytes, imports);
      const instance = moduleResult?.instance || moduleResult;
      const exportsObject = instance && instance.exports ? instance.exports : {};
      const adapterFactory = root.__MOLT_SYMPHONY_KERNEL_WASM_ADAPTER__;
      if (typeof adapterFactory !== "function") {
        throw new Error(
          "WASM loaded but adapter is missing. Set window.__MOLT_SYMPHONY_KERNEL_WASM_ADAPTER__."
        );
      }
      const adapted = adapterFactory(exportsObject, instance, kernel);
      if (!installWasmAdapter(adapted)) {
        throw new Error("WASM adapter did not expose kernel functions.");
      }
      profileState.counters.wasm_load_successes += 1;
      profileState.last_error = null;
    } catch (err) {
      profileState.mode = "js_fallback";
      profileState.wasm_status = "failed";
      profileState.counters.wasm_load_failures += 1;
      profileState.last_error = err instanceof Error ? err.message : String(err);
    } finally {
      observeLatency("wasm_load", nowMs() - started);
      root.__MOLT_SYMPHONY_KERNEL_PROFILE__ = profileState;
    }
  }

  void loadWasmKernel();
})();
