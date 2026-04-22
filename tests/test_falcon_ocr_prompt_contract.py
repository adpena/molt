from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

from tests.helpers.falcon_ocr_paths import FALCON_OCR_TOKENIZER_PATH

ROOT = Path(__file__).resolve().parents[1]
TOKENIZER_PATH = FALCON_OCR_TOKENIZER_PATH

OFFICIAL_INSTRUCTIONS = {
    "plain": "Extract the text content from this image.",
    "text": "Extract the text content from this image.",
    "formula": "Extract the formula content from this image.",
    "table": "Extract the table content from this image.",
    "caption": "Extract the text content from this image.",
    "footnote": "Extract the text content from this image.",
    "list-item": "Extract the text content from this image.",
    "page-footer": "Extract the text content from this image.",
    "page-header": "Extract the text content from this image.",
    "section-header": "Extract the text content from this image.",
    "title": "Extract the text content from this image.",
}


def _official_prompt_ids() -> dict[str, list[int]]:
    pytest.importorskip("tokenizers")
    if not TOKENIZER_PATH.exists():
        pytest.skip(f"Falcon-OCR tokenizer not found at {TOKENIZER_PATH}")
    from tokenizers import Tokenizer

    tokenizer = Tokenizer.from_file(str(TOKENIZER_PATH))
    return {
        category: tokenizer.encode(
            f"<|image|>{instruction}\n<|OCR_PLAIN|>"
        ).ids
        for category, instruction in OFFICIAL_INSTRUCTIONS.items()
    }


def _cloudflare_prompt_ids() -> dict[str, list[int]]:
    if shutil.which("node") is None:
        pytest.skip("node is required for Falcon-OCR JS prompt contract tests")
    script = """
      const mod = await import('./deploy/cloudflare/tokenizer.js');
      const categories = [
        'plain', 'text', 'formula', 'table', 'caption', 'footnote',
        'list-item', 'page-footer', 'page-header', 'section-header', 'title'
      ];
      const out = Object.fromEntries(
        categories.map((category) => [category, mod.buildFalconOcrPromptIds(category)])
      );
      process.stdout.write(JSON.stringify(out));
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout)


def test_cloudflare_prompt_ids_match_official_falcon_ocr_tokenizer() -> None:
    assert _cloudflare_prompt_ids() == _official_prompt_ids()


def test_cloudflare_plain_prompt_uses_image_placeholder_not_start_token() -> None:
    prompt = _cloudflare_prompt_ids()["plain"]

    assert prompt[0] == 227  # <|image|>, replaced by the image token block upstream.
    assert 229 not in prompt  # <|start_of_image|> is not the OCR task prompt.
    assert prompt[-1] == 257  # <|OCR_PLAIN|>


def test_cpu_inference_does_not_inject_hidden_ocr_prompt() -> None:
    source = (ROOT / "deploy" / "cloudflare" / "inference-cpu.js").read_text(
        encoding="utf-8"
    )

    assert "OCR_PLAIN_TOKEN" not in source
    assert "prefixIds.push(257)" not in source


def test_browser_wasm_fallback_uses_official_plain_prompt() -> None:
    source = (ROOT / "deploy" / "browser" / "falcon-ocr-loader.js").read_text(
        encoding="utf-8"
    )
    plain_prompt = _official_prompt_ids()["plain"]

    assert "new Int32Array([1])" not in source
    assert f"new Int32Array(FALCON_OCR_PLAIN_PROMPT_IDS)" in source
    for token_id in plain_prompt:
        assert str(token_id) in source


def test_browser_loader_matches_wasm_driver_init_and_decode_contract() -> None:
    source = (ROOT / "deploy" / "browser" / "falcon-ocr-loader.js").read_text(
        encoding="utf-8"
    )

    assert "scales.json" not in source
    assert "JSON.parse(configJson)" not in source
    assert "this._instance.exports.init(weightsBuffer, configJson)" in source
    assert "decode_tokens" not in source
    assert ".join(' ')" not in source
    assert "this._tokenizer.decode(Array.from(tokenIds))" in source
    assert "this.weightsVariant = config.weightsVariant || 'falcon-ocr-int8-sharded'" in source


def test_browser_loader_defaults_point_at_worker_artifacts() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Falcon-OCR JS loader tests")
    script = """
      const { FalconOCR } = await import('./deploy/browser/falcon-ocr-loader.js');
      const loader = new FalconOCR();
      process.stdout.write(JSON.stringify({
        wasmUrl: loader.wasmUrl,
        tokenizerUrl: loader.tokenizerUrl,
        weightsVariant: loader.weightsVariant,
        weightsBaseUrl: loader.weightsBaseUrl
      }));
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    defaults = json.loads(run.stdout)
    assert defaults == {
        "wasmUrl": "https://falcon-ocr.adpena.workers.dev/wasm/falcon-ocr.wasm",
        "tokenizerUrl": "https://falcon-ocr.adpena.workers.dev/tokenizer.json",
        "weightsVariant": "falcon-ocr-int8-sharded",
        "weightsBaseUrl": "https://falcon-ocr.adpena.workers.dev/weights/falcon-ocr-int8-sharded",
    }


