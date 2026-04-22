from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _run_queue_script(script: str) -> dict[str, object]:
    if shutil.which("node") is None:
        pytest.skip("node is required for Cloudflare queue OCR contract tests")
    run = subprocess.run(
        ["node", "--input-type=module", "-e", script],
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(run.stdout)


def test_cloudflare_queue_batch_ocr_rejects_blank_ai_output() -> None:
    result = _run_queue_script(
        """
        const mod = await import('./deploy/cloudflare/queue-batch-ocr.js');
        const calls = { put: [], ack: 0, retry: 0, incremented: [] };
        const env = {
          AI: {
            async run() {
              return { response: '   ' };
            }
          },
          CACHE: {
            async put(key, value) {
              calls.put.push([key, JSON.parse(value)]);
            },
            async get() {
              return JSON.stringify({ count: 1, completed: 0, failed: 0, status: 'processing' });
            }
          }
        };
        const batch = {
          messages: [
            {
              body: { batchId: 'batch-1', index: 0, imageData: 'img' },
              ack() { calls.ack += 1; },
              retry() { calls.retry += 1; },
            }
          ]
        };
        try {
          await mod.processQueueBatch(batch, env);
        } catch (err) {
          calls.error = err.message;
        }
        process.stdout.write(JSON.stringify(calls));
        """
    )

    assert result["ack"] == 0
    assert result["retry"] == 1
    assert [key for key, _value in result["put"]] == ["batch:batch-1:meta"]
    assert result["put"][0][1]["failed"] == 1
    assert result["put"][0][1]["completed"] == 0
