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


def test_cloudflare_ocr_token_decode_requires_tokenizer() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        try {
          mod.decodeOcrTokenArray([1, 2, 3], null);
          process.stdout.write(JSON.stringify({ ok: true }));
        } catch (err) {
          process.stdout.write(JSON.stringify({ ok: false, message: err.message }));
        }
        """
    )

    assert result["ok"] is False
    assert "tokenizer" in result["message"].lower()


def test_cloudflare_ocr_token_decode_uses_tokenizer() -> None:
    result = _run_ocr_api_script(
        """
        const mod = await import('./deploy/cloudflare/ocr_api.js');
        const tokenizer = {
          decode(ids) {
            return ids.join(':');
          }
        };
        process.stdout.write(JSON.stringify({
          text: mod.decodeOcrTokenArray(new Int32Array([4, 5, 6]), tokenizer)
        }));
        """
    )

    assert result["text"] == "4:5:6"


def test_cloudflare_tokenizer_decode_preserves_edge_whitespace() -> None:
    result = _run_ocr_api_script(
        """
        const { TokenizerDecoder } = await import('./deploy/cloudflare/tokenizer.js');
        const tokenizer = new TokenizerDecoder(
          new Map([
            [1, '  lead'],
            [2, 'mid'],
            [3, 'trail  '],
            [4, '   '],
          ]),
          new Set()
        );
        process.stdout.write(JSON.stringify({
          leading: tokenizer.decode([1]),
          trailing: tokenizer.decode([3]),
          whitespace_only: tokenizer.decode([4]),
          combined: tokenizer.decode([1, 2, 3])
        }));
        """
    )

    assert result == {
        "leading": "  lead",
        "trailing": "trail  ",
        "whitespace_only": "   ",
        "combined": "  leadmidtrail  ",
    }
