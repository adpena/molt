"""
molt.gpu.transformer — Transformer architecture components.

Provides: MultiHeadAttention, TransformerBlock, PositionalEncoding,
CausalMask, and a complete TransformerDecoder for text generation.
"""

from .tensor import Tensor, tensor_scaled_dot_product_attention
from .nn import Linear, LayerNorm, Dropout, Sequential, GELU
import math


class MultiHeadAttention:
    """Multi-head scaled dot-product attention.

    Q, K, V projections + output projection.
    Supports causal masking for autoregressive generation.
    """
    def __init__(self, embed_dim, num_heads, causal=False):
        assert embed_dim % num_heads == 0
        self.embed_dim = embed_dim
        self.num_heads = num_heads
        self.head_dim = embed_dim // num_heads
        self.causal = causal
        self.scale = 1.0 / math.sqrt(self.head_dim)

        self.q_proj = Linear(embed_dim, embed_dim, bias=False)
        self.k_proj = Linear(embed_dim, embed_dim, bias=False)
        self.v_proj = Linear(embed_dim, embed_dim, bias=False)
        self.out_proj = Linear(embed_dim, embed_dim, bias=False)

    def __call__(self, x: Tensor, mask=None, kv_cache=None) -> Tensor:
        squeezed = False
        if x.ndim == 2:
            x = x.reshape(1, *x.shape)
            squeezed = True
        if x.ndim != 3:
            raise ValueError(f"attention input must be 2D or 3D, got {x.shape}")

        batch, seq_len, _ = x.shape
        q = self.q_proj(x).reshape(batch, seq_len, self.num_heads, self.head_dim).permute(0, 2, 1, 3)
        k = self.k_proj(x).reshape(batch, seq_len, self.num_heads, self.head_dim).permute(0, 2, 1, 3)
        v = self.v_proj(x).reshape(batch, seq_len, self.num_heads, self.head_dim).permute(0, 2, 1, 3)

        attn_mask = mask
        if kv_cache is not None:
            prefix_len = len(kv_cache)
            kv_cache.append(k, v)
            try:
                if self.causal and attn_mask is None:
                    attn_mask = _causal_cache_attention_mask(seq_len, len(kv_cache), prefix_len)
                elif (
                    attn_mask is not None
                    and isinstance(attn_mask, Tensor)
                    and attn_mask.ndim == 2
                ):
                    attn_mask = attn_mask.reshape(1, 1, attn_mask.shape[0], attn_mask.shape[1])
                out = kv_cache.attention(q, scale=self.scale, mask=attn_mask)
            except Exception:
                truncate = getattr(kv_cache, "truncate", None)
                if callable(truncate):
                    truncate(prefix_len)
                raise
        else:
            if self.causal and attn_mask is None:
                attn_mask = _causal_attention_mask(seq_len)
            elif attn_mask is not None and isinstance(attn_mask, Tensor) and attn_mask.ndim == 2:
                attn_mask = attn_mask.reshape(1, 1, seq_len, seq_len)

            out = tensor_scaled_dot_product_attention(q, k, v, attn_mask, self.scale)
        out = out.permute(0, 2, 1, 3).reshape(batch, seq_len, self.embed_dim)
        out = self.out_proj(out)
        if squeezed:
            out = out.reshape(seq_len, self.embed_dim)
        return out

    def load_weights(self, prefix, weights):
        self.q_proj.load_weights(
            weights.get(f"{prefix}.q_proj.weight"),
            weights.get(f"{prefix}.q_proj.bias"))
        self.k_proj.load_weights(
            weights.get(f"{prefix}.k_proj.weight"),
            weights.get(f"{prefix}.k_proj.bias"))
        self.v_proj.load_weights(
            weights.get(f"{prefix}.v_proj.weight"),
            weights.get(f"{prefix}.v_proj.bias"))
        self.out_proj.load_weights(
            weights.get(f"{prefix}.out_proj.weight"),
            weights.get(f"{prefix}.out_proj.bias"))


