from __future__ import annotations

import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

import tools.cloudflare_demo_verify as cloudflare_demo_verify


class _LiveVerifierHandler(BaseHTTPRequestHandler):
    response_map = {
        "/generate/1": (
            200,
            "text/plain; charset=utf-8",
            b"microGPT\n\n  1. ada\n",
        ),
        "/generate/1-nul": (
            200,
            "text/plain; charset=utf-8",
            b"\x00microGPT\n",
        ),
    }

    def do_GET(self) -> None:  # noqa: N802
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


def _run_http_server() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _LiveVerifierHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    return server, f"http://{host}:{port}"


def test_http_verifier_accepts_expected_route_contract(tmp_path: Path) -> None:
    server, base_url = _run_http_server()
    try:
        report = cloudflare_demo_verify.verify_http_matrix(
            base_url,
            [
                cloudflare_demo_verify.EndpointCase(
                    name="generate_one",
                    path="/generate/1",
                    expected_status=200,
                    expected_content_type_prefix="text/plain",
                    body_contains=("microGPT", "1. ada"),
                ),
            ],
            artifact_root=tmp_path,
        )
    finally:
        server.shutdown()
        server.server_close()

    assert report.ok
    assert report.failures == []


def test_http_verifier_rejects_nul_prefixed_body(tmp_path: Path) -> None:
    server, base_url = _run_http_server()
    try:
        with pytest.raises(cloudflare_demo_verify.VerificationError):
            cloudflare_demo_verify.verify_http_matrix(
                base_url,
                [
                    cloudflare_demo_verify.EndpointCase(
                        name="generate_one_nul",
                        path="/generate/1-nul",
                        expected_status=200,
                        expected_content_type_prefix="text/plain",
                    ),
                ],
                artifact_root=tmp_path,
            )
    finally:
        server.shutdown()
        server.server_close()
