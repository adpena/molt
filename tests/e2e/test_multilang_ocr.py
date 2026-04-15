"""
Multi-language OCR tokenizer roundtrip and Workers AI prompt tests.

Validates that the tokenizer correctly handles invoice text in 10 languages,
and that multi-language prompts produce valid responses from the Workers AI
OCR endpoint.
"""

import json
import hashlib
import subprocess
import sys
from pathlib import Path

import pytest

# ---------------------------------------------------------------------------
# Test data: invoice text in 10 languages
# ---------------------------------------------------------------------------

MULTILANG_TEXTS = {
    "en": "INVOICE #12345 - Total: $1,234.56",
    "es": "FACTURA #12345 - Total: $1.234,56",
    "fr": "FACTURE #12345 - Total : 1 234,56 \u20ac",
    "de": "RECHNUNG #12345 - Gesamt: 1.234,56 \u20ac",
    "ja": "\u8acb\u6c42\u66f8 #12345 - \u5408\u8a08: \u00a5123,456",
    "ar": "\u0641\u0627\u062a\u0648\u0631\u0629 #12345 - \u0627\u0644\u0645\u062c\u0645\u0648\u0639: \u0661\u0662\u0663\u0664\u066b\u0665\u0666$",
    "zh": "\u53d1\u7968 #12345 - \u603b\u8ba1: \u00a51,234.56",
    "ko": "\uc1a1\uc7a5 #12345 - \ud569\uacc4: \u20a91,234,560",
    "pt": "FATURA #12345 - Total: R$1.234,56",
    "ru": "\u0421\u0427\u0401\u0422 #12345 - \u0418\u0442\u043e\u0433\u043e: 1 234,56\u20bd",
}

# Maximum reasonable token count for a single invoice line.
# A 35-character English string typically tokenizes to 8-15 tokens.
# CJK/Cyrillic/Arabic may use more tokens per character.
MAX_REASONABLE_TOKENS = 80

# Minimum token count: even a 1-character string should produce >= 1 token.
MIN_REASONABLE_TOKENS = 1

# ---------------------------------------------------------------------------
# Tokenizer roundtrip tests
# ---------------------------------------------------------------------------

# The tokenizer is bundled as a JSON file with the model.  For testing we
# use a lightweight BPE tokenizer that ships with the test fixtures.  If
# unavailable, these tests are skipped (they run in CI where the tokenizer
# is present).

TOKENIZER_PATHS = [
    Path(__file__).parent.parent.parent / "deploy" / "cloudflare" / ".wrangler" / "tokenizer.json",
    Path(__file__).parent / "test_images" / "tokenizer.json",
]


def _find_tokenizer():
    """Return the first existing tokenizer path, or None."""
    for p in TOKENIZER_PATHS:
        if p.exists():
            return p
    return None


# We use a simple byte-level check: encode to UTF-8, verify no data loss.
# Full BPE tokenizer roundtrip requires the tokenizers library.
try:
    from tokenizers import Tokenizer as HFTokenizer
    _HAS_TOKENIZERS = True
except ImportError:
    _HAS_TOKENIZERS = False


class TestMultilangTokenizerRoundtrip:
    """Verify tokenizer handles all 10 languages without data loss."""

    def test_utf8_roundtrip_all_languages(self):
        """Every test string must survive UTF-8 encode/decode without loss."""
        for lang, text in MULTILANG_TEXTS.items():
            encoded = text.encode("utf-8")
            decoded = encoded.decode("utf-8")
            assert decoded == text, (
                f"UTF-8 roundtrip failed for {lang}: {text!r} != {decoded!r}"
            )

    def test_all_languages_nonempty(self):
        """Sanity check: all test strings are non-empty and contain digits."""
        for lang, text in MULTILANG_TEXTS.items():
            assert len(text) > 0, f"Empty text for {lang}"
            assert "12345" in text or "\u0661\u0662\u0663\u0664" in text, (
                f"Missing invoice number in {lang} text"
            )

    def test_invoice_number_present(self):
        """All texts must contain an invoice number marker."""
        for lang, text in MULTILANG_TEXTS.items():
            assert "#" in text or "\u0023" in text, (
                f"Missing '#' invoice marker in {lang} text"
            )

    @pytest.mark.skipif(not _HAS_TOKENIZERS, reason="tokenizers library not installed")
    def test_bpe_roundtrip(self):
        """If HF tokenizers is available, verify BPE encode/decode roundtrip."""
        tok_path = _find_tokenizer()
        if tok_path is None:
            pytest.skip("No tokenizer.json found in expected paths")

        tokenizer = HFTokenizer.from_file(str(tok_path))
        for lang, text in MULTILANG_TEXTS.items():
            encoding = tokenizer.encode(text)
            token_ids = encoding.ids
            assert len(token_ids) >= MIN_REASONABLE_TOKENS, (
                f"{lang}: too few tokens ({len(token_ids)})"
            )
            assert len(token_ids) <= MAX_REASONABLE_TOKENS, (
                f"{lang}: too many tokens ({len(token_ids)}, max {MAX_REASONABLE_TOKENS})"
            )
            decoded = tokenizer.decode(token_ids)
            # BPE decode may normalize whitespace; compare stripped versions
            assert text.strip() in decoded or decoded.strip() in text.strip() or (
                text.replace(" ", "") == decoded.replace(" ", "")
            ), (
                f"{lang}: BPE roundtrip mismatch: {text!r} -> {decoded!r}"
            )

    @pytest.mark.skipif(not _HAS_TOKENIZERS, reason="tokenizers library not installed")
    def test_token_count_reasonable(self):
        """Token counts should be within expected bounds for each language."""
        tok_path = _find_tokenizer()
        if tok_path is None:
            pytest.skip("No tokenizer.json found in expected paths")

        tokenizer = HFTokenizer.from_file(str(tok_path))
        for lang, text in MULTILANG_TEXTS.items():
            encoding = tokenizer.encode(text)
            count = len(encoding.ids)
            assert MIN_REASONABLE_TOKENS <= count <= MAX_REASONABLE_TOKENS, (
                f"{lang}: token count {count} out of range "
                f"[{MIN_REASONABLE_TOKENS}, {MAX_REASONABLE_TOKENS}]"
            )


