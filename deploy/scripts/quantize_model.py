#!/usr/bin/env python3
"""Quantize Falcon-OCR weights from F32 to INT8/INT4 for Cloudflare Workers deployment.

Produces:
  - model.safetensors  (quantized weights in safetensors format)
  - scales.json        (per-tensor scale factors for dequantization)
  - config.json        (copy of model config)

Quantization strategy:
  - Per-tensor symmetric quantization for weight matrices (>= 2D, >= 1024 elements)
  - Small tensors (norms, sinks, freqs) kept as F32 for precision
  - INT8: val = scale * int8_val  (1 byte per param, ~257 MB)
  - INT4: val = scale * int4_val  (0.5 bytes per param, ~129 MB, packed 2 per byte)

INT4 packing: two 4-bit signed values packed per byte, low nibble first.
  byte = (val_hi & 0xF) << 4 | (val_lo & 0xF)
  Signed range: [-8, 7]

Usage:
  python3 deploy/scripts/quantize_model.py [--bits 8|4] [--output-dir DIR]
"""
import struct
import json
import os
import sys
import argparse
import time


SNAP = os.path.expanduser(
    "~/.cache/molt/falcon-ocr/models--tiiuae--Falcon-OCR/snapshots/"
    "3a4d95a8b0008f7430df30a82cf35e6c3b6bcb66"
)

# Tensors with these substrings are kept as F32 (too small or precision-critical).
KEEP_F32_PATTERNS = ["norm.weight", "sinks", "freqs_cis"]

# Mixed-precision: these layer patterns are kept as F32 even when quantizing.
# Attention Q/K/V projections and embedding layers are most sensitive to
# quantization error. Feed-forward layers tolerate quantization well.
#
# Rationale (from quantization literature):
#   - First/last layers (embeddings, output projection) have highest impact
#     on accuracy when quantized — they see the full distribution.
#   - Attention Q/K/V projections accumulate error through softmax, which
#     amplifies small perturbations. Keeping them F32 preserves attention quality.
#   - Feed-forward layers (w13, w2) are the bulk of parameters and tolerate
#     INT8 quantization with < 0.5% accuracy loss.
MIXED_PRECISION_F32_PATTERNS = [
    "img_projector.weight",      # Image embedding — first layer, critical
    "tok_embeddings.weight",     # Token embedding — critical for output quality
    "output.weight",             # Output projection — critical for predictions
    "attention.wqkv.weight",     # Q/K/V projections — attention is sensitive
]


def read_safetensors(path):
    """Parse a safetensors file into a dict of {name: {shape, dtype, data}}."""
    with open(path, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_size))
        data_start = 8 + header_size
        tensors = {}
        for name, info in header.items():
            if name == "__metadata__":
                continue
            offset_start, offset_end = info["data_offsets"]
            f.seek(data_start + offset_start)
            raw = f.read(offset_end - offset_start)
            tensors[name] = {
                "shape": info["shape"],
                "dtype": info["dtype"],
                "data": raw,
            }
    return tensors


def should_keep_f32(name, mixed_precision=False):
    """Return True if this tensor should stay F32.

    Args:
        name: Tensor name.
        mixed_precision: If True, also keep attention/embedding layers as F32.
    """
    for pattern in KEEP_F32_PATTERNS:
        if pattern in name:
            return True
    if mixed_precision:
        for pattern in MIXED_PRECISION_F32_PATTERNS:
            if pattern in name:
                return True
    return False


def quantize_tensor_int8(name, floats):
    """Per-tensor symmetric INT8: val = scale * int8_val."""
    amax = 0.0
    for v in floats:
        a = abs(v)
        if a > amax:
            amax = a
    if amax == 0.0:
        amax = 1.0
    scale = amax / 127.0
    inv_scale = 1.0 / scale
    result = bytearray(len(floats))
    for i, v in enumerate(floats):
        q = round(v * inv_scale)
        if q > 127:
            q = 127
        elif q < -128:
            q = -128
        result[i] = q & 0xFF
    return bytes(result), scale


