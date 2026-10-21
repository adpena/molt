"""The Cloudflare endpoint verifier, run against an in-process HTTP server.

The server stands in for a deployed worker and answers each route with a
fixed status, headers and body, so every check the verifier makes has a known
right answer: status, Content-Type, header and body sentinels, binary
prefixes, Cloudflare's ``Error 1102`` page, retries while a deployment
converges, network failures, and the CLI's exit status and summary.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator

import pytest

import tools.cloudflare_demo_endpoint_verify as endpoint_verify
import tools.cloudflare_demo_verify as cloudflare_demo_verify
from tools.cloudflare_demo_verify import EndpointCase, MatrixFailed

_TEXT = {"Content-Type": "text/plain; charset=utf-8"}
_WASM_MAGIC = b"\x00asm\x01\x00\x00\x00"

# (method, path) -> responses served in order; the last one repeats.
_ROUTES: dict[tuple[str, str], list[tuple[int, dict[str, str], bytes]]] = {
    ("GET", "/generate/1"): [(200, _TEXT, b"microGPT\n\n  1. ada\n")],
    ("GET", "/generate/1-nul"): [(200, _TEXT, b"\x00microGPT\n")],
    ("GET", "/fib/1102"): [
        (200, _TEXT, b"Worker error: Error 1102 - Worker exceeded CPU time limit\n")
    ],
    ("GET", "/fib/wrong-status"): [(500, _TEXT, b"oops")],
    ("GET", "/fib/wrong-ct"): [(200, {"Content-Type": "application/json"}, b"{}")],
    ("GET", "/asset.wasm"): [
        (
            200,
            {"Content-Type": "application/wasm", "Cache-Control": "immutable"},
            _WASM_MAGIC + bytes(1 << 20),
        )
    ],
    ("HEAD", "/asset.wasm"): [
        (200, {"Content-Type": "application/wasm", "Cache-Control": "immutable"}, b"")
    ],
    ("POST", "/echo"): [(201, {"Content-Type": "application/json"}, b"")],
    ("GET", "/converging"): [
        (503, _TEXT, b"deploying"),
        (200, _TEXT, b"ready"),
    ],
}


class _WorkerStub(BaseHTTPRequestHandler):
    served: dict[tuple[str, str], int]
    seen: list[dict[str, object]]

    def _answer(self, method: str) -> None:
        length = int(self.headers.get("Content-Length") or 0)
        request_body = self.rfile.read(length)
        self.seen.append(
            {
                "method": method,
                "path": self.path,
                "headers": dict(self.headers),
                "body": request_body,
            }
        )
        responses = _ROUTES.get((method, self.path), [(404, _TEXT, b"missing")])
        count = self.served.get((method, self.path), 0)
        self.served[(method, self.path)] = count + 1
        status, headers, body = responses[min(count, len(responses) - 1)]
        if (method, self.path) == ("POST", "/echo"):
            body = json.dumps({"received": request_body.decode("utf-8")}).encode()
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if method != "HEAD":
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass  # A binary case closes after reading its prefix.

    def do_GET(self) -> None:  # noqa: N802 - HTTP handler API
        self._answer("GET")

    def do_HEAD(self) -> None:  # noqa: N802 - HTTP handler API
        self._answer("HEAD")

    def do_POST(self) -> None:  # noqa: N802 - HTTP handler API
        self._answer("POST")

    def log_message(self, _format: str, *_args: object) -> None:
        return


@pytest.fixture()
def worker() -> Iterator[tuple[str, list[dict[str, object]]]]:
    seen: list[dict[str, object]] = []
    handler = type("_Bound", (_WorkerStub,), {"served": {}, "seen": seen})
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    # A short poll interval keeps shutdown from waiting the default 0.5 s.
    thread = threading.Thread(
        target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True
    )
    thread.start()
    host, port = server.server_address
    try:
        yield f"http://{host}:{port}", seen
    finally:
        server.shutdown()
        server.server_close()


def _verify(base_url: str, tmp_path: Path, *cases: EndpointCase, attempts: int = 1):
    return cloudflare_demo_verify.verify_http_matrix(
        base_url,
        cases,
        artifact_root=tmp_path,
        attempts=attempts,
        retry_delay_s=0.0,
    )


def _failure(base_url: str, tmp_path: Path, case: EndpointCase) -> str:
    with pytest.raises(MatrixFailed) as raised:
        _verify(base_url, tmp_path, case)
    [result] = raised.value.report.cases
    assert not result.ok
    assert raised.value.report.failures == [f"{case.name}: {result.reason}"]
    return result.reason or ""


def test_accepts_the_expected_route_contract(worker, tmp_path: Path) -> None:
    base_url, _ = worker
    report = _verify(
        base_url,
        tmp_path,
        EndpointCase(
            name="generate_one",
            path="/generate/1",
            expected_content_type_prefix="text/plain",
            body_contains=("microGPT", "1. ada"),
        ),
    )
    assert report.ok
    summary = json.loads((tmp_path / "summary.json").read_text(encoding="utf-8"))
    assert summary["ok"] is True
    assert summary["cases"][0]["status"] == 200
    assert summary["cases"][0]["attempts"] == 1


_BROKEN_RESPONSES = [
    (EndpointCase(name="nul", path="/generate/1-nul"), "NUL byte"),
    (EndpointCase(name="cpu", path="/fib/1102"), "Error 1102"),
    (
        EndpointCase(name="status", path="/fib/wrong-status"),
        "expected status 200, got 500",
    ),
    (
        EndpointCase(
            name="status_set",
            path="/fib/wrong-status",
            expected_statuses=(200, 503),
        ),
        "expected status 200 or 503, got 500",
    ),
    (
        EndpointCase(
            name="type",
            path="/fib/wrong-ct",
            expected_content_type_prefix="text/plain",
        ),
        "expected Content-Type prefix 'text/plain', got 'application/json'",
    ),
    (
        EndpointCase(name="sentinel", path="/generate/1", body_contains=("55",)),
        "missing body sentinel '55'",
    ),
    (
        EndpointCase(name="forbidden", path="/generate/1", body_not_contains=("ada",)),
        "unexpected body sentinel 'ada'",
    ),
    (
        EndpointCase(
            name="header",
            path="/asset.wasm",
            expected_headers=(("Cache-Control", "no-store"),),
            body_prefix=_WASM_MAGIC,
        ),
        "expected header Cache-Control containing 'no-store', got 'immutable'",
    ),
    (
        EndpointCase(
            name="absent_header",
            path="/generate/1",
            expected_headers=(("Access-Control-Allow-Origin", ""),),
        ),
        "expected header Access-Control-Allow-Origin containing '', got None",
    ),
    (
        EndpointCase(name="prefix", path="/generate/1", body_prefix=_WASM_MAGIC),
        "expected body prefix",
    ),
]


@pytest.mark.parametrize(
    ("case", "reason"),
    _BROKEN_RESPONSES,
    ids=[case.name for case, _ in _BROKEN_RESPONSES],
)
def test_rejects_each_broken_response_shape(
    worker, tmp_path: Path, case: EndpointCase, reason: str
) -> None:
    base_url, _ = worker
    assert reason in _failure(base_url, tmp_path, case)


def test_binary_case_reads_only_its_prefix_and_head_checks_headers(
    worker, tmp_path: Path
) -> None:
    base_url, seen = worker
    asset_headers = (("Cache-Control", "immutable"), ("Content-Length", ""))
    report = _verify(
        base_url,
        tmp_path,
        EndpointCase(
            name="wasm",
            path="/asset.wasm",
            expected_content_type_prefix="application/wasm",
            expected_headers=asset_headers,
            body_prefix=_WASM_MAGIC,
        ),
        EndpointCase(
            name="wasm_head",
            path="/asset.wasm",
            method="HEAD",
            expected_headers=asset_headers,
        ),
    )
    assert [case.ok for case in report.cases] == [True, True]
    assert [request["method"] for request in seen] == ["GET", "HEAD"]


def test_sends_the_case_method_headers_and_body(worker, tmp_path: Path) -> None:
    base_url, seen = worker
    _verify(
        base_url,
        tmp_path,
        EndpointCase(
            name="echo",
            path="/echo",
            method="POST",
            request_headers=(("Origin", "https://app.example"),),
            body=b'{"image": "x"}',
            expected_statuses=(201,),
            body_contains=('"received"',),
        ),
    )
    [request] = seen
    assert request["method"] == "POST"
    assert request["body"] == b'{"image": "x"}'
    headers = request["headers"]
    assert isinstance(headers, dict)
    assert headers["Origin"] == "https://app.example"
    # Cloudflare answers the default Python-urllib user agent with 403.
    assert headers["User-Agent"] == "molt-cloudflare-verify/1.0"


def test_retries_until_a_deployment_converges(worker, tmp_path: Path) -> None:
    base_url, seen = worker
    case = EndpointCase(name="converging", path="/converging", body_contains=("ready",))
    report = _verify(base_url, tmp_path, case, attempts=3)
    [result] = report.cases
    assert result.ok
    assert result.attempts == 2
    assert len(seen) == 2


def test_reports_the_last_attempt_when_retries_run_out(worker, tmp_path: Path) -> None:
    base_url, seen = worker
    case = EndpointCase(name="broken", path="/fib/wrong-status")
    with pytest.raises(MatrixFailed) as raised:
        _verify(base_url, tmp_path, case, attempts=3)
    [result] = raised.value.report.cases
    assert result.attempts == 3
    assert len(seen) == 3


def test_records_a_network_failure_as_a_failed_case(tmp_path: Path) -> None:
    probe = ThreadingHTTPServer(("127.0.0.1", 0), _WorkerStub)
    host, port = probe.server_address
    probe.server_close()  # Nothing listens on this port any more.
    case = EndpointCase(name="down", path="/health")
    reason = _failure(f"http://{host}:{port}", tmp_path, case)
    assert reason.startswith("GET /health failed:")


def test_falcon_ocr_matrix_takes_its_origin_from_the_worker_config(
    tmp_path: Path,
) -> None:
    config = tmp_path / "wrangler.toml"
    config.write_text('[vars]\nCORS_ORIGIN = "https://app.example"\n', encoding="utf-8")
    cases = cloudflare_demo_verify.build_falcon_ocr_matrix(config)
    origins = {
        value
        for case in cases
        for name, value in case.request_headers + case.expected_headers
        if name in {"Origin", "Access-Control-Allow-Origin"}
    }
    assert origins == {"https://app.example"}
    assert len({case.name for case in cases}) == len(cases)


def test_cli_exit_status_and_summary(
    worker, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    base_url, _ = worker
    passing = EndpointCase(name="generate_one", path="/generate/1")
    failing = EndpointCase(name="cpu", path="/fib/1102")
    argv = ["--base-url", base_url, "--case-set", "matrix"]

    monkeypatch.setattr(endpoint_verify, "_select_cases", lambda _set: (passing,))
    ok_root = tmp_path / "ok"
    assert endpoint_verify.main([*argv, "--artifact-root", str(ok_root)]) == 0
    summary = json.loads((ok_root / "http" / "summary.json").read_text("utf-8"))
    assert summary["ok"] is True

    monkeypatch.setattr(
        endpoint_verify, "_select_cases", lambda _set: (passing, failing)
    )
    bad_root = tmp_path / "bad"
    assert endpoint_verify.main([*argv, "--artifact-root", str(bad_root)]) == 1
    summary = json.loads((bad_root / "http" / "summary.json").read_text("utf-8"))
    assert summary["ok"] is False
    assert [case["ok"] for case in summary["cases"]] == [True, False]


def test_cli_verifies_the_url_wrangler_printed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    log = tmp_path / "wrangler.log"
    log.write_text(
        "Uploaded falcon-ocr\nDeployed falcon-ocr triggers\n"
        "  https://falcon-ocr.example.workers.dev\n",
        encoding="utf-8",
    )
    seen: list[str] = []

    def fake_verify(base_url, cases, **_kwargs):
        seen.append(base_url)
        return cloudflare_demo_verify.VerificationReport(
            ok=True, failures=[], cases=(), artifact_root=tmp_path, transport="http"
        )

    monkeypatch.setattr(endpoint_verify, "verify_http_matrix", fake_verify)
    argv = ["--wrangler-log", str(log), "--case-set", "falcon-ocr"]
    assert endpoint_verify.main([*argv, "--artifact-root", str(tmp_path)]) == 0
    assert seen == ["https://falcon-ocr.example.workers.dev"]

    log.write_text("Deploy failed\n", encoding="utf-8")
    with pytest.raises(SystemExit, match="no workers.dev URL"):
        endpoint_verify.main(argv)


def test_cli_refuses_source_transport_for_an_http_only_case_set() -> None:
    with pytest.raises(SystemExit) as raised:
        endpoint_verify.main(["--case-set", "falcon-ocr", "--transport", "source"])
    assert raised.value.code == 2
