"""The MCP tool definition of the Falcon-OCR worker against the worker itself.

``deploy/mcp/ocr_tool.json`` is what an AI agent reads before it calls the
worker: the price and payment header, the image limit, the accepted formats
and the shape of the answer. Each test reads a fact from that file and
checks the worker's real behavior under Node, so the definition cannot drift
from the server it describes.
"""

from __future__ import annotations

import json
import shutil
import tomllib
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
MCP = json.loads((ROOT / "deploy/mcp/ocr_tool.json").read_text(encoding="utf-8"))
WRANGLER = tomllib.loads(
    (ROOT / "deploy/cloudflare/wrangler.toml").read_text(encoding="utf-8")
)
# A 1x1 RGBA PNG, which the worker decodes in pure JS under Node.
PNG_B64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/58BAwAI/AL+"
    "hc2rNAAAAABJRU5ErkJggg=="
)
_JSON_TYPES = {
    "string": str,
    "number": (int, float),
    "integer": int,
    "array": list,
    "object": dict,
}


def _run(script: str) -> dict:
    if shutil.which("node") is None:
        pytest.skip("node is required for the MCP contract tests")
    run = run_guarded_test_process(
        ["node", "--input-type=module", "-e", script],
        prefix="MOLT_NODE_TEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout.strip().splitlines()[-1])


def test_price_header_and_network_are_the_worker_x402_terms() -> None:
    auth = MCP["authentication"]
    header = auth["header"]
    result = _run(
        f"""
        const {{ verifyX402 }} = await import('./deploy/cloudflare/x402.js');
        const env = {{ X402_WALLET_ADDRESS: '0xa1', X402_VERIFICATION_URL: 'https://v.example' }};
        const decide = async (headers) => {{
          const request = new Request('https://w.example/ocr', {{ method: 'POST', headers }});
          const result = await verifyX402(request, env, 'rid', {{}});
          return result.response.json();
        }};
        process.stdout.write(JSON.stringify({{
          unpaid: await decide({{}}),
          garbled: await decide({{ '{header}': '!!!' }}),
        }}) + '\\n');
        """
    )

    terms = result["unpaid"]["payment_required"]
    amount, currency = auth["pricing"]["per_request"].split()
    assert (float(amount), currency) == (terms["price_usd"], "USD")
    assert auth["pricing"]["payment_network"] == terms["network"]
    assert auth["type"] == "x402"
    # The worker reads the payment proof from the header the definition names.
    assert result["garbled"]["error"].startswith(f"Failed to parse {header} header")


def test_image_limit_is_the_worker_limit_at_both_sides() -> None:
    limit = MCP["rate_limits"]["max_image_size_bytes"]
    assert int(WRANGLER["vars"]["MAX_IMAGE_BYTES"]) == limit
    result = _run(
        f"""
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = (size, format) => JSON.stringify({{
          image: Buffer.alloc(size).toString('base64'), format,
        }});
        const status = async (env, size, format) => (await mod.handleOcrRequest(
          new Request('https://w.example/ocr', {{
            method: 'POST', headers: {{ 'Content-Type': 'application/json' }},
            body: body(size, format),
          }}), null, env, {{}}, 'rid')).status;
        const results = {{}};
        // The deployed limit, and the code's default when no limit is set.
        for (const [name, env] of [['wrangler', {{ MAX_IMAGE_BYTES: '{limit}' }}], ['default', {{}}]]) {{
          results[name] = {{
            over: await status(env, {limit + 1}, 'image/png'),
            // At the limit the size passes; the GIF format then answers 415.
            at: await status(env, {limit}, 'image/gif'),
          }};
        }}
        process.stdout.write(JSON.stringify(results) + '\\n');
        """
    )

    for name, statuses in result.items():
        assert statuses == {"over": 413, "at": 415}, name


def _assert_matches(value: object, schema: dict, where: str) -> None:
    """Check ``value`` against the JSON Schema keywords this definition uses."""
    assert set(schema) <= {
        "type",
        "properties",
        "required",
        "items",
        "enum",
        "minimum",
        "maximum",
        "default",
        "description",
    }, where
    assert isinstance(value, _JSON_TYPES[schema["type"]]), where
    if "enum" in schema:
        assert value in schema["enum"], (where, value)
    if "minimum" in schema:
        assert value >= schema["minimum"], where
    if "maximum" in schema:
        assert value <= schema["maximum"], where
    if schema["type"] == "array":
        for index, item in enumerate(value):
            _assert_matches(item, schema["items"], f"{where}[{index}]")
    if schema["type"] == "object":
        assert set(schema.get("required", ())) <= set(value), where
        for name, child in schema.get("properties", {}).items():
            if name in value:
                _assert_matches(value[name], child, f"{where}.{name}")


def test_ocr_tool_schemas_describe_the_worker_request_and_answer() -> None:
    tool = next(t for t in MCP["tools"] if t["name"] == "ocr_extract_text")
    formats = tool["inputSchema"]["properties"]["format"]["enum"]
    result = _run(
        f"""
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const request = (format) => new Request('https://w.example/ocr', {{
          method: 'POST', headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{ image: '{PNG_B64}', format }}),
        }});
        const backend = {{ ocrTokens: () => [5, 6] }};
        const tokenizer = {{ decode: () => 'INVOICE 42' }};
        const answers = {{}};
        for (const device of ['wasm', 'cpu']) {{
          const response = await mod.handleOcrRequest(
            request('image/png'), backend, {{}}, {{}}, 'rid', device, tokenizer);
          answers[device] = {{ status: response.status, body: await response.json() }};
        }}
        // Every declared format passes validation; GIF does not.
        const formats = {{}};
        for (const format of {json.dumps(formats + ["image/gif"])}) {{
          const response = await mod.handleOcrRequest(
            request(format), backend, {{}}, {{}}, 'rid', 'wasm', tokenizer);
          formats[format] = response.status;
        }}
        process.stdout.write(JSON.stringify({{ answers, formats }}) + '\\n');
        """
    )

    _assert_matches(
        {"image": PNG_B64, "format": "image/png"}, tool["inputSchema"], "input"
    )
    for device, answer in result["answers"].items():
        assert answer["status"] == 200, device
        _assert_matches(answer["body"], tool["outputSchema"], f"output[{device}]")
    assert result["formats"].pop("image/gif") == 415
    assert set(result["formats"]) == set(formats)
    assert set(result["formats"].values()) == {200}
