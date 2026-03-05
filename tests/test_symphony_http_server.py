from __future__ import annotations

import http.client
import json
import urllib.error
import urllib.request
from typing import Any

from molt.symphony.http_server import DashboardServer


class _Provider:
    def __init__(self) -> None:
        self.refresh_calls = 0
        self.retry_now_calls: list[str] = []

    def snapshot_state(self) -> dict[str, Any]:
        return {
            "generated_at": "2026-03-04T00:00:00Z",
            "counts": {"running": 0, "retrying": 0},
            "running": [],
            "retrying": [],
            "codex_totals": {
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0,
                "seconds_running": 0.0,
            },
            "rate_limits": None,
        }

    def snapshot_durable_memory(self, limit: int = 120) -> dict[str, Any]:
        return {
            "enabled": True,
            "root": "/Volumes/APDataStore/Molt/logs/symphony/durable_memory",
            "queue_depth": 0,
            "dropped_rows": 0,
            "last_sync_utc": "2026-03-04T00:00:00Z",
            "files": {
                "jsonl": {
                    "exists": True,
                    "size_bytes": 2048,
                    "modified_at": "2026-03-04T00:00:00Z",
                },
                "duckdb": {
                    "exists": True,
                    "size_bytes": 4096,
                    "modified_at": "2026-03-04T00:00:00Z",
                },
                "parquet": {
                    "exists": True,
                    "size_bytes": 1024,
                    "modified_at": "2026-03-04T00:00:00Z",
                },
            },
            "recent_events": [
                {
                    "recorded_at": "2026-03-04T00:00:00Z",
                    "kind": "codex_event",
                    "issue_identifier": "MOL-1",
                    "message": "turn_completed",
                }
            ][: max(limit, 1)],
        }

    def snapshot_issue(self, issue_identifier: str) -> dict[str, Any] | None:
        if issue_identifier != "MOL-1":
            return None
        return {"issue_identifier": issue_identifier, "status": "running"}

    def request_refresh(self) -> bool:
        self.refresh_calls += 1
        return True

    def request_retry_now(self, issue_identifier: str) -> dict[str, Any]:
        issue = issue_identifier.strip()
        if not issue:
            return {"ok": False, "error": "missing_issue_identifier"}
        self.retry_now_calls.append(issue)
        return {"ok": True, "issue_identifier": issue, "queued": True}

    def run_dashboard_tool(
        self, tool_name: str, payload: dict[str, Any]
    ) -> dict[str, Any]:
        tool = tool_name.strip().lower()
        if not tool:
            return {"ok": False, "error": "missing_tool"}
        return {
            "ok": True,
            "tool": tool,
            "issue_identifier": payload.get("issue_identifier"),
        }


def _read_json(url: str, method: str = "GET") -> tuple[int, dict[str, Any]]:
    req = urllib.request.Request(url, method=method)
    with urllib.request.urlopen(req, timeout=5.0) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
        return int(resp.status), payload


def _read_text(url: str) -> tuple[int, str]:
    with urllib.request.urlopen(url, timeout=5.0) as resp:
        payload = resp.read().decode("utf-8")
        return int(resp.status), payload


def test_refresh_endpoint_includes_requested_at() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, payload = _read_json(
            f"http://127.0.0.1:{port}/api/v1/refresh", method="POST"
        )
        assert status == 202
        assert payload["queued"] is True
        assert payload["requested_at"].endswith("Z")
    finally:
        server.stop()


def test_unknown_post_route_returns_not_found() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/not-a-route", method="POST"
        )
        with urllib.request.urlopen(req, timeout=5.0):  # pragma: no cover
            raise AssertionError("expected HTTPError")
    except urllib.error.HTTPError as exc:
        payload = json.loads(exc.read().decode("utf-8"))
        assert exc.code == 404
        assert payload["error"]["code"] == "not_found"
    finally:
        server.stop()


def test_dashboard_contains_realtime_ui() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, body = _read_text(f"http://127.0.0.1:{port}/")
        assert status == 200
        assert "Molt Symphony Control" in body
        assert "Health & Throughput KPIs" in body
        assert "Human Action Queue" in body
        assert "Tool Launcher" in body
        assert "Intervention Activity" in body
        assert "Agent Trace" in body
        assert "trace-modal" in body
        assert "Profiling & Hotspots" in body
        assert "Agent Telemetry Workspace" in body
        assert "verbosity" in body
        assert "Interventions" in body
        assert "Durable Memory" in body
        assert "durable telemetry" in body
        assert "view-tab" in body
        assert "set_max_concurrent_agents" in body
        assert "/api/v1/durable" in body
        assert "/api/v1/stream" in body
        assert "EventSource" in body
    finally:
        server.stop()


def test_durable_endpoint_returns_payload() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, payload = _read_json(f"http://127.0.0.1:{port}/api/v1/durable?limit=25")
        assert status == 200
        assert payload["enabled"] is True
        assert payload["files"]["jsonl"]["exists"] is True
        assert payload["recent_events"][0]["issue_identifier"] == "MOL-1"
    finally:
        server.stop()


def test_retry_now_endpoint_queues_intervention() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/interventions/retry-now",
            method="POST",
            data=json.dumps({"issue_identifier": "MOL-88"}).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=5.0) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
            assert int(resp.status) == 202
        assert payload["ok"] is True
        assert provider.retry_now_calls == ["MOL-88"]
    finally:
        server.stop()


def test_dashboard_tool_endpoint_runs_tool() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/tools/run",
            method="POST",
            data=json.dumps(
                {"tool": "inspect_issue", "issue_identifier": "MOL-55"}
            ).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=5.0) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
            assert int(resp.status) == 202
        assert payload["ok"] is True
        assert payload["tool"] == "inspect_issue"
        assert payload["issue_identifier"] == "MOL-55"
    finally:
        server.stop()


def test_state_stream_emits_sse_payload() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/api/v1/stream?interval_ms=250", timeout=5.0
        ) as resp:
            assert int(resp.status) == 200
            assert "text/event-stream" in (resp.headers.get("Content-Type") or "")
            data_line = ""
            id_line = ""
            for _ in range(16):
                line = resp.readline().decode("utf-8").strip()
                if line.startswith("id: "):
                    id_line = line.removeprefix("id: ")
                if line.startswith("data: "):
                    data_line = line.removeprefix("data: ")
                    break
            assert id_line
            assert data_line
            payload = json.loads(data_line)
            assert payload["counts"]["running"] == 0
            assert payload["counts"]["retrying"] == 0
    finally:
        server.stop()


def test_state_endpoint_etag_supports_conditional_get() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        etag = resp.getheader("ETag") or ""
        body = resp.read()
        conn.close()
        assert etag
        assert body

        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state", headers={"If-None-Match": etag})
        resp = conn.getresponse()
        assert resp.status == 304
        assert (resp.getheader("ETag") or "") == etag
        assert resp.read() == b""
        conn.close()
    finally:
        server.stop()
