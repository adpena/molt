from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _run_ocr_api_script(script: str) -> dict[str, object]:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare OCR API contract tests")
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout)


def test_cloudflare_ocr_decode_rejects_empty_token_array() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const tokenizer = { decode() { return ''; } };
        try {
          mod.decodeOcrTokenArray([], tokenizer);
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "no tokens" in result["message"].lower()


def test_cloudflare_ocr_decode_rejects_whitespace_only_text() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const tokenizer = { decode() { return '   '; } };
        try {
          mod.decodeOcrTokenArray([1], tokenizer);
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "empty text" in result["message"].lower()


def test_cloudflare_ocr_result_contract_rejects_malformed_cache_payloads() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const samples = [
          null,
          {},
          { text: '', tokens: [1] },
          { text: '   ', tokens: [1] },
          { text: 'invoice', tokens: [] },
          { text: 'invoice', tokens: ['bad'] },
          { error: 'backend failed', text: 'invoice', tokens: [1] },
        ];
        const messages = [];
        for (const sample of samples) {
          try {
            mod.normalizeOcrResultPayload(sample, 'cached OCR result');
            messages.push({ ok: true });
          } catch (err) {
            messages.push({ ok: false, message: err.message });
          }
        }
        const valid = mod.normalizeOcrResultPayload({
          text: '  Invoice 42  ',
          tokens: [10, 11],
          timestamp: 123,
        }, 'cached OCR result');
        process.stdout.write(JSON.stringify({ messages, valid }));
        """
    )

    assert all(item["ok"] is False for item in result["messages"])
    assert result["valid"]["text"] == "  Invoice 42  "
    assert result["valid"]["tokens"] == [10, 11]
    assert result["valid"]["timestamp"] == 123


def test_cloudflare_batch_ocr_rejects_malformed_cached_payload() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({ images: [btoa('cached-image-bytes')] });
        const request = new Request('https://example.invalid/ocr/batch', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const cacheOps = {
          async hashBytes() { return 'hash-1'; },
          async getCached() { return { text: '', tokens: [1], timestamp: Date.now() }; },
          setCached() { throw new Error('must not write malformed cached result'); },
        };
        const response = await mod.handleBatchOcr(
          request,
          { ocrTokens() { return [1]; } },
          {},
          {},
          'rid-batch',
          'wasm',
          cacheOps,
          { decode() { return 'decoded'; } },
        );
        process.stdout.write(JSON.stringify(await response.json()));
        """
    )

    assert result["results"][0]["tokens"] == []
    assert "invalid cached ocr result" in result["results"][0]["error"].lower()
    assert "cache" not in result["results"][0]


def test_cloudflare_table_ocr_rejects_invalid_model_output() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({
          image: btoa('not-real-image-but-table-handler-only-base64s-it'),
          format: 'image/png'
        });
        const request = new Request('https://example.invalid/ocr/table', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: 'not json at all' };
            }
          }
        };
        const response = await mod.handleTableOcr(request, env, {}, 'rid-table');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid" in result["body"]["error"].lower()
    assert result["body"]["request_id"] == "rid-table"


def test_cloudflare_table_ocr_rejects_malformed_table_schema() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({
          image: btoa('not-real-image-but-table-handler-only-base64s-it'),
          format: 'image/png'
        });
        const request = new Request('https://example.invalid/ocr/table', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: '{"tables":[{"headers":["A"],"data":"bad"}]}' };
            }
          }
        };
        const response = await mod.handleTableOcr(request, env, {}, 'rid-table');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid" in result["body"]["error"].lower()


def test_cloudflare_table_ocr_rejects_inconsistent_table_dimensions() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({
          image: btoa('not-real-image-but-table-handler-only-base64s-it'),
          format: 'image/png'
        });
        const request = new Request('https://example.invalid/ocr/table', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: '{"tables":[{"headers":["A"],"data":[["x","y"]],"rows":1,"cols":1}]}' };
            }
          }
        };
        const response = await mod.handleTableOcr(request, env, {}, 'rid-table');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid" in result["body"]["error"].lower()


