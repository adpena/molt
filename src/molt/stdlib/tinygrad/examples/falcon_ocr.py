"""
Falcon-OCR reimplemented using the tinygrad Tensor API.

This is a port of the legacy free-function API to the new tinygrad-conformant
Tensor class. Same architecture, same weights, same special tokens.

When compiled with `molt build falcon_ocr.py --target wasm`, this produces a
WASM binary that runs Falcon-OCR in browsers and Cloudflare Workers.

Public entry points (what molt exports):
    init(weights_bytes: bytes, config_json: str) -> None
    ocr_tokens(width: int, height: int, rgb: bytes,
               prompt_ids: list[int], max_new_tokens: int) -> list[int]
"""

from __future__ import annotations

import array
import json
import math
import os
import _intrinsics
from _intrinsics import require_intrinsic as _require_intrinsic

# GPU primitive intrinsic — required for WASM stdlib enforcement
_gpu_device = _require_intrinsic("molt_gpu_prim_device")

from molt.gpu import Buffer, alloc
from tinygrad.tensor import Tensor
from molt.gpu.interop import load_safetensors_bytes

_config: FalconOCRConfig | None = None
_tok_embeddings: Tensor | None = None
_img_projector: Tensor | None = None
_layers: list[tuple[Tensor, Tensor, Tensor, Tensor, Tensor]] | None = None
_norm_weight: Tensor | None = None
_output: Tensor | None = None
_freqs: list | None = None
_freqs_len = 0


def _load_optional_intrinsic(name: str):
    loader = getattr(_intrinsics, "load_intrinsic", None)
    if callable(loader):
        return loader(name)
    require = getattr(_intrinsics, "require_intrinsic", None)
    if callable(require):
        try:
            return require(name)
        except RuntimeError:
            return None
    return None


_MOLT_GPU_ROPE_APPLY_CONTIGUOUS = _load_optional_intrinsic(
    "molt_gpu_rope_apply_contiguous"
)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

class FalconOCRConfig:
    def __init__(
        self,
        dim: int = 768,
        n_layers: int = 22,
        n_heads: int = 16,
        head_dim: int = 64,
        n_kv_heads: int = 8,
        ffn_dim: int = 2304,
        vocab_size: int = 65536,
        max_seq_len: int = 8192,
        rope_theta: float = 10000.0,
        norm_eps: float = 1e-5,
        rms_inner_eps: float = 1e-6,
        channel_size: int = 3,
        spatial_patch_size: int = 16,
        temporal_patch_size: int = 1,
        eos_id: int = 11,
        img_id: int = 227,
        img_row_sep_id: int = 228,
        img_start_id: int = 229,
        img_end_id: int = 230,
        coord_token_id: int = 240,
        size_token_id: int = 241,
        image_cls_token_id: int = 244,
        image_reg_1_token_id: int = 245,
        image_reg_2_token_id: int = 246,
        image_reg_3_token_id: int = 247,
        image_reg_4_token_id: int = 248,
        seg_token_id: int = 262,
    ) -> None:
        self.dim = dim
        self.n_layers = n_layers
        self.n_heads = n_heads
        self.head_dim = head_dim
        self.n_kv_heads = n_kv_heads
        self.ffn_dim = ffn_dim
        self.vocab_size = vocab_size
        self.max_seq_len = max_seq_len
        self.rope_theta = rope_theta
        self.norm_eps = norm_eps
        self.rms_inner_eps = rms_inner_eps
        self.channel_size = channel_size
        self.spatial_patch_size = spatial_patch_size
        self.temporal_patch_size = temporal_patch_size
        self.eos_id = eos_id
        self.img_id = img_id
        self.img_row_sep_id = img_row_sep_id
        self.img_start_id = img_start_id
        self.img_end_id = img_end_id
        self.coord_token_id = coord_token_id
        self.size_token_id = size_token_id
        self.image_cls_token_id = image_cls_token_id
        self.image_reg_1_token_id = image_reg_1_token_id
        self.image_reg_2_token_id = image_reg_2_token_id
        self.image_reg_3_token_id = image_reg_3_token_id
        self.image_reg_4_token_id = image_reg_4_token_id
        self.seg_token_id = seg_token_id

    @classmethod
    def from_json(cls, s: str) -> "FalconOCRConfig":
        data = json.loads(s)
        known = {
            "dim", "n_layers", "n_heads", "head_dim", "n_kv_heads", "ffn_dim",
            "vocab_size", "max_seq_len", "rope_theta", "norm_eps",
            "rms_inner_eps", "channel_size", "spatial_patch_size",
            "temporal_patch_size", "eos_id", "img_id", "img_row_sep_id",
            "img_start_id", "img_end_id", "coord_token_id", "size_token_id",
            "image_cls_token_id", "image_reg_1_token_id",
            "image_reg_2_token_id", "image_reg_3_token_id",
            "image_reg_4_token_id", "seg_token_id",
        }
        kwargs = {k: v for k, v in data.items() if k in known}
        return cls(**kwargs)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _rms_norm(x: Tensor, eps: float) -> Tensor:
    """Unit-scale RMSNorm: x / sqrt(mean(x^2) + eps)."""
    return x.rms_norm(eps)


