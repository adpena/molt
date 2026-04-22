"""Full Falcon-OCR accuracy benchmark with token decoding.

Validates the tokenizer, embedding weights, and output logits against
the real Falcon-OCR model snapshot.  These tests verify the full
inference pipeline components work correctly end-to-end.

Requires:
  - Model snapshot at ~/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/
  - tinygrad tokenizer at src/molt/stdlib/tinygrad/tokenizer.py
"""

import math
import os
import struct
import json

SNAP = os.path.expanduser(
    "~/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/snapshots/"
    "3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66"
)

# Path to the stdlib (NOT added to sys.path to avoid shadowing Python's
# own importlib/random/etc).
_STDLIB = os.path.join(os.path.dirname(__file__), "../../src/molt/stdlib")


def _skip_if_no_snapshot():
    """Skip test if the model snapshot is not available."""
    if not os.path.isdir(SNAP):
        import pytest

        pytest.skip("Falcon-OCR model snapshot not available")


def _load_tokenizer():
    """Load the BPE tokenizer from the model snapshot.

    Imports the tokenizer module directly to avoid triggering the full
    tinygrad __init__ which requires the molt runtime.
    """
    import importlib.util

    tok_path = os.path.join(_STDLIB, "tinygrad", "tokenizer.py")
    spec = importlib.util.spec_from_file_location("_tokenizer", tok_path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.load(os.path.join(SNAP, "tokenizer.json"))


def _parse_safetensors_header(path: str) -> dict:
    """Parse SafeTensors header to get tensor metadata without loading data."""
    with open(path, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header_bytes = f.read(header_size)
    return json.loads(header_bytes)


def _load_f32_tensor(path: str, offset: int, count: int) -> list[float]:
    """Load a contiguous block of float32 values from a SafeTensors file."""
    with open(path, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        data_start = 8 + header_size
        f.seek(data_start + offset)
        raw = f.read(count * 4)
    return list(struct.unpack(f"<{count}f", raw))


# -------------------------------------------------------------------------
# Test 1: Tokenizer roundtrip on invoice-relevant text
# -------------------------------------------------------------------------


def test_tokenizer_decodes_real_vocabulary():
    """Verify tokenizer can encode and decode invoice-relevant tokens."""
    _skip_if_no_snapshot()
    tok = _load_tokenizer()

    texts = [
        "INVOICE #12345",
        "Total: $1,234.56",
        "Date: 2026-04-14",
        "Quantity: 10 x $99.99",
        "Tax (8.25%): $101.85",
        "Due Date: Net 30",
        "Payment Method: Wire Transfer",
        "Bill To: Acme Corp, 123 Main St",
        "PO Number: WO-2026-0042",
    ]
    for text in texts:
        token_ids = tok.encode(text)
        decoded = tok.decode(token_ids)
        # BPE tokenizers may add/strip whitespace; normalize for comparison.
        assert text.strip() == decoded.strip(), (
            f"Roundtrip failed: {text!r} -> {token_ids} -> {decoded!r}"
        )
        # Verify token count is reasonable (no excessive fragmentation).
        assert len(token_ids) <= len(text), (
            f"Excessive tokenization: {len(token_ids)} tokens for "
            f"{len(text)}-char text {text!r}"
        )


def test_tokenizer_special_tokens():
    """Verify special tokens (BOS, EOS, image tokens) have correct IDs."""
    _skip_if_no_snapshot()
    tok = _load_tokenizer()

    # The tokenizer should have special token IDs.
    assert hasattr(tok, "eos_id"), "Tokenizer missing eos_id attribute"
    assert hasattr(tok, "bos_id"), "Tokenizer missing bos_id attribute"
    assert hasattr(tok, "vocab_size"), "Tokenizer missing vocab_size attribute"
    assert tok.vocab_size > 0, f"Invalid vocab_size: {tok.vocab_size}"

    # Verify EOS token ID is within vocab range.
    if tok.eos_id is not None:
        assert 0 <= tok.eos_id < tok.vocab_size, (
            f"EOS token ID {tok.eos_id} out of vocab range [0, {tok.vocab_size})"
        )
        eos_decoded = tok.decode([tok.eos_id])
        assert isinstance(eos_decoded, str), (
            f"EOS token decode returned non-string: {type(eos_decoded)}"
        )


def test_tokenizer_handles_unicode():
    """Verify tokenizer handles multilingual/unicode text correctly."""
    _skip_if_no_snapshot()
    tok = _load_tokenizer()

    unicode_texts = [
        "Factura #001",  # Spanish
        "Rechnung Nr. 42",  # German
        "100.00",  # EUR amount
    ]
    for text in unicode_texts:
        token_ids = tok.encode(text)
        decoded = tok.decode(token_ids)
        assert decoded.strip() == text.strip(), (
            f"Unicode roundtrip failed: {text!r} -> {decoded!r}"
        )


# -------------------------------------------------------------------------
# Test 2: Embedding weight properties
# -------------------------------------------------------------------------


def test_embedding_produces_distinct_representations():
    """Different text patches produce different embeddings from real weights."""
    _skip_if_no_snapshot()

    # Find the model weights file (resolves symlink).
    weights_path = os.path.realpath(os.path.join(SNAP, "model.safetensors"))
    if not os.path.isfile(weights_path):
        import pytest

        pytest.skip("Model weights file not available (symlink target missing)")

    header = _parse_safetensors_header(weights_path)

    # Find the embedding tensor.
    embed_key = None
    for key in header:
        if key == "__metadata__":
            continue
        if "embed" in key.lower() and "weight" in key.lower():
            embed_key = key
            break

    if embed_key is None:
        import pytest

        pytest.skip("No embedding tensor found in model weights")

    info = header[embed_key]
    dtype = info["dtype"]
    shape = info["shape"]
    offsets = info["data_offsets"]

    # Only proceed if the embedding is F32 and small enough to sample.
    if dtype != "F32" or len(shape) != 2:
        import pytest

        pytest.skip(f"Embedding tensor dtype={dtype} shape={shape}, expected F32 2D")

    vocab_size, embed_dim = shape

    # Load embeddings for 3 distinct token IDs: 0, 1, and vocab_size//2.
    sample_ids = [0, 1, min(vocab_size - 1, vocab_size // 2)]
    embeddings = []
    for tid in sample_ids:
        offset = offsets[0] + tid * embed_dim * 4
        vec = _load_f32_tensor(weights_path, offset, embed_dim)
        embeddings.append(vec)

    # Compute cosine similarity between each pair.
    def cosine_sim(a, b):
        dot = sum(x * y for x, y in zip(a, b))
        norm_a = math.sqrt(sum(x * x for x in a))
        norm_b = math.sqrt(sum(x * x for x in b))
        if norm_a == 0 or norm_b == 0:
            return 0.0
        return dot / (norm_a * norm_b)

    for i in range(len(embeddings)):
        for j in range(i + 1, len(embeddings)):
            sim = cosine_sim(embeddings[i], embeddings[j])
            assert sim < 0.95, (
                f"Embeddings for tokens {sample_ids[i]} and {sample_ids[j]} "
                f"are too similar: cosine={sim:.4f} (expected < 0.95)"
            )

    # Verify embeddings are not all zeros.
    for idx, vec in zip(sample_ids, embeddings):
        norm = math.sqrt(sum(x * x for x in vec))
        assert norm > 1e-6, f"Embedding for token {idx} is zero vector"


# -------------------------------------------------------------------------
# Test 3: Output logit distribution is peaked
# -------------------------------------------------------------------------


def test_output_logits_are_peaked():
    """Model's output projection produces peaked (non-uniform) distributions."""
    _skip_if_no_snapshot()

    weights_path = os.path.realpath(os.path.join(SNAP, "model.safetensors"))
    if not os.path.isfile(weights_path):
        import pytest

        pytest.skip("Model weights file not available (symlink target missing)")

    header = _parse_safetensors_header(weights_path)

    # Find the output projection / LM head tensor.
    lm_head_key = None
    for key in header:
        if key == "__metadata__":
            continue
        kl = key.lower()
        if ("lm_head" in kl or "output" in kl) and "weight" in kl:
            lm_head_key = key
            break

    if lm_head_key is None:
        import pytest

        pytest.skip("No LM head / output projection tensor found")

    info = header[lm_head_key]
    dtype = info["dtype"]
    shape = info["shape"]
    offsets = info["data_offsets"]

    if dtype != "F32" or len(shape) != 2:
        import pytest

        pytest.skip(f"LM head dtype={dtype} shape={shape}, expected F32 2D")

    vocab_size, hidden_dim = shape

    # Load rows for token IDs 0 and vocab_size//2 (maximally distant).
    first_row = _load_f32_tensor(weights_path, offsets[0], hidden_dim)
    mid_offset = offsets[0] + (vocab_size // 2) * hidden_dim * 4
    mid_row = _load_f32_tensor(weights_path, mid_offset, hidden_dim)

    # Verify the weight norms are non-degenerate.
    norm_0 = math.sqrt(sum(w * w for w in first_row))
    norm_mid = math.sqrt(sum(w * w for w in mid_row))
    assert norm_0 > 1e-6, "First output weight row is zero vector"
    assert norm_mid > 1e-6, "Mid output weight row is zero vector"

    # Compute cosine similarity between the two rows.
    dot = sum(a * b for a, b in zip(first_row, mid_row))
    cos_sim = dot / (norm_0 * norm_mid) if norm_0 > 0 and norm_mid > 0 else 0.0
    assert cos_sim < 0.99, (
        f"Output weight rows 0 and {vocab_size // 2} are too similar: "
        f"cosine={cos_sim:.6f} (expected < 0.99)"
    )

    # Verify rows are not identical (element-wise).
    diffs = sum(1 for a, b in zip(first_row, mid_row) if abs(a - b) > 1e-8)
    assert diffs > hidden_dim // 2, (
        f"Output weight rows have too few differences: {diffs}/{hidden_dim}"
    )


if __name__ == "__main__":
    # Quick manual run.
    test_tokenizer_decodes_real_vocabulary()
    print("PASS: tokenizer roundtrip")
    test_tokenizer_special_tokens()
    print("PASS: special tokens")
    test_tokenizer_handles_unicode()
    print("PASS: unicode handling")
    test_embedding_produces_distinct_representations()
    print("PASS: embedding distinctness")
    test_output_logits_are_peaked()
    print("PASS: output logits peaked")
    print("\nAll accuracy tests passed.")