def test_cloudflare_table_ocr_accepts_explicit_empty_tables_json() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({
          image: btoa('not-real-image-but-table-handler-only-base64s-it'),
          format: 'image/png'
        });
        const request = new Request('https://example.invalid/ocr/table', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: '{"tables":[]}' };
            }
          }
        };
        const response = await mod.handleTableOcr(request, env, {}, 'rid-table');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
        }));
        """
    )

    assert result["status"] == 200
    assert result["body"]["tables"] == []
    assert result["body"]["table_count"] == 0


def test_cloudflare_detailed_ocr_rejects_invalid_model_output_without_cache_write() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const calls = { put: 0 };
        const body = JSON.stringify({ image: btoa('detail-image-bytes') });
        const request = new Request('https://example.invalid/ocr/detailed', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: 'not json at all' };
            }
          },
          OCR_CACHE: {
            async get() { return null; },
            async put() { calls.put += 1; },
          },
        };
        const response = await mod.handleDetailedOcr(request, env, {}, 'rid-detail');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
          calls,
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid detailed ocr model output" in result["body"]["error"].lower()
    assert result["calls"]["put"] == 0


def test_cloudflare_detailed_ocr_rejects_malformed_cached_page() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const calls = { ai: 0 };
        const body = JSON.stringify({ image: btoa('detail-image-bytes') });
        const request = new Request('https://example.invalid/ocr/detailed', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              calls.ai += 1;
              return { response: '{"text":"should not run","blocks":[],"tables":[],"metadata":{"language":"en","orientation":0,"has_handwriting":false}}' };
            }
          },
          OCR_CACHE: {
            async get() {
              return { text: '   ', blocks: [], tables: [], metadata: {} };
            },
            async put() { throw new Error('must not write after malformed cache hit'); },
          },
        };
        const response = await mod.handleDetailedOcr(request, env, {}, 'rid-detail');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
          calls,
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid cached detailed ocr page" in result["body"]["error"].lower()
    assert result["calls"]["ai"] == 0


def test_cloudflare_detailed_ocr_rejects_malformed_cached_block() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const body = JSON.stringify({ image: btoa('detail-image-bytes') });
        const request = new Request('https://example.invalid/ocr/detailed', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body,
        });
        const env = {
          AI: {
            async run() {
              return { response: '{"text":"should not run","blocks":[],"tables":[],"metadata":{"language":"en","orientation":0,"has_handwriting":false}}' };
            }
          },
          OCR_CACHE: {
            async get() {
              return { text: 'ok', blocks: [null], tables: [], metadata: {} };
            },
          },
        };
        const response = await mod.handleDetailedOcr(request, env, {}, 'rid-detail');
        process.stdout.write(JSON.stringify({
          status: response.status,
          body: await response.json(),
        }));
        """
    )

    assert result["status"] == 502
    assert "invalid cached detailed ocr page" in result["body"]["error"].lower()


def test_cloudflare_template_extract_rejects_invalid_layout_model_output() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const env = {
          AI: {
            async run() {
              return { response: 'not json at all' };
            }
          }
        };
        try {
          await mod.handleTemplateExtractFast(env, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "template extraction model output" in result["message"].lower()


def test_cloudflare_template_extract_rejects_invalid_layout_schema() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const env = {
          AI: {
            async run() {
              return { response: '{}' };
            }
          }
        };
        try {
          await mod.handleTemplateExtractFast(env, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "sections" in result["message"].lower()


def test_cloudflare_template_extract_rejects_malformed_cached_payload() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const env = {
          OCR_CACHE: {
            async get() {
              return { template: null, detected_sections: [], confidence: 0.5 };
            },
            async put() { throw new Error('must not write after invalid cache'); },
          },
          AI: {
            async run() {
              return { response: '{"sections":["header"]}' };
            }
          },
        };
        try {
          await mod.handleTemplateExtractFast(env, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "cached template extraction result" in result["message"].lower()


def test_cloudflare_template_extract_rejects_shallow_cached_template() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const env = {
          OCR_CACHE: {
            async get() {
              return { template: {}, detected_sections: ['header'], confidence: 0.5 };
            },
          },
          AI: {
            async run() {
              return { response: '{"sections":["header"]}' };
            }
          },
        };
        try {
          await mod.handleTemplateExtractFast(env, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "cached template extraction result" in result["message"].lower()


def test_cloudflare_template_extract_requires_workers_ai_binding() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        try {
          await mod.handleTemplateExtractFast({}, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "workers ai" in result["message"].lower()


def test_cloudflare_worker_timeout_wrapper_preserves_non_timeout_errors() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/worker.js');
        const observed = {};
        try {
          await mod.withTimeout(() => {
            throw new Error('Falcon OCR decoded empty text');
          }, 1000, 'OCR inference');
        } catch (err) {
          observed.nonTimeout = {
            name: err.name,
            message: err.message,
            isTimeout: mod.isOperationTimeoutError(err),
          };
        }
        try {
          await mod.withTimeout(() => new Promise(() => {}), 1, 'OCR inference');
        } catch (err) {
          observed.timeout = {
            name: err.name,
            message: err.message,
            isTimeout: mod.isOperationTimeoutError(err),
          };
        }
        process.stdout.write(JSON.stringify(observed));
        """
    )

    assert result["nonTimeout"] == {
        "name": "Error",
        "message": "Falcon OCR decoded empty text",
        "isTimeout": False,
    }
    assert result["timeout"]["name"] == "OperationTimeoutError"
    assert result["timeout"]["isTimeout"] is True


def test_cloudflare_worker_shared_ocr_payload_rebinds_request_id() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/worker.js');
        const shared = mod.stripRequestScopedJsonFields({
          text: 'Invoice 42',
          tokens: [1, 2],
          request_id: 'rid-first',
        });
        const rebound = mod.attachRequestIdToJsonBody(JSON.stringify(shared), 'rid-second');
        process.stdout.write(JSON.stringify({
          shared,
          rebound: JSON.parse(rebound),
        }));
        """
    )

    assert "request_id" not in result["shared"]
    assert result["rebound"]["text"] == "Invoice 42"
    assert result["rebound"]["tokens"] == [1, 2]
    assert result["rebound"]["request_id"] == "rid-second"
