"""End-to-end tests for ``tools.cloudflare_demo_deploy_verify``.

Probes the live-deploy verifier against an in-process HTTP server that
mimics the Cloudflare demo Worker's success and failure shapes:

- 200 OK with valid sentinel
- NUL-prefixed body (WASM corruption)
- Cloudflare ``Error 1102`` body (CPU time exceeded)
- Wrong status code
- Wrong Content-Type
- Sentinel missing
- Transient failure followed by success on retry
- Hard failure after retries are exhausted

These exercises the full retry+report pipeline without making network
requests, so the cloudflare deploy lane has unit-grade coverage of the
post-deploy validation tool.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator

import pytest

import tools.cloudflare_demo_deploy_verify as deploy_verify


def _build_response_map() -> (
    dict[str, tuple[int, str, bytes]] | dict[str, tuple[int, str, bytes]]
):
    return {
        "/": (200, "text/html; charset=utf-8", b"<!DOCTYPE html><html></html>"),
        "/fib/10": (200, "text/plain; charset=utf-8", b"fib(10) = 55\n"),
        "/fib/nul": (200, "text/plain; charset=utf-8", b"\x00fib(10)=55\n"),
        "/fib/1102": (
            200,
            "text/plain; charset=utf-8",
            b"Worker error: Error 1102 - Worker exceeded CPU time limit\n",
        ),
        "/fib/wrong-status": (500, "text/plain; charset=utf-8", b"oops"),
        "/fib/wrong-ct": (200, "application/json", b"{\"n\": 55}"),
        "/fib/no-sentinel": (200, "text/plain; charset=utf-8", b"different output\n"),
    }


class _DeployVerifyHandler(BaseHTTPRequestHandler):
    response_map: dict[str, tuple[int, str, bytes]] = {}

    def do_GET(self) -> None:  # noqa: N802 - HTTP handler API
        status, content_type, body = self.response_map.get(
            self.path,
            (404, "text/plain; charset=utf-8", b"missing"),
        )
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


@pytest.fixture()
def live_server() -> Iterator[str]:
    handler_cls = type(
        "_BoundHandler",
        (_DeployVerifyHandler,),
        {"response_map": _build_response_map()},
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_cls)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    try:
        yield f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.server_close()


@pytest.fixture()
def no_retry_sleep(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(deploy_verify, "_RETRY_DELAY_SECONDS", 0)


def test_probe_once_passes_for_valid_endpoint(live_server: str) -> None:
    spec = deploy_verify.ProbeSpec("/fib/10", 200, "text/plain", "55")
    result = deploy_verify._probe_once(live_server, spec)
    assert result.passed
    assert result.failure_reason is None
    assert result.status_code == 200
    assert result.content_type is not None
    assert result.content_type.startswith("text/plain")
    assert result.body_snippet is not None
    assert "55" in result.body_snippet


def test_probe_once_rejects_nul_prefixed_body(live_server: str) -> None:
    spec = deploy_verify.ProbeSpec("/fib/nul", 200, "text/plain", "55")
    result = deploy_verify._probe_once(live_server, spec)
    assert not result.passed
    assert result.failure_reason is not None
    assert "NUL-prefixed" in result.failure_reason


def test_probe_once_rejects_cloudflare_error_1102(live_server: str) -> None:
    spec = deploy_verify.ProbeSpec("/fib/1102", 200, "text/plain", None)
    result = deploy_verify._probe_once(live_server, spec)
    assert not result.passed
    assert result.failure_reason is not None
    assert "Error 1102" in result.failure_reason


def test_probe_once_rejects_wrong_status(live_server: str) -> None:
    """A non-2xx response surfaces as an HTTPError-derived failure.

    ``urllib.request.urlopen`` raises ``HTTPError`` for any non-2xx status,
    so the probe records the status code and a ``HTTP error <code>`` reason
    rather than reaching the explicit ``Expected status`` branch. Either
    failure mode must be reported as a non-passing probe with the actual
    upstream status code preserved.
    """
    spec = deploy_verify.ProbeSpec("/fib/wrong-status", 200, "text/plain", None)
    result = deploy_verify._probe_once(live_server, spec)
    assert not result.passed
    assert result.status_code == 500
    assert result.failure_reason is not None
    assert "500" in result.failure_reason


def test_probe_once_rejects_wrong_content_type(live_server: str) -> None:
    spec = deploy_verify.ProbeSpec("/fib/wrong-ct", 200, "text/plain", None)
    result = deploy_verify._probe_once(live_server, spec)
    assert not result.passed
    assert result.failure_reason is not None
    assert "Content-Type" in result.failure_reason


def test_probe_once_rejects_missing_sentinel(live_server: str) -> None:
    spec = deploy_verify.ProbeSpec("/fib/no-sentinel", 200, "text/plain", "55")
    result = deploy_verify._probe_once(live_server, spec)
    assert not result.passed
    assert result.failure_reason is not None
    assert "Sentinel '55'" in result.failure_reason


def test_probe_with_retries_recovers_on_second_attempt(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(deploy_verify, "_RETRY_DELAY_SECONDS", 0)
    spec = deploy_verify.ProbeSpec("/fib/10", 200, "text/plain", "55")

    flaky_sequence = iter(
        [
            deploy_verify.ProbeResult(
                path="/fib/10",
                passed=False,
                failure_reason="transient",
            ),
            deploy_verify.ProbeResult(
                path="/fib/10",
                passed=True,
                status_code=200,
                content_type="text/plain; charset=utf-8",
                body_snippet="fib(10) = 55",
            ),
        ]
    )

    def fake_probe_once(_url: str, _spec: deploy_verify.ProbeSpec) -> deploy_verify.ProbeResult:
        return next(flaky_sequence)

    monkeypatch.setattr(deploy_verify, "_probe_once", fake_probe_once)

    result = deploy_verify.probe_with_retries("http://unused", spec, retries=2)
    assert result.passed
    assert result.attempts == 2


def test_probe_with_retries_exhausts_attempts(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(deploy_verify, "_RETRY_DELAY_SECONDS", 0)
    spec = deploy_verify.ProbeSpec("/fib/10", 200, "text/plain", "55")
    call_count = {"n": 0}

    def fake_probe_once(_url: str, _spec: deploy_verify.ProbeSpec) -> deploy_verify.ProbeResult:
        call_count["n"] += 1
        return deploy_verify.ProbeResult(
            path=spec.path,
            passed=False,
            failure_reason="hard failure",
        )

    monkeypatch.setattr(deploy_verify, "_probe_once", fake_probe_once)

    result = deploy_verify.probe_with_retries("http://unused", spec, retries=2)
    assert not result.passed
    assert result.attempts == 3  # initial + 2 retries
    assert call_count["n"] == 3


def test_main_writes_report_and_returns_zero_on_success(
    live_server: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(deploy_verify, "_RETRY_DELAY_SECONDS", 0)
    monkeypatch.setattr(
        deploy_verify,
        "PROBE_PATHS",
        [
            deploy_verify.ProbeSpec("/", 200, "text/html", None),
            deploy_verify.ProbeSpec("/fib/10", 200, "text/plain", "55"),
        ],
    )

    artifact_root = tmp_path / "report"
    rc = deploy_verify.main(
        [
            "--live-base-url",
            live_server,
            "--artifact-root",
            str(artifact_root),
            "--retries",
            "0",
        ]
    )
    assert rc == 0

    report_path = artifact_root / "deploy_verify_report.json"
    assert report_path.exists()
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    assert payload["total"] == 2
    assert payload["passed"] == 2
    assert payload["failed"] == 0
    assert payload["live_base_url"] == live_server
    assert {entry["path"] for entry in payload["results"]} == {"/", "/fib/10"}
    assert all(entry["passed"] for entry in payload["results"])


def test_main_returns_nonzero_on_failure(
    live_server: str,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(deploy_verify, "_RETRY_DELAY_SECONDS", 0)
    monkeypatch.setattr(
        deploy_verify,
        "PROBE_PATHS",
        [
            deploy_verify.ProbeSpec("/", 200, "text/html", None),
            deploy_verify.ProbeSpec("/fib/1102", 200, "text/plain", None),
        ],
    )

    artifact_root = tmp_path / "report"
    rc = deploy_verify.main(
        [
            "--live-base-url",
            live_server,
            "--artifact-root",
            str(artifact_root),
            "--retries",
            "0",
        ]
    )
    assert rc == 1

    report_path = artifact_root / "deploy_verify_report.json"
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    assert payload["passed"] == 1
    assert payload["failed"] == 1
    failing = next(entry for entry in payload["results"] if not entry["passed"])
    assert "Error 1102" in (failing["failure_reason"] or "")
