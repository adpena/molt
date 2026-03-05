      const streamChip = document.getElementById("stream-chip");
      const updatedChip = document.getElementById("updated-chip");
      const refreshBtn = document.getElementById("refresh-btn");
      const reloadBtn = document.getElementById("reload-btn");
      const attentionList = document.getElementById("attention-list");
      const attentionSummary = document.getElementById("attention-summary");
      const toolSelect = document.getElementById("tool-select");
      const toolIssueInput = document.getElementById("tool-issue");
      const toolRunButton = document.getElementById("tool-run-btn");
      const toolResult = document.getElementById("tool-result");
      const traceModal = document.getElementById("trace-modal");
      const tracePanel = document.getElementById("trace-panel");
      const traceSubtitle = document.getElementById("trace-subtitle");
      const traceSummary = document.getElementById("trace-summary");
      const traceEvents = document.getElementById("trace-events");
      const traceRefreshButton = document.getElementById("trace-refresh-btn");
      const traceCloseButton = document.getElementById("trace-close-btn");
      const runningWrap = document.getElementById("running-wrap");
      const retryWrap = document.getElementById("retry-wrap");
      const profilingWrap = document.getElementById("profiling-wrap");
      const securityWrap = document.getElementById("security-wrap");
      const eventsWrap = document.getElementById("events");
      const interventionActivity = document.getElementById("intervention-activity");
      const rateWrap = document.getElementById("rate-wrap");
      const durableSummary = document.getElementById("durable-summary");
      const durableFiles = document.getElementById("durable-files");
      const durableEvents = document.getElementById("durable-events");
      const workspaceWrap = document.getElementById("agent-workspace");
      const workspaceMeta = document.getElementById("workspace-meta");
      const layoutSelect = document.getElementById("workspace-layout");
      const verbositySelect = document.getElementById("workspace-verbosity");
      const viewTabs = Array.from(document.querySelectorAll(".view-tab"));
      const viewPanels = Array.from(document.querySelectorAll("[data-views]"));
      const LAYOUT_CHOICES = ["columns", "rows", "grid"];
      const VERBOSITY_CHOICES = ["compact", "normal", "verbose"];
      const VIEW_CHOICES = [
        "overview",
        "interventions",
        "agents",
        "performance",
        "memory",
        "all",
      ];
      const TRANSPORT_CHOICES = ["auto", "sse", "poll"];
      const STORAGE_KEY = "molt.symphony.dashboard.workspace.v1";
      const VIEW_STORAGE_KEY = "molt.symphony.dashboard.view.v1";
      const TRANSPORT_STORAGE_KEY = "molt.symphony.dashboard.transport.v1";
      const AUTH_STORAGE_KEY = "molt.symphony.dashboard.auth_token.v1";
      let stream = null;
      let pollTimer = null;
      let reconnectTimer = null;
      let staleWatchdog = null;
      let lastFrameAt = 0;
      let reconnectAttempts = 0;
      let workspaceLayout = "grid";
      let workspaceVerbosity = "normal";
      let activeDashboardView = "overview";
      let paneOrder = [];
      let dragPaneId = "";
      let latestState = {};
      const localActionStatus = new Map();
      const pendingRetries = new Set();
      let traceIssueIdentifier = "";
      let tracePollTimer = null;
      let traceFetchSerial = 0;
      let latestGeneratedAt = "";
      let latestStateEtag = "";
      let durableMemoryState = {};
      let durablePollTimer = null;
      let durableFetchInFlight = false;
      let traceFetchInFlight = false;
      let traceLastFocusedElement = null;
      let streamStatusState = { message: "", mode: "" };
      let streamIntervalMs = 1000;
      let fallbackPollIntervalMs = 2500;
      let staleAfterMs = 7000;
      let pendingRenderState = null;
      let renderFrame = 0;
      const panelSignatures = new Map();
      const transportMetrics = {
        stateFetchTotal: 0,
        stateFetch200: 0,
        stateFetch304: 0,
        stateFetchErrors: 0,
        sseFrames: 0,
        sseReconnects: 0,
        fallbackPollTicks: 0,
        renderCommits: 0,
        renderFrames: 0,
      };
      let requestedTransport = "auto";
      let activeTransport = "idle";
      let stateTransport = null;
      let pollErrorStreak = 0;
      let pollNotModifiedStreak = 0;
      let pollScheduledDelayMs = 0;
      let lastPollStatusLabel = "";
      const authToken = (() => {
        try {
          const params = new URLSearchParams(window.location.search || "");
          const fromQuery = String(params.get("token") || "").trim();
          if (fromQuery) {
            window.localStorage.setItem(AUTH_STORAGE_KEY, fromQuery);
            return fromQuery;
          }
          const fromStorage = String(window.localStorage.getItem(AUTH_STORAGE_KEY) || "").trim();
          return fromStorage;
        } catch (_err) {
          return "";
        }
      })();

      function toObject(value) {
        return value && typeof value === "object" && !Array.isArray(value) ? value : {};
      }

      function toArray(value) {
        return Array.isArray(value) ? value : [];
      }

      function toNumber(value, fallback = 0) {
        const parsed = Number(value);
        return Number.isFinite(parsed) ? parsed : fallback;
      }

      function escapeHtml(value) {
        return String(value ?? "")
          .replaceAll("&", "&amp;")
          .replaceAll("<", "&lt;")
          .replaceAll(">", "&gt;")
          .replaceAll('"', "&quot;");
      }

      function formatNumber(value) {
        const num = toNumber(value, 0);
        return Number.isFinite(num) ? num.toLocaleString() : "0";
      }

      function formatTime(value) {
        if (!value) return "n/a";
        try {
          const date = new Date(value);
          if (Number.isNaN(date.getTime())) return "n/a";
          return date.toLocaleTimeString();
        } catch (_err) {
          return "n/a";
        }
      }

      function relTime(value) {
        if (!value) return "n/a";
        const now = Date.now();
        const then = Date.parse(value);
        if (!Number.isFinite(then)) return "n/a";
        const sec = Math.max(Math.floor((now - then) / 1000), 0);
        if (sec < 60) return `${sec}s ago`;
        if (sec < 3600) return `${Math.floor(sec / 60)}m ago`;
        return `${Math.floor(sec / 3600)}h ago`;
      }

      function formatDurationSeconds(value) {
        const sec = Math.max(toNumber(value, 0), 0);
        if (!Number.isFinite(sec)) return "0s";
        if (sec < 60) return `${Math.floor(sec)}s`;
        if (sec < 3600) return `${Math.floor(sec / 60)}m`;
        return `${Math.floor(sec / 3600)}h`;
      }

      function withAuthPath(path) {
        if (!authToken) return path;
        const url = new URL(path, window.location.origin);
        if (!url.searchParams.has("token")) {
          url.searchParams.set("token", authToken);
        }
        return `${url.pathname}${url.search}`;
      }

      function buildApiHeaders({
        cacheControl = true,
        includeJsonContentType = false,
        includeCsrf = false,
      } = {}) {
        const headers = {};
        if (cacheControl) {
          headers["Cache-Control"] = "no-cache";
        }
        if (includeJsonContentType) {
          headers["Content-Type"] = "application/json";
        }
        if (authToken) {
          headers["Authorization"] = `Bearer ${authToken}`;
        }
        if (includeCsrf) {
          headers["X-Symphony-CSRF"] = "1";
        }
        return headers;
      }

      function formatEpochSeconds(value) {
        const num = toNumber(value, NaN);
        if (!Number.isFinite(num) || num <= 0) return "n/a";
        const date = new Date(num * 1000);
        if (Number.isNaN(date.getTime())) return "n/a";
        return date.toLocaleString();
      }

      function parseRetryAfterHeader(value) {
        const text = String(value || "").trim();
        if (!text) return 0;
        const numeric = Number(text);
        if (Number.isFinite(numeric) && numeric > 0) {
          return Math.max(Math.round(numeric * 1000), 1000);
        }
        const dateMs = Date.parse(text);
        if (!Number.isFinite(dateMs)) return 0;
        return Math.max(Math.round(dateMs - Date.now()), 1000);
      }

      function deriveDashboardCadence(state) {
        const runtime = toObject(state.runtime);
        const profile = toObject(runtime.dashboard_profile);
        const mode = String(profile.mode || "normal").toLowerCase();
        return {
          mode,
          streamIntervalMs: Math.max(500, toNumber(profile.stream_interval_ms, mode === "gentle" ? 5000 : 1000)),
          fallbackPollIntervalMs: Math.max(
            1000,
            toNumber(profile.fallback_poll_interval_ms, mode === "gentle" ? 15000 : 2500)
          ),
          staleAfterMs: Math.max(3000, toNumber(profile.stale_after_ms, mode === "gentle" ? 20000 : 7000)),
        };
      }

      function setMeter(id, ratio) {
        const node = document.getElementById(id);
        if (!node) return;
        const clamped = Math.max(0, Math.min(1, ratio));
        node.style.width = `${Math.round(clamped * 100)}%`;
      }

      function normalizeLayout(value) {
        return LAYOUT_CHOICES.includes(value) ? value : "grid";
      }

      function normalizeVerbosity(value) {
        return VERBOSITY_CHOICES.includes(value) ? value : "normal";
      }

      function normalizeDashboardView(value) {
        return VIEW_CHOICES.includes(value) ? value : "overview";
      }

      function normalizeTransportChoice(value) {
        const normalized = String(value || "").trim().toLowerCase();
        return TRANSPORT_CHOICES.includes(normalized) ? normalized : "auto";
      }

      function resolveTransportPreference() {
        const params = new URLSearchParams(window.location.search || "");
        const fromQuery = normalizeTransportChoice(params.get("transport"));
        if (fromQuery !== "auto") {
          return fromQuery;
        }
        try {
          const fromStorage = normalizeTransportChoice(
            window.localStorage.getItem(TRANSPORT_STORAGE_KEY)
          );
          return fromStorage;
        } catch (_err) {
          return "auto";
        }
      }

      function arraysEqual(left, right) {
        if (left.length !== right.length) return false;
        for (let idx = 0; idx < left.length; idx += 1) {
          if (left[idx] !== right[idx]) return false;
        }
        return true;
      }

      function stableSignature(value) {
        try {
          return JSON.stringify(value) || "";
        } catch (_err) {
          return String(value ?? "");
        }
      }

      function updatePanel(key, value, renderFn) {
        const sig = stableSignature(value);
        if (panelSignatures.get(key) === sig) return false;
        panelSignatures.set(key, sig);
        renderFn();
        return true;
      }

      function persistWorkspacePrefs() {
        try {
          window.localStorage.setItem(
            STORAGE_KEY,
            JSON.stringify({
              layout: workspaceLayout,
              verbosity: workspaceVerbosity,
              pane_order: paneOrder,
            })
          );
        } catch (_err) {
          // ignore storage write failures
        }
      }

      function persistDashboardView() {
        try {
          window.localStorage.setItem(VIEW_STORAGE_KEY, activeDashboardView);
        } catch (_err) {
          // ignore storage write failures
        }
      }

      function loadWorkspacePrefs() {
        try {
          const raw = window.localStorage.getItem(STORAGE_KEY);
          if (!raw) return;
          const prefs = toObject(JSON.parse(raw));
          workspaceLayout = normalizeLayout(prefs.layout);
          workspaceVerbosity = normalizeVerbosity(prefs.verbosity);
          paneOrder = toArray(prefs.pane_order)
            .map((value) => String(value || ""))
            .filter((value) => value.length > 0);
        } catch (_err) {
          // ignore malformed storage payloads
        }
      }

      function loadDashboardView() {
        try {
          const raw = window.localStorage.getItem(VIEW_STORAGE_KEY);
          if (!raw) return;
          activeDashboardView = normalizeDashboardView(raw);
        } catch (_err) {
          // ignore malformed storage payloads
        }
      }

      function applyDashboardView() {
        viewPanels.forEach((panel) => {
          const allowed = String(panel.getAttribute("data-views") || "")
            .split(" ")
            .map((value) => value.trim().toLowerCase())
            .filter((value) => value.length > 0);
          const visible =
            activeDashboardView === "all" ||
            allowed.includes(activeDashboardView);
          panel.classList.toggle("hidden-view", !visible);
          panel.style.display = visible ? "" : "none";
        });
        viewTabs.forEach((tab) => {
          const tabView = normalizeDashboardView(tab.dataset.view || "overview");
          tab.classList.toggle("active", tabView === activeDashboardView);
          tab.setAttribute("aria-selected", tabView === activeDashboardView ? "true" : "false");
          tab.setAttribute("tabindex", tabView === activeDashboardView ? "0" : "-1");
        });
      }

      function setDashboardView(view, persist = true) {
        const normalized = normalizeDashboardView(view);
        if (persist && normalized === activeDashboardView) {
          return;
        }
        activeDashboardView = normalized;
        if (persist) {
          persistDashboardView();
        }
        applyDashboardView();
      }

      function badgeClassForState(value) {
        const normalized = String(value || "").toLowerCase();
        if (
          normalized.includes("error") ||
          normalized.includes("fail") ||
          normalized.includes("panic")
        ) {
          return "danger";
        }
        if (normalized.includes("retry") || normalized.includes("warn")) {
          return "warn";
        }
        return "ok";
      }

      function deriveAgentPanes(state) {
        const sourcePanes = toArray(state.agent_panes);
        const fallbackRunning = toArray(state.running);
        const source = sourcePanes.length ? sourcePanes : fallbackRunning;
        const seenPaneIds = new Set();
        return source.map((entryValue, index) => {
          const entry = toObject(entryValue);
          const issueId = entry.issue_identifier || entry.issue_id || "";
          const tokens = toObject(entry.tokens);
          const basePaneId = String(
            entry.pane_id ||
              entry.agent_id ||
              entry.id ||
              entry.worker_id ||
              entry.agent ||
              entry.agent_name ||
              issueId ||
              `agent-${index + 1}`
          );
          let paneId = basePaneId || `agent-${index + 1}`;
          if (seenPaneIds.has(paneId)) {
            let suffix = 2;
            while (seenPaneIds.has(`${paneId}-${suffix}`)) {
              suffix += 1;
            }
            paneId = `${paneId}-${suffix}`;
          }
          seenPaneIds.add(paneId);
          const agentLabel = String(
            entry.agent ||
              entry.agent_name ||
              entry.worker ||
              entry.worker_name ||
              entry.session_name ||
              entry.name ||
              issueId ||
              `Agent ${index + 1}`
          );
          return {
            paneId,
            agentLabel,
            issueId,
            role: String(entry.role || entry.worker_role || "executor"),
            state: String(entry.state || entry.status || "running"),
            turns: toNumber(entry.turn_count, toNumber(entry.turns, 0)),
            totalTokens: toNumber(
              entry.total_tokens ?? tokens.total_tokens ?? entry.tokens_total,
              0
            ),
            lastEvent: String(entry.last_event || entry.event || entry.activity || "n/a"),
            updatedAt: entry.last_event_at || entry.updated_at || entry.generated_at || "",
            raw: entry,
          };
        });
      }

      function orderAgentPanes(panes) {
        if (!panes.length) {
          if (paneOrder.length) {
            paneOrder = [];
            persistWorkspacePrefs();
          }
          return [];
        }
        if (!paneOrder.length) {
          paneOrder = panes.map((pane) => pane.paneId);
          persistWorkspacePrefs();
          return panes;
        }
        const byId = new Map(panes.map((pane) => [pane.paneId, pane]));
        const ordered = [];
        paneOrder.forEach((paneId) => {
          const pane = byId.get(paneId);
          if (!pane) return;
          ordered.push(pane);
          byId.delete(paneId);
        });
        panes.forEach((pane) => {
          if (!byId.has(pane.paneId)) return;
          ordered.push(pane);
          byId.delete(pane.paneId);
        });
        const nextOrder = ordered.map((pane) => pane.paneId);
        if (!arraysEqual(nextOrder, paneOrder)) {
          paneOrder = nextOrder;
          persistWorkspacePrefs();
        }
        return ordered;
      }

      function renderAgentPaneDetails(pane) {
        const lines = [
          `<div class="agent-line">Role: ${escapeHtml(pane.role || "executor")}</div>`,
          `<div class="agent-line">Last event: ${escapeHtml(pane.lastEvent)}</div>`,
          `<div class="agent-line">Updated: ${escapeHtml(relTime(pane.updatedAt))}</div>`,
        ];
        if (workspaceVerbosity !== "compact") {
          lines.unshift(
            `<div class="agent-line">Turns: ${formatNumber(pane.turns)} | Tokens: ${formatNumber(
              pane.totalTokens
            )}</div>`
          );
        }
        if (workspaceVerbosity !== "verbose") {
          return `<div class="agent-lines">${lines.join("")}</div>`;
        }
        let rawJson = "{}";
        try {
          rawJson = JSON.stringify(toObject(pane.raw), null, 2) || "{}";
        } catch (_err) {
          rawJson = "{}";
        }
        if (rawJson.length > 8000) {
          rawJson = `${rawJson.slice(0, 8000)}\n...`;
        }
        return `
          <div class="agent-lines">${lines.join("")}</div>
          <pre class="agent-json mono">${escapeHtml(rawJson)}</pre>
        `;
      }

      function renderAgentWorkspace(state) {
        const panes = orderAgentPanes(deriveAgentPanes(state));
        workspaceWrap.dataset.layout = workspaceLayout;
        workspaceWrap.dataset.verbosity = workspaceVerbosity;
        if (!panes.length) {
          workspaceWrap.innerHTML = '<div class="empty">No agent telemetry panes yet.</div>';
          workspaceMeta.textContent = "No agent panes loaded yet.";
          return;
        }
        workspaceWrap.innerHTML = panes
          .map((pane) => {
            const issueLine = pane.issueId
              ? `Issue ${escapeHtml(pane.issueId)}`
              : "No issue bound";
            const dragTag =
              workspaceLayout === "grid"
                ? '<span class="drag-tag">drag</span>'
                : '<span class="drag-tag">locked</span>';
            return `
              <article
                class="agent-pane"
                data-pane-id="${escapeHtml(pane.paneId)}"
                draggable="${workspaceLayout === "grid"}"
              >
                <div class="agent-pane-head">
                  <div>
                    <div class="agent-pane-title mono">${escapeHtml(pane.agentLabel)}</div>
                    <div class="agent-pane-sub">${issueLine}</div>
                  </div>
                  <div class="agent-pane-badges">
                    <button type="button" class="agent-ref" data-agent-issue="${escapeHtml(
                      pane.issueId || ""
                    )}">trace</button>
                    <span class="badge">${escapeHtml(pane.role || "executor")}</span>
                    <span class="badge ${badgeClassForState(pane.state)}">${escapeHtml(
              pane.state
            )}</span>
                    ${dragTag}
                  </div>
                </div>
                ${renderAgentPaneDetails(pane)}
              </article>
            `;
          })
          .join("");
        const dragHint =
          workspaceLayout === "grid"
            ? "Drag panes to reorder."
            : "Switch to grid layout to reorder panes.";
        workspaceMeta.textContent = `${formatNumber(panes.length)} pane(s) | ${dragHint}`;
      }

      function clearDropTargets() {
        workspaceWrap.querySelectorAll(".drop-target").forEach((node) => {
          node.classList.remove("drop-target");
        });
      }

      function movePaneInOrder(sourceId, targetId) {
        if (!sourceId || !targetId || sourceId === targetId) return;
        const fromIndex = paneOrder.indexOf(sourceId);
        const toIndex = paneOrder.indexOf(targetId);
        if (fromIndex < 0 || toIndex < 0) return;
        const nextOrder = paneOrder.slice();
        nextOrder.splice(fromIndex, 1);
        nextOrder.splice(toIndex, 0, sourceId);
        if (arraysEqual(nextOrder, paneOrder)) return;
        paneOrder = nextOrder;
        persistWorkspacePrefs();
        renderAgentWorkspace(latestState);
      }

      function handlePaneDragStart(event) {
        if (workspaceLayout !== "grid") return;
        const target = event.target;
        if (!(target instanceof Element)) return;
        const pane = target.closest(".agent-pane");
        if (!pane) return;
        dragPaneId = pane.dataset.paneId || "";
        pane.classList.add("dragging");
        if (event.dataTransfer) {
          event.dataTransfer.effectAllowed = "move";
          event.dataTransfer.setData("text/plain", dragPaneId);
        }
      }

      function handlePaneDragOver(event) {
        if (workspaceLayout !== "grid") return;
        const target = event.target;
        if (!(target instanceof Element)) return;
        const pane = target.closest(".agent-pane");
        if (!pane) return;
        event.preventDefault();
        clearDropTargets();
        if ((pane.dataset.paneId || "") !== dragPaneId) {
          pane.classList.add("drop-target");
        }
      }

      function handlePaneDrop(event) {
        if (workspaceLayout !== "grid") return;
        const target = event.target;
        if (!(target instanceof Element)) return;
        const pane = target.closest(".agent-pane");
        if (!pane) return;
        event.preventDefault();
        const targetId = pane.dataset.paneId || "";
        movePaneInOrder(dragPaneId, targetId);
        dragPaneId = "";
        clearDropTargets();
      }

      function handlePaneDragEnd() {
        dragPaneId = "";
        workspaceWrap.querySelectorAll(".dragging").forEach((node) => {
          node.classList.remove("dragging");
        });
        clearDropTargets();
      }

      function setStreamStatus(message, mode = "") {
        const next = { message: String(message || ""), mode: String(mode || "") };
        if (
          next.message === streamStatusState.message &&
          next.mode === streamStatusState.mode
        ) {
          return;
        }
        streamStatusState = next;
        streamChip.textContent = message;
        streamChip.className = "status-chip";
        if (mode === "live") streamChip.classList.add("live");
        if (mode === "warn") streamChip.classList.add("warn");
        if (mode === "connecting") streamChip.classList.add("connecting");
      }

      function buildClientTelemetrySnapshot() {
        return {
          generated_at: new Date().toISOString(),
          requested_transport: requestedTransport,
          active_transport: activeTransport,
          stream_interval_ms: streamIntervalMs,
          fallback_poll_interval_ms: fallbackPollIntervalMs,
          stale_after_ms: staleAfterMs,
          render: {
            commits: transportMetrics.renderCommits,
            frames: transportMetrics.renderFrames,
          },
          fetch: {
            total: transportMetrics.stateFetchTotal,
            ok_200: transportMetrics.stateFetch200,
            not_modified_304: transportMetrics.stateFetch304,
            errors: transportMetrics.stateFetchErrors,
          },
          stream: {
            frames: transportMetrics.sseFrames,
            reconnects: transportMetrics.sseReconnects,
            fallback_poll_ticks: transportMetrics.fallbackPollTicks,
          },
        };
      }

      function publishClientTelemetry() {
        window.__MOLT_SYMPHONY_CLIENT_TELEMETRY__ = buildClientTelemetrySnapshot();
      }

      function stopPolling() {
        if (pollTimer) {
          clearTimeout(pollTimer);
          pollTimer = null;
        }
        pollScheduledDelayMs = 0;
        lastPollStatusLabel = "";
      }

      function _setFallbackStatus(delayMs, reason = "") {
        const delay = Math.max(250, Math.round(toNumber(delayMs, fallbackPollIntervalMs)));
        const adaptive = delay !== Math.round(fallbackPollIntervalMs);
        let label = adaptive
          ? `Polling fallback (${delay}ms adaptive)`
          : `Polling fallback (${delay}ms)`;
        if (String(reason).startsWith("rate_limited")) {
          label = `Polling fallback (${delay}ms, server rate-limited)`;
        } else if (String(reason).startsWith("fetch_error")) {
          label = `Polling fallback (${delay}ms, transient fetch error)`;
        }
        if (label === lastPollStatusLabel) return;
        lastPollStatusLabel = label;
        setStreamStatus(label, "warn");
      }

      function _nextFallbackDelayMs({ changed, hadError, retryAfterMs = 0 }) {
        const baseMs = Math.max(250, Math.round(fallbackPollIntervalMs));
        if (hadError) {
          pollErrorStreak = Math.min(pollErrorStreak + 1, 8);
          pollNotModifiedStreak = 0;
          if (retryAfterMs > 0) {
            return Math.min(
              Math.max(Math.round(retryAfterMs), baseMs),
              120000
            );
          }
          return Math.min(baseMs * Math.pow(2, Math.min(pollErrorStreak, 4)), 60000);
        }
        pollErrorStreak = 0;
        if (changed === false) {
          pollNotModifiedStreak = Math.min(pollNotModifiedStreak + 1, 8);
          const factor = 1 + pollNotModifiedStreak * 0.5;
          return Math.min(Math.round(baseMs * factor), 30000);
        }
        pollNotModifiedStreak = 0;
        return baseMs;
      }

      function stopStream() {
        if (stream) {
          stream.close();
          stream = null;
        }
      }

      function stopReconnectTimer() {
        if (reconnectTimer) {
          clearTimeout(reconnectTimer);
          reconnectTimer = null;
        }
      }

      function ensureWatchdog() {
        if (staleWatchdog) return;
        staleWatchdog = setInterval(() => {
          if (!stream || !lastFrameAt) return;
          if (Date.now() - lastFrameAt > staleAfterMs) {
            setStreamStatus("Live stream stale, using fallback", "warn");
            startPollingFallback();
            scheduleReconnect();
          }
        }, 2000);
      }

      function createStateTransportController() {
        return {
          start(manual = false) {
            if (requestedTransport === "poll") {
              startPollingFallback(true);
              return;
            }
            connectStream(manual);
          },
          restart(manual = false) {
            if (requestedTransport === "poll") {
              startPollingFallback(true);
              return;
            }
            connectStream(manual);
          },
          stop() {
            stopReconnectTimer();
            stopStream();
            stopPolling();
            activeTransport = "idle";
          },
          refreshCadence() {
            if (requestedTransport === "poll") {
              startPollingFallback(true);
              return;
            }
            if (stream) {
              connectStream(false);
              return;
            }
            if (pollTimer) {
              startPollingFallback(true);
            }
          },
        };
      }

      function applyTransportCadence(state) {
        const cadence = deriveDashboardCadence(state);
        const changed =
          cadence.streamIntervalMs !== streamIntervalMs ||
          cadence.fallbackPollIntervalMs !== fallbackPollIntervalMs ||
          cadence.staleAfterMs !== staleAfterMs;
        streamIntervalMs = cadence.streamIntervalMs;
        fallbackPollIntervalMs = cadence.fallbackPollIntervalMs;
        staleAfterMs = cadence.staleAfterMs;
        if (!changed) return;
        if (stateTransport) {
          stateTransport.refreshCadence();
        }
      }

      function renderAttention(state) {
        const attention = toArray(state.attention);
        const actions = toArray(state.manual_actions);
        const runningRows = toArray(state.running);
        const retryRows = toArray(state.retrying);
        const latestActionByIssue = new Map();
        const runningByIssue = new Map();
        const retryByIssue = new Map();
        actions.forEach((actionValue) => {
          const action = toObject(actionValue);
          const issueId = String(action.issue_identifier || "");
          if (!issueId) return;
          latestActionByIssue.set(issueId, action);
        });
        runningRows.forEach((rowValue) => {
          const row = toObject(rowValue);
          runningByIssue.set(String(row.issue_identifier || row.issue_id || ""), row);
        });
        retryRows.forEach((rowValue) => {
          const row = toObject(rowValue);
          retryByIssue.set(String(row.issue_identifier || row.issue_id || ""), row);
        });
        const byKind = new Map();
        attention.forEach((itemValue) => {
          const item = toObject(itemValue);
          const key = String(item.kind || "attention");
          byKind.set(key, (byKind.get(key) || 0) + 1);
        });
        const summaryText = attention.length
          ? `${formatNumber(attention.length)} intervention item(s) · ${Array.from(byKind.entries())
              .map(([kind, count]) => `${kind}: ${count}`)
              .join(" · ")}`
          : "No intervention items. System is currently healthy.";
        attentionSummary.textContent = summaryText;
        if (!attention.length) {
          attentionList.innerHTML =
            '<div class="empty"><span class="badge ok">Healthy</span> No action required right now.</div>';
          return;
        }
        attentionList.innerHTML = attention
          .map((itemValue) => {
            const item = toObject(itemValue);
            const issueId = String(item.issue_identifier || item.issue_id || "unknown");
            const isSystem = issueId.toUpperCase() === "SYSTEM";
            const running = toObject(runningByIssue.get(issueId));
            const retry = toObject(retryByIssue.get(issueId));
            const issueUrl = String(item.issue_url || running.url || retry.url || "");
            const issueLink = issueUrl
              ? `<a href="${escapeHtml(issueUrl)}" target="_blank" rel="noreferrer">Open Linear</a>`
              : "";
            const inspectLink = isSystem
              ? ""
              : `<a href="${escapeHtml(
                  withAuthPath(`/api/v1/${encodeURIComponent(issueId)}`)
                )}" target="_blank" rel="noreferrer">Inspect JSON</a>`;
            const traceButton = isSystem
              ? ""
              : `<button type="button" class="agent-ref" data-agent-issue="${escapeHtml(
                  issueId
                )}">Trace</button>`;
            const localStatus = localActionStatus.get(issueId);
            const latestAction = toObject(latestActionByIssue.get(issueId));
            const pending = pendingRetries.has(issueId);
            const statusText = localStatus
              ? `${localStatus.status} · ${localStatus.message}`
              : latestAction.action
                ? `${latestAction.action}: ${latestAction.status || "updated"}`
                : "";
            const retryNowButton = isSystem
              ? ""
              : `<button type="button" data-action="retry-now" data-issue="${escapeHtml(
                  issueId
                )}" ${pending ? "disabled" : ""}>${pending ? "Retrying..." : "Retry now"}</button>`;
            const extraSummary = [
              running.state ? `state: ${running.state}` : "",
              retry.attempt ? `retry attempt: ${retry.attempt}` : "",
              retry.due_in_seconds ? `due in ${retry.due_in_seconds}s` : "",
            ]
              .filter((entry) => entry.length > 0)
              .join(" · ");
            return `
              <div class="attention-item">
                <div class="head">
                  ${
                    isSystem
                      ? `<span class="mono">${escapeHtml(issueId)}</span>`
                      : `<button type="button" class="agent-ref mono" data-agent-issue="${escapeHtml(
                          issueId
                        )}">${escapeHtml(issueId)}</button>`
                  }
                  <span class="badge warn">${escapeHtml(item.kind || "attention")}</span>
                </div>
                <div class="msg">${escapeHtml(item.message || "")}</div>
                <div class="hint">${escapeHtml(item.suggested_action || "")}</div>
                ${extraSummary ? `<div class="hint">${escapeHtml(extraSummary)}</div>` : ""}
                ${statusText ? `<div class="action-status">${escapeHtml(statusText)}</div>` : ""}
                <div class="attention-tools">
                  ${issueLink}
                  ${inspectLink}
                  ${traceButton}
                  ${retryNowButton}
                </div>
              </div>
            `;
          })
          .join("");
      }

      function renderInterventionActivity(state) {
        const actions = toArray(state.manual_actions);
        if (!actions.length) {
          interventionActivity.innerHTML =
            '<div class="empty">No manual intervention actions recorded yet.</div>';
          return;
        }
        interventionActivity.innerHTML = actions
          .slice()
          .reverse()
          .slice(0, 20)
          .map((actionValue) => {
            const action = toObject(actionValue);
            const ok = Boolean(action.ok);
            return `
              <div class="activity-item">
                <div class="head">
                  <span class="mono">${escapeHtml(action.action || "action")}</span>
                  <span class="badge ${ok ? "ok" : "warn"}">${escapeHtml(
              action.status || (ok ? "ok" : "error")
            )}</span>
                </div>
                <div class="meta">${escapeHtml(action.issue_identifier || "global")}</div>
                <div class="meta">${escapeHtml(action.message || "")}</div>
                <div class="meta">${escapeHtml(relTime(action.at))}</div>
              </div>
            `;
          })
          .join("");
      }

      function formatBytes(bytes) {
        const value = toNumber(bytes, 0);
        if (!Number.isFinite(value) || value <= 0) return "0 B";
        const units = ["B", "KB", "MB", "GB", "TB"];
        let size = value;
        let idx = 0;
        while (size >= 1024 && idx < units.length - 1) {
          size /= 1024;
          idx += 1;
        }
        return `${size.toFixed(size >= 10 || idx === 0 ? 0 : 1)} ${units[idx]}`;
      }

      function renderDurableMemory() {
        if (!durableSummary || !durableFiles || !durableEvents) {
          return;
        }
        const payload = toObject(durableMemoryState);
        const enabled = Boolean(payload.enabled);
        const files = toObject(payload.files);
        const jsonl = toObject(files.jsonl);
        const duckdb = toObject(files.duckdb);
        const parquet = toObject(files.parquet);
        const root = String(payload.root || "");
        const events = toArray(payload.recent_events);
        const backups = toArray(payload.backups);
        const lastBackup = toObject(payload.last_backup);
        const integrity = toObject(payload.last_integrity_check);
        const kindCounts = toObject(payload.kind_counts);
        const statusText = enabled ? "Enabled" : "Disabled";
        const statusClass = enabled ? "ok" : "warn";
        durableSummary.innerHTML = `
          <div class="durable-kpi">
            <div class="label">Status</div>
            <div class="value"><span class="badge ${statusClass}">${escapeHtml(
              statusText
            )}</span></div>
          </div>
          <div class="durable-kpi">
            <div class="label">Root</div>
            <div class="value mono">${escapeHtml(root || "n/a")}</div>
          </div>
          <div class="durable-kpi">
            <div class="label">Queue Depth</div>
            <div class="value">${formatNumber(payload.queue_depth || 0)}</div>
          </div>
          <div class="durable-kpi">
            <div class="label">Dropped Rows</div>
            <div class="value">${formatNumber(payload.dropped_rows || 0)}</div>
          </div>
          <div class="durable-kpi">
            <div class="label">Backups</div>
            <div class="value">${formatNumber(backups.length)}</div>
          </div>
          <div class="durable-kpi">
            <div class="label">Integrity</div>
            <div class="value">${escapeHtml(
              integrity.ok == null
                ? "not checked"
                : integrity.ok
                  ? "ok"
                  : "failed"
            )}</div>
          </div>
        `;
        durableFiles.innerHTML = `
          <div class="durable-file">
            <span class="mono">events.jsonl</span>
            <span>${formatBytes(jsonl.size_bytes)} · ${escapeHtml(relTime(
              jsonl.modified_at
            ))}</span>
          </div>
          <div class="durable-file">
            <span class="mono">events.duckdb</span>
            <span>${formatBytes(duckdb.size_bytes)} · ${escapeHtml(relTime(
              duckdb.modified_at
            ))}</span>
          </div>
          <div class="durable-file">
            <span class="mono">events.parquet</span>
            <span>${formatBytes(parquet.size_bytes)} · ${escapeHtml(relTime(
              parquet.modified_at
            ))}</span>
          </div>
          <div class="durable-file">
            <span class="mono">last sync</span>
            <span>${escapeHtml(relTime(payload.last_sync_utc))}</span>
          </div>
          <div class="durable-file">
            <span class="mono">last backup</span>
            <span>${escapeHtml(relTime(lastBackup.created_at))}</span>
          </div>
          <div class="durable-file">
            <span class="mono">last integrity check</span>
            <span>${escapeHtml(relTime(integrity.checked_at))}</span>
          </div>
        `;
        if (!events.length) {
          durableEvents.innerHTML =
            '<div class="empty">No durable telemetry events recorded yet.</div>';
          return;
        }
        const kindSummary = Object.entries(kindCounts)
          .map(([kind, count]) => `${kind}:${formatNumber(count)}`)
          .join(" · ");
        durableEvents.innerHTML = `
          <div class="hint">${escapeHtml(kindSummary || "recent event trail")}</div>
          ${events
            .slice()
            .reverse()
            .slice(0, 80)
            .map((eventValue) => {
              const event = toObject(eventValue);
              const issueIdentifier = String(event.issue_identifier || "");
              const canTrace = issueIdentifier && issueIdentifier.toUpperCase() !== "SYSTEM";
              const issueBadge = canTrace
                ? `<button type="button" class="agent-ref mono" data-agent-issue="${escapeHtml(
                    issueIdentifier
                  )}">${escapeHtml(issueIdentifier)}</button>`
                : `<span class="mono">${escapeHtml(issueIdentifier || "SYSTEM")}</span>`;
              const kind = String(event.kind || "event");
              const message = String(
                event.message || event.detail || event.error || event.status || ""
              );
              return `
                <article class="durable-event">
                  <div class="head">
                    <div class="event-tags">
                      ${issueBadge}
                      <span class="badge">${escapeHtml(kind)}</span>
                    </div>
                    <div class="meta">${escapeHtml(relTime(event.recorded_at || event.at))}</div>
                  </div>
                  <div class="msg">${escapeHtml(message)}</div>
                </article>
              `;
            })
            .join("")}
        `;
      }

      function renderToolLauncher(state) {
        const issues = new Map();

        const processItem = (item) => {
          const obj = toObject(item);
          const id = String(obj.issue_identifier || obj.issue_id || "");
          if (!id || id === "SYSTEM") return;

          let context = "";
          if (obj.kind) {
            context = `Attention: [${obj.kind}] - ${obj.message || "Needs intervention"}`;
          } else if (obj.worker_role || obj.role) {
            const role = obj.role || obj.worker_role || "worker";
            const st = obj.state || obj.status || "active";
            const turns = obj.turn_count || obj.turns || 0;
            const event = obj.last_event || obj.event || "processing";
            context = `Running: [${role}] state=${st} · turns=${turns} · event=${event}`;
          } else if (obj.due_at) {
            const attempt = obj.attempt || 1;
            const err = obj.error || obj.message || "unknown error";
            context = `Retrying: attempt=${attempt} · error=${err}`;
          }

          issues.set(id, { title: obj.title || "", context });
        };

        toArray(state.attention).forEach(processItem);
        toArray(state.running).forEach(processItem);
        toArray(state.retrying).forEach(processItem);

        const datalist = document.getElementById("tool-issue-datalist");
        if (!datalist) return;

        datalist.innerHTML = Array.from(issues.entries())
          .map(([id, info]) => {
            const label = info.title ? `${id} - ${info.title}` : id;
            return `<option value="${escapeHtml(label)}">${escapeHtml(info.context)}</option>`;
          })
          .join("");
      }

      function renderRunning(state) {
        const rows = toArray(state.running);
        if (!rows.length) {
          runningWrap.innerHTML = '<div class="empty">No active sessions.</div>';
          return;
        }
        runningWrap.innerHTML = `
          <div class="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Issue</th>
                  <th>State</th>
                  <th>Turns</th>
                  <th>Last Event</th>
                  <th>Tokens</th>
                  <th>Updated</th>
                  <th>Trace</th>
                </tr>
              </thead>
              <tbody>
                ${rows
                  .map((rowValue) => {
                    const row = toObject(rowValue);
                    const tokens = toObject(row.tokens);
                    const totalTokens = toNumber(tokens.total_tokens, 0);
                    const issueId = row.issue_identifier || row.issue_id || "unknown";
                    const issueCell = `
                      <button type="button" class="agent-ref mono" data-agent-issue="${escapeHtml(
                        issueId
                      )}">${escapeHtml(issueId)}</button>
                      ${
                        row.url
                          ? `<a href="${escapeHtml(
                              row.url
                            )}" target="_blank" rel="noreferrer">linear</a>`
                          : ""
                      }
                    `;
                    return `
                      <tr>
                        <td class="mono">${issueCell}</td>
                        <td>${escapeHtml(row.state || "running")}</td>
                        <td>${formatNumber(row.turn_count)}</td>
                        <td>${escapeHtml(row.last_event || "n/a")}</td>
                        <td>${formatNumber(totalTokens)}</td>
                        <td title="${escapeHtml(row.last_event_at || "")}">${relTime(
                      row.last_event_at
                    )}</td>
                        <td><button type="button" class="agent-ref" data-agent-issue="${escapeHtml(
                          issueId
                        )}">open</button></td>
                      </tr>
                    `;
                  })
                  .join("")}
              </tbody>
            </table>
          </div>
        `;
      }

      function renderRetry(state) {
        const rows = toArray(state.retrying);
        if (!rows.length) {
          retryWrap.innerHTML = '<div class="empty">Retry queue is empty.</div>';
          return;
        }
        retryWrap.innerHTML = `
          <div class="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Issue</th>
                  <th>Attempt</th>
                  <th>Due In</th>
                  <th>Error</th>
                  <th>Trace</th>
                </tr>
              </thead>
              <tbody>
                ${rows
                  .map((rowValue) => {
                    const row = toObject(rowValue);
                    const issueId = row.issue_identifier || row.issue_id || "unknown";
                    return `
                      <tr>
                        <td class="mono"><button type="button" class="agent-ref mono" data-agent-issue="${escapeHtml(
                          issueId
                        )}">${escapeHtml(issueId)}</button></td>
                        <td>${formatNumber(row.attempt)}</td>
                        <td>${formatNumber(row.due_in_seconds)}s</td>
                        <td>${escapeHtml(row.error || row.message || "")}</td>
                        <td><button type="button" class="agent-ref" data-agent-issue="${escapeHtml(
                          issueId
                        )}">open</button></td>
                      </tr>
                    `;
                  })
                  .join("")}
              </tbody>
            </table>
          </div>
        `;
      }

      function renderProfiling(state) {
        const profiling = toObject(state.profiling);
        const hotspots = [
          ...toArray(profiling.hotspots),
          ...toArray(state.hotspots),
          ...toArray(state.profiling_hotspots),
        ];
        if (!hotspots.length) {
          profilingWrap.innerHTML =
            '<div class="empty">No profiling hotspot telemetry available yet.</div>';
          return;
        }
        profilingWrap.innerHTML = `<div class="profiling-list">${hotspots
          .slice(0, 8)
          .map((rowValue) => {
            const row = toObject(rowValue);
            const name =
              row.label || row.name || row.scope || row.operation || "unknown hotspot";
            const p95 = toNumber(row.p95_ms || row.p95, 0);
            const avg = toNumber(row.avg_ms || row.avg, 0);
            const samples = toNumber(row.samples || row.count, 0);
            const calls = toNumber(row.calls, 0);
            return `
              <div class="profiling-item">
                <div class="head">
                  <span class="name mono">${escapeHtml(name)}</span>
                  <span class="badge warn">p95 ${escapeHtml(p95.toFixed(1))} ms</span>
                </div>
                <div class="meta">avg ${escapeHtml(avg.toFixed(1))} ms | samples ${formatNumber(
              samples
            )} | calls ${formatNumber(calls)}</div>
              </div>
            `;
          })
          .join("")}</div>`;
      }

      function renderSecurity(state) {
        if (!securityWrap) return;
        const runtime = toObject(state.runtime);
        const security = toObject(runtime.http_security);
        const counters = toObject(security.counters);
        const events = toObject(security.events);
        const secretGuard = toObject(events.secret_guard_blocked);
        const rows = [
          ["unauthorized", toNumber(counters.unauthorized, 0)],
          ["origin_denied", toNumber(counters.origin_denied, 0)],
          ["csrf_denied", toNumber(counters.csrf_denied, 0)],
          ["overload_rejected", toNumber(counters.overload_rejected, 0)],
          ["stream_capacity_rejected", toNumber(counters.stream_capacity_rejected, 0)],
          ["method_not_allowed", toNumber(counters.method_not_allowed, 0)],
          ["not_found", toNumber(counters.not_found, 0)],
          ["secret_guard_blocked", toNumber(secretGuard.total, 0)],
        ];
        securityWrap.innerHTML = `
          <div class="attention-item">
            <div class="head">
              <span class="mono">profile=${escapeHtml(String(security.profile || "local"))}</span>
              <span class="badge ${Boolean(security.nonlocal_bind) ? "warn" : "ok"}">
                bind ${escapeHtml(String(security.bind_host || "127.0.0.1"))}
              </span>
            </div>
            <div class="hint">
              query_token=${escapeHtml(String(Boolean(security.allow_query_token)))} ·
              dashboard_enabled=${escapeHtml(String(Boolean(security.dashboard_enabled)))}
            </div>
            <div class="kv-list" style="margin-top: 10px;">
              ${rows
                .map(
                  ([name, value]) => `
                    <div class="kv-item">
                      <div class="key mono">${escapeHtml(String(name))}</div>
                      <div class="value mono">${formatNumber(value)}</div>
                    </div>
                  `
                )
                .join("")}
            </div>
            <div class="hint">
              last secret-guard block: ${escapeHtml(relTime(secretGuard.last_at))}
            </div>
          </div>
        `;
      }

      function renderEvents(state) {
        const rows = toArray(state.recent_events);
        if (!rows.length) {
          eventsWrap.innerHTML = '<div class="empty">No recent orchestration events.</div>';
          return;
        }
        eventsWrap.innerHTML = rows
          .slice(0, 50)
          .map((rowValue) => {
            const row = toObject(rowValue);
            const issueId = row.issue_identifier || row.issue_id || "unknown";
            return `
            <div class="event">
              <div class="line">
                <button type="button" class="agent-ref mono" data-agent-issue="${escapeHtml(
                  issueId
                )}">${escapeHtml(issueId)}</button>
                ${escapeHtml(row.event || "")}
              </div>
              <div class="meta">
                ${escapeHtml(row.message || "")}
                ${row.at ? " | " + escapeHtml(formatTime(row.at)) : ""}
              </div>
              ${
                row.detail
                  ? `<div class="meta">${escapeHtml(String(row.detail || ""))}</div>`
                  : ""
              }
            </div>
          `;
          })
          .join("");
      }

      function describeRateLimitState(state) {
        const suspension = toObject(state.suspension);
        const limits = toObject(state.rate_limits);
        const credits = toObject(limits.credits);
        const primary = toObject(limits.primary);
        const secondary = toObject(limits.secondary);
        const runtime = toObject(state.runtime);
        const cadence = deriveDashboardCadence(state);
        const pollIntervalMs = Math.max(1000, toNumber(runtime.poll_interval_ms, 30000));
        const creditExhausted = credits.hasCredits === false;
        const primaryUsed = toNumber(primary.usedPercent, 0);
        const secondaryUsed = toNumber(secondary.usedPercent, 0);
        const windowSaturated = primaryUsed >= 99.9 || secondaryUsed >= 99.9;
        const suspended = Boolean(suspension.active);
        const resumeAtEpoch = toNumber(suspension.resume_at_epoch_seconds, NaN);
        const dueInSecondsFromEpoch = Number.isFinite(resumeAtEpoch)
          ? Math.max(Math.round(resumeAtEpoch - Date.now() / 1000), 0)
          : null;
        const dueInSeconds =
          suspension.due_in_seconds != null
            ? Math.round(toNumber(suspension.due_in_seconds, 0))
            : dueInSecondsFromEpoch;
        const dueIn =
          dueInSeconds != null ? formatDurationSeconds(dueInSeconds) : "n/a";
        const primaryResetAt = primary.resetsAt || primary.resetAt || null;
        const secondaryResetAt = secondary.resetsAt || secondary.resetAt || null;

        let headline = "No provider throttle detected.";
        let detail =
          "Symphony is running in normal cadence. Polling and stream intervals are set for responsiveness.";
        let level = "ok";
        const resumeAtText = Number.isFinite(resumeAtEpoch)
          ? formatEpochSeconds(resumeAtEpoch)
          : "n/a";
        const resumeSource = String(suspension.resume_source || "unknown");
        const resumeReason = String(suspension.resume_reason || "");

        if (creditExhausted) {
          headline = "Provider credits are exhausted (not a local concurrency cap).";
          detail =
            `Codex reports credits.hasCredits=false, so work is paused until credits return. ` +
            `Auto-resume in about ${dueIn} (target ${resumeAtText}; source ${resumeSource}).`;
          level = "danger";
        } else if (windowSaturated || suspended) {
          headline = "Provider rate-limit window is active.";
          detail =
            `Symphony is in gentle mode and will auto-resume in about ${dueIn}. ` +
            `Primary reset: ${formatEpochSeconds(primaryResetAt)} · Secondary reset: ${formatEpochSeconds(
              secondaryResetAt
            )} · Resume target: ${resumeAtText} (${resumeSource}${
              resumeReason ? `/${resumeReason}` : ""
            }).`;
          level = "warn";
        }

        return {
          headline,
          detail,
          level,
          cadence,
          pollIntervalMs,
          dueIn,
          primaryResetAt,
          secondaryResetAt,
        };
      }

      function renderRateLimits(state) {
        const limits = toObject(state.rate_limits);
        const runtime = toObject(state.runtime);
        const status = describeRateLimitState(state);
        const cadence = status.cadence;
        const policy = toObject(runtime.rate_limit_policy);
        const summaryHtml = `
          <div class="attention-item">
            <div class="head">
              <span class="mono">${escapeHtml(status.headline)}</span>
              <span class="badge ${status.level === "danger" ? "warn" : "ok"}">${escapeHtml(
                cadence.mode
              )} cadence</span>
            </div>
            <div class="msg">${escapeHtml(status.detail)}</div>
            <div class="hint">
              Orchestrator poll: ${formatNumber(status.pollIntervalMs)}ms ·
              Dashboard stream: ${formatNumber(cadence.streamIntervalMs)}ms ·
              Fallback poll: ${formatNumber(cadence.fallbackPollIntervalMs)}ms ·
              Stale timeout: ${formatNumber(cadence.staleAfterMs)}ms
            </div>
            <div class="hint">
              Max concurrent agents: ${formatNumber(runtime.max_concurrent_agents || 0)}
            </div>
            <div class="hint">
              Retry policy: default resume ${formatNumber(policy.default_resume_seconds || 0)}s ·
              auth resume ${formatNumber(policy.auth_resume_seconds || 0)}s ·
              max backoff ${formatNumber(policy.max_retry_backoff_ms || 0)}ms
            </div>
            <div class="hint">
              Transport: requested=${escapeHtml(requestedTransport)} · active=${escapeHtml(activeTransport)} ·
              state fetches ${formatNumber(transportMetrics.stateFetchTotal)} (200=${formatNumber(
                transportMetrics.stateFetch200
              )}, 304=${formatNumber(transportMetrics.stateFetch304)}, err=${formatNumber(
                transportMetrics.stateFetchErrors
              )}) ·
              sse frames ${formatNumber(transportMetrics.sseFrames)} · reconnects ${formatNumber(
                transportMetrics.sseReconnects
              )} ·
              poll ticks ${formatNumber(transportMetrics.fallbackPollTicks)} ·
              poll delay ${formatNumber(
                pollScheduledDelayMs > 0 ? pollScheduledDelayMs : fallbackPollIntervalMs
              )}ms ·
              render commits ${formatNumber(transportMetrics.renderCommits)}
            </div>
          </div>
        `;
        const entries = Object.entries(limits);
        if (!entries.length) {
          rateWrap.innerHTML = summaryHtml + '<div class="empty">No rate limit telemetry recorded.</div>';
          return;
        }
        const rows = entries
          .map(([key, value]) => {
            let valStr = "";
            if (typeof value === 'object' && value !== null) {
              const obj = toObject(value);
              if (obj.usedPercent != null) valStr += `${obj.usedPercent}% used`;
              if (obj.resetsAt != null || obj.resetAt != null) {
                const r = obj.resetsAt || obj.resetAt;
                if (valStr) valStr += " | ";
                valStr += `resets ${relTime(r)}`;
              }
              if (obj.hasCredits != null) {
                if (valStr) valStr += " | ";
                valStr += `credits: ${obj.hasCredits}`;
              }
              if (obj.balance != null) {
                if (valStr) valStr += " | ";
                valStr += `balance: ${obj.balance}`;
              }
              if (!valStr) valStr = JSON.stringify(obj);
            } else {
              valStr = String(value);
            }
            return `
              <div class="kv-item">
                <div class="key mono">${escapeHtml(key)}</div>
                <div class="value mono">${escapeHtml(valStr)}</div>
              </div>
            `;
          })
          .join("");
        rateWrap.innerHTML = summaryHtml + `<div class="kv-list" style="margin-top: 12px">${rows}</div>`;
      }

      function renderKpis(state) {
        const counts = toObject(state.counts);
        const totals = toObject(state.codex_totals);
        const attention = toArray(state.attention);
        const retryCount = toNumber(counts.retrying, toArray(state.retrying).length);
        const throughput = toNumber(
          state.tokens_per_second || totals.tokens_per_second || state.throughput_tps,
          0
        );
        const totalTokens = toNumber(totals.total_tokens, 0);
        const completed = toNumber(counts.completed, 0);
        const runtime = toNumber(totals.seconds_running, 0);
        const healthEl = document.getElementById("kpi-health");
        const healthMetaEl = document.getElementById("kpi-health-meta");

        document.getElementById("kpi-running").textContent = formatNumber(counts.running);
        document.getElementById("kpi-retrying").textContent = formatNumber(retryCount);
        document.getElementById("kpi-throughput").textContent = `${formatNumber(
          throughput
        )} / sec`;
        document.getElementById("kpi-throughput-meta").textContent = `${formatNumber(
          totalTokens
        )} total tokens`;
        document.getElementById("kpi-progress").textContent = formatNumber(
          totals.turns_completed
        );
        document.getElementById("kpi-progress-meta").textContent = `${formatNumber(
          completed
        )} completed issues`;
        document.getElementById("kpi-runtime").textContent = formatDurationSeconds(runtime);
        setMeter("meter-running", Math.min(toNumber(counts.running, 0) / 8, 1));
        setMeter("meter-retrying", Math.min(retryCount / 8, 1));
        setMeter("meter-throughput", Math.min(throughput / 8000, 1));
        setMeter("meter-progress", Math.min(toNumber(totals.turns_completed, 0) / 40, 1));
        setMeter("meter-runtime", Math.min(runtime / 3600, 1));

        if (attention.length) {
          healthEl.textContent = "Needs Action";
          healthMetaEl.textContent = `${formatNumber(attention.length)} human action item(s)`;
          setMeter("meter-health", 1);
          return;
        }
        if (retryCount > 0) {
          healthEl.textContent = "Degraded";
          healthMetaEl.textContent = `${formatNumber(retryCount)} item(s) in retry queue`;
          setMeter("meter-health", 0.55);
          return;
        }
        healthEl.textContent = "Stable";
        healthMetaEl.textContent = "No blocking alerts";
        setMeter("meter-health", 0.22);
      }

      function commitRenderState(state) {
        transportMetrics.renderCommits += 1;
        const safeState = toObject(state);
        const generatedAt = String(safeState.generated_at || "");
        if (generatedAt && latestGeneratedAt && generatedAt < latestGeneratedAt) {
          return;
        }
        if (generatedAt) {
          latestGeneratedAt = generatedAt;
        }
        latestState = safeState;
        applyTransportCadence(safeState);
        updatePanel(
          "kpis",
          {
            counts: safeState.counts,
            codex_totals: safeState.codex_totals,
            attention_count: toArray(safeState.attention).length,
            retry_count: toArray(safeState.retrying).length,
          },
          () => renderKpis(safeState)
        );
        updatePanel(
          "attention",
          {
            attention: safeState.attention,
            manual_actions: safeState.manual_actions,
            running: safeState.running,
            retrying: safeState.retrying,
            local_status: Array.from(localActionStatus.entries()),
            pending_retries: Array.from(pendingRetries.values()).sort(),
          },
          () => renderAttention(safeState)
        );
        updatePanel(
          "running",
          { running: safeState.running },
          () => renderRunning(safeState)
        );
        updatePanel(
          "retry",
          { retrying: safeState.retrying },
          () => renderRetry(safeState)
        );
        updatePanel(
          "profiling",
          {
            profiling: safeState.profiling,
            hotspots: safeState.hotspots,
            profiling_hotspots: safeState.profiling_hotspots,
          },
          () => renderProfiling(safeState)
        );
        updatePanel(
          "security",
          {
            runtime: safeState.runtime,
          },
          () => renderSecurity(safeState)
        );
        updatePanel(
          "events",
          { recent_events: safeState.recent_events },
          () => renderEvents(safeState)
        );
        updatePanel(
          "intervention_activity",
          { manual_actions: safeState.manual_actions },
          () => renderInterventionActivity(safeState)
        );
        updatePanel(
          "rate_limits",
          {
            rate_limits: safeState.rate_limits,
            suspension: safeState.suspension,
            runtime: safeState.runtime,
          },
          () => renderRateLimits(safeState)
        );
        updatePanel(
          "agent_workspace",
          {
            agent_panes: safeState.agent_panes,
            running: safeState.running,
            layout: workspaceLayout,
            verbosity: workspaceVerbosity,
            pane_order: paneOrder,
          },
          () => renderAgentWorkspace(safeState)
        );
        updatePanel(
          "tool_launcher",
          {
            attention: safeState.attention,
            running: safeState.running,
            retrying: safeState.retrying,
          },
          () => renderToolLauncher(safeState)
        );
        updatePanel(
          "durable_memory",
          { durable: durableMemoryState },
          () => renderDurableMemory()
        );
        const updatedText = `Updated ${formatTime(safeState.generated_at)}`;
        if (updatedChip.textContent !== updatedText) {
          updatedChip.textContent = updatedText;
        }
        publishClientTelemetry();
      }

      function renderState(state) {
        const safeState = toObject(state);
        const generatedAt = String(safeState.generated_at || "");
        if (generatedAt && latestGeneratedAt && generatedAt < latestGeneratedAt) {
          return;
        }
        pendingRenderState = safeState;
        if (renderFrame) {
          return;
        }
        const schedule =
          typeof window.requestAnimationFrame === "function"
            ? window.requestAnimationFrame.bind(window)
            : (callback) => window.setTimeout(callback, 16);
        renderFrame = schedule(() => {
          transportMetrics.renderFrames += 1;
          renderFrame = 0;
          const nextState = pendingRenderState;
          pendingRenderState = null;
          if (!nextState) return;
          commitRenderState(nextState);
        });
      }

      async function fetchState() {
        transportMetrics.stateFetchTotal += 1;
        try {
          const headers = buildApiHeaders({ cacheControl: true });
          if (latestStateEtag) {
            headers["If-None-Match"] = latestStateEtag;
          }
          const resp = await fetch(withAuthPath("/api/v1/state"), { headers });
          if (resp.status === 304) {
            transportMetrics.stateFetch304 += 1;
            publishClientTelemetry();
            return { changed: false, status: 304, retryAfterMs: 0 };
          }
          if (!resp.ok) {
            const retryAfterMs = parseRetryAfterHeader(resp.headers.get("Retry-After"));
            let errorCode = "";
            try {
              const payload = await resp.json();
              errorCode = String(toObject(payload.error).code || "");
            } catch (_err) {
              errorCode = "";
            }
            const err = new Error(`state_fetch_failed:${resp.status}`);
            err.status = resp.status;
            err.retryAfterMs = retryAfterMs;
            err.errorCode = errorCode;
            throw err;
          }
          transportMetrics.stateFetch200 += 1;
          const etag = String(resp.headers.get("ETag") || "").trim();
          if (etag) {
            latestStateEtag = etag;
          }
          const data = await resp.json();
          renderState(data);
          publishClientTelemetry();
          return { changed: true, status: 200, retryAfterMs: 0 };
        } catch (_err) {
          transportMetrics.stateFetchErrors += 1;
          publishClientTelemetry();
          throw _err;
        }
      }

      async function fetchDurableMemory() {
        if (durableFetchInFlight) return false;
        durableFetchInFlight = true;
        try {
          const resp = await fetch(withAuthPath("/api/v1/durable?limit=140"), {
            headers: buildApiHeaders({ cacheControl: true }),
          });
          if (!resp.ok) {
            throw new Error(`durable_fetch_failed:${resp.status}`);
          }
          durableMemoryState = await resp.json();
          renderState(latestState);
          return true;
        } catch (_err) {
          return false;
        } finally {
          durableFetchInFlight = false;
        }
      }

      async function triggerRefresh() {
        refreshBtn.disabled = true;
        try {
          const resp = await fetch(withAuthPath("/api/v1/refresh"), {
            method: "POST",
            headers: buildApiHeaders({ includeCsrf: true }),
          });
          if (!resp.ok) throw new Error(`refresh_failed:${resp.status}`);
          await fetchState();
        } catch (_err) {
          setStreamStatus("Refresh failed", "warn");
        } finally {
          refreshBtn.disabled = false;
        }
      }

      async function triggerRetryNow(issueIdentifier) {
        if (!issueIdentifier) return;
        pendingRetries.add(issueIdentifier);
        localActionStatus.set(issueIdentifier, {
          status: "pending",
          message: "Submitting retry request...",
          at: new Date().toISOString(),
        });
        renderState(latestState);
        try {
          const resp = await fetch(withAuthPath("/api/v1/interventions/retry-now"), {
            method: "POST",
            headers: buildApiHeaders({
              includeJsonContentType: true,
              includeCsrf: true,
            }),
            body: JSON.stringify({ issue_identifier: issueIdentifier }),
          });
          const payload = await resp.json();
          if (!resp.ok || payload.ok === false) {
            localActionStatus.set(issueIdentifier, {
              status: payload.status || "failed",
              message: payload.message || `retry_now_failed:${resp.status}`,
              at: new Date().toISOString(),
            });
            setStreamStatus("Retry-now action failed", "warn");
            return;
          }
          localActionStatus.set(issueIdentifier, {
            status: payload.status || "queued",
            message: payload.message || "Retry request accepted.",
            at: new Date().toISOString(),
          });
          setStreamStatus("Retry request queued", "live");
        } catch (_err) {
          setStreamStatus("Retry-now action failed", "warn");
        } finally {
          pendingRetries.delete(issueIdentifier);
          renderState(latestState);
          await fetchState().catch(() => {
            // keep UI responsive even if a state refresh races with transport reconnects
          });
        }
      }

      async function runDashboardTool() {
        const tool = String(toolSelect?.value || "").trim();
        let issueIdentifier = String(toolIssueInput?.value || "").trim();
        if (issueIdentifier.includes(" - ")) {
          issueIdentifier = issueIdentifier.split(" - ")[0].trim();
        }
        if (!tool) return;
        const needsIssueIdentifier = !(
          tool === "refresh_cycle" ||
          tool === "durable_backup" ||
          tool === "durable_integrity_check"
        );
        if (!needsIssueIdentifier) {
          issueIdentifier = "";
        }
        if (toolRunButton) {
          toolRunButton.disabled = true;
        }
        const body = {
          tool,
          issue_identifier: issueIdentifier,
        };
        if (tool === "set_max_concurrent_agents") {
          body.value = issueIdentifier;
        }
        try {
          const resp = await fetch(withAuthPath("/api/v1/tools/run"), {
            method: "POST",
            headers: buildApiHeaders({
              includeJsonContentType: true,
              includeCsrf: true,
            }),
            body: JSON.stringify(body),
          });
          const payload = await resp.json();
          toolResult.textContent = JSON.stringify(payload, null, 2);
          if (!resp.ok || payload.ok === false) {
            setStreamStatus("Tool run failed", "warn");
            return;
          }
          await fetchState();
        } catch (_err) {
          toolResult.textContent = "Tool run failed.";
          setStreamStatus("Tool run failed", "warn");
        } finally {
          if (toolRunButton) {
            toolRunButton.disabled = false;
          }
        }
      }

      function stopTracePolling() {
        if (tracePollTimer) {
          clearTimeout(tracePollTimer);
          tracePollTimer = null;
        }
      }

      function closeTraceModal() {
        stopTracePolling();
        traceIssueIdentifier = "";
        traceModal?.classList.remove("open");
        traceModal?.setAttribute("aria-hidden", "true");
        document.body.classList.remove("modal-open");
        traceSubtitle.textContent = "Select an agent to inspect.";
        traceSummary.innerHTML = '<div class="empty">No trace selected.</div>';
        traceEvents.innerHTML =
          '<div class="empty">Click an issue/agent pill to open live trace.</div>';
        if (traceLastFocusedElement instanceof HTMLElement) {
          traceLastFocusedElement.focus();
        }
        traceLastFocusedElement = null;
      }

      function traceFocusableElements() {
        if (!traceModal) return [];
        const nodes = traceModal.querySelectorAll(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'
        );
        return Array.from(nodes).filter((node) => {
          if (!(node instanceof HTMLElement)) return false;
          if (node.hasAttribute("disabled")) return false;
          if (node.getAttribute("aria-hidden") === "true") return false;
          return node.offsetParent !== null;
        });
      }

      function focusTracePrimaryControl() {
        if (traceCloseButton instanceof HTMLElement) {
          traceCloseButton.focus();
          return;
        }
        if (tracePanel instanceof HTMLElement) {
          tracePanel.focus();
        }
      }

      function traceEventTone(eventName) {
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

      function traceStatusClass(status) {
        const norm = String(status || "unknown").toLowerCase();
        if (norm.includes("run")) return "status-running";
        if (norm.includes("retry")) return "status-retrying";
        if (norm.includes("block") || norm.includes("fail")) return "status-blocked";
        return "";
      }

      function renderTraceIssue(payloadValue) {
        const payload = toObject(payloadValue);
        const status = String(payload.status || "unknown");
        const running = toObject(payload.running);
        const attempts = toObject(payload.attempts);
        const retry = toObject(payload.retry);
        const recent = toArray(payload.recent_events).slice().reverse().slice(0, 80);
        traceSubtitle.textContent = `${payload.issue_identifier || traceIssueIdentifier} · ${status}`;
        const statusClass = traceStatusClass(status);
        traceSummary.innerHTML = `
          <div class="trace-status-row">
            <span class="trace-status-pill ${statusClass}">${escapeHtml(status)}</span>
            <span class="meta">${escapeHtml(relTime(payload.generated_at || payload.updated_at || null))}</span>
          </div>
          <div class="trace-summary-grid">
            <div class="trace-stat">
              <div class="label">Issue State</div>
              <div class="value">${escapeHtml(running.state || "n/a")}</div>
            </div>
            <div class="trace-stat">
              <div class="label">Turns</div>
              <div class="value">${formatNumber(running.turn_count || 0)}</div>
            </div>
            <div class="trace-stat">
              <div class="label">Role</div>
              <div class="value">${escapeHtml(running.worker_role || "n/a")}</div>
            </div>
            <div class="trace-stat">
              <div class="label">Retry Attempt</div>
              <div class="value">${escapeHtml(
                attempts.current_retry_attempt != null
                  ? String(attempts.current_retry_attempt)
                  : "n/a"
              )}</div>
            </div>
            <div class="trace-stat">
              <div class="label">Retry Due</div>
              <div class="value">${escapeHtml(
                retry.due_in_seconds != null ? `${retry.due_in_seconds}s` : "n/a"
              )}</div>
            </div>
            <div class="trace-stat">
              <div class="label">Last Event</div>
              <div class="value">${escapeHtml(running.last_event || "n/a")}</div>
            </div>
          </div>
          ${
            payload.last_error
              ? `<div class="event-detail"><strong>Last error:</strong> ${escapeHtml(
                  String(payload.last_error)
                )}</div>`
              : ""
          }
        `;
        if (!recent.length) {
          traceEvents.innerHTML =
            '<div class="empty">No trace events yet for this agent.</div>';
          return;
        }
        traceEvents.innerHTML = recent
          .map((eventValue) => {
            const event = toObject(eventValue);
            const tone = traceEventTone(event.event || "");
            return `
              <article class="trace-event ${tone}">
                <div class="head">
                  <div class="event-tags">
                    <div class="event-name mono">${escapeHtml(event.event || "event")}</div>
                    <span class="event-type">${escapeHtml(tone)}</span>
                  </div>
                  <div class="meta">${escapeHtml(relTime(event.at))}</div>
                </div>
                <div class="event-body">${escapeHtml(event.message || "")}</div>
                ${
                  event.detail
                    ? `<div class="event-detail">${escapeHtml(String(event.detail || ""))}</div>`
                    : ""
                }
              </article>
            `;
          })
          .join("");
      }

      async function fetchTraceIssue() {
        if (!traceIssueIdentifier) return;
        if (traceFetchInFlight) return;
        traceFetchInFlight = true;
        const expectedIssue = traceIssueIdentifier;
        const serial = ++traceFetchSerial;
        try {
          const resp = await fetch(
            withAuthPath(`/api/v1/${encodeURIComponent(expectedIssue)}`),
            { headers: buildApiHeaders({ cacheControl: true }) }
          );
          if (serial !== traceFetchSerial || expectedIssue !== traceIssueIdentifier) {
            return;
          }
          if (!resp.ok) {
            traceSummary.innerHTML =
              '<div class="empty">Trace metadata unavailable for this issue.</div>';
            traceEvents.innerHTML = `<div class="empty">Trace not available (${resp.status}).</div>`;
            return;
          }
          const payload = await resp.json();
          if (serial !== traceFetchSerial || expectedIssue !== traceIssueIdentifier) {
            return;
          }
          renderTraceIssue(payload);
        } catch (_err) {
          if (serial !== traceFetchSerial || expectedIssue !== traceIssueIdentifier) {
            return;
          }
          traceSummary.innerHTML =
            '<div class="empty">Trace metadata fetch failed.</div>';
          traceEvents.innerHTML = '<div class="empty">Trace fetch failed.</div>';
        } finally {
          traceFetchInFlight = false;
          if (traceIssueIdentifier === expectedIssue) {
            stopTracePolling();
            tracePollTimer = setTimeout(fetchTraceIssue, 1200);
          }
        }
      }

      function openTraceModal(issueIdentifier) {
        const issue = String(issueIdentifier || "").trim();
        if (!issue) return;
        if (document.activeElement instanceof HTMLElement) {
          traceLastFocusedElement = document.activeElement;
        }
        traceIssueIdentifier = issue;
        traceModal?.classList.add("open");
        traceModal?.setAttribute("aria-hidden", "false");
        document.body.classList.add("modal-open");
        traceSubtitle.textContent = `${issue} · loading`;
        traceSummary.innerHTML = '<div class="empty">Loading trace summary...</div>';
        traceEvents.innerHTML = '<div class="empty">Loading live trace...</div>';
        stopTracePolling();
        fetchTraceIssue();
        window.setTimeout(focusTracePrimaryControl, 0);
      }

      function startPollingFallback(forceRestart = false) {
        if (pollTimer && !forceRestart) {
          return;
        }
        if (pollTimer) {
          stopPolling();
        }
        if (forceRestart) {
          pollErrorStreak = 0;
          pollNotModifiedStreak = 0;
        }
        activeTransport = "poll";
        _setFallbackStatus(fallbackPollIntervalMs);
        const run = async () => {
          transportMetrics.fallbackPollTicks += 1;
          let delayMs = fallbackPollIntervalMs;
          let statusReason = "";
          try {
            const result = await fetchState();
            delayMs = _nextFallbackDelayMs({
              changed: Boolean(result.changed),
              hadError: false,
            });
          } catch (err) {
            const retryAfterMs = toNumber(err?.retryAfterMs, 0);
            const statusCode = toNumber(err?.status, 0);
            const errorCode = String(err?.errorCode || "").toLowerCase();
            const isRateLimited = statusCode === 429 || errorCode === "rate_limited";
            delayMs = _nextFallbackDelayMs({
              changed: false,
              hadError: true,
              retryAfterMs,
            });
            statusReason = isRateLimited ? "rate_limited" : "fetch_error";
          }
          if (activeTransport !== "poll") {
            return;
          }
          pollScheduledDelayMs = Math.max(250, Math.round(delayMs));
          _setFallbackStatus(pollScheduledDelayMs, statusReason);
          pollTimer = setTimeout(run, pollScheduledDelayMs);
        };
        run();
      }

      function scheduleReconnect() {
        if (reconnectTimer) return;
        const delayMs = Math.min(12000, 1000 * Math.pow(2, Math.min(reconnectAttempts, 4)));
        activeTransport = "reconnecting";
        transportMetrics.sseReconnects += 1;
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          reconnectAttempts += 1;
          if (stateTransport) {
            stateTransport.start(false);
            return;
          }
          connectStream();
        }, delayMs);
      }

      function connectStream(manual = false) {
        if (requestedTransport === "poll") {
          startPollingFallback(true);
          return;
        }
        stopReconnectTimer();
        stopStream();
        lastFrameAt = 0;
        if (!window.EventSource || typeof EventSource !== "function") {
          activeTransport = "poll";
          startPollingFallback();
          return;
        }
        if (manual) {
          reconnectAttempts = 0;
        }
        activeTransport = "sse";
        setStreamStatus(`Connecting live stream (${streamIntervalMs}ms)...`, "connecting");
        ensureWatchdog();
        stream = new EventSource(
          withAuthPath(`/api/v1/stream?interval_ms=${streamIntervalMs}`)
        );
        stream.onopen = () => {
          reconnectAttempts = 0;
          lastFrameAt = Date.now();
          stopPolling();
          activeTransport = "sse";
          setStreamStatus(`Live stream (${streamIntervalMs}ms)`, "live");
        };
        stream.onerror = () => {
          activeTransport = "reconnecting";
          setStreamStatus("Stream reconnecting...", "warn");
          startPollingFallback();
          scheduleReconnect();
        };
        stream.addEventListener("state", (event) => {
          try {
            lastFrameAt = Date.now();
            transportMetrics.sseFrames += 1;
            if (event.lastEventId) {
              latestStateEtag = String(event.lastEventId);
            }
            renderState(JSON.parse(event.data));
            stopPolling();
            activeTransport = "sse";
            setStreamStatus(`Live stream (${streamIntervalMs}ms)`, "live");
          } catch (_err) {
            // ignore malformed frames
          }
        });
      }

      function startDurablePolling() {
        if (durablePollTimer) return;
        const run = async () => {
          await fetchDurableMemory();
        };
        durablePollTimer = setInterval(run, 10000);
        run();
      }

      loadWorkspacePrefs();
      loadDashboardView();
      requestedTransport = resolveTransportPreference();
      try {
        window.localStorage.setItem(TRANSPORT_STORAGE_KEY, requestedTransport);
      } catch (_err) {
        // ignore storage write failures
      }
      stateTransport = createStateTransportController();
      setDashboardView(activeDashboardView, false);
      layoutSelect.value = workspaceLayout;
      verbositySelect.value = workspaceVerbosity;
      const dashboardGrid = document.querySelector(".dashboard-grid");
      if (dashboardGrid) dashboardGrid.dataset.layout = workspaceLayout;
      workspaceWrap.addEventListener("dragstart", handlePaneDragStart);
      workspaceWrap.addEventListener("dragover", handlePaneDragOver);
      workspaceWrap.addEventListener("drop", handlePaneDrop);
      workspaceWrap.addEventListener("dragend", handlePaneDragEnd);
      layoutSelect.addEventListener("change", () => {
        workspaceLayout = normalizeLayout(layoutSelect.value);
        layoutSelect.value = workspaceLayout;
        const dashboardGrid = document.querySelector(".dashboard-grid");
        if (dashboardGrid) dashboardGrid.dataset.layout = workspaceLayout;
        persistWorkspacePrefs();
        renderState(latestState);
      });
      verbositySelect.addEventListener("change", () => {
        workspaceVerbosity = normalizeVerbosity(verbositySelect.value);
        verbositySelect.value = workspaceVerbosity;
        persistWorkspacePrefs();
        renderState(latestState);
      });
      refreshBtn.addEventListener("click", triggerRefresh);
      reloadBtn.addEventListener("click", () => stateTransport?.restart(true));
      document.querySelector(".top-nav")?.addEventListener("click", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const tab = target.closest(".view-tab");
        if (!(tab instanceof HTMLButtonElement)) return;
        setDashboardView(tab.dataset.view || "overview", true);
      });
      document.querySelector(".top-nav")?.addEventListener("keydown", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const tab = target.closest(".view-tab");
        if (!(tab instanceof HTMLButtonElement)) return;
        if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
          event.preventDefault();
          const dir = event.key === "ArrowRight" ? 1 : -1;
          const idx = viewTabs.indexOf(tab);
          if (idx < 0) return;
          const nextIdx = (idx + dir + viewTabs.length) % viewTabs.length;
          const nextTab = viewTabs[nextIdx];
          if (!(nextTab instanceof HTMLButtonElement)) return;
          setDashboardView(nextTab.dataset.view || "overview", true);
          nextTab.focus();
          return;
        }
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        setDashboardView(tab.dataset.view || "overview", true);
      });
      attentionList.addEventListener("click", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const button = target.closest("button[data-action='retry-now']");
        if (!(button instanceof HTMLButtonElement)) return;
        const issueIdentifier = String(button.dataset.issue || "");
        triggerRetryNow(issueIdentifier);
      });
      document.addEventListener("click", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const agentRef = target.closest(".agent-ref[data-agent-issue]");
        if (!agentRef) return;
        const issueIdentifier = String(agentRef.getAttribute("data-agent-issue") || "").trim();
        if (!issueIdentifier) return;
        if (target.closest("button[data-action='retry-now']")) return;
        openTraceModal(issueIdentifier);
      });
      traceRefreshButton?.addEventListener("click", () => {
        fetchTraceIssue();
      });
      traceCloseButton?.addEventListener("click", () => {
        closeTraceModal();
      });
      traceModal?.addEventListener("click", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        if (target === traceModal) {
          closeTraceModal();
        }
      });
      document.addEventListener("keydown", (event) => {
        const traceOpen = Boolean(traceModal?.classList.contains("open"));
        if (!traceOpen) return;
        if (event.key === "Escape") {
          event.preventDefault();
          closeTraceModal();
          return;
        }
        if (event.key !== "Tab") return;
        const focusable = traceFocusableElements();
        if (!focusable.length) {
          event.preventDefault();
          focusTracePrimaryControl();
          return;
        }
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        const active = document.activeElement;
        const activeInsideModal = Boolean(active && traceModal?.contains(active));
        if (event.shiftKey) {
          if (!activeInsideModal || active === first) {
            event.preventDefault();
            last.focus();
          }
          return;
        }
        if (!activeInsideModal || active === last) {
          event.preventDefault();
          first.focus();
        }
      });
      toolRunButton?.addEventListener("click", () => {
        runDashboardTool();
      });
      toolSelect?.addEventListener("change", () => {
        const selected = String(toolSelect?.value || "").trim();
        if (!toolIssueInput) return;
        if (selected === "set_max_concurrent_agents") {
          toolIssueInput.placeholder = "Max concurrent agents (e.g. 2)";
          toolIssueInput.disabled = false;
          return;
        }
        if (
          selected === "refresh_cycle" ||
          selected === "durable_backup" ||
          selected === "durable_integrity_check"
        ) {
          toolIssueInput.placeholder = "Not required for this tool";
          toolIssueInput.value = "";
          toolIssueInput.disabled = true;
          return;
        }
        toolIssueInput.placeholder = "Issue identifier (e.g. MOL-42)";
        toolIssueInput.disabled = false;
      });
      toolSelect?.dispatchEvent(new Event("change"));
      stateTransport.start(false);
      fetchState().catch(() => stateTransport?.start(false));
      startDurablePolling();
