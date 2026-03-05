from __future__ import annotations

import json
import sys
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from typing import Any, Protocol
from urllib.parse import parse_qs, unquote, urlsplit


class StateProvider(Protocol):
    def snapshot_state(self) -> dict[str, Any]: ...

    def snapshot_issue(self, issue_identifier: str) -> dict[str, Any] | None: ...

    def request_refresh(self) -> bool: ...

    def request_retry_now(self, issue_identifier: str) -> dict[str, Any]: ...

    def run_dashboard_tool(
        self, tool_name: str, payload: dict[str, Any]
    ) -> dict[str, Any]: ...


class _QuietThreadingHTTPServer(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request: Any, client_address: Any) -> None:
        exc_type, _, _ = sys.exc_info()
        if exc_type in {BrokenPipeError, ConnectionResetError, TimeoutError}:
            return
        super().handle_error(request, client_address)


@dataclass(slots=True)
class DashboardServer:
    provider: StateProvider
    port: int
    _server: _QuietThreadingHTTPServer | None = field(default=None, init=False)
    _thread: Thread | None = field(default=None, init=False)

    def start(self) -> int:
        provider = self.provider

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:  # noqa: A003
                return

            def _write_json(self, status: int, payload: dict[str, Any]) -> None:
                data = json.dumps(payload, ensure_ascii=True).encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def _write_html(self, status: int, body: str) -> None:
                data = body.encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self) -> None:  # noqa: N802
                parsed = urlsplit(self.path)
                path = parsed.path

                if path == "/":
                    self._handle_dashboard()
                    return
                if path == "/api/v1/state":
                    self._write_json(HTTPStatus.OK, provider.snapshot_state())
                    return
                if path == "/api/v1/stream":
                    self._handle_state_stream(parsed.query)
                    return
                if path.startswith("/api/v1/"):
                    issue_identifier = unquote(path.removeprefix("/api/v1/"))
                    payload = provider.snapshot_issue(issue_identifier)
                    if payload is None:
                        self._write_json(
                            HTTPStatus.NOT_FOUND,
                            {
                                "error": {
                                    "code": "issue_not_found",
                                    "message": (
                                        f"Unknown issue identifier: {issue_identifier}"
                                    ),
                                }
                            },
                        )
                        return
                    self._write_json(HTTPStatus.OK, payload)
                    return
                self._write_json(
                    HTTPStatus.NOT_FOUND,
                    {
                        "error": {
                            "code": "not_found",
                            "message": f"Unknown route: {path}",
                        }
                    },
                )

            def do_POST(self) -> None:  # noqa: N802
                parsed = urlsplit(self.path)
                path = parsed.path

                if path == "/api/v1/refresh":
                    queued = provider.request_refresh()
                    self._write_json(
                        HTTPStatus.ACCEPTED,
                        {
                            "queued": queued,
                            "coalesced": not queued,
                            "requested_at": datetime.now(UTC)
                            .isoformat()
                            .replace("+00:00", "Z"),
                            "operations": ["poll", "reconcile"],
                        },
                    )
                    return
                if path == "/api/v1/interventions/retry-now":
                    payload = self._read_json_payload()
                    issue_identifier = str(payload.get("issue_identifier", "")).strip()
                    result = provider.request_retry_now(issue_identifier)
                    status = (
                        HTTPStatus.ACCEPTED
                        if bool(result.get("ok"))
                        else HTTPStatus.NOT_FOUND
                    )
                    self._write_json(int(status), result)
                    return
                if path == "/api/v1/tools/run":
                    payload = self._read_json_payload()
                    tool_name = str(payload.get("tool") or "").strip()
                    result = provider.run_dashboard_tool(tool_name, payload)
                    status = (
                        HTTPStatus.ACCEPTED
                        if bool(result.get("ok"))
                        else HTTPStatus.BAD_REQUEST
                    )
                    self._write_json(int(status), result)
                    return

                if (
                    path == "/"
                    or path == "/api/v1/state"
                    or path == "/api/v1/stream"
                    or path == "/api/v1/interventions/retry-now"
                    or path == "/api/v1/tools/run"
                    or path.startswith("/api/v1/")
                ):
                    self._write_json(
                        HTTPStatus.METHOD_NOT_ALLOWED,
                        {
                            "error": {
                                "code": "method_not_allowed",
                                "message": f"Method POST not allowed for {path}",
                            }
                        },
                    )
                    return

                self._write_json(
                    HTTPStatus.NOT_FOUND,
                    {
                        "error": {
                            "code": "not_found",
                            "message": f"Unknown route: {path}",
                        }
                    },
                )

            def _read_json_payload(self) -> dict[str, Any]:
                content_length_raw = self.headers.get("Content-Length") or "0"
                try:
                    content_length = int(content_length_raw)
                except ValueError:
                    content_length = 0
                if content_length <= 0:
                    return {}
                if content_length > 262_144:
                    return {}
                body = self.rfile.read(content_length)
                if not body:
                    return {}
                try:
                    decoded = json.loads(body.decode("utf-8"))
                except (UnicodeDecodeError, json.JSONDecodeError):
                    return {}
                if isinstance(decoded, dict):
                    return decoded
                return {}

            def _handle_dashboard(self) -> None:
                self._write_html(HTTPStatus.OK, _DASHBOARD_HTML)

            def _handle_state_stream(self, query: str) -> None:
                interval_ms = _coerce_stream_interval_ms(query)
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "keep-alive")
                self.end_headers()
                try:
                    self.wfile.write(b": stream-open\n\n")
                    self.wfile.flush()
                    next_heartbeat = time.monotonic() + 15.0
                    while True:
                        payload = provider.snapshot_state()
                        encoded = json.dumps(payload, ensure_ascii=True)
                        event = f"event: state\ndata: {encoded}\n\n".encode("utf-8")
                        self.wfile.write(event)
                        self.wfile.flush()
                        deadline = time.monotonic() + (interval_ms / 1000.0)
                        while True:
                            remaining = deadline - time.monotonic()
                            if remaining <= 0:
                                break
                            sleep_window = min(remaining, 1.0)
                            time.sleep(sleep_window)
                            if time.monotonic() >= next_heartbeat:
                                self.wfile.write(b": heartbeat\n\n")
                                self.wfile.flush()
                                next_heartbeat = time.monotonic() + 15.0
                except (BrokenPipeError, ConnectionResetError, TimeoutError):
                    return

        server = _QuietThreadingHTTPServer(("127.0.0.1", self.port), Handler)
        self._server = server
        self._thread = Thread(
            target=server.serve_forever, name="symphony-http", daemon=True
        )
        self._thread.start()
        return int(server.server_port)

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
        if self._thread is not None:
            self._thread.join(timeout=2.0)