def _repeat_kv(x: Tensor, n_rep: int) -> Tensor:
    """Tile KV heads to match Q heads."""
    if n_rep == 1:
        return x
    return x.repeat_axis(2, n_rep)


# ---------------------------------------------------------------------------
# RoPE (1D temporal)
# ---------------------------------------------------------------------------

def precompute_freqs_cis_1d(dim: int, max_seq_len: int, theta: float) -> list:
    """Precompute cos/sin tables for 1D RoPE."""
    freqs = array.array("f", [0.0] * dim)
    inv_dim = 1.0 / dim
    for i in range(dim):
        freqs[i] = 1.0 / (theta ** (i * inv_dim))
    cos = math.cos
    sin = math.sin
    cos_vals = array.array("f")
    sin_vals = array.array("f")
    for pos in range(max_seq_len):
        for f in freqs:
            angle = pos * f
            cos_vals.append(cos(angle))
            sin_vals.append(sin(angle))
    cos_buf = Buffer(cos_vals.tobytes(), float, len(cos_vals), format_char="f")
    sin_buf = Buffer(sin_vals.tobytes(), float, len(sin_vals), format_char="f")
    return [cos_buf, sin_buf, dim]


def apply_rope_1d(x: Tensor, freqs: list, seq_len: int) -> Tensor:
    """Apply 1D RoPE to the first half of each head's channels."""
    B, S, H, D = x.shape
    cos_buf, sin_buf, freq_dim = freqs
    if _MOLT_GPU_ROPE_APPLY_CONTIGUOUS is not None:
        out_bits = _MOLT_GPU_ROPE_APPLY_CONTIGUOUS(
            x._buf._data,
            x._buf.format_char,
            cos_buf._data,
            sin_buf._data,
            freq_dim,
            B,
            S,
            H,
            D,
            seq_len,
            x._buf.format_char,
        )
        out_buf = Buffer(
            out_bits,
            x._dtype,
            x.size,
            format_char=x._buf.format_char,
        )
        return Tensor(out_buf, shape=(B, S, H, D), dtype=x._dtype)

    half = D // 2
    flat = x._data_list()
    out_buf = alloc(len(flat), float, format_char=x._buf.format_char)
    for i, value in enumerate(flat):
        out_buf[i] = value
    for b in range(B):
        for s in range(min(S, seq_len)):
            freq_base = s * freq_dim
            for h in range(H):
                base = ((b * S + s) * H + h) * D
                for i in range(half):
                    if i < freq_dim:
                        cos_v = cos_buf[freq_base + i]
                        sin_v = sin_buf[freq_base + i]
                    else:
                        cos_v = 1.0
                        sin_v = 0.0
                    x0 = flat[base + i]
                    x1 = flat[base + i + half] if (i + half) < D else 0.0
                    out_buf[base + i] = x0 * cos_v - x1 * sin_v
                    if (i + half) < D:
                        out_buf[base + i + half] = x0 * sin_v + x1 * cos_v
    return Tensor(out_buf, shape=(B, S, H, D))


# ---------------------------------------------------------------------------
# Modules (using plain Python classes -- molt compiles these)
# ---------------------------------------------------------------------------

def _apply_rms_norm_weight(x: Tensor, weight: Tensor, eps: float) -> Tensor:
    return _rms_norm(x, eps) * weight


