"""Access contract of the Falcon-OCR Cloudflare Worker, run through its fetch handler.

The worker serves the paid OCR API, so four access rules matter: CORS names
one configured origin and never the caller's, OCR endpoints accept only POST,
once a wallet is configured an OCR call from an API client or a foreign origin
needs a valid x402 payment proof while ``/health`` stays free, and the public
R2 assets answer GET and HEAD alike without payment. Each test sends real
requests to ``deploy/cloudflare/worker.js`` under Node with stub bindings and
reads the responses.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process


ROOT = Path(__file__).resolve().parents[1]
RESULT_MARKER = "@@WORKER_ACCESS@@ "
WALLET = "0x00000000000000000000000000000000000000a1"
FOREIGN_ORIGIN = "https://attacker.example"

_HARNESS = """
const workerModule = await import('./deploy/cloudflare/worker.js');
const rateLimiter = {
  idFromName(name) { return name; },
  get() { return { async fetch() { return new Response('ok', { status: 200 }); } }; },
};
async function call(method, path, headers = {}, env = {}, body = undefined) {
  const response = await workerModule.default.fetch(
    new Request(`https://falcon-ocr.example${path}`, { method, headers, body }),
    { RATE_LIMITER: rateLimiter, ...env },
    { waitUntil() {} },
  );
  const bytes = new Uint8Array(await response.arrayBuffer());
  let json = null;
  try { json = JSON.parse(new TextDecoder().decode(bytes)); } catch {}
  return {
    status: response.status,
    headers: Object.fromEntries(response.headers),
    json,
    body: Array.from(bytes),
  };
}
const results = {};
"""


def _run_worker(cases: str) -> dict[str, dict]:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare Worker access tests")
    script = (
        _HARNESS
        + cases
        + f"\nprocess.stdout.write('\\n{RESULT_MARKER}' + JSON.stringify(results) + '\\n');\n"
    )
    run = run_guarded_test_process(
        ["node", "--input-type=module", "-e", script],
        prefix="MOLT_NODE_TEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    lines = [line for line in run.stdout.splitlines() if line.startswith(RESULT_MARKER)]
    assert len(lines) == 1, run.stdout[-2000:]
    return json.loads(lines[0][len(RESULT_MARKER) :])


def test_preflight_names_only_the_configured_origin() -> None:
    results = _run_worker(
        f"""
        results.defaulted = await call('OPTIONS', '/ocr', {{ Origin: '{FOREIGN_ORIGIN}' }});
        results.configured = await call(
          'OPTIONS', '/ocr', {{ Origin: '{FOREIGN_ORIGIN}' }},
          {{ CORS_ORIGIN: 'https://app.example' }},
        );
        """
    )

    expected_origin = {
        "defaulted": "https://freeinvoicemaker.app",
        "configured": "https://app.example",
    }
    for case, origin in expected_origin.items():
        response = results[case]
        assert response["status"] == 204, case
        assert response["headers"]["access-control-allow-origin"] == origin, case
        allowed = {
            name.strip()
            for name in response["headers"]["access-control-allow-headers"].split(",")
        }
        assert {"X-Payment-402", "X-Request-ID"} <= allowed, case


def test_ocr_endpoint_rejects_other_methods() -> None:
    results = _run_worker(
        f"""
        results.get = await call(
          'GET', '/ocr', {{ 'User-Agent': 'FalconOCR-Client/1.0', 'X-Request-ID': 'rid-405' }},
          {{ X402_WALLET_ADDRESS: '{WALLET}' }},
        );
        """
    )

    response = results["get"]
    assert response["status"] == 405
    assert response["json"] == {"error": "Method not allowed", "request_id": "rid-405"}


def test_ocr_requires_payment_from_api_clients_and_foreign_origins() -> None:
    results = _run_worker(
        f"""
        const env = {{ X402_WALLET_ADDRESS: '{WALLET}' }};
        const api = {{ 'User-Agent': 'FalconOCR-Client/1.0', 'Content-Type': 'application/json' }};
        results.apiClient = await call('POST', '/ocr', api, env, '{{}}');
        results.foreignOrigin = await call(
          'POST', '/ocr', {{ ...api, Origin: '{FOREIGN_ORIGIN}' }}, env, '{{}}');
        results.malformedProof = await call(
          'POST', '/ocr', {{ ...api, 'X-Payment-402': 'not-a-proof' }}, env, '{{}}');
        results.configuredOrigin = await call(
          'POST', '/ocr', {{ ...api, Origin: 'https://freeinvoicemaker.app' }}, env, '{{}}');
        results.devMode = await call('POST', '/ocr', api, {{}}, '{{}}');
        """
    )

    for case in ("apiClient", "foreignOrigin", "malformedProof"):
        response = results[case]
        assert response["status"] == 402, case
        assert response["json"]["payment_required"]["recipient"] == WALLET, case
        assert response["headers"]["x-payment-recipient"] == WALLET, case
        for header in ("x-payment-version", "x-payment-network", "x-payment-amount"):
            assert response["headers"].get(header), (case, header)
    assert results["apiClient"]["json"]["error"] == "Missing X-Payment-402 header"
    assert "X-Payment-402" in results["malformedProof"]["json"]["error"]
    # The configured browser origin and a worker without a wallet skip payment.
    assert results["configuredOrigin"]["status"] != 402
    assert results["devMode"]["status"] != 402


def test_health_answers_without_payment() -> None:
    results = _run_worker(
        f"""
        results.health = await call(
          'GET', '/health', {{ 'X-Request-ID': 'rid-health' }},
          {{ X402_WALLET_ADDRESS: '{WALLET}' }},
        );
        """
    )

    # With no model loaded the health document reports "loading" with 503;
    # either way it is the health document, not a payment demand.
    response = results["health"]
    assert response["status"] in {200, 503}
    assert "payment_required" not in response["json"]
    assert response["json"]["request_id"] == "rid-health"
    assert response["json"]["status"] == "loading"
    assert set(response["json"]["ocr_engines"]) == {"paddle-ocr", "falcon-ocr"}


def test_public_assets_answer_get_and_head_alike() -> None:
    results = _run_worker(
        f"""
        const objects = new Map([
          ['models/falcon-ocr/falcon-ocr.wasm', new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0])],
          ['models/falcon-ocr-int4/config.json', new TextEncoder().encode('{{"dim":768}}')],
          ['browser/loader.js', new TextEncoder().encode('export {{}};')],
        ]);
        const weights = {{
          async get(key) {{
            const bytes = objects.get(key);
            return bytes ? {{ size: bytes.length, body: new Blob([bytes]).stream() }} : null;
          }},
        }};
        const env = {{ X402_WALLET_ADDRESS: '{WALLET}', WEIGHTS: weights }};
        const paths = {{
          wasm: '/wasm/falcon-ocr.wasm',
          config: '/weights/falcon-ocr-int4/config.json',
          browser: '/browser/loader.js',
          missing: '/wasm/absent.wasm',
          health: '/health',
        }};
        for (const [name, path] of Object.entries(paths)) {{
          results[name + 'Get'] = await call('GET', path, {{ 'X-Request-ID': 'rid' }}, env);
          results[name + 'Head'] = await call('HEAD', path, {{ 'X-Request-ID': 'rid' }}, env);
        }}
        """
    )

    expected = {
        "wasm": ("application/wasm", "public, max-age=86400, immutable", 8),
        "config": ("application/json", "public, max-age=86400, immutable", 11),
        "browser": ("application/javascript", "public, max-age=3600", 10),
    }
    for name, (content_type, cache_control, length) in expected.items():
        get, head = results[f"{name}Get"], results[f"{name}Head"]
        assert get["status"] == 200, name
        assert get["headers"]["content-type"] == content_type, name
        assert get["headers"]["cache-control"] == cache_control, name
        assert get["headers"]["content-length"] == str(length), name
        assert len(get["body"]) == length, name
        # RFC 9110, section 9.3.2: HEAD sends GET's status and headers, no body.
        assert head["status"] == get["status"], name
        assert head["headers"] == get["headers"], name
        assert head["body"] == [], name
    assert results["wasmGet"]["body"] == [0, 97, 115, 109, 1, 0, 0, 0]
    assert results["missingGet"]["status"] == 404
    assert results["missingGet"]["json"]["key"] == "models/falcon-ocr/absent.wasm"
    assert results["missingHead"]["status"] == 404
    assert results["missingHead"]["body"] == []
    # HEAD covers every GET route, not only the assets.
    assert results["healthHead"]["status"] == results["healthGet"]["status"]
    assert results["healthGet"]["json"]["request_id"] == "rid"
    assert results["healthHead"]["body"] == []
