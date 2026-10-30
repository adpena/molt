"""The enjoice handoff sources, run under Node against the real Worker.

``deploy/enjoice/`` is the canonical source of the OCR client that the
enjoice app copies. Node 26 strips TypeScript types on import; a resolve hook
adds the ``.ts`` extension to extensionless relative imports, as the app's
bundler does. The client's network calls go to the Falcon-OCR Worker's own
fetch handler in the same process, so the client and the server it targets
are tested together, offline.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]

_HARNESS = """
import { registerHooks } from 'node:module';
registerHooks({
  resolve(specifier, context, next) {
    try {
      return next(specifier, context);
    } catch (err) {
      const bare = specifier.startsWith('.') && !/\\.[cm]?[jt]s$/.test(specifier);
      if (err.code === 'ERR_MODULE_NOT_FOUND' && bare) {
        return next(`${specifier}.ts`, context);
      }
      throw err;
    }
  },
});
const worker = (await import('./deploy/cloudflare/worker.js')).default;
const backendModule = await import('./deploy/enjoice/ocr-backend-molt.ts');
const sessionModule = await import('./deploy/enjoice/falcon-ocr-molt.ts');
const rateLimiter = {
  idFromName: (name) => name,
  get: () => ({ fetch: async () => new Response('ok') }),
};
const requests = [];
function serveWith(env) {
  globalThis.fetch = async (url, init) => {
    requests.push({ url: String(url), method: init?.method ?? 'GET' });
    return worker.fetch(new Request(url, init), { RATE_LIMITER: rateLimiter, ...env }, {
      waitUntil() {},
    });
  };
}
const results = {};
"""


def _run(cases: str) -> dict:
    if shutil.which("node") is None:
        pytest.skip("node is required for the enjoice handoff tests")
    script = (
        _HARNESS
        + cases
        + "\nprocess.stdout.write(JSON.stringify({ results, requests }) + '\\n');\n"
    )
    run = run_guarded_test_process(
        ["node", "--no-warnings", "--input-type=module", "-e", script],
        prefix="MOLT_PYTEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout.strip().splitlines()[-1])


def test_template_extraction_round_trips_through_the_worker() -> None:
    sections = ["header", "line_items", "totals"]
    payload = _run(
        f"""
        const layout = {{
          sections: {json.dumps(sections)}, header_align: 'left',
          parties_layout: 'two-column', totals_position: 'right',
          logo_position: 'left', compact: false, heading_size: 'md',
        }};
        const ai = (response) => ({{ async run() {{ return {{ response }}; }} }});
        const backend = new backendModule.MoltOcrBackend({{
          workerUrl: 'https://falcon-ocr.example/',
        }});
        serveWith({{ AI: ai(JSON.stringify(layout)) }});
        results.ok = await backend.extractTemplate(btoa('invoice image'));
        // The worker's client error reaches the caller as the thrown message.
        serveWith({{ AI: ai(JSON.stringify(layout)) }});
        try {{
          await backend.extractTemplate('!!!');
          results.badImage = 'no error';
        }} catch (err) {{
          results.badImage = err.message;
        }}
        """
    )

    ok = payload["results"]["ok"]
    assert {"template", "confidence", "detected_sections", "time_ms"} <= set(ok)
    assert ok["detected_sections"] == sections
    assert isinstance(ok["confidence"], (int, float))
    assert isinstance(ok["time_ms"], (int, float))
    assert isinstance(ok["template"], dict)
    assert payload["results"]["badImage"] == "'image' is not valid base64"
    # The trailing slash of the configured URL does not double.
    assert {request["url"] for request in payload["requests"]} == {
        "https://falcon-ocr.example/template/extract"
    }


def test_nemotron_has_no_default_endpoint() -> None:
    payload = _run(
        """
        serveWith({});
        results.unconfigured = await backendModule.isNemotronAvailable('');
        results.automatic = backendModule.selectOcrEngine();
        results.forced = backendModule.selectOcrEngine('nemotron-v2');
        """
    )

    results = payload["results"]
    # No endpoint, no network call: the probe answers false by itself.
    assert results["unconfigured"] is False
    assert payload["requests"] == []
    # Automatic selection never routes to the external GPU service.
    assert results["automatic"] != "nemotron-v2"
    assert results["forced"] == "nemotron-v2"


def test_token_decoding_keeps_unknown_ids_and_edge_whitespace() -> None:
    payload = _run(
        """
        const idToToken = new Map([[1, '\\u2581Total'], [2, ':'], [3, '\\u2581']]);
        const vocab = { idToToken, tokenToId: new Map() };
        results.text = sessionModule.decodeTokens([3, 1, 2, 99, 3], vocab);
        """
    )

    # Sentencepiece word marks become spaces; an unknown id stays visible.
    assert payload["results"]["text"] == "  Total:[UNK:99] "