def test_browser_tokenizer_decoder_uses_byte_level_bpe() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Falcon-OCR JS tokenizer tests")
    script = """
      const { TokenizerDecoder } = await import('./deploy/browser/falcon-ocr-loader.js');
      const decoder = TokenizerDecoder.fromJSON(JSON.stringify({
        model: { vocab: { 'Ġ': 0, H: 1, i: 2, 'Ċ': 3 } },
        added_tokens: [{ id: 4, content: '<|end|>', special: true }]
      }));
      process.stdout.write(JSON.stringify(decoder.decode([0, 1, 2, 3, 4])));
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    assert json.loads(run.stdout) == " Hi\n"


def test_cloudflare_worker_routes_browser_artifacts_through_r2_keys() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare Worker route tests")
    script = """
      const workerModule = await import('./deploy/cloudflare/worker.js');
      const encoder = new TextEncoder();
      const fixtures = new Map([
        ['models/falcon-ocr/falcon-ocr.wasm', encoder.encode('wasm-binary')],
        ['models/falcon-ocr/tokenizer.json', encoder.encode('{"model":{"vocab":{}}}')],
        ['models/falcon-ocr-int8-sharded/model.safetensors.index.json', encoder.encode('{"weight_map":{}}')],
        ['models/falcon-ocr-int8-sharded/config.json', encoder.encode('{"dim":64}')],
        ['models/falcon-ocr-int8-sharded/scales.json', encoder.encode('{"x":1}')],
        ['models/falcon-ocr-int8-sharded/shard-00001-of-00002.safetensors', encoder.encode('shard-bytes')],
      ]);
      const calls = [];
      const env = {
        CORS_ORIGIN: 'https://freeinvoicemaker.app',
        WEIGHTS: {
          async get(key) {
            calls.push(key);
            const body = fixtures.get(key);
            if (!body) return null;
            return { body, size: body.byteLength };
          }
        }
      };
      const ctx = { waitUntil() {} };
      const routes = [
        ['/wasm/falcon-ocr.wasm', 'application/wasm', 'wasm-binary'],
        ['/tokenizer.json', 'application/json', '{"model":{"vocab":{}}}'],
        ['/weights/falcon-ocr-int8-sharded/model.safetensors.index.json', 'application/json', '{"weight_map":{}}'],
        ['/weights/falcon-ocr-int8-sharded/config.json', 'application/json', '{"dim":64}'],
        ['/weights/falcon-ocr-int8-sharded/scales.json', 'application/json', '{"x":1}'],
        ['/weights/falcon-ocr-int8-sharded/shard-00001-of-00002.safetensors', 'application/octet-stream', 'shard-bytes'],
      ];
      const results = [];
      for (const [path, expectedType, expectedText] of routes) {
        const response = await workerModule.default.fetch(
          new Request(`https://falcon-ocr.adpena.workers.dev${path}`),
          env,
          ctx,
        );
        results.push({
          path,
          status: response.status,
          contentType: response.headers.get('content-type'),
          cacheControl: response.headers.get('cache-control'),
          contentLength: response.headers.get('content-length'),
          expectedType,
          text: await response.text(),
          expectedText,
        });
      }
      process.stdout.write(JSON.stringify({ calls, results }));
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    payload = json.loads(run.stdout)
    assert payload["calls"] == [
        "models/falcon-ocr/falcon-ocr.wasm",
        "models/falcon-ocr/tokenizer.json",
        "models/falcon-ocr-int8-sharded/model.safetensors.index.json",
        "models/falcon-ocr-int8-sharded/config.json",
        "models/falcon-ocr-int8-sharded/scales.json",
        "models/falcon-ocr-int8-sharded/shard-00001-of-00002.safetensors",
    ]
    for result in payload["results"]:
        assert result["status"] == 200
        assert result["contentType"] == result["expectedType"]
        assert "immutable" in result["cacheControl"]
        assert int(result["contentLength"]) == len(result["expectedText"].encode())
        assert result["text"] == result["expectedText"]