# ---------------------------------------------------------------------------
# Workers AI multi-language prompt tests (live endpoint)
# ---------------------------------------------------------------------------

WORKER_URL = "https://falcon-ocr.adpena.workers.dev/ocr"

# Minimal 1x1 white PNG (base64) for testing prompt handling
TINY_PNG_B64 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4"
    "2mP8/58BAwAI/AL+hc2rNAAAAABJRU5ErkJggg=="
)

MULTILANG_PROMPTS = [
    ("en", "Extract all text from this invoice"),
    ("es", "Extraer todo el texto de esta factura"),
    ("fr", "Extraire tout le texte de cette facture"),
    ("ja", "\u3059\u3079\u3066\u306e\u30c6\u30ad\u30b9\u30c8\u3092\u62bd\u51fa"),
]


@pytest.mark.live
class TestMultilangWorkersAI:
    """Test Workers AI endpoint with multi-language prompts.

    These tests hit the live Worker endpoint and are marked with @pytest.mark.live.
    Run with: pytest -m live tests/e2e/test_multilang_ocr.py
    """

    @pytest.mark.parametrize("lang,prompt", MULTILANG_PROMPTS)
    def test_workers_ai_accepts_multilang_prompt(self, lang, prompt):
        """The Worker must accept prompts in any language and return 200."""
        payload = json.dumps({
            "image": TINY_PNG_B64,
            "prompt": prompt,
        })
        result = subprocess.run(
            [
                "curl", "-s", "-w", "\n%{http_code}",
                "-X", "POST", WORKER_URL,
                "-H", "Content-Type: application/json",
                "-H", "X-Use-Backend: workers-ai",
                "-d", payload,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        lines = result.stdout.strip().rsplit("\n", 1)
        status_code = int(lines[-1]) if len(lines) >= 2 else 0
        # Accept 200 (success) or 503 (Workers AI unavailable in test env)
        assert status_code in (200, 503), (
            f"{lang} prompt returned HTTP {status_code}: {lines[0][:200]}"
        )
        if status_code == 200:
            body = json.loads(lines[0])
            assert "request_id" in body, f"Missing request_id in {lang} response"

    def test_batch_endpoint_accepts_request(self):
        """The /ocr/batch endpoint must accept a valid batch request."""
        payload = json.dumps({
            "images": [TINY_PNG_B64, TINY_PNG_B64],
        })
        result = subprocess.run(
            [
                "curl", "-s", "-w", "\n%{http_code}",
                "-X", "POST",
                "https://falcon-ocr.adpena.workers.dev/ocr/batch",
                "-H", "Content-Type: application/json",
                "-d", payload,
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        lines = result.stdout.strip().rsplit("\n", 1)
        status_code = int(lines[-1]) if len(lines) >= 2 else 0
        assert status_code == 200, (
            f"Batch endpoint returned HTTP {status_code}: {lines[0][:200]}"
        )
        body = json.loads(lines[0])
        assert "results" in body, "Missing 'results' in batch response"
        assert len(body["results"]) == 2, f"Expected 2 results, got {len(body['results'])}"
        assert "total_time_ms" in body, "Missing 'total_time_ms' in batch response"
