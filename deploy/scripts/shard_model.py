#!/usr/bin/env python3
"""Shard a quantized Falcon-OCR model into multiple safetensors files.

Workers can't load a single 129 MB file into memory (V8 isolate limit).
This script splits the model into shards small enough to load individually.
Each shard is loaded, tensors extracted, then the shard buffer discarded
before loading the next shard.

Output:
  model-00001-of-NNNNN.safetensors
  model-00002-of-NNNNN.safetensors
  ...
  model.safetensors.index.json  (maps tensor names -> shard filenames)
  scales.json                   (copied from source)
  config.json                   (copied from source)

Usage:
  python3 deploy/scripts/shard_model.py [--input-dir DIR] [--output-dir DIR] [--max-shard-mb 30]
"""

import struct
import json
import os
import sys
import argparse
import shutil


def read_safetensors(path):
    """Parse a safetensors file into a list of (name, {shape, dtype, data})."""
    with open(path, "rb") as f:
        header_size = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_size))
        data_start = 8 + header_size
        tensors = []
        for name in sorted(header.keys()):
            if name == "__metadata__":
                continue
            info = header[name]
            offset_start, offset_end = info["data_offsets"]
            f.seek(data_start + offset_start)
            raw = f.read(offset_end - offset_start)
            tensors.append(
                (
                    name,
                    {
                        "shape": info["shape"],
                        "dtype": info["dtype"],
                        "data": raw,
                    },
                )
            )
    return tensors


def write_safetensors(path, tensors):
    """Write safetensors format from list of (name, {shape, dtype, data})."""
    header = {}
    offset = 0
    for name, info in tensors:
        data_len = len(info["data"])
        header[name] = {
            "dtype": info["dtype"],
            "shape": info["shape"],
            "data_offsets": [offset, offset + data_len],
        }
        offset += data_len

    header_bytes = json.dumps(header, separators=(",", ":")).encode("utf-8")
    padding = (8 - (len(header_bytes) % 8)) % 8
    header_bytes += b" " * padding

    with open(path, "wb") as f:
        f.write(struct.pack("<Q", len(header_bytes)))
        f.write(header_bytes)
        for _, info in tensors:
            f.write(info["data"])


def main():
    parser = argparse.ArgumentParser(description="Shard a quantized model")
    parser.add_argument(
        "--input-dir",
        type=str,
        default=os.path.expanduser("~/.cache/molt/falcon-ocr/quantized-int4"),
        help="Input directory with model.safetensors",
    )
    parser.add_argument(
        "--output-dir",
        type=str,
        default=None,
        help="Output directory (default: input-dir/sharded)",
    )
    parser.add_argument(
        "--max-shard-mb",
        type=float,
        default=30.0,
        help="Maximum shard size in MB (default: 30)",
    )
    args = parser.parse_args()

    input_dir = args.input_dir
    output_dir = args.output_dir or os.path.join(input_dir, "sharded")
    max_shard_bytes = int(args.max_shard_mb * 1024 * 1024)

    os.makedirs(output_dir, exist_ok=True)

    src = os.path.join(input_dir, "model.safetensors")
    if not os.path.exists(src):
        print(f"ERROR: {src} not found")
        sys.exit(1)

    print(f"Reading {src}...")
    tensors = read_safetensors(src)
    print(f"  {len(tensors)} tensors")

    # Group tensors into shards by size
    shards = []
    current_shard = []
    current_size = 0

    for name, info in tensors:
        tensor_size = len(info["data"])
        # If single tensor > max shard size, it gets its own shard
        if current_shard and current_size + tensor_size > max_shard_bytes:
            shards.append(current_shard)
            current_shard = []
            current_size = 0
        current_shard.append((name, info))
        current_size += tensor_size

    if current_shard:
        shards.append(current_shard)

    num_shards = len(shards)
    print(f"  Splitting into {num_shards} shards (max {args.max_shard_mb} MB each)")

    # Write shards and build index
    weight_map = {}
    total_written = 0

    for i, shard_tensors in enumerate(shards):
        shard_name = f"model-{i + 1:05d}-of-{num_shards:05d}.safetensors"
        shard_path = os.path.join(output_dir, shard_name)

        shard_size = sum(len(t[1]["data"]) for t in shard_tensors)
        print(
            f"  Shard {i + 1}/{num_shards}: {shard_name} "
            f"({len(shard_tensors)} tensors, {shard_size / 1024**2:.1f} MB)"
        )

        write_safetensors(shard_path, shard_tensors)
        total_written += os.path.getsize(shard_path)

        for name, _ in shard_tensors:
            weight_map[name] = shard_name

    # Write index
    index = {
        "metadata": {
            "total_size": total_written,
            "num_shards": num_shards,
        },
        "weight_map": weight_map,
    }
    index_path = os.path.join(output_dir, "model.safetensors.index.json")
    with open(index_path, "w") as f:
        json.dump(index, f, indent=2)
    print(f"\n  Index: {index_path}")

    # Copy scales and config
    for filename in ["scales.json", "config.json"]:
        src_file = os.path.join(input_dir, filename)
        if os.path.exists(src_file):
            dst_file = os.path.join(output_dir, filename)
            shutil.copy2(src_file, dst_file)
            print(f"  Copied: {dst_file}")

    print(
        f"\nTotal sharded size: {total_written / 1024**2:.1f} MB in {num_shards} shards"
    )
    print(f"Output: {output_dir}")


if __name__ == "__main__":
    main()