def test_cloudflare_worker_int8_uses_single_sharded_prefix() -> None:
    source = (ROOT / "deploy" / "cloudflare" / "worker.js").read_text(
        encoding="utf-8"
    )

    assert 'const int8Prefix = "models/falcon-ocr-int8-sharded"' in source
    assert "`${int8Prefix}/model.safetensors.index.json`" in source
    assert "`${int8Prefix}/config.json`" in source
    assert "`${int8Prefix}/scales.json`" in source
    assert "`${int8Prefix}/${shardName}`" in source
    assert "models/falcon-ocr-int8/model.safetensors.index.json" not in source
    assert "`models/falcon-ocr-int8/${shardName}`" not in source


def test_cloudflare_worker_serves_browser_tokenizer_artifact() -> None:
    source = (ROOT / "deploy" / "cloudflare" / "worker.js").read_text(
        encoding="utf-8"
    )

    assert 'path === "/tokenizer.json"' in source
    assert '"models/falcon-ocr/tokenizer.json"' in source
    assert '"application/json"' in source


def test_cloudflare_worker_rate_limit_uses_durable_object_not_kv_counter() -> None:
    worker = (ROOT / "deploy" / "cloudflare" / "worker.js").read_text(
        encoding="utf-8"
    )
    wrangler = (ROOT / "deploy" / "cloudflare" / "wrangler.toml").read_text(
        encoding="utf-8"
    )

    assert "export class RateLimiter" in worker
    assert "env.RATE_LIMITER.idFromName" in worker
    assert "rateLimiter.fetch" in worker
    assert "env.CACHE.get(rateKey)" not in worker
    assert "env.CACHE.put(rateKey" not in worker
    assert '{ name = "RATE_LIMITER", class_name = "RateLimiter" }' in wrangler
    assert 'new_sqlite_classes = ["RateLimiter"]' in wrangler


def test_gpu_proxy_requires_supported_https_provider() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare GPU proxy tests")
    script = """
      const proxy = await import('./deploy/cloudflare/gpu-proxy.js');
      const states = [
        proxy.gpuInferenceStatus({}),
        proxy.gpuInferenceStatus({
          GPU_INFERENCE_URL: 'http://gpu.invalid/predict',
          GPU_INFERENCE_KEY: 'secret',
          GPU_INFERENCE_PROVIDER: 'runpod'
        }),
        proxy.gpuInferenceStatus({
          GPU_INFERENCE_URL: 'https://gpu.invalid/predict',
          GPU_INFERENCE_KEY: 'secret',
          GPU_INFERENCE_PROVIDER: 'unknown'
        }),
        proxy.gpuInferenceStatus({
          GPU_INFERENCE_URL: 'https://gpu.invalid/predict',
          GPU_INFERENCE_KEY: 'secret',
          GPU_INFERENCE_PROVIDER: 'runpod'
        }),
      ];
      process.stdout.write(JSON.stringify(states));
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    missing, http_url, bad_provider, ok = json.loads(run.stdout)
    assert missing["configured"] is False
    assert "required" in missing["error"]
    assert http_url["configured"] is False
    assert "https" in http_url["error"]
    assert bad_provider["configured"] is False
    assert "Unsupported" in bad_provider["error"]
    assert ok == {
        "configured": True,
        "provider": "runpod",
        "endpoint": "gpu.invalid",
    }


def test_cloudflare_worker_gpu_backend_route_uses_proxy_contract() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare Worker GPU route tests")
    script = """
      const workerModule = await import('./deploy/cloudflare/worker.js');
      const fetchCalls = [];
      globalThis.fetch = async (url, init) => {
        fetchCalls.push({
          url: String(url),
          method: init.method,
          auth: init.headers.Authorization,
          body: JSON.parse(init.body)
        });
        return new Response(JSON.stringify({ output: { text: 'GPU OCR' } }), {
          status: 200,
          headers: { 'content-type': 'application/json' }
        });
      };
      const env = {
        CORS_ORIGIN: 'https://freeinvoicemaker.app',
        GPU_INFERENCE_URL: 'https://gpu.example.invalid/run',
        GPU_INFERENCE_KEY: 'secret-token',
        GPU_INFERENCE_PROVIDER: 'runpod',
        RATE_LIMITER: {
          idFromName(name) { return name; },
          get(_id) { return { fetch: async () => new Response('{}', { status: 200 }) }; }
        }
      };
      const request = new Request('https://falcon-ocr.adpena.workers.dev/ocr', {
        method: 'POST',
        headers: {
          Origin: 'https://freeinvoicemaker.app',
          'Content-Type': 'application/json',
          'X-Use-Backend': 'gpu'
        },
        body: JSON.stringify({ image: 'abc123', max_tokens: 7, category: 'table' })
      });
      const response = await workerModule.default.fetch(request, env, { waitUntil() {} });
      process.stdout.write('\\nRESULT:' + JSON.stringify({
        status: response.status,
        payload: await response.json(),
        fetchCalls
      }) + '\\nENDRESULT\\n');
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    result = json.loads(run.stdout.split('RESULT:')[-1].split('ENDRESULT')[0].strip())
    assert result["status"] == 200
    assert result["payload"]["text"] == "GPU OCR"
    assert result["payload"]["backend"] == "gpu-runpod"
    assert result["fetchCalls"] == [
        {
            "url": "https://gpu.example.invalid/run",
            "method": "POST",
            "auth": "Bearer secret-token",
            "body": {
                "input": {
                    "image": "abc123",
                    "category": "table",
                    "max_tokens": 7,
                }
            },
        }
    ]


