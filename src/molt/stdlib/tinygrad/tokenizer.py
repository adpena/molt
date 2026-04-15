"""
BPE tokenizer for Falcon-OCR.

Loads the HuggingFace tokenizers-format tokenizer.json and provides
encode/decode methods for converting between text and token IDs.

This tokenizer implements:
  - Byte-level BPE (same as GPT-2/Llama/Falcon family)
  - Special token handling (BOS, EOS, PAD, image tokens, etc.)
  - Added token injection (524 special tokens for Falcon-OCR)

Public API:
    load(path: str) -> Tokenizer
    Tokenizer.encode(text: str) -> list[int]
    Tokenizer.decode(token_ids: list[int]) -> str
"""

from __future__ import annotations

import json
import os


# ---------------------------------------------------------------------------
# Byte-level BPE utilities
# ---------------------------------------------------------------------------

def _bytes_to_unicode() -> dict[int, str]:
    """Map byte values to unicode characters (GPT-2 byte-level BPE convention).

    Returns a mapping from byte value (0-255) to a unicode character.
    Printable ASCII chars map to themselves; non-printable bytes map to
    offset unicode characters starting at U+0100.
    """
    bs = (
        list(range(ord("!"), ord("~") + 1))
        + list(range(ord("\xa1"), ord("\xac") + 1))
        + list(range(ord("\xae"), ord("\xff") + 1))
    )
    cs = list(bs)
    n = 0
    for b in range(256):
        if b not in bs:
            bs.append(b)
            cs.append(256 + n)
            n += 1
    return {b: chr(c) for b, c in zip(bs, cs)}


_BYTE_ENCODER = _bytes_to_unicode()
_BYTE_DECODER = {v: k for k, v in _BYTE_ENCODER.items()}


class Tokenizer:
    """BPE tokenizer loaded from a HuggingFace tokenizer.json file.

    Supports encode (text -> token IDs) and decode (token IDs -> text)
    with proper handling of special tokens and byte-level BPE merges.
    """

    __slots__ = (
        "_vocab",          # str -> int: token string to ID
        "_vocab_inv",      # int -> str: ID to token string
        "_merges",         # list of (str, str) merge pairs, priority-ordered
        "_merge_rank",     # dict (str, str) -> int: merge pair to priority
        "_added_tokens",   # dict str -> int: special/added tokens
        "_added_ids",      # dict int -> str: reverse mapping
        "_eos_id",         # int: end of sequence token ID
        "_bos_id",         # int: beginning of sequence token ID (if any)
        "_pad_id",         # int: padding token ID
    )

    def __init__(
        self,
        vocab: dict[str, int],
        merges: list[tuple[str, str]],
        added_tokens: dict[str, int],
        eos_id: int = 11,
        bos_id: int = 17,
        pad_id: int = 0,
    ) -> None:
        self._vocab = vocab
        self._vocab_inv = {v: k for k, v in vocab.items()}
        self._merges = merges
        self._merge_rank = {pair: i for i, pair in enumerate(merges)}
        self._added_tokens = added_tokens
        self._added_ids = {v: k for k, v in added_tokens.items()}
        self._eos_id = eos_id
        self._bos_id = bos_id
        self._pad_id = pad_id

    @property
    def eos_id(self) -> int:
        return self._eos_id

    @property
    def bos_id(self) -> int:
        return self._bos_id

    @property
    def pad_id(self) -> int:
        return self._pad_id

    @property
    def vocab_size(self) -> int:
        return len(self._vocab)

    def _bpe(self, word: list[str]) -> list[str]:
        """Apply BPE merges to a list of characters/subwords until convergence."""
        if len(word) <= 1:
            return word

        while len(word) > 1:
            # Find the highest-priority (lowest-rank) merge pair
            best_pair = None
            best_rank = len(self._merges)

            for i in range(len(word) - 1):
                pair = (word[i], word[i + 1])
                rank = self._merge_rank.get(pair)
                if rank is not None and rank < best_rank:
                    best_rank = rank
                    best_pair = pair

            if best_pair is None:
                break

            # Apply the merge
            new_word: list[str] = []
            i = 0
            while i < len(word):
                if (
                    i < len(word) - 1
                    and word[i] == best_pair[0]
                    and word[i + 1] == best_pair[1]
                ):
                    new_word.append(best_pair[0] + best_pair[1])
                    i += 2
                else:
                    new_word.append(word[i])
                    i += 1
            word = new_word

        return word

    def encode(self, text: str) -> list[int]:
        """Encode text to a list of token IDs.

        Handles added/special tokens by scanning for them first, then
        applies byte-level BPE to the remaining text segments.
        """
        if not text:
            return []

        # Build sorted list of added tokens by length (longest first)
        # to ensure greedy matching
        sorted_added = sorted(
            self._added_tokens.keys(), key=len, reverse=True
        )

        # Split text on added tokens
        segments: list[tuple[str, bool]] = []  # (text, is_special)
        remaining = text
        while remaining:
            found = False
            for token_str in sorted_added:
                if remaining.startswith(token_str):
                    segments.append((token_str, True))
                    remaining = remaining[len(token_str):]
                    found = True
                    break
            if not found:
                # Find the next special token position
                next_pos = len(remaining)
                for token_str in sorted_added:
                    pos = remaining.find(token_str)
                    if pos != -1 and pos < next_pos:
                        next_pos = pos
                if next_pos > 0:
                    segments.append((remaining[:next_pos], False))
                    remaining = remaining[next_pos:]
                else:
                    segments.append((remaining, False))
                    remaining = ""

        # Encode each segment
        ids: list[int] = []
        for segment_text, is_special in segments:
            if is_special:
                ids.append(self._added_tokens[segment_text])
            else:
                ids.extend(self._encode_ordinary(segment_text))

        return ids

    def _encode_ordinary(self, text: str) -> list[int]:
        """Encode ordinary (non-special) text using byte-level BPE."""
        if not text:
            return []

        # Convert text to byte-level unicode representation
        encoded_chars = [_BYTE_ENCODER[b] for b in text.encode("utf-8")]

        # Apply BPE merges
        bpe_tokens = self._bpe(encoded_chars)

        # Look up token IDs
        ids: list[int] = []
        for token in bpe_tokens:
            token_id = self._vocab.get(token)
            if token_id is not None:
                ids.append(token_id)
            else:
                # Unknown subword -- encode each character individually
                for ch in token:
                    ch_id = self._vocab.get(ch)
                    if ch_id is not None:
                        ids.append(ch_id)
                    # If still not found, skip (should not happen with
                    # byte-level BPE since all 256 bytes are in vocab)

        return ids

    def decode(self, token_ids: list[int]) -> str:
        """Decode a list of token IDs back to text.

        Handles special tokens by inserting their string representation.
        Regular BPE tokens are decoded via byte-level unicode mapping.
        """
        if not token_ids:
            return ""

        parts: list[str] = []
        for token_id in token_ids:
            # Check added tokens first
            if token_id in self._added_ids:
                parts.append(self._added_ids[token_id])
            elif token_id in self._vocab_inv:
                parts.append(self._vocab_inv[token_id])
            # else: unknown token, skip

        # Join all token strings
        text = "".join(parts)

        # Decode byte-level BPE: convert unicode chars back to bytes
        byte_list = bytearray()
        for ch in text:
            if ch in _BYTE_DECODER:
                byte_list.append(_BYTE_DECODER[ch])
            else:
                # Special token characters pass through as UTF-8
                byte_list.extend(ch.encode("utf-8"))

        return byte_list.decode("utf-8", errors="replace")

    def decode_skip_special(self, token_ids: list[int]) -> str:
        """Decode token IDs, skipping all special/added tokens."""
        filtered = [
            tid for tid in token_ids
            if tid not in self._added_ids
        ]
        return self.decode(filtered)


