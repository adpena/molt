from __future__ import annotations

import http.client
import json
import sys
import urllib.error
import urllib.request
from typing import Any

import pytest

from molt.symphony.http_server import DashboardServer, _state_hasher_from_env


class _Provider:
    def __init__(self) -> None:
        self.refresh_calls = 0
        self.retry_now_calls: list[str] = []
        self.state_calls = 0
        self.state_version = 0

    def snapshot_state(self) -> dict[str, Any]:
        self.state_calls += 1
        return {
            "generated_at": f"2026-03-04T00:00:{self.state_version:02d}Z",
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
        self.state_version += 1
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


def _read_json_request(req: urllib.request.Request) -> tuple[int, dict[str, Any]]:
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


def test_post_dashboard_css_returns_method_not_allowed_json() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/dashboard.css", method="POST", data=b""
        )
        with urllib.request.urlopen(req, timeout=5.0):  # pragma: no cover
            raise AssertionError("expected HTTPError")
    except urllib.error.HTTPError as exc:
        payload = json.loads(exc.read().decode("utf-8"))
        assert exc.code == 405
        assert payload["error"]["code"] == "method_not_allowed"
    finally:
        server.stop()


def test_put_state_route_returns_method_not_allowed_json() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state", method="PUT", data=b"{}"
        )
        with urllib.request.urlopen(req, timeout=5.0):  # pragma: no cover
            raise AssertionError("expected HTTPError")
    except urllib.error.HTTPError as exc:
        payload = json.loads(exc.read().decode("utf-8"))
        assert exc.code == 405
        assert payload["error"]["code"] == "method_not_allowed"
    finally:
        server.stop()


def test_delete_unknown_route_returns_not_found_json() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/definitely-not-a-route", method="DELETE"
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
        assert '<link rel="stylesheet" href="/dashboard.css" />' in body
        assert '<script src="/dashboard.js"></script>' in body
        assert "Health & Throughput KPIs" in body
        assert "Human Action Queue" in body
        assert "Tool Launcher" in body
        assert "Intervention Activity" in body
        assert "Agent Trace" in body
        assert "trace-modal" in body
        assert "Profiling & Hotspots" in body
        assert "Security Telemetry" in body
        assert "Agent Telemetry Workspace" in body
        assert "verbosity" in body
        assert "Interventions" in body
        assert "Durable Memory" in body
        assert "view-tab" in body
        assert "set_max_concurrent_agents" in body
        assert "durable_backup" in body
        assert "durable_integrity_check" in body
        assert "/api/v1/durable" in body
        assert "/api/v1/stream" in body
    finally:
        server.stop()


def test_dashboard_static_assets_are_served() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/dashboard.css", timeout=5.0
        ) as resp:
            css = resp.read().decode("utf-8")
            assert int(resp.status) == 200
            assert "text/css" in (resp.headers.get("Content-Type") or "")
            assert (
                resp.headers.get("Cache-Control") or ""
            ) == "public, max-age=300, immutable"
            assert (resp.headers.get("Referrer-Policy") or "") == "no-referrer"
            assert (
                resp.headers.get("Permissions-Policy") or ""
            ) == "interest-cohort=()"
            assert (
                resp.headers.get("Cross-Origin-Opener-Policy") or ""
            ) == "same-origin"
            assert (
                resp.headers.get("Cross-Origin-Resource-Policy") or ""
            ) == "same-origin"
            css_etag = resp.headers.get("ETag")
            assert css_etag
            assert ":root {" in css
            assert ".dashboard-shell" in css
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/dashboard.css",
            method="GET",
            headers={"If-None-Match": str(css_etag)},
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 304
        assert (exc_info.value.headers.get("ETag") or "") == str(css_etag)
        assert (exc_info.value.headers.get("Referrer-Policy") or "") == "no-referrer"
        assert (
            exc_info.value.headers.get("Permissions-Policy") or ""
        ) == "interest-cohort=()"
        assert exc_info.value.read() == b""
        with urllib.request.urlopen(
            f"http://127.0.0.1:{port}/dashboard.js", timeout=5.0
        ) as resp:
            js = resp.read().decode("utf-8")
            assert int(resp.status) == 200
            assert "application/javascript" in (resp.headers.get("Content-Type") or "")
            assert (
                resp.headers.get("Cache-Control") or ""
            ) == "public, max-age=300, immutable"
            assert (resp.headers.get("Referrer-Policy") or "") == "no-referrer"
            assert (
                resp.headers.get("Permissions-Policy") or ""
            ) == "interest-cohort=()"
            js_etag = resp.headers.get("ETag")
            assert js_etag
            assert "EventSource" in js
            assert "fetchState" in js
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/dashboard.js",
            method="GET",
            headers={"If-None-Match": str(js_etag)},
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 304
        assert (exc_info.value.headers.get("ETag") or "") == str(js_etag)
        assert (exc_info.value.headers.get("Referrer-Policy") or "") == "no-referrer"
        assert (
            exc_info.value.headers.get("Permissions-Policy") or ""
        ) == "interest-cohort=()"
        assert exc_info.value.read() == b""
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