def test_cloudflare_worker_gpu_backend_rejects_unconfigured_proxy() -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare Worker GPU route tests")
    script = """
      const workerModule = await import('./deploy/cloudflare/worker.js');
      const env = {
        CORS_ORIGIN: 'https://freeinvoicemaker.app',
        RATE_LIMITER: {
          idFromName(name) { return name; },
          get(_id) { return { fetch: async () => new Response('{}', { status: 200 }) }; }
        }
      };
      const request = new Request('https://falcon-ocr.adpena.workers.dev/ocr', {
        method: 'POST',
        headers: {
          Origin: 'https://freeinvoicemaker.app',
          'Content-Type': 'application/json',
          'X-Use-Backend': 'gpu'
        },
        body: JSON.stringify({ image: 'abc123' })
      });
      const response = await workerModule.default.fetch(request, env, { waitUntil() {} });
      process.stdout.write('\\nRESULT:' + JSON.stringify({ status: response.status, payload: await response.json() }) + '\\nENDRESULT\\n');
    """
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    result = json.loads(run.stdout.split('RESULT:')[-1].split('ENDRESULT')[0].strip())
    assert result["status"] == 501
    assert result["payload"]["error"] == "GPU inference not configured"
    assert "required" in result["payload"]["detail"]


def test_enjoice_migration_doc_points_at_worker_wasm_and_tokenizer_artifacts() -> None:
    source = (ROOT / "docs" / "integration" / "enjoice-ocr-migration.md").read_text(
        encoding="utf-8"
    )

    assert "https://falcon-ocr.adpena.workers.dev/wasm/falcon-ocr.wasm" in source
    assert "/tokenizer.json" in source
    assert "https://falcon-ocr.freeinvoicemaker.workers.dev/falcon-ocr.wasm" not in source


def test_falcon_ocr_manifest_fixture_matches_browser_driver_boundary() -> None:
    manifest = json.loads(
        (
            ROOT / "tests" / "fixtures" / "falcon_ocr" / "driver-manifest.base.json"
        ).read_text(encoding="utf-8")
    )

    assert manifest["target"] == "falcon.browser_webgpu"
    assert manifest["artifacts"] == {
        "app_wasm": {"url": "/output.wasm"},
        "runtime_wasm": {"url": "/molt_runtime.wasm"},
        "config_json": {"url": "/config.json"},
        "tokenizer_json": {"url": "/tokenizer.json"},
    }
    assert manifest["weights"]["base_url"] == "https://weights.example.invalid/falcon"
    assert manifest["weights"]["files"] == [
        {"path": "config.json", "url": "config.json"},
        {"path": "model.safetensors", "url": "model.safetensors"},
    ]
    assert manifest["exports"] == {
        "init": "main_molt__init",
        "ocrTokens": "main_molt__ocr_tokens",
    }