def _attention_call(
    cfg: FalconOCRConfig,
    wqkv: Tensor,
    wo: Tensor,
    _sinks: Tensor,
    x: Tensor,
    freqs: list,
    mask: Tensor | None,
) -> Tensor:
    B, S, _ = x.shape
    n_heads = cfg.n_heads
    n_kv_heads = cfg.n_kv_heads
    n_rep = n_heads // n_kv_heads
    head_dim = cfg.head_dim
    q_dim = n_heads * head_dim
    kv_dim = n_kv_heads * head_dim

    h = _rms_norm(x, cfg.rms_inner_eps)
    q, k, v = h.dot(wqkv).split_last_dim((q_dim, kv_dim, kv_dim))
    xq = q.reshape(B, S, n_heads, head_dim)
    xk = k.reshape(B, S, n_kv_heads, head_dim)
    xv = v.reshape(B, S, n_kv_heads, head_dim)

    xq = _rms_norm(xq, cfg.rms_inner_eps)
    xk = _rms_norm(xk, cfg.rms_inner_eps)

    xq = apply_rope_1d(xq, freqs, S)
    xk = apply_rope_1d(xk, freqs, S)

    xk = _repeat_kv(xk, n_rep)
    xv = _repeat_kv(xv, n_rep)

    xq = xq.permute(0, 2, 1, 3)
    xk = xk.permute(0, 2, 1, 3)
    xv = xv.permute(0, 2, 1, 3)
    if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
        print(
            f"[falcon shapes] attention q={xq.shape} k={xk.shape} v={xv.shape} mask={None if mask is None else mask.shape}"
        )

    scale = 1.0 / math.sqrt(head_dim)
    out = xq.scaled_dot_product_attention(xk, xv, mask, scale)
    out = out.permute(0, 2, 1, 3)
    out = out.reshape(B, S, n_heads * head_dim)
    return out.dot(wo)


def _feed_forward_call(cfg: FalconOCRConfig, w13: Tensor, w2: Tensor, x: Tensor) -> Tensor:
    h = _rms_norm(x, cfg.rms_inner_eps)
    return h.dot(w13).squared_relu_gate_interleaved().dot(w2)


def _transformer_block_call(
    cfg: FalconOCRConfig,
    layer: tuple[Tensor, Tensor, Tensor, Tensor, Tensor],
    x: Tensor,
    freqs: list,
    mask: Tensor | None,
) -> Tensor:
    wqkv, wo, sinks, w13, w2 = layer
    x = x + _attention_call(cfg, wqkv, wo, sinks, x, freqs, mask)
    x = x + _feed_forward_call(cfg, w13, w2, x)
    return x


def _freqs_for(seq_len: int) -> list:
    global _freqs, _freqs_len
    assert _config is not None
    target = max(1, seq_len)
    if _freqs is None or _freqs_len < target:
        _freqs = precompute_freqs_cis_1d(
            _config.head_dim // 2, target, _config.rope_theta
        )
        _freqs_len = target
    return _freqs


def _compute_temporal_positions(token_ids: list[int]) -> list[int]:
    assert _config is not None
    no_increase_set = {
        _config.img_id,
        _config.image_reg_1_token_id,
        _config.image_reg_2_token_id,
        _config.image_reg_3_token_id,
        _config.image_reg_4_token_id,
        _config.img_end_id,
    }
    pos: list[int] = []
    running = 0
    for tid in token_ids:
        if tid not in no_increase_set:
            running += 1
        pos.append(running - 1)
    return pos


def _gather_freqs_for_positions(freqs: list, positions: list[int]) -> list:
    cos_buf, sin_buf, freq_dim = freqs
    cos_vals = array.array("f")
    sin_vals = array.array("f")
    for pos in positions:
        base = pos * freq_dim
        for i in range(freq_dim):
            cos_vals.append(cos_buf[base + i])
            sin_vals.append(sin_buf[base + i])
    return [
        Buffer(cos_vals.tobytes(), float, len(cos_vals), format_char="f"),
        Buffer(sin_vals.tobytes(), float, len(sin_vals), format_char="f"),
        freq_dim,
    ]


def _build_hybrid_mask_state(token_ids: list[int]) -> tuple[list[bool], list[int], list[tuple[int, int]]]:
    assert _config is not None
    in_block = [False] * len(token_ids)
    block_idx = [-1] * len(token_ids)
    block_bounds: list[list[int]] = []
    depth = 0
    current_block = -1
    for i, tid in enumerate(token_ids):
        is_soi = tid == _config.image_cls_token_id
        is_eoi = tid == _config.img_end_id
        if is_soi:
            depth += 1
            current_block += 1
            block_bounds.append([i, i + 1])
        if depth > 0:
            in_block[i] = True
            block_idx[i] = current_block
            block_bounds[current_block][1] = i + 1
        if is_eoi and depth > 0:
            depth -= 1
    return in_block, block_idx, [(start, end) for start, end in block_bounds]