def load(path: str) -> Tokenizer:
    """Load a tokenizer from a HuggingFace tokenizer.json file.

    Args:
        path: Path to tokenizer.json (or directory containing it).

    Returns:
        Initialized Tokenizer instance.
    """
    if os.path.isdir(path):
        path = os.path.join(path, "tokenizer.json")

    with open(path) as f:
        data = json.load(f)

    # Extract vocab
    model = data["model"]
    assert model["type"] == "BPE", f"Expected BPE tokenizer, got {model['type']}"
    vocab: dict[str, int] = model["vocab"]

    # Extract merges: either list of "token1 token2" strings or list of [token1, token2] pairs
    merges: list[tuple[str, str]] = []
    for merge_entry in model["merges"]:
        if isinstance(merge_entry, str):
            parts = merge_entry.split(" ", 1)
            assert len(parts) == 2, f"Invalid merge format: {merge_entry!r}"
            merges.append((parts[0], parts[1]))
        elif isinstance(merge_entry, list):
            assert len(merge_entry) == 2, f"Invalid merge pair: {merge_entry!r}"
            merges.append((merge_entry[0], merge_entry[1]))
        else:
            raise ValueError(f"Unexpected merge format: {type(merge_entry)}")

    # Extract added tokens
    added_tokens: dict[str, int] = {}
    for token_info in data.get("added_tokens", []):
        added_tokens[token_info["content"]] = token_info["id"]

    # Determine special token IDs from added tokens
    eos_id = added_tokens.get("<|end_of_text|>", 11)
    bos_id = added_tokens.get("<|begin_of_text|>", 17)
    pad_id = added_tokens.get("<|pad|>", 0)

    return Tokenizer(
        vocab=vocab,
        merges=merges,
        added_tokens=added_tokens,
        eos_id=eos_id,
        bos_id=bos_id,
        pad_id=pad_id,
    )
