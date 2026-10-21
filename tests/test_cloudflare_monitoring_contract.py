"""The Falcon-OCR worker's request log, run under Node.

``deploy/cloudflare/monitoring.js`` writes one JSON line per request. The
line is analytics, not an audit trail, so it holds the request's shape and
outcome and none of the client's data: no IP address, no raw user agent and
no image bytes. The test sends a request that carries all three and reads the
line that ``withMonitoring`` emits.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
CLIENT_IP = "203.0.113.77"
USER_AGENT = (
    "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 "
    "(KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1"
)
IMAGE_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ"

_SCRIPT = f"""
const {{ withMonitoring }} = await import('./deploy/cloudflare/monitoring.js');
const lines = [];
console.log = (line) => lines.push(JSON.parse(line));
const points = [];
const env = {{ MODEL_VERSION: '9.9.9', ANALYTICS: {{ writeDataPoint(p) {{ points.push(p); }} }} }};
const pending = [];
const ctx = {{ waitUntil(promise) {{ pending.push(promise); }} }};
const request = new Request('https://falcon-ocr.example/ocr', {{
  method: 'POST',
  headers: {{ 'CF-Connecting-IP': '{CLIENT_IP}', 'User-Agent': '{USER_AGENT}' }},
  body: JSON.stringify({{ image: '{IMAGE_B64}' }}),
}});
await withMonitoring({{
  request, rid: 'rid-ok', path: '/ocr', env, ctx,
  handler: async () => new Response('{{}}', {{ status: 200 }}),
}});
const failure = new Error('Weights not found: models/x\\n    at load (/srv/worker.js:1:1)');
try {{
  await withMonitoring({{
    request, rid: 'rid-fail', path: '/ocr', env, ctx,
    handler: async () => {{ throw failure; }},
  }});
}} catch (err) {{
  if (err !== failure) throw err;
}}
await Promise.all(pending);
process.stdout.write(JSON.stringify({{ lines, points }}) + '\\n');
"""


def _run() -> dict:
    if shutil.which("node") is None:
        pytest.skip("node is required for the monitoring contract test")
    run = run_guarded_test_process(
        ["node", "--input-type=module", "-e", _SCRIPT],
        prefix="MOLT_NODE_TEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout.strip().splitlines()[-1])


def test_request_log_holds_the_request_shape_and_no_client_data() -> None:
    payload = _run()
    ok, failed = payload["lines"]

    assert set(ok) == {
        "request_id",
        "timestamp",
        "method",
        "path",
        "status_code",
        "latency_ms",
        "device_type",
        "browser",
        "model_version",
    }
    assert (ok["request_id"], ok["method"], ok["path"]) == ("rid-ok", "POST", "/ocr")
    assert (ok["status_code"], ok["model_version"]) == (200, "9.9.9")
    # The user agent becomes two coarse classes, never the raw string.
    assert (ok["device_type"], ok["browser"]) == ("mobile", "safari")

    assert failed["status_code"] == 500
    assert failed["error_category"] == "MODEL_LOAD_FAILED"
    # One line of the message; no stack frames or server paths.
    assert failed["error_message"] == "Weights not found: models/x"

    # Analytics Engine gets one data point per request, from the same record.
    assert [point["blobs"][0] for point in payload["points"]] == ["rid-ok", "rid-fail"]
    assert payload["points"][0]["indexes"] == ["9.9.9"]

    emitted = json.dumps(payload)
    for client_data in (CLIENT_IP, USER_AGENT, IMAGE_B64, "/srv/worker.js"):
        assert client_data not in emitted