def test_retry_now_endpoint_accepts_form_encoded_payload() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/interventions/retry-now",
            method="POST",
            data=b"issue_identifier=MOL-89",
            headers={"Content-Type": "application/x-www-form-urlencoded"},
        )
        with urllib.request.urlopen(req, timeout=5.0) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
            assert int(resp.status) == 202
        assert payload["ok"] is True
        assert payload["issue_identifier"] == "MOL-89"
        assert provider.retry_now_calls == ["MOL-89"]
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
        assert (resp.getheader("Referrer-Policy") or "") == "no-referrer"
        assert (resp.getheader("Permissions-Policy") or "") == "interest-cohort=()"
        assert resp.read() == b""
        conn.close()
    finally:
        server.stop()


def test_state_endpoint_uses_shared_snapshot_cache_for_burst_reads() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        _ = resp.read()
        conn.close()

        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        _ = resp.read()
        conn.close()
        assert provider.state_calls == 1
    finally:
        server.stop()


def test_refresh_invalidates_state_snapshot_cache() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        first_payload = json.loads(resp.read().decode("utf-8"))
        conn.close()

        status, _payload = _read_json(
            f"http://127.0.0.1:{port}/api/v1/refresh", method="POST"
        )
        assert status == 202

        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        second_payload = json.loads(resp.read().decode("utf-8"))
        conn.close()

        assert first_payload["generated_at"] != second_payload["generated_at"]
        assert provider.state_calls == 2
    finally:
        server.stop()


def test_state_endpoint_supports_external_hasher_helper(monkeypatch: object) -> None:
    provider = _Provider()
    helper_cmd = f"{sys.executable} tools/symphony_state_hasher.py"
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER", helper_cmd)
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        etag = resp.getheader("ETag") or ""
        assert etag.startswith('W/"')
        _ = resp.read()
        conn.close()
    finally:
        server.stop()


def test_state_hasher_env_prefers_frame_with_text_fallback(monkeypatch: object) -> None:
    helper_cmd = f"{sys.executable} tools/symphony_state_hasher.py"
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER", helper_cmd)
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER_PREFER_FRAME", "1")
    hasher = _state_hasher_from_env()
    assert hasher is not None
    assert "--stdio-frame" in hasher.command
    assert hasher.frame_mode is True
    assert hasher.fallback_command is not None
    assert "--stdio" in hasher.fallback_command


def test_state_hasher_env_can_force_text_mode(monkeypatch: object) -> None:
    helper_cmd = f"{sys.executable} tools/symphony_state_hasher.py"
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER", helper_cmd)
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER_PREFER_FRAME", "0")
    hasher = _state_hasher_from_env()
    assert hasher is not None
    assert "--stdio" in hasher.command
    assert "--stdio-frame" not in hasher.command
    assert hasher.frame_mode is False


def test_state_endpoint_falls_back_when_helper_is_invalid(monkeypatch: object) -> None:
    provider = _Provider()
    monkeypatch.setenv("MOLT_SYMPHONY_STATE_HASH_HELPER", "nonexistent-helper-cmd")
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        etag = resp.getheader("ETag") or ""
        assert etag.startswith('W/"')
        _ = resp.read()
        conn.close()
    finally:
        server.stop()


def test_security_headers_present_on_dashboard_and_state() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/")
        resp = conn.getresponse()
        assert resp.status == 200
        assert resp.getheader("X-Content-Type-Options") == "nosniff"
        assert resp.getheader("X-Frame-Options") == "DENY"
        assert resp.getheader("Referrer-Policy") == "no-referrer"
        assert resp.getheader("Cross-Origin-Opener-Policy") == "same-origin"
        assert resp.getheader("Cross-Origin-Resource-Policy") == "same-origin"
        assert "frame-ancestors 'none'" in (
            resp.getheader("Content-Security-Policy") or ""
        )
        _ = resp.read()
        conn.close()

        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5.0)
        conn.request("GET", "/api/v1/state")
        resp = conn.getresponse()
        assert resp.status == 200
        assert resp.getheader("X-Content-Type-Options") == "nosniff"
        assert resp.getheader("X-Frame-Options") == "DENY"
        assert resp.getheader("Cross-Origin-Opener-Policy") == "same-origin"
        assert resp.getheader("Cross-Origin-Resource-Policy") == "same-origin"
        _ = resp.read()
        conn.close()
    finally:
        server.stop()


def test_api_token_required_for_state_when_configured(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_API_TOKEN", "secret-token")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state", method="GET"
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 401

        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state",
            method="GET",
            headers={"Authorization": "Bearer secret-token"},
        )
        status, payload = _read_json_request(req)
        assert status == 200
        assert payload["counts"]["running"] == 0
    finally:
        server.stop()