def quantize_tensor_int4(name, floats):
    """Per-tensor symmetric INT4: val = scale * int4_val, packed 2 per byte."""
    amax = 0.0
    for v in floats:
        a = abs(v)
        if a > amax:
            amax = a
    if amax == 0.0:
        amax = 1.0
    scale = amax / 7.0
    inv_scale = 1.0 / scale
    n = len(floats)
    # Pad to even count
    padded_n = n + (n % 2)
    packed = bytearray(padded_n // 2)
    for i in range(0, padded_n, 2):
        v_lo = floats[i] if i < n else 0.0
        v_hi = floats[i + 1] if i + 1 < n else 0.0
        q_lo = round(v_lo * inv_scale)
        q_hi = round(v_hi * inv_scale)
        q_lo = max(-8, min(7, q_lo))
        q_hi = max(-8, min(7, q_hi))
        packed[i // 2] = ((q_hi & 0xF) << 4) | (q_lo & 0xF)
    return bytes(packed), scale


def write_safetensors(path, tensors_ordered):
    """Write safetensors format.

    tensors_ordered: list of (name, {shape, dtype, data}) in insertion order.
    """
    # Build header
    header = {}
    offset = 0
    for name, info in tensors_ordered:
        data_len = len(info["data"])
        header[name] = {
            "dtype": info["dtype"],
            "shape": info["shape"],
            "data_offsets": [offset, offset + data_len],
        }
        offset += data_len

    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    # Align header to 8 bytes (safetensors spec)
    padding = (8 - (len(header_bytes) % 8)) % 8
    header_bytes += b" " * padding

    with open(path, "wb") as f:
        f.write(struct.pack("<Q", len(header_bytes)))
        f.write(header_bytes)
        for _, info in tensors_ordered:
            f.write(info["data"])


def main():
    parser = argparse.ArgumentParser(description="Quantize Falcon-OCR model")
    parser.add_argument("--bits", type=int, choices=[4, 8], default=8,
                        help="Quantization bits (default: 8)")
    parser.add_argument("--output-dir", type=str, default=None,
                        help="Output directory (default: ~/.cache/molt/falcon-ocr/quantized-intN)")
    parser.add_argument("--model-dir", type=str, default=SNAP,
                        help="Source model directory")
    parser.add_argument("--mixed-precision", action="store_true", default=False,
                        help="Keep attention Q/K/V and embedding layers as F32 "
                             "(best quality within memory constraints)")
    args = parser.parse_args()

    bits = args.bits
    mixed_precision = args.mixed_precision
    model_dir = args.model_dir
    suffix = f"-mixed" if mixed_precision else ""
    output_dir = args.output_dir or os.path.expanduser(
        f"~/.cache/molt/falcon-ocr/quantized-int{bits}{suffix}"
    )
    os.makedirs(output_dir, exist_ok=True)

    # Resolve symlinks for the snapshot directory
    safetensors_path = os.path.realpath(os.path.join(model_dir, "model.safetensors"))
    config_path = os.path.realpath(os.path.join(model_dir, "config.json"))

    print(f"Quantizing to INT{bits}{' (mixed precision)' if mixed_precision else ''}")
    print(f"  Source: {safetensors_path}")
    print(f"  Output: {output_dir}")
    if mixed_precision:
        print(f"  F32 patterns (base): {KEEP_F32_PATTERNS}")
        print(f"  F32 patterns (mixed): {MIXED_PRECISION_F32_PATTERNS}")
    print()

    if not os.path.exists(safetensors_path):
        print(f"ERROR: Source weights not found: {safetensors_path}")
        sys.exit(1)

    print("Reading safetensors... ", end="", flush=True)
    t0 = time.time()
    tensors = read_safetensors(safetensors_path)
    print(f"done ({time.time() - t0:.1f}s, {len(tensors)} tensors)")

    quantize_fn = quantize_tensor_int8 if bits == 8 else quantize_tensor_int4
    dtype_tag = "I8" if bits == 8 else "I4"

    scales = {}
    q_tensors = []
    total_original = 0
    total_quantized = 0
    quantized_count = 0
    kept_count = 0

    print("Quantizing tensors:")
    for name in sorted(tensors.keys()):
        t = tensors[name]
        original_bytes = len(t["data"])
        total_original += original_bytes

        if t["dtype"] != "F32" or should_keep_f32(name, mixed_precision):
            q_tensors.append((name, t))
            total_quantized += original_bytes
            kept_count += 1
            reason = "kept"
            if mixed_precision and any(p in name for p in MIXED_PRECISION_F32_PATTERNS):
                reason = "kept (mixed-precision: accuracy-critical)"
            print(f"  [F32] {name}: {t['shape']} ({original_bytes:,} bytes, {reason})")
            continue

        n = len(t["data"]) // 4
        floats = struct.unpack(f"<{n}f", t["data"])
        q_data, scale = quantize_fn(name, floats)
        scales[name] = scale

        q_tensors.append((name, {
            "shape": t["shape"],
            "dtype": dtype_tag,
            "data": q_data,
        }))
        total_quantized += len(q_data)
        quantized_count += 1
        ratio = len(q_data) / original_bytes * 100
        print(f"  [INT{bits}] {name}: {t['shape']} "
              f"({original_bytes:,} -> {len(q_data):,} bytes, {ratio:.0f}%)")

    print()
    print(f"Quantized {quantized_count} tensors, kept {kept_count} as F32")
    print(f"Original: {total_original / 1024**2:.1f} MB")
    print(f"Quantized: {total_quantized / 1024**2:.1f} MB "
          f"({total_quantized / total_original * 100:.1f}%)")

    # Write quantized safetensors
    out_path = os.path.join(output_dir, "model.safetensors")
    print(f"\nWriting {out_path}... ", end="", flush=True)
    t0 = time.time()
    write_safetensors(out_path, q_tensors)
    actual_size = os.path.getsize(out_path)
    print(f"done ({time.time() - t0:.1f}s, {actual_size / 1024**2:.1f} MB)")

    # Write scales
    scales_path = os.path.join(output_dir, "scales.json")
    with open(scales_path, "w") as f:
        json.dump(scales, f, indent=2)
    print(f"Wrote {scales_path} ({len(scales)} entries)")

    # Copy config
    if os.path.exists(config_path):
        import shutil
        out_config = os.path.join(output_dir, "config.json")
        shutil.copy2(config_path, out_config)
        print(f"Copied {out_config}")

    # Add quantization metadata to config
    out_config = os.path.join(output_dir, "config.json")
    with open(out_config, "r") as f:
        config = json.load(f)
    all_f32_patterns = list(KEEP_F32_PATTERNS)
    if mixed_precision:
        all_f32_patterns.extend(MIXED_PRECISION_F32_PATTERNS)
    config["quantization"] = {
        "method": f"symmetric_int{bits}{'_mixed' if mixed_precision else ''}",
        "bits": bits,
        "mixed_precision": mixed_precision,
        "per_tensor_scales": True,
        "kept_f32_patterns": all_f32_patterns,
    }
    # Add rms_inner_eps if not present (inference-cpu.js needs it)
    if "rms_inner_eps" not in config:
        config["rms_inner_eps"] = config.get("norm_eps", 1e-5)
    with open(out_config, "w") as f:
        json.dump(config, f, indent=2)

    print()
    print("=" * 60)
    print(f"Quantization complete: INT{bits}")
    print(f"  Model:  {out_path} ({actual_size / 1024**2:.1f} MB)")
    print(f"  Scales: {scales_path}")
    print(f"  Config: {out_config}")
    print()
    fits_128 = actual_size < 128 * 1024 * 1024
    fits_256 = actual_size < 256 * 1024 * 1024
    if fits_128:
        print("  Status: FITS in Workers Free plan (128 MB)")
    elif fits_256:
        print("  Status: FITS in Workers Paid plan (256 MB), NOT Free plan")
        print("  Consider INT4 for Free plan: --bits 4")
    else:
        print("  Status: TOO LARGE for Workers. Try --bits 4")


if __name__ == "__main__":
    main()
