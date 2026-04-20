/**
 * Minimal BPE tokenizer decoder for Falcon-OCR.
 *
 * Only implements decode (token IDs -> text), not encode.
 * Parses the HuggingFace tokenizer.json format and builds an
 * inverted vocab map (id -> piece) for O(1) lookup.
 *
 * The tokenizer.json is loaded from R2 on model init and cached
 * for the lifetime of the Worker isolate.
 */

export const FALCON_OCR_CATEGORY_PROMPT_IDS = Object.freeze({
  plain: Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  text: Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  formula: Object.freeze([227, 46021, 790, 12211, 3463, 1211, 1112, 6883, 537, 709, 257]),
  table: Object.freeze([227, 46021, 790, 4336, 3463, 1211, 1112, 6883, 537, 709, 257]),
  caption: Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  footnote: Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  "list-item": Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  "page-footer": Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  "page-header": Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  "section-header": Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
  title: Object.freeze([227, 46021, 790, 2757, 3463, 1211, 1112, 6883, 537, 709, 257]),
});

export function buildFalconOcrPromptIds(category = "plain") {
  const key = String(category || "plain").trim().toLowerCase();
  const ids = FALCON_OCR_CATEGORY_PROMPT_IDS[key] || FALCON_OCR_CATEGORY_PROMPT_IDS.plain;
  return Array.from(ids);
}

/**
 * @typedef {Object} TokenizerConfig
 * @property {Map<number, string>} vocab - Token ID to text piece mapping
 * @property {Set<number>} specialIds - IDs of special tokens (to skip in output)
 */

export class TokenizerDecoder {
  /**
   * @param {Map<number, string>} vocab - Token ID to text piece mapping
   * @param {Set<number>} specialIds - IDs of special tokens
   */
  constructor(vocab, specialIds) {
    this.vocab = vocab;
    this.specialIds = specialIds;
  }

  /**
   * Parse a HuggingFace tokenizer.json and build the decoder.
   *
   * Handles the standard format with model.vocab (piece -> id) and
   * added_tokens array.  Special tokens (marked with special: true)
   * are tracked separately so they can be filtered from output.
   *
   * @param {string} tokenizerJson - Raw JSON string
   * @returns {TokenizerDecoder}
   */
  static fromJSON(tokenizerJson) {
    const data = JSON.parse(tokenizerJson);
    const vocab = new Map();
    const specialIds = new Set();

    // Primary vocab: model.vocab is { piece: id }
    if (data.model && data.model.vocab) {
      for (const [piece, id] of Object.entries(data.model.vocab)) {
        vocab.set(id, piece);
      }
    }

    // Added tokens override/extend the vocab.
    // Special tokens are tracked separately for filtering.
    if (data.added_tokens) {
      for (const token of data.added_tokens) {
        vocab.set(token.id, token.content);
        if (token.special) {
          specialIds.add(token.id);
        }
      }
    }

    return new TokenizerDecoder(vocab, specialIds);
  }

  /**
   * Decode an array of token IDs to text.
   *
   * Filters out special tokens (EOS, PAD, etc.) and applies
   * sentencepiece space marker replacement (U+2581 -> space).
   *
   * @param {number[]} tokenIds
   * @returns {string}
   */
  decode(tokenIds) {
    const pieces = [];
    for (let i = 0; i < tokenIds.length; i++) {
      const id = tokenIds[i];
      if (this.specialIds.has(id)) continue;
      const piece = this.vocab.get(id);
      if (piece !== undefined) {
        pieces.push(piece);
      } else {
        pieces.push(`[UNK:${id}]`);
      }
    }
    return pieces
      .join("")
      .replace(/\u2581/g, " ") // sentencepiece space marker -> space
      .trim();
  }

  /**
   * Decode with explicit special ID filtering.
   *
   * @param {number[]} tokenIds
   * @param {Set<number>} extraSpecialIds - Additional IDs to skip
   * @returns {string}
   */
  decodeSkipSpecial(tokenIds, extraSpecialIds = new Set()) {
    const combined = new Set([...this.specialIds, ...extraSpecialIds]);
    const filtered = tokenIds.filter((id) => !combined.has(id));
    // Use raw decode path (specialIds already filtered)
    const pieces = [];
    for (let i = 0; i < filtered.length; i++) {
      const piece = this.vocab.get(filtered[i]);
      if (piece !== undefined) {
        pieces.push(piece);
      } else {
        pieces.push(`[UNK:${filtered[i]}]`);
      }
    }
    return pieces
      .join("")
      .replace(/\u2581/g, " ")
      .trim();
  }
}
