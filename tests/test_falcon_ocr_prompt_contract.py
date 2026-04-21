from __future__ import annotations

import json
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
TOKENIZER_PATH = (
    Path("/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights")
    / "tokenizer.json"
)

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
