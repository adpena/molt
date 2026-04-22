from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
WORKER_TS = ROOT / "drivers" / "cloudflare" / "thin_adapter" / "worker.ts"


def _run_worker_script(script_text: str, cwd: Path) -> subprocess.CompletedProcess[str]:
    if shutil.which("node") is None:
        pytest.skip("node is required for cloudflare worker driver tests")
    script = cwd / "run_worker.mjs"
    script.write_text(script_text, encoding="utf-8")
    return subprocess.run(
        ["node", "--experimental-strip-types", str(script)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_thin_adapter_worker_serves_manifest_route(tmp_path: Path) -> None:
    result = _run_worker_script(
        f"""
import {{ createThinAssetWorker }} from {WORKER_TS.as_uri()!r};

const worker = createThinAssetWorker({{ target: "falcon.browser_webgpu" }});
const response = await worker.fetch(
  new Request("https://example.invalid/driver-manifest.json"),
  {{
    ASSETS: {{
      async fetch() {{
        return new Response(
          JSON.stringify({{
            version: 1,
            target: "falcon.browser_webgpu",
            artifacts: {{
              app_wasm: {{ url: "/app.wasm" }},
              runtime_wasm: {{ url: "/molt_runtime.wasm" }},
              config_json: {{ url: "/config.json" }},
              browser_loader: {{ url: "/browser.js" }},
            }},
            weights: {{
              base_url: null,
              files: [{{ path: "weights.bin", url: "weights.bin" }}],
            }},
          }}),
          {{ headers: {{ "content-type": "application/json" }} }},
        );
      }},
    }},
    WEIGHTS_BASE_URL: "https://weights.example.invalid/falcon",
  }},
);
console.log(await response.text());
""".lstrip(),
        tmp_path,
    )
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload["target"] == "falcon.browser_webgpu"
    assert payload["weights"]["base_url"] == "https://weights.example.invalid/falcon"


def test_thin_adapter_worker_passthroughs_non_manifest_assets(tmp_path: Path) -> None:
    result = _run_worker_script(
        f"""
import {{ createThinAssetWorker }} from {WORKER_TS.as_uri()!r};

const worker = createThinAssetWorker({{ target: "falcon.browser_webgpu" }});
const response = await worker.fetch(
  new Request("https://example.invalid/app.wasm"),
  {{
    ASSETS: {{
      async fetch(request) {{
        return new Response(`asset:${{new URL(request.url).pathname}}`);
      }},
    }},
    WEIGHTS_BASE_URL: "https://weights.example.invalid/falcon",
  }},
);
console.log(await response.text());
""".lstrip(),
        tmp_path,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "asset:/app.wasm"


def test_thin_adapter_worker_uses_same_origin_weights_when_binding_present(
    tmp_path: Path,
) -> None:
    result = _run_worker_script(
        f"""
import {{ createThinAssetWorker }} from {WORKER_TS.as_uri()!r};

const worker = createThinAssetWorker({{ target: "falcon.browser_webgpu" }});
const response = await worker.fetch(
  new Request("https://falcon.freeinvoicemaker.app/driver-manifest.json"),
  {{
    ASSETS: {{
      async fetch() {{
        return new Response(
          JSON.stringify({{
            version: 1,
            target: "falcon.browser_webgpu",
            artifacts: {{
              app_wasm: {{ url: "/app.wasm" }},
              runtime_wasm: {{ url: "/molt_runtime.wasm" }},
              config_json: {{ url: "/config.json" }},
              browser_loader: {{ url: "/browser.js" }},
            }},
            weights: {{
              base_url: null,
              files: [{{ path: "model.safetensors", url: "model.safetensors" }}],
            }},
          }}),
          {{ headers: {{ "content-type": "application/json" }} }},
        );
      }},
    }},
    WEIGHTS: {{
      async get() {{
        return null;
      }},
    }},
  }},
);
console.log(await response.text());
""".lstrip(),
        tmp_path,
    )
    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert (
        payload["weights"]["base_url"] == "https://falcon.freeinvoicemaker.app/weights"
    )


def test_thin_adapter_worker_serves_weight_assets_from_r2_binding(
    tmp_path: Path,
) -> None:
    result = _run_worker_script(
        f"""
import {{ createThinAssetWorker }} from {WORKER_TS.as_uri()!r};

const worker = createThinAssetWorker({{ target: "falcon.browser_webgpu" }});
const response = await worker.fetch(
  new Request("https://falcon.freeinvoicemaker.app/weights/model.safetensors"),
  {{
    ASSETS: {{
      async fetch() {{
        return new Response("unexpected");
      }},
    }},
    WEIGHTS: {{
      async get(key) {{
        if (key !== "model.safetensors") return null;
        return {{
          body: new ReadableStream({{
            start(controller) {{
              controller.enqueue(new TextEncoder().encode("weights"));
              controller.close();
            }},
          }}),
          httpMetadata: {{
            contentType: "application/octet-stream",
            cacheControl: "public, max-age=31536000, immutable",
          }},
          size: 7,
          etag: "abc123",
        }};
      }},
    }},
  }},
);
console.log(response.status);
console.log(response.headers.get("content-type"));
console.log(response.headers.get("cache-control"));
console.log(response.headers.get("etag"));
console.log(await response.text());
""".lstrip(),
        tmp_path,
    )
    assert result.returncode == 0, result.stderr
    lines = result.stdout.strip().splitlines()
    assert lines == [
        "200",
        "application/octet-stream",
        "public, max-age=31536000, immutable",
        '"abc123"',
        "weights",
    ]
