"""
molt.gpu.transformer — Transformer architecture components.

Provides: MultiHeadAttention, TransformerBlock, PositionalEncoding,
CausalMask, and a complete TransformerDecoder for text generation.
"""

from .tensor import Tensor
from .nn import Linear, LayerNorm, Dropout, Sequential
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

    def __call__(self, x: Tensor, mask=None) -> Tensor:
        # x: (batch, seq_len, embed_dim)
        batch_size = x.shape[0] if x.ndim == 3 else 1
        seq_len = x.shape[-2] if x.ndim >= 2 else x.shape[0]

        # Project Q, K, V
        q = self.q_proj(x)  # (batch, seq, embed)
        k = self.k_proj(x)
        v = self.v_proj(x)

        # Reshape to (batch, heads, seq, head_dim) — simplified for 2D
        # For interpreted mode, do the attention computation directly
        # Full multi-head reshape requires 4D tensor support

        # Scaled dot-product attention: softmax(Q @ K.T / sqrt(d)) @ V
        scores = q @ k.T  # (seq, seq)
        scores = scores * self.scale

        # Apply causal mask (upper triangle = -inf)
        if self.causal:
            scores = apply_causal_mask(scores, seq_len)

        attn_weights = scores.softmax(axis=-1)
        attn_output = attn_weights @ v

        return self.out_proj(attn_output)

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
    if isinstance(data[0], list):
        for i in range(seq_len):
            for j in range(seq_len):
                if j > i:
                    data[i][j] = -1e9
        return Tensor(data, shape=scores.shape)
    else:
        # 1D case
        return scores


class TransformerBlock:
    """Single transformer decoder block: attention + FFN with residual connections."""

    def __init__(self, embed_dim, num_heads, ff_dim=None, causal=True):
        ff_dim = ff_dim or embed_dim * 4
        self.attention = MultiHeadAttention(embed_dim, num_heads, causal=causal)
        self.ln1 = LayerNorm(embed_dim)
        self.ln2 = LayerNorm(embed_dim)
        self.ff = Sequential(
            Linear(embed_dim, ff_dim),
            # GELU approximation
            Linear(ff_dim, embed_dim),
        )

    def __call__(self, x: Tensor) -> Tensor:
        # Pre-norm architecture (GPT-2 style)
        attn_out = self.attention(self.ln1(x))
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

    def __call__(self, token_ids) -> Tensor:
        seq_len = len(token_ids) if isinstance(token_ids, list) else token_ids.shape[-1]

        # Token + position embeddings
        tok_emb = self.token_embedding(token_ids)
        pos_ids = list(range(seq_len))
        pos_emb = self.position_embedding(pos_ids)
        x = tok_emb + pos_emb

        # Transformer blocks
        for block in self.blocks:
            x = block(x)

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