def test_mutating_requests_require_csrf_for_browser_origin(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_API_TOKEN", "secret-token")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    origin = f"http://127.0.0.1:{port}"
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/refresh",
            method="POST",
            headers={
                "Authorization": "Bearer secret-token",
                "Origin": origin,
            },
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 403

        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/refresh",
            method="POST",
            headers={
                "Authorization": "Bearer secret-token",
                "Origin": origin,
                "X-Symphony-CSRF": "1",
            },
        )
        status, payload = _read_json_request(req)
        assert status == 202
        assert payload["queued"] is True
    finally:
        server.stop()


def test_state_includes_http_security_telemetry() -> None:
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, payload = _read_json(f"http://127.0.0.1:{port}/api/v1/state")
        assert status == 200
        runtime = payload["runtime"]
        telemetry = runtime["http_security"]
        assert telemetry["bind_host"] == "127.0.0.1"
        assert telemetry["dashboard_enabled"] is True
        assert telemetry["counters"]["unauthorized"] >= 0
    finally:
        server.stop()


def test_unauthorized_counter_increments(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_API_TOKEN", "secret-token")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state", method="GET"
        )
        with pytest.raises(urllib.error.HTTPError):
            urllib.request.urlopen(req, timeout=5.0)

        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state",
            method="GET",
            headers={"Authorization": "Bearer secret-token"},
        )
        status, payload = _read_json_request(req)
        assert status == 200
        assert payload["runtime"]["http_security"]["counters"]["unauthorized"] >= 1
    finally:
        server.stop()


def test_production_profile_requires_api_token(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_SECURITY_PROFILE", "production")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    with pytest.raises(RuntimeError):
        server.start()


def test_nonlocal_bind_requires_opt_in(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_BIND_HOST", "0.0.0.0")
    monkeypatch.delenv("MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND", raising=False)
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    with pytest.raises(RuntimeError):
        server.start()


def test_dashboard_disabled_in_production(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_SECURITY_PROFILE", "production")
    monkeypatch.setenv("MOLT_SYMPHONY_API_TOKEN", "secret-token")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/",
            method="GET",
            headers={"Authorization": "Bearer secret-token"},
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 404
    finally:
        server.stop()


def test_query_token_can_be_disabled(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_API_TOKEN", "secret-token")
    monkeypatch.setenv("MOLT_SYMPHONY_ALLOW_QUERY_TOKEN", "0")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state?token=secret-token",
            method="GET",
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 401
    finally:
        server.stop()


def test_state_rate_limit_returns_retry_after_and_updates_counter(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS", "1")
    monkeypatch.setenv("MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS", "60")
    provider = _Provider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, payload = _read_json(f"http://127.0.0.1:{port}/api/v1/state")
        assert status == 200
        assert payload["counts"]["running"] == 0

        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state",
            method="GET",
        )
        with pytest.raises(urllib.error.HTTPError) as exc_info:
            urllib.request.urlopen(req, timeout=5.0)
        assert exc_info.value.code == 429
        retry_after = exc_info.value.headers.get("Retry-After")
        assert retry_after is not None
        assert int(retry_after) >= 1
        rate_payload = json.loads(exc_info.value.read().decode("utf-8"))
        assert rate_payload["error"]["code"] == "rate_limited"

        # Use a distinct principal to fetch state and confirm the rate-limit counter was recorded.
        req = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/v1/state",
            method="GET",
            headers={"X-Symphony-Token": "alternate-principal"},
        )
        status, payload = _read_json_request(req)
        assert status == 200
        counters = payload["runtime"]["http_security"]["counters"]
        assert counters["rate_limited"] >= 1
    finally:
        server.stop()


def test_state_redacts_sensitive_values() -> None:
    linear_token = "lin_api_" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    openai_token = "sk-" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    github_token = "ghp_" + "ABCDEFGHIJKLMNOPQRSTUV12345"
    slack_token = "xoxb-" + "ABCDEFGHIJKLMNOPQRSTUV12345"

    class _SensitiveProvider(_Provider):
        def snapshot_state(self) -> dict[str, Any]:
            payload = super().snapshot_state()
            payload["runtime"] = {
                "api_token": linear_token,
                "nested": {
                    "authorization": openai_token,
                    "message": github_token,
                },
            }
            payload["rate_limits"] = {"secret": slack_token}
            return payload

    provider = _SensitiveProvider()
    server = DashboardServer(provider=provider, port=0)
    port = server.start()
    try:
        status, payload = _read_json(f"http://127.0.0.1:{port}/api/v1/state")
        assert status == 200
        encoded = json.dumps(payload, sort_keys=True)
        assert linear_token not in encoded
        assert openai_token not in encoded
        assert github_token not in encoded
        assert slack_token not in encoded
        assert "<redacted>" in encoded
    finally:
        server.stop()
