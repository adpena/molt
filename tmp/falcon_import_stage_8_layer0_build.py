from pathlib import Path

from main_molt import (
    Attention,
    FalconOCRConfig,
    FeedForward,
    TransformerBlock,
    precompute_freqs_cis_1d,
)
from molt.gpu.interop import load_safetensors
from molt.gpu.tensor import zeros

cfg = FalconOCRConfig.from_json(
    Path(
        "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/config.json"
    ).read_text()
)
state = load_safetensors(
    "/Users/adpena/Projects/enjoice/experiments/tinygrad-molt/falcon-ocr/weights/model.safetensors"
)
attn = Attention(
    cfg,
    state["layers.0.attention.wqkv.weight"],
    state["layers.0.attention.wo.weight"],
    state.get("layers.0.attention.sinks", zeros(cfg.n_heads)),
)
ffn = FeedForward(
    cfg,
    state["layers.0.feed_forward.w13.weight"],
    state["layers.0.feed_forward.w2.weight"],
)
block = TransformerBlock(attn, ffn)
freqs = precompute_freqs_cis_1d(cfg.head_dim // 2, cfg.max_seq_len, cfg.rope_theta)
print(type(block).__name__, len(freqs))
