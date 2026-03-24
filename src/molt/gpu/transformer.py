"""
molt.gpu.transformer — Transformer architecture components.

Provides: MultiHeadAttention, TransformerBlock, PositionalEncoding,
CausalMask, and a complete TransformerDecoder for text generation.
"""

from .tensor import Tensor
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

    def __call__(self, x: Tensor, mask=None) -> Tensor:
        # x: (seq_len, embed_dim) — 2D tensor
        seq_len = x.shape[-2] if x.ndim >= 2 else x.shape[0]

        # Project Q, K, V — each is (seq_len, embed_dim)
        q = self.q_proj(x)
        k = self.k_proj(x)
        v = self.v_proj(x)

        # Multi-head attention via chunking along embed_dim.
        # Split each (seq_len, embed_dim) into num_heads chunks of head_dim,
        # compute attention per head, then concatenate.
        q_data = q._data_list()
        k_data = k._data_list()
        v_data = v._data_list()

        head_outputs = []  # will collect (seq_len, head_dim) per head

        for h in range(self.num_heads):
            hd = self.head_dim
            # Extract head slice: columns [h*hd : (h+1)*hd] from (seq_len, embed_dim)
            q_head = []
            k_head = []
            v_head = []
            for s in range(seq_len):
                row_start = s * self.embed_dim + h * hd
                q_head.extend(q_data[row_start:row_start + hd])
                k_head.extend(k_data[row_start:row_start + hd])
                v_head.extend(v_data[row_start:row_start + hd])

            # q_head, k_head, v_head are each (seq_len, head_dim)
            q_h = Tensor(q_head, shape=(seq_len, hd))
            k_h = Tensor(k_head, shape=(seq_len, hd))
            v_h = Tensor(v_head, shape=(seq_len, hd))

            # Scaled dot-product attention: softmax(Q @ K.T / sqrt(d)) @ V
            scores = q_h @ k_h.T  # (seq_len, seq_len)
            scores = scores * self.scale

            if self.causal:
                scores = apply_causal_mask(scores, seq_len)

            attn_weights = scores.softmax(axis=-1)
            attn_out = attn_weights @ v_h  # (seq_len, head_dim)
            head_outputs.append(attn_out._data_list())

        # Concatenate heads: interleave head_dim columns back to embed_dim
        concat = [0.0] * (seq_len * self.embed_dim)
        for s in range(seq_len):
            for h in range(self.num_heads):
                hd = self.head_dim
                src_start = s * hd
                dst_start = s * self.embed_dim + h * hd
                for d in range(hd):
                    concat[dst_start + d] = head_outputs[h][src_start + d]

        concat_tensor = Tensor(concat, shape=(seq_len, self.embed_dim))
        return self.out_proj(concat_tensor)

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