def _build_hybrid_mask_from_state(
    seq_len: int,
    prefix_in_block: list[bool],
    prefix_block_idx: list[int],
    block_bounds: list[tuple[int, int]],
) -> Tensor:
    values = array.array("f", [-1.0e9]) * (seq_len * seq_len)
    row_zero = array.array("f", [0.0]) * seq_len
    prefix_len = len(prefix_in_block)
    for q in range(seq_len):
        row_base = q * seq_len
        causal_len = q + 1
        values[row_base : row_base + causal_len] = row_zero[:causal_len]
        if q < prefix_len and prefix_in_block[q]:
            start, end = block_bounds[prefix_block_idx[q]]
            values[row_base + start : row_base + end] = row_zero[: end - start]
    return Tensor(
        Buffer(values.tobytes(), float, len(values), format_char="f"),
        shape=(1, 1, seq_len, seq_len),
        dtype=float,
    )


def _rgb_to_patches(rgb: bytes, width: int, height: int) -> Tensor:
    assert _config is not None
    p = _config.spatial_patch_size
    c = _config.channel_size
    if width % p != 0 or height % p != 0:
        raise ValueError(
            f"image dims {width}x{height} must be a multiple of patch size {p}"
        )
    n_w = width // p
    n_h = height // p

    floats = array.array("f", ((b / 255.0) * 2.0 - 1.0 for b in rgb))
    arr = Tensor(
        Buffer(floats.tobytes(), float, len(floats), format_char="f"),
        shape=(height, width, c),
        dtype=float,
    )
    arr = arr.reshape(n_h, p, n_w, p, c)
    arr = arr.permute(0, 2, 1, 3, 4)
    out = arr.reshape(n_h * n_w, p * p * c)
    if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
        print(
            f"[falcon shapes] rgb_to_patches width={width} height={height} patch_shape={out.shape}"
        )
    return out


def _build_image_block_ids(n_patches: int) -> list[int]:
    assert _config is not None
    block = [
        _config.image_cls_token_id,
        _config.image_reg_1_token_id,
        _config.image_reg_2_token_id,
        _config.image_reg_3_token_id,
        _config.image_reg_4_token_id,
    ]
    for _ in range(n_patches):
        block.append(_config.img_id)
    block.append(_config.img_end_id)
    return block