def apply_causal_mask(scores: Tensor, seq_len: int) -> Tensor:
    """Apply causal (lower-triangular) mask to attention scores."""
    data = scores.to_list()
    if isinstance(data, list) and data and isinstance(data[0], list):
        for i in range(min(seq_len, len(data))):
            for j in range(min(seq_len, len(data[i]))):
                if j > i:
                    data[i][j] = float('-inf')
        # Flatten nested list for Tensor constructor
        flat = []
        for row in data:
            flat.extend(row)
        return Tensor(flat, shape=scores.shape)
    else:
        return scores


def _causal_attention_mask(seq_len: int) -> Tensor:
    flat = []
    for i in range(seq_len):
        for j in range(seq_len):
            flat.append(0.0 if j <= i else float("-inf"))
    return Tensor(flat, shape=(1, 1, seq_len, seq_len))


def _causal_cache_attention_mask(query_len: int, total_len: int, prefix_len: int) -> Tensor:
    flat = []
    for query_index in range(query_len):
        allowed = prefix_len + query_index
        for key_index in range(total_len):
            flat.append(0.0 if key_index <= allowed else float("-inf"))
    return Tensor(flat, shape=(1, 1, query_len, total_len))


class TransformerBlock:
    """Single transformer decoder block: attention + FFN with residual connections."""

    def __init__(self, embed_dim, num_heads, ff_dim=None, causal=True):
        ff_dim = ff_dim or embed_dim * 4
        self.attention = MultiHeadAttention(embed_dim, num_heads, causal=causal)
        self.ln1 = LayerNorm(embed_dim)
        self.ln2 = LayerNorm(embed_dim)
        self.ff = Sequential(
            Linear(embed_dim, ff_dim),
            GELU(),
            Linear(ff_dim, embed_dim),
        )

    def __call__(self, x: Tensor, kv_cache=None) -> Tensor:
        # Pre-norm architecture (GPT-2 style)
        attn_out = self.attention(self.ln1(x), kv_cache=kv_cache)
        x = x + attn_out  # residual
        ff_out = self.ff(self.ln2(x))
        x = x + ff_out  # residual
        return x


class TransformerDecoder:
    """Complete transformer decoder for text generation.

    Usage:
        model = TransformerDecoder(vocab_size=50257, embed_dim=768,
                                   num_heads=12, num_layers=12)
        logits = model(token_ids)  # (seq_len, vocab_size)
    """

    def __init__(self, vocab_size, embed_dim, num_heads, num_layers, max_seq_len=2048):
        self.embed_dim = embed_dim
        self.token_embedding = Embedding(vocab_size, embed_dim)
        self.position_embedding = Embedding(max_seq_len, embed_dim)
        self.blocks = [TransformerBlock(embed_dim, num_heads) for _ in range(num_layers)]
        self.ln_final = LayerNorm(embed_dim)
        self.lm_head = Linear(embed_dim, vocab_size, bias=False)

    def __call__(self, token_ids, *, kv_caches=None) -> Tensor:
        seq_len = len(token_ids) if isinstance(token_ids, list) else token_ids.shape[-1]
        prefix_len = 0
        if kv_caches is not None:
            if len(kv_caches) != len(self.blocks):
                raise ValueError("kv_caches length must match number of transformer blocks")
            if kv_caches:
                prefix_len = len(kv_caches[0])
                for cache in kv_caches[1:]:
                    if len(cache) != prefix_len:
                        raise ValueError("all kv_caches must have the same prefix length")

        # Token + position embeddings
        tok_emb = self.token_embedding(token_ids)
        pos_ids = list(range(prefix_len, prefix_len + seq_len))
        pos_emb = self.position_embedding(pos_ids)
        x = tok_emb + pos_emb

        # Transformer blocks
        for index, block in enumerate(self.blocks):
            block_cache = None if kv_caches is None else kv_caches[index]
            x = block(x, kv_cache=block_cache)

        x = self.ln_final(x)
        logits = self.lm_head(x)
        return logits


class Embedding:
    """Lookup table embedding wrapping nn.Embedding.

    Accepts raw lists or Tensors as indices, converting as needed.
    """
    def __init__(self, num_embeddings, embedding_dim):
        from .nn import Embedding as _Emb
        self._emb = _Emb(num_embeddings, embedding_dim)

    def __call__(self, indices):
        if not isinstance(indices, Tensor):
            indices = Tensor(indices)
        return self._emb(indices)

    def load_weights(self, weight):
        self._emb.load_weights(weight)