def _coerce_stream_interval_ms(query: str) -> int:
    values = parse_qs(query).get("interval_ms", [])
    if not values:
        return 1000
    raw = values[0]
    try:
        parsed = int(raw)
    except (TypeError, ValueError):
        return 1000
    return max(250, min(parsed, 10000))


_DASHBOARD_HTML = """<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Molt Symphony Control</title>
    <style>
      :root {
        --bg: #060606;
        --surface: #101010;
        --surface-alt: #161616;
        --ink: #f2f2f2;
        --ink-soft: #a6a6a6;
        --line: #2c2c2c;
        --brand: #f2f2f2;
        --brand-soft: #1f1f1f;
        --ok: #dddddd;
        --ok-soft: #1f1f1f;
        --warn: #bfbfbf;
        --warn-soft: #1b1b1b;
        --danger: #8f8f8f;
        --danger-soft: #151515;
        --shadow: 0 24px 44px -28px rgba(0, 0, 0, 0.65);
      }
      * {
        box-sizing: border-box;
      }
      body {
        margin: 0;
        color: var(--ink);
        font-family: "Aptos", "Avenir Next", "Segoe UI", sans-serif;
        background:
          radial-gradient(1150px 620px at 12% -8%, #2a2a2a 0%, transparent 62%),
          radial-gradient(960px 520px at 92% 0%, #1f1f1f 0%, transparent 60%),
          var(--bg);
        min-height: 100vh;
      }
      .dashboard-shell {
        max-width: min(1960px, calc(100vw - 24px));
        margin: 0 auto;
        padding: 24px 16px 44px;
        animation: fade-in 420ms ease-out;
      }
      .topbar {
        display: flex;
        flex-wrap: wrap;
        gap: 14px;
        align-items: center;
        justify-content: space-between;
        margin-bottom: 14px;
      }
      .title-wrap {
        display: grid;
        gap: 6px;
      }
      h1 {
        margin: 0;
        font-family: "Avenir Next", "Trebuchet MS", sans-serif;
        font-weight: 700;
        letter-spacing: -0.025em;
        line-height: 1.1;
      }
      .subtitle {
        color: var(--ink-soft);
        font-size: 0.95rem;
      }
      .controls {
        display: flex;
        gap: 8px;
        align-items: center;
        flex-wrap: wrap;
      }
      .top-nav {
        display: inline-flex;
        flex-wrap: wrap;
        gap: 8px;
        margin: 0 0 12px;
      }
      .view-tab {
        appearance: none;
        border: 1px solid var(--line);
        background: #111111;
        color: var(--ink-soft);
        border-radius: 999px;
        padding: 6px 12px;
        font-size: 0.83rem;
        font-weight: 600;
        cursor: pointer;
        transition: background 180ms ease, color 180ms ease, border-color 180ms ease;
      }
      .view-tab.active {
        border-color: var(--brand);
        background: var(--brand-soft);
        color: #ffffff;
      }
      .status-chip {
        border: 1px solid var(--line);
        background: #111111;
        border-radius: 999px;
        padding: 6px 11px;
        font-size: 0.82rem;
        color: var(--ink-soft);
      }
      .status-chip.live {
        border-color: #777777;
        color: #f0f0f0;
        background: var(--brand-soft);
      }
      .status-chip.warn {
        border-color: #5d5d5d;
        color: #d0d0d0;
        background: var(--warn-soft);
      }
      button {
        appearance: none;
        border: 1px solid var(--brand);
        background: #f2f2f2;
        color: #050505;
        border-radius: 11px;
        padding: 8px 12px;
        font-size: 0.9rem;
        font-weight: 600;
        cursor: pointer;
        transition: transform 120ms ease, box-shadow 140ms ease;
      }
      button:hover {
        transform: translateY(-1px);
        box-shadow: 0 14px 22px -16px rgba(255, 255, 255, 0.28);
      }
      button.secondary {
        border-color: var(--line);
        background: #151515;
        color: var(--ink);
      }
      button:disabled {
        opacity: 0.65;
        cursor: default;
      }
      select {
        border: 1px solid var(--line);
        border-radius: 9px;
        background: #141414;
        color: var(--ink);
        padding: 6px 8px;
        font-size: 0.84rem;
      }
      .control-group {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 0.82rem;
        color: var(--ink-soft);
      }
      a {
        color: #efefef;
      }
      a:hover {
        color: #ffffff;
      }
      .panel {
        background: linear-gradient(180deg, var(--surface) 0%, var(--surface-alt) 100%);
        border: 1px solid var(--line);
        border-radius: 16px;
        padding: 14px;
        box-shadow: var(--shadow);
      }
      .panel.hidden-view {
        display: none;
      }
      .section-head {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        gap: 8px;
        margin-bottom: 10px;
      }
      .section-head h2 {
        margin: 0;
        font-family: "Trebuchet MS", "Avenir Next", sans-serif;
        letter-spacing: -0.015em;
        font-size: 1rem;
      }
      .section-head .hint {
        color: var(--ink-soft);
        font-size: 0.8rem;
      }
      .kpi-band {
        margin-top: 2px;
        margin-bottom: 12px;
      }
      .kpi-grid {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 12px;
      }
      .kpi {
        background: #111111;
        border: 1px solid var(--line);
        border-radius: 13px;
        padding: 14px;
        min-height: 104px;
        animation: fade-in 260ms ease-out;
      }
      .kpi .label {
        color: var(--ink-soft);
        font-size: 0.78rem;
        margin-bottom: 7px;
        text-transform: uppercase;
        letter-spacing: 0.05em;
      }
      .kpi .value {
        font-size: 1.5rem;
        font-weight: 700;
        line-height: 1.08;
      }
      .kpi .meta {
        margin-top: 7px;
        color: var(--ink-soft);
        font-size: 0.8rem;
      }
      .meter {
        margin-top: 9px;
        height: 4px;
        border-radius: 999px;
        background: #1f1f1f;
        overflow: hidden;
      }
      .meter-fill {
        height: 100%;
        width: 0%;
        border-radius: 999px;
        background: linear-gradient(90deg, #7f7f7f, #f0f0f0);
        transition: width 320ms ease;
      }
      .dashboard-grid {
        margin-top: 12px;
        display: grid;
        grid-template-columns: repeat(12, minmax(0, 1fr));
        gap: 12px;
      }
      .panel-queue {
        grid-column: span 8;
      }
      .panel-tools {
        grid-column: span 4;
      }
      .panel-rate {
        grid-column: span 4;
      }
      .panel-running {
        grid-column: span 8;
      }
      .panel-retry {
        grid-column: span 4;
      }
      .panel-profiling {
        grid-column: span 6;
      }
      .panel-events {
        grid-column: span 6;
      }
      .panel-activity {
        grid-column: span 6;
      }
      .panel-workspace {
        grid-column: span 12;
      }
      .workspace-controls {
        display: inline-flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
      }
      .attention-summary {
        margin: -2px 0 8px;
        color: var(--ink-soft);
        font-size: 0.8rem;
      }
      .tool-launcher {
        display: grid;
        gap: 8px;
      }
      .tool-row {
        display: grid;
        grid-template-columns: 1fr 1.2fr auto;
        gap: 8px;
      }
      .tool-row input,
      .tool-row select {
        width: 100%;
      }
      .tool-result {
        margin-top: 4px;
        border: 1px solid #303030;
        border-radius: 9px;
        background: #0d0d0d;
        color: var(--ink-soft);
        font-size: 0.78rem;
        padding: 8px;
        white-space: pre-wrap;
        max-height: 160px;
        overflow: auto;
      }
      .action-status {
        margin-top: 6px;
        font-size: 0.78rem;
        color: var(--ink-soft);
      }
      .activity-list {
        display: grid;
        gap: 7px;
        max-height: 280px;
        overflow: auto;
      }
      .activity-item {
        border: 1px solid #2e2e2e;
        border-radius: 9px;
        background: #0e0e0e;
        padding: 8px;
      }
      .activity-item .head {
        display: flex;
        justify-content: space-between;
        gap: 8px;
      }
      .workspace-meta {
        color: var(--ink-soft);
        font-size: 0.82rem;
        margin-bottom: 8px;
      }
      .agent-workspace {
        display: grid;
        gap: 10px;
      }
      .agent-workspace[data-layout="grid"] {
        grid-template-columns: repeat(2, minmax(0, 1fr));
      }
      .agent-workspace[data-layout="columns"] {
        grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
      }
      .agent-workspace[data-layout="rows"] {
        grid-template-columns: 1fr;
      }
      .agent-pane {
        border: 1px solid #323232;
        border-radius: 12px;
        background: #0d0d0d;
        padding: 10px;
        transition: transform 160ms ease, border-color 200ms ease;
      }
      .agent-pane:hover {
        transform: translateY(-1px);
        border-color: #4a4a4a;
      }
      .agent-pane.dragging {
        opacity: 0.55;
      }
      .agent-pane.drop-target {
        outline: 2px dashed #b8b8b8;
        outline-offset: 2px;
      }
      .agent-pane-head {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        align-items: flex-start;
        margin-bottom: 8px;
      }
      .agent-pane-title {
        font-weight: 700;
        font-size: 0.88rem;
      }
      .agent-pane-sub {
        margin-top: 3px;
        font-size: 0.78rem;
        color: var(--ink-soft);
      }
      .agent-pane-badges {
        display: inline-flex;
        gap: 6px;
        align-items: center;
      }
      .drag-tag {
        border: 1px dashed #4a4a4a;
        border-radius: 999px;
        padding: 2px 7px;
        color: var(--ink-soft);
        font-size: 0.72rem;
      }
      .agent-lines {
        display: grid;
        gap: 4px;
      }
      .agent-line {
        font-size: 0.81rem;
        color: var(--ink-soft);
      }
      .agent-json {
        margin-top: 8px;
        max-height: 200px;
        overflow: auto;
        background: #090909;
        border: 1px solid #333333;
        border-radius: 8px;
        padding: 8px;
        white-space: pre-wrap;
      }
      .attention-list {
        display: grid;
        gap: 8px;
      }
      .attention-item {
        border: 1px solid #3d3d3d;
        background: var(--danger-soft);
        border-radius: 10px;
        padding: 10px;
      }
      .attention-item .head {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        font-weight: 700;
        font-size: 0.88rem;
      }
      .attention-item .msg {
        margin-top: 6px;
        font-size: 0.85rem;
      }
      .attention-item .hint {
        margin-top: 6px;
        font-size: 0.79rem;
        color: #b4b4b4;
      }
      .attention-tools {
        margin-top: 8px;
        display: flex;
        gap: 8px;
        flex-wrap: wrap;
      }
      .attention-tools a,
      .attention-tools button {
        border-radius: 8px;
        border: 1px solid var(--line);
        background: #141414;
        color: var(--ink);
        padding: 5px 8px;
        font-size: 0.75rem;
        text-decoration: none;
      }
      .attention-tools button {
        cursor: pointer;
      }
      .attention-tools button:hover,
      .attention-tools a:hover {
        border-color: #8a8a8a;
      }
      .empty {
        border: 1px dashed var(--line);
        border-radius: 10px;
        padding: 14px;
        color: var(--ink-soft);
        background: #0d0d0d;
      }
      table {
        width: 100%;
        border-collapse: collapse;
        font-size: 0.88rem;
      }
      th,
      td {
        text-align: left;
        padding: 8px;
        border-bottom: 1px solid #303030;
        vertical-align: top;
      }
      th {
        color: var(--ink-soft);
        font-size: 0.75rem;
        letter-spacing: 0.04em;
        text-transform: uppercase;
      }
      .table-scroll {
        width: 100%;
        overflow-x: auto;
      }
      .mono {
        font-family: "Consolas", "IBM Plex Mono", "SFMono-Regular", monospace;
        font-size: 0.82rem;
      }
      .badge {
        display: inline-block;
        border-radius: 999px;
        padding: 2px 8px;
        font-size: 0.74rem;
        font-weight: 700;
        border: 1px solid transparent;
      }
      .badge.ok {
        color: var(--ok);
        border-color: #575757;
        background: var(--ok-soft);
      }
      .badge.warn {
        color: var(--warn);
        border-color: #545454;
        background: var(--warn-soft);
      }
      .badge.danger {
        color: var(--danger);
        border-color: #454545;
        background: var(--danger-soft);
      }
      .kv-list {
        display: grid;
        gap: 6px;
      }
      .kv-item {
        display: grid;
        grid-template-columns: minmax(120px, 36%) 1fr;
        gap: 8px;
        padding: 7px 8px;
        border-radius: 8px;
        background: #0f0f0f;
        border: 1px solid #2e2e2e;
        font-size: 0.84rem;
      }
      .kv-item .key {
        color: var(--ink-soft);
      }
      .profiling-list {
        display: grid;
        gap: 8px;
      }
      .profiling-item {
        border: 1px solid #2f2f2f;
        border-radius: 10px;
        background: #0f0f0f;
        padding: 9px 10px;
      }
      .profiling-item .head {
        display: flex;
        justify-content: space-between;
        gap: 8px;
        align-items: center;
      }
      .profiling-item .name {
        font-size: 0.86rem;
        font-weight: 700;
      }
      .profiling-item .meta {
        margin-top: 5px;
        color: var(--ink-soft);
        font-size: 0.79rem;
      }
      .events {
        max-height: 440px;
        overflow: auto;
        padding-right: 4px;
      }
      .event {
        border-bottom: 1px solid #2f2f2f;
        padding: 8px 0;
      }
      .event:last-child {
        border-bottom: 0;
      }
      .event .line {
        font-size: 0.86rem;
      }
      .event .meta {
        margin-top: 2px;
        font-size: 0.78rem;
        color: var(--ink-soft);
      }
      .agent-ref {
        border: 1px solid #3a3a3a;
        border-radius: 999px;
        background: #141414;
        color: var(--ink);
        padding: 3px 8px;
        font-size: 0.74rem;
        cursor: pointer;
      }
      .agent-ref:hover {
        border-color: #8c8c8c;
      }
      .trace-modal {
        position: fixed;
        inset: 0;
        display: none;
        align-items: stretch;
        justify-content: flex-end;
        background: rgba(0, 0, 0, 0.58);
        z-index: 200;
      }
      .trace-modal.open {
        display: flex;
        animation: fade-in 160ms ease-out;
      }
      .trace-panel {
        width: min(760px, 96vw);
        height: 100%;
        background: linear-gradient(180deg, #101010, #090909);
        border-left: 1px solid #2f2f2f;
        display: grid;
        grid-template-rows: auto auto 1fr;
        box-shadow: -22px 0 48px -30px rgba(0, 0, 0, 0.8);
      }
      .trace-head {
        padding: 14px 16px 10px;
        border-bottom: 1px solid #2b2b2b;
        display: flex;
        justify-content: space-between;
        gap: 10px;
        align-items: flex-start;
      }
      .trace-title {
        margin: 0;
        font-size: 1.05rem;
      }
      .trace-sub {
        margin-top: 6px;
        color: var(--ink-soft);
        font-size: 0.8rem;
      }
      .trace-controls {
        display: inline-flex;
        gap: 8px;
      }
      .trace-summary {
        padding: 10px 16px;
        border-bottom: 1px solid #2b2b2b;
        display: grid;
        gap: 10px;
      }
      .trace-status-row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
      }
      .trace-status-pill {
        display: inline-flex;
        align-items: center;
        border-radius: 999px;
        padding: 3px 10px;
        font-size: 0.78rem;
        font-weight: 700;
        border: 1px solid #3b3b3b;
        background: #161616;
      }
      .trace-status-pill.status-running {
        border-color: #2f7f67;
        background: rgba(27, 72, 58, 0.35);
        color: #9fe6cb;
      }
      .trace-status-pill.status-retrying {
        border-color: #7f6b3e;
        background: rgba(84, 66, 30, 0.35);
        color: #f4d39f;
      }
      .trace-status-pill.status-blocked {
        border-color: #7f4a4a;
        background: rgba(89, 40, 40, 0.35);
        color: #f0b2b2;
      }
      .trace-summary-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(142px, 1fr));
        gap: 8px;
      }
      .trace-stat {
        border: 1px solid #2f2f2f;
        border-radius: 9px;
        background: #0d0d0d;
        padding: 7px 8px;
      }
      .trace-stat .label {
        color: var(--ink-soft);
        font-size: 0.72rem;
        text-transform: uppercase;
        letter-spacing: 0.05em;
      }
      .trace-stat .value {
        margin-top: 4px;
        font-size: 0.82rem;
      }
      .trace-stream {
        padding: 10px 16px 16px;
        overflow: auto;
      }
      .trace-events {
        display: grid;
        gap: 8px;
      }
      .trace-event {
        border: 1px solid #2f2f2f;
        border-left: 3px solid #444444;
        border-radius: 10px;
        background: #0f0f0f;
        padding: 8px 9px;
      }
      .trace-event.ok {
        border-left-color: #3b8f74;
      }
      .trace-event.warn {
        border-left-color: #8f7642;
      }
      .trace-event.danger {
        border-left-color: #8f4a4a;
      }
      .trace-event.info {
        border-left-color: #446a8f;
      }
      .trace-event .head {
        display: flex;
        justify-content: space-between;
        gap: 8px;
      }
      .trace-event .event-tags {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        flex-wrap: wrap;
      }
      .trace-event .event-type {
        border: 1px solid #333333;
        border-radius: 999px;
        padding: 2px 7px;
        font-size: 0.69rem;
        color: var(--ink-soft);
      }
      .trace-event .event-name {
        font-size: 0.8rem;
        font-weight: 700;
      }
      .trace-event .event-body {
        margin-top: 4px;
        font-size: 0.81rem;
        color: var(--ink);
        white-space: pre-wrap;
      }
      .trace-event .event-detail {
        margin-top: 4px;
        font-size: 0.79rem;
        color: var(--ink-soft);
        white-space: pre-wrap;
      }
      .footer {
        margin-top: 16px;
        font-size: 0.8rem;
        color: var(--ink-soft);
      }
      @media (min-width: 1540px) {
        .dashboard-grid {
          grid-template-columns: repeat(16, minmax(0, 1fr));
        }
        .panel-queue {
          grid-column: span 6;
        }
        .panel-tools {
          grid-column: span 5;
        }
        .panel-rate {
          grid-column: span 5;
        }
        .panel-running {
          grid-column: span 5;
        }
        .panel-retry {
          grid-column: span 5;
        }
        .panel-profiling {
          grid-column: span 6;
        }
        .panel-events {
          grid-column: span 5;
        }
        .panel-activity {
          grid-column: span 5;
        }
        .panel-workspace {
          grid-column: span 16;
        }
      }
      @media (max-width: 1180px) {
        .kpi-grid {
          grid-template-columns: repeat(2, minmax(0, 1fr));
        }
        .panel-queue,
        .panel-tools,
        .panel-rate,
        .panel-running,
        .panel-retry,
        .panel-profiling,
        .panel-events,
        .panel-activity {
          grid-column: span 12;
        }
      }
      @media (max-width: 640px) {
        .dashboard-shell {
          padding-left: 10px;
          padding-right: 10px;
        }
        .kpi-grid {
          grid-template-columns: 1fr;
        }
        .top-nav {
          width: 100%;
        }
        .controls {
          width: 100%;
          justify-content: flex-start;
        }
        .workspace-controls {
          justify-content: flex-start;
        }
        .status-chip {
          font-size: 0.78rem;
        }
        .tool-row {
          grid-template-columns: 1fr;
        }
        .trace-panel {
          width: 100vw;
        }
        h1 {
          font-size: 1.45rem;
        }
      }
      @keyframes fade-in {
        from {
          opacity: 0;
          transform: translateY(4px);
        }
        to {
          opacity: 1;
          transform: translateY(0);
        }
      }
    </style>
  </head>
  <body>
    <div class="dashboard-shell">
      <div class="topbar">
        <div class="title-wrap">
          <h1>Molt Symphony Control</h1>
          <div class="subtitle">
            Realtime orchestration command center with resilient stream telemetry.
          </div>
        </div>
        <div class="controls">
          <span id="stream-chip" class="status-chip">Connecting...</span>
          <span id="updated-chip" class="status-chip">No updates yet</span>
          <label class="control-group" for="workspace-layout">
            <span>layout</span>
            <select id="workspace-layout" aria-label="layout">
              <option value="columns">columns</option>
              <option value="rows">rows</option>
              <option value="grid" selected>grid</option>
            </select>
          </label>
          <label class="control-group" for="workspace-verbosity">
            <span>verbosity</span>
            <select id="workspace-verbosity" aria-label="verbosity">
              <option value="compact">compact</option>
              <option value="normal" selected>normal</option>
              <option value="verbose">verbose</option>
            </select>
          </label>
          <button id="refresh-btn" type="button">Run Refresh Cycle</button>
          <button id="reload-btn" type="button" class="secondary">Reconnect Stream</button>
        </div>
      </div>

      <div class="top-nav" role="tablist" aria-label="dashboard views">
        <button type="button" class="view-tab active" data-view="overview">Overview</button>
        <button type="button" class="view-tab" data-view="interventions">Interventions</button>
        <button type="button" class="view-tab" data-view="agents">Agents</button>
        <button type="button" class="view-tab" data-view="performance">Performance</button>
        <button type="button" class="view-tab" data-view="all">All Panels</button>
      </div>

      <div class="panel kpi-band" data-views="overview performance all">
        <div class="section-head">
          <h2>Health & Throughput KPIs</h2>
          <div class="hint">System posture and output velocity at a glance</div>
        </div>
        <div class="kpi-grid">
          <div class="kpi">
            <div class="label">System Health</div>
            <div id="kpi-health" class="value">Stable</div>
            <div id="kpi-health-meta" class="meta">No blocking alerts</div>
            <div class="meter"><div id="meter-health" class="meter-fill"></div></div>
          </div>
          <div class="kpi">
            <div class="label">Running Sessions</div>
            <div id="kpi-running" class="value">0</div>
            <div class="meta">Live issue execution</div>
            <div class="meter"><div id="meter-running" class="meter-fill"></div></div>
          </div>
          <div class="kpi">
            <div class="label">Retry Queue</div>
            <div id="kpi-retrying" class="value">0</div>
            <div class="meta">Items awaiting replay</div>
            <div class="meter"><div id="meter-retrying" class="meter-fill"></div></div>
          </div>
          <div class="kpi">
            <div class="label">Token Throughput</div>
            <div id="kpi-throughput" class="value">0 / sec</div>
            <div id="kpi-throughput-meta" class="meta">0 total tokens</div>
            <div class="meter"><div id="meter-throughput" class="meter-fill"></div></div>
          </div>
          <div class="kpi">
            <div class="label">Completed Turns</div>
            <div id="kpi-progress" class="value">0</div>
            <div id="kpi-progress-meta" class="meta">0 completed issues</div>
            <div class="meter"><div id="meter-progress" class="meter-fill"></div></div>
          </div>
          <div class="kpi">
            <div class="label">Runtime Duration</div>
            <div id="kpi-runtime" class="value">0s</div>
            <div class="meta">Codex execution window</div>
            <div class="meter"><div id="meter-runtime" class="meter-fill"></div></div>
          </div>
        </div>
      </div>

      <div class="dashboard-grid">
        <div class="panel panel-queue" data-views="interventions overview all">
          <div class="section-head">
            <h2>Human Action Queue</h2>
            <div class="hint">Needs triage and direct intervention</div>
          </div>
          <div id="attention-summary" class="attention-summary"></div>
          <div id="attention-list" class="attention-list"></div>
        </div>
        <div class="panel panel-tools" data-views="interventions all">
          <div class="section-head">
            <h2>Tool Launcher</h2>
            <div class="hint">Run direct intervention workflows</div>
          </div>
          <div class="tool-launcher">
            <div class="tool-row">
              <select id="tool-select" aria-label="tool-select">
                <option value="refresh_cycle">refresh_cycle</option>
                <option value="retry_now">retry_now</option>
                <option value="stop_worker">stop_worker</option>
                <option value="inspect_issue">inspect_issue</option>
                <option value="set_max_concurrent_agents">set_max_concurrent_agents</option>
              </select>
              <input
                id="tool-issue"
                type="text"
                class="mono"
                placeholder="Issue identifier (e.g. MOL-42)"
                aria-label="tool-issue-identifier"
              />
              <button id="tool-run-btn" type="button" class="secondary">Run Tool</button>
            </div>
            <div class="hint">
              `set_max_concurrent_agents` expects a numeric value (for example: `2`).
            </div>
            <pre id="tool-result" class="tool-result mono">No tool run yet.</pre>
          </div>
        </div>
        <div class="panel panel-rate" data-views="overview performance all">
          <div class="section-head">
            <h2>Rate Limits</h2>
            <div class="hint">Provider budget envelope</div>
          </div>
          <div id="rate-wrap"></div>
        </div>

        <div class="panel panel-running" data-views="overview agents all">
          <div class="section-head">
            <h2>Active Sessions</h2>
            <div class="hint">Running issue workers and live state</div>
          </div>
          <div id="running-wrap"></div>
        </div>
        <div class="panel panel-retry" data-views="interventions overview all">
          <div class="section-head">
            <h2>Retry Queue</h2>
            <div class="hint">Deferred retries and next attempt windows</div>
          </div>
          <div id="retry-wrap"></div>
        </div>

        <div class="panel panel-profiling" data-views="performance all">
          <div class="section-head">
            <h2>Profiling & Hotspots</h2>
            <div class="hint">Latency concentration and execution heat</div>
          </div>
          <div id="profiling-wrap"></div>
        </div>
        <div class="panel panel-events" data-views="interventions performance all">
          <div class="section-head">
            <h2>Recent Events</h2>
            <div class="hint">Most recent orchestration timeline</div>
          </div>
          <div id="events" class="events"></div>
        </div>
        <div class="panel panel-activity" data-views="interventions all">
          <div class="section-head">
            <h2>Intervention Activity</h2>
            <div class="hint">Outcome trail for manual actions</div>
          </div>
          <div id="intervention-activity" class="activity-list"></div>
        </div>
        <div class="panel panel-workspace" data-views="agents interventions all">
          <div class="section-head">
            <h2>Agent Telemetry Workspace</h2>
            <div class="workspace-controls">
              <span class="hint">per-agent panes</span>
            </div>
          </div>
          <div id="workspace-meta" class="workspace-meta">No agent panes loaded yet.</div>
          <div
            id="agent-workspace"
            class="agent-workspace"
            data-layout="grid"
            data-verbosity="normal"
          ></div>
        </div>
      </div>

      <div id="trace-modal" class="trace-modal" aria-hidden="true">
        <aside class="trace-panel" role="dialog" aria-label="Agent Trace">
          <div class="trace-head">
            <div>
              <h3 class="trace-title">Agent Trace</h3>
              <div id="trace-subtitle" class="trace-sub">Select an agent to inspect.</div>
            </div>
            <div class="trace-controls">
              <button id="trace-refresh-btn" type="button" class="secondary">Refresh</button>
              <button id="trace-close-btn" type="button" class="secondary">Close</button>
            </div>
          </div>
          <div id="trace-summary" class="trace-summary">No trace selected.</div>
          <div class="trace-stream">
            <div id="trace-events" class="trace-events">
              <div class="empty">Click an issue/agent pill to open live trace.</div>
            </div>
          </div>
        </aside>
      </div>

      <div class="footer">
        Endpoints: <span class="mono">/api/v1/state</span>,
        <span class="mono">/api/v1/stream</span>,
        <span class="mono">/api/v1/refresh</span>,
        <span class="mono">/api/v1/interventions/retry-now</span>,
        <span class="mono">/api/v1/tools/run</span>,
        <span class="mono">/api/v1/&lt;issue_identifier&gt;</span>
      </div>
    </div>

    <script>
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
      const traceSubtitle = document.getElementById("trace-subtitle");
      const traceSummary = document.getElementById("trace-summary");
      const traceEvents = document.getElementById("trace-events");
      const traceRefreshButton = document.getElementById("trace-refresh-btn");
      const traceCloseButton = document.getElementById("trace-close-btn");
      const runningWrap = document.getElementById("running-wrap");
      const retryWrap = document.getElementById("retry-wrap");
      const profilingWrap = document.getElementById("profiling-wrap");
      const eventsWrap = document.getElementById("events");
      const interventionActivity = document.getElementById("intervention-activity");
      const rateWrap = document.getElementById("rate-wrap");
      const workspaceWrap = document.getElementById("agent-workspace");
      const workspaceMeta = document.getElementById("workspace-meta");
      const layoutSelect = document.getElementById("workspace-layout");
      const verbositySelect = document.getElementById("workspace-verbosity");
      const viewTabs = Array.from(document.querySelectorAll(".view-tab"));
      const viewPanels = Array.from(document.querySelectorAll("[data-views]"));
      const LAYOUT_CHOICES = ["columns", "rows", "grid"];
      const VERBOSITY_CHOICES = ["compact", "normal", "verbose"];
      const VIEW_CHOICES = ["overview", "interventions", "agents", "performance", "all"];
      const STORAGE_KEY = "molt.symphony.dashboard.workspace.v1";
      const VIEW_STORAGE_KEY = "molt.symphony.dashboard.view.v1";
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
      let streamIntervalMs = 1000;
      let fallbackPollIntervalMs = 2500;
      let staleAfterMs = 7000;
      let pendingRenderState = null;
      let renderFrame = 0;
      const panelSignatures = new Map();

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

      function formatEpochSeconds(value) {
        const num = toNumber(value, NaN);
        if (!Number.isFinite(num) || num <= 0) return "n/a";
        const date = new Date(num * 1000);
        if (Number.isNaN(date.getTime())) return "n/a";
        return date.toLocaleString();
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
            allowed.includes("all") ||
            allowed.includes(activeDashboardView);
          panel.classList.toggle("hidden-view", !visible);
          panel.style.display = visible ? "" : "none";
        });
        viewTabs.forEach((tab) => {
          const tabView = normalizeDashboardView(tab.dataset.view || "overview");
          tab.classList.toggle("active", tabView === activeDashboardView);
          tab.setAttribute("aria-selected", tabView === activeDashboardView ? "true" : "false");
        });
      }

      function setDashboardView(view, persist = true) {
        activeDashboardView = normalizeDashboardView(view);
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
                data-agent-issue="${escapeHtml(pane.issueId || "")}"
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
        streamChip.textContent = message;
        streamChip.className = "status-chip";
        if (mode === "live") streamChip.classList.add("live");
        if (mode === "warn") streamChip.classList.add("warn");
      }

      function stopPolling() {
        if (pollTimer) {
          clearInterval(pollTimer);
          pollTimer = null;
        }
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
        if (stream) {
          connectStream(false);
        } else if (pollTimer) {
          startPollingFallback(true);
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
              : `<a href="/api/v1/${encodeURIComponent(issueId)}" target="_blank" rel="noreferrer">Inspect JSON</a>`;
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
        const dueIn = suspension.due_in_seconds != null ? `${Math.round(toNumber(suspension.due_in_seconds, 0))}s` : "n/a";
        const primaryResetAt = primary.resetsAt || primary.resetAt || null;
        const secondaryResetAt = secondary.resetsAt || secondary.resetAt || null;

        let headline = "No provider throttle detected.";
        let detail =
          "Symphony is running in normal cadence. Polling and stream intervals are set for responsiveness.";
        let level = "ok";

        if (creditExhausted) {
          headline = "Provider credits are exhausted (not a local concurrency cap).";
          detail =
            `Codex reports \`credits.hasCredits=false\`, so work is paused until credits return. ` +
            `Auto-resume in about ${dueIn}.`;
          level = "danger";
        } else if (windowSaturated || suspended) {
          headline = "Provider rate-limit window is active.";
          detail =
            `Symphony is in gentle mode and will auto-resume in about ${dueIn}. ` +
            `Primary reset: ${formatEpochSeconds(primaryResetAt)} · Secondary reset: ${formatEpochSeconds(
              secondaryResetAt
            )}.`;
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
        const limits = state.rate_limits;
        const runtime = toObject(state.runtime);
        const status = describeRateLimitState(state);
        const cadence = status.cadence;
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
          </div>
        `;
        if (!limits) {
          rateWrap.innerHTML =
            summaryHtml + '<div class="empty">No rate limit payload received yet.</div>';
          return;
        }
        if (Array.isArray(limits)) {
          rateWrap.innerHTML = `${summaryHtml}<pre class="mono" style="margin:0;white-space:pre-wrap;">${escapeHtml(
            JSON.stringify(limits, null, 2)
          )}</pre>`;
          return;
        }
        if (typeof limits !== "object") {
          rateWrap.innerHTML = `${summaryHtml}<div class="empty mono">${escapeHtml(
            limits
          )}</div>`;
          return;
        }
        const entries = Object.entries(toObject(limits));
        if (!entries.length) {
          rateWrap.innerHTML = `${summaryHtml}<div class="empty">Rate limit payload is empty.</div>`;
          return;
        }
        rateWrap.innerHTML = `${summaryHtml}<div class="kv-list">${entries
          .slice(0, 16)
          .map(
            ([key, value]) => `
              <div class="kv-item">
                <div class="key mono">${escapeHtml(key)}</div>
                <div class="value mono">${escapeHtml(
                  typeof value === "object" ? JSON.stringify(value) : String(value)
                )}</div>
              </div>
            `
          )
          .join("")}</div>`;
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
        applyDashboardView();
        updatePanel(
          "kpis",
          {
            counts: safeState.counts,
            codex_totals: safeState.codex_totals,
            attention_count: toArray(safeState.attention).length,
            retry_count: toArray(safeState.retrying).length,
            generated_at: generatedAt,
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
          { running: safeState.running, generated_at: generatedAt },
          () => renderRunning(safeState)
        );
        updatePanel(
          "retry",
          { retrying: safeState.retrying, generated_at: generatedAt },
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
          "events",
          { recent_events: safeState.recent_events, generated_at: generatedAt },
          () => renderEvents(safeState)
        );
        updatePanel(
          "intervention_activity",
          { manual_actions: safeState.manual_actions, generated_at: generatedAt },
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
        const updatedText = `Updated ${formatTime(safeState.generated_at)}`;
        if (updatedChip.textContent !== updatedText) {
          updatedChip.textContent = updatedText;
        }
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
          renderFrame = 0;
          const nextState = pendingRenderState;
          pendingRenderState = null;
          if (!nextState) return;
          commitRenderState(nextState);
        });
      }

      async function fetchState() {
        const resp = await fetch("/api/v1/state");
        if (!resp.ok) throw new Error(`state_fetch_failed:${resp.status}`);
        const data = await resp.json();
        renderState(data);
      }

      async function triggerRefresh() {
        refreshBtn.disabled = true;
        try {
          const resp = await fetch("/api/v1/refresh", { method: "POST" });
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
          const resp = await fetch("/api/v1/interventions/retry-now", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ issue_identifier: issueIdentifier }),
          });
          const payload = await resp.json();
          if (!resp.ok || payload.ok === false) {
            localActionStatus.set(issueIdentifier, {
              status: payload.status || "failed",
              message: payload.message || `retry_now_failed:${resp.status}`,
              at: new Date().toISOString(),
            });
            renderState(latestState);
            throw new Error(`retry_now_failed:${resp.status}`);
          }
          localActionStatus.set(issueIdentifier, {
            status: payload.status || "queued",
            message: payload.message || "Retry request accepted.",
            at: new Date().toISOString(),
          });
          setStreamStatus("Retry request queued", "live");
          await fetchState();
        } catch (_err) {
          setStreamStatus("Retry-now action failed", "warn");
        } finally {
          pendingRetries.delete(issueIdentifier);
          renderState(latestState);
        }
      }

      async function runDashboardTool() {
        const tool = String(toolSelect?.value || "").trim();
        const issueIdentifier = String(toolIssueInput?.value || "").trim();
        if (!tool) return;
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
          const resp = await fetch("/api/v1/tools/run", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
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
          clearInterval(tracePollTimer);
          tracePollTimer = null;
        }
      }

      function closeTraceModal() {
        stopTracePolling();
        traceIssueIdentifier = "";
        traceModal.classList.remove("open");
        traceModal.setAttribute("aria-hidden", "true");
        traceSubtitle.textContent = "Select an agent to inspect.";
        traceSummary.innerHTML = '<div class="empty">No trace selected.</div>';
        traceEvents.innerHTML =
          '<div class="empty">Click an issue/agent pill to open live trace.</div>';
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
        const expectedIssue = traceIssueIdentifier;
        const serial = ++traceFetchSerial;
        try {
          const resp = await fetch(`/api/v1/${encodeURIComponent(expectedIssue)}`);
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
        }
      }

      function openTraceModal(issueIdentifier) {
        const issue = String(issueIdentifier || "").trim();
        if (!issue) return;
        traceIssueIdentifier = issue;
        traceModal.classList.add("open");
        traceModal.setAttribute("aria-hidden", "false");
        traceSubtitle.textContent = `${issue} · loading`;
        traceSummary.innerHTML = '<div class="empty">Loading trace summary...</div>';
        traceEvents.innerHTML = '<div class="empty">Loading live trace...</div>';
        fetchTraceIssue();
        stopTracePolling();
        tracePollTimer = setInterval(fetchTraceIssue, 1200);
      }

      function startPollingFallback(forceRestart = false) {
        if (pollTimer && !forceRestart) {
          return;
        }
        if (pollTimer) {
          stopPolling();
        }
        setStreamStatus(`Polling fallback (${fallbackPollIntervalMs}ms)`, "warn");
        const run = async () => {
          try {
            await fetchState();
          } catch (_err) {
            // keep polling
          }
        };
        pollTimer = setInterval(run, fallbackPollIntervalMs);
        run();
      }

      function scheduleReconnect() {
        if (reconnectTimer) return;
        const delayMs = Math.min(12000, 1000 * Math.pow(2, Math.min(reconnectAttempts, 4)));
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          reconnectAttempts += 1;
          connectStream();
        }, delayMs);
      }

      function connectStream(manual = false) {
        stopReconnectTimer();
        stopStream();
        lastFrameAt = 0;
        if (!window.EventSource || typeof EventSource !== "function") {
          startPollingFallback();
          return;
        }
        if (manual) {
          reconnectAttempts = 0;
        }
        setStreamStatus(`Connecting live stream (${streamIntervalMs}ms)...`);
        ensureWatchdog();
        stream = new EventSource(`/api/v1/stream?interval_ms=${streamIntervalMs}`);
        stream.onopen = () => {
          reconnectAttempts = 0;
          lastFrameAt = Date.now();
          stopPolling();
          setStreamStatus(`Live stream (${streamIntervalMs}ms)`, "live");
        };
        stream.onerror = () => {
          setStreamStatus("Stream reconnecting...", "warn");
          startPollingFallback();
          scheduleReconnect();
        };
        stream.addEventListener("state", (event) => {
          try {
            lastFrameAt = Date.now();
            renderState(JSON.parse(event.data));
            stopPolling();
            setStreamStatus(`Live stream (${streamIntervalMs}ms)`, "live");
          } catch (_err) {
            // ignore malformed frames
          }
        });
      }

      loadWorkspacePrefs();
      loadDashboardView();
      setDashboardView(activeDashboardView, false);
      layoutSelect.value = workspaceLayout;
      verbositySelect.value = workspaceVerbosity;
      workspaceWrap.addEventListener("dragstart", handlePaneDragStart);
      workspaceWrap.addEventListener("dragover", handlePaneDragOver);
      workspaceWrap.addEventListener("drop", handlePaneDrop);
      workspaceWrap.addEventListener("dragend", handlePaneDragEnd);
      layoutSelect.addEventListener("change", () => {
        workspaceLayout = normalizeLayout(layoutSelect.value);
        layoutSelect.value = workspaceLayout;
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
      reloadBtn.addEventListener("click", () => connectStream(true));
      document.querySelector(".top-nav")?.addEventListener("click", (event) => {
        const target = event.target;
        if (!(target instanceof Element)) return;
        const tab = target.closest(".view-tab");
        if (!(tab instanceof HTMLButtonElement)) return;
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
        const agentRef = target.closest("[data-agent-issue]");
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
        if (event.key === "Escape" && traceModal.classList.contains("open")) {
          closeTraceModal();
        }
      });
      toolRunButton?.addEventListener("click", () => {
        runDashboardTool();
      });
      toolSelect?.addEventListener("change", () => {
        const selected = String(toolSelect?.value || "").trim();
        if (!toolIssueInput) return;
        toolIssueInput.placeholder =
          selected === "set_max_concurrent_agents"
            ? "Max concurrent agents (e.g. 2)"
            : "Issue identifier (e.g. MOL-42)";
      });
      connectStream(false);
      fetchState().catch(() => startPollingFallback());
    </script>
  </body>
</html>
"""