def _generate(
    prompt_ids: list[int],
    patch_features: Tensor,
    max_new_tokens: int,
) -> list[int]:
    assert _config is not None
    assert _tok_embeddings is not None
    assert _img_projector is not None
    assert _layers is not None
    assert _norm_weight is not None
    assert _output is not None

    n_patches = patch_features.shape[0]
    if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
        print(
            f"[falcon shapes] generate patch_features_shape={patch_features.shape} n_patches={n_patches} prompt_ids_len={len(prompt_ids)}"
        )
    prefix_ids = list(prompt_ids)
    prefix_ids.extend(_build_image_block_ids(n_patches))
    ids = list(prefix_ids)
    prompt_len = len(prefix_ids)
    if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
        print(
            f"[falcon shapes] prefix_ids_len={len(prefix_ids)} prefix_ids={prefix_ids}"
        )

    prefix_embed = _tok_embeddings.gather(0, Tensor(prefix_ids))
    image_positions = [i for i, tid in enumerate(prefix_ids) if tid == _config.img_id]
    if image_positions:
        projected_patches = patch_features.dot(_img_projector)
        prefix_embed = prefix_embed.scatter(0, Tensor(image_positions), projected_patches)

    positions: list[int] = []
    running = 0
    for tid in prefix_ids:
        if tid not in {
            _config.img_id,
            _config.image_reg_1_token_id,
            _config.image_reg_2_token_id,
            _config.image_reg_3_token_id,
            _config.image_reg_4_token_id,
            _config.img_end_id,
        }:
            running += 1
        positions.append(running - 1)
    if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
        print(f"[falcon shapes] positions_len={len(positions)}")
    temporal_no_increase = {
        _config.img_id,
        _config.image_reg_1_token_id,
        _config.image_reg_2_token_id,
        _config.image_reg_3_token_id,
        _config.image_reg_4_token_id,
        _config.img_end_id,
    }
    running_position = positions[-1] + 1 if positions else 0
    prefix_in_block, prefix_block_idx, block_bounds = _build_hybrid_mask_state(
        prefix_ids
    )
    generated_embeds: Tensor | None = None
    dim = _config.dim

    for step in range(max_new_tokens):
        T = len(ids)
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=start T={T}")
        if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
            print(
                f"[falcon shapes] step={step} T={T} ids_len={len(ids)} positions_len={len(positions)} running_position={running_position}"
            )
        embed_rows = (
            prefix_embed
            if generated_embeds is None
            else Tensor.cat(prefix_embed, generated_embeds, dim=0)
        )
        h = embed_rows.reshape(1, T, dim)
        freqs = _gather_freqs_for_positions(_freqs_for(T), positions)
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=freqs_ready")
        if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
            print(
                f"[falcon shapes] step={step} freqs_dim={freqs[2]} freqs_cos_size={freqs[0].size} freqs_sin_size={freqs[1].size}"
            )
        mask = _build_hybrid_mask_from_state(
            T, prefix_in_block, prefix_block_idx, block_bounds
        )
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=mask_ready")
        if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
            print(f"[falcon shapes] step={step} mask_shape={mask.shape}")

        for layer_idx, layer in enumerate(_layers):
            if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
                print(f"[falcon step] step={step} stage=layer_start idx={layer_idx}")
            h = _transformer_block_call(_config, layer, h, freqs, mask)
            if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
                print(f"[falcon step] step={step} stage=layer_done idx={layer_idx}")
        h = _apply_rms_norm_weight(h, _norm_weight, _config.norm_eps)
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=norm_done")

        last_tensor = h.reshape(T, dim).gather(0, Tensor([T - 1]))
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=last_ready")
        logits = last_tensor.dot(_output)
        if os.environ.get("MOLT_TRACE_FALCON_STEP") == "1":
            print(f"[falcon step] step={step} stage=logits_ready")
        if os.environ.get("MOLT_TRACE_FALCON_SHAPES") == "1":
            print(
                f"[falcon shapes] step={step} h={h.shape} last={last_tensor.shape} logits={logits.shape}"
            )
        next_id = logits.argmax().item()
        ids.append(int(next_id))

        if next_id == _config.eos_id:
            break
        if step + 1 >= max_new_tokens:
            break

        next_embed = _tok_embeddings.gather(0, Tensor([int(next_id)]))
        generated_embeds = (
            next_embed
            if generated_embeds is None
            else Tensor.cat(generated_embeds, next_embed, dim=0)
        )
        if next_id not in temporal_no_increase:
            running_position += 1
        positions.append(running_position - 1)

    return ids[prompt_len:]


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

def init(weights_bytes: bytes, config_json: str) -> None:
    """Initialize model from SafeTensors bytes and config JSON."""
    global _config, _tok_embeddings, _img_projector, _layers, _norm_weight, _output, _freqs, _freqs_len
    _config = FalconOCRConfig.from_json(config_json)
    state = load_safetensors_bytes(weights_bytes)
    _tok_embeddings = state["tok_embeddings.weight"]
    _img_projector = state["img_projector.weight"]
    _layers = []
    for i in range(_config.n_layers):
        prefix = f"layers.{i}"
        _layers.append(
            (
                state[f"{prefix}.attention.wqkv.weight"],
                state[f"{prefix}.attention.wo.weight"],
                state.get(f"{prefix}.attention.sinks", Tensor.zeros(_config.n_heads)),
                state[f"{prefix}.feed_forward.w13.weight"],
                state[f"{prefix}.feed_forward.w2.weight"],
            )
        )
    _norm_weight = state["norm.weight"]
    _output = state["output.weight"]
    _freqs = None
    _freqs_len = 0


def ocr_tokens(
    width: int,
    height: int,
    rgb: bytes,
    prompt_ids: list[int],
    max_new_tokens: int = 512,
) -> list[int]:
    """Run OCR on a single image. Returns generated token IDs."""
    if _config is None or _tok_embeddings is None:
        raise RuntimeError("init() must be called before ocr_tokens()")
    if len(rgb) != width * height * _config.channel_size:
        raise ValueError(
            f"rgb length {len(rgb)} != {width}x{height}x{_config.channel_size}"
        )
    patches = _rgb_to_patches(rgb, width, height)
    return _generate(prompt_ids, patches, max_new_tokens)
