from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _run_ai_fallback_script(script: str) -> dict[str, object]:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare AI fallback contract tests")
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout)


def test_workers_ai_ocr_preserves_edge_whitespace() -> None:
    result = _run_ai_fallback_script(
        """
        const mod = await import('./deploy/cloudflare/ai-fallback.js');
        const env = {
          AI: {
            async run() {
              return { response: '  Invoice 42  ' };
            }
          }
        };
        const result = await mod.runWorkersAiOcr(env, new Uint8Array([1, 2, 3]));
        process.stdout.write(JSON.stringify({ text: result.text }));
        """
    )

    assert result["text"] == "  Invoice 42  "


def test_workers_ai_ocr_rejects_whitespace_only_text() -> None:
    result = _run_ai_fallback_script(
        """
        const mod = await import('./deploy/cloudflare/ai-fallback.js');
        const calls = [];
        const env = {
          AI: {
            async run(modelId) {
              calls.push(modelId);
              return { response: '   ' };
            }
          }
        };
        try {
          await mod.runWorkersAiOcr(env, new Uint8Array([1, 2, 3]));
          process.stdout.write(JSON.stringify({ ok: true, calls }));
        } catch (err) {
          process.stdout.write(JSON.stringify({
            ok: false,
            calls,
            message: err.message
          }));
        }
        """
    )

    assert result["ok"] is False
    assert result["calls"]
    assert "empty response" in result["message"]
