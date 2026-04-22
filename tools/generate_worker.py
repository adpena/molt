#!/usr/bin/env python3
"""Generate a Cloudflare Worker entry point from template."""

from __future__ import annotations
import argparse
import json
from pathlib import Path

TEMPLATE_PATH = Path(__file__).parent / "wasm_worker_template.js"


def generate_worker(
    output: Path,
    capabilities: list[str],
    tmp_quota_mb: int = 32,
    wasm_filename: str = "worker_linked.wasm",
) -> None:
    template = TEMPLATE_PATH.read_text()
    if not all(c.isalnum() or c in "._-" for c in wasm_filename):
        raise ValueError(f"Invalid wasm_filename: {wasm_filename!r}")
    template = template.replace("{{TMP_QUOTA_MB}}", str(int(tmp_quota_mb)))
    template = template.replace("{{CAPABILITIES}}", json.dumps(capabilities))
    template = template.replace("{{WASM_FILENAME}}", wasm_filename)
    output.write_text(template)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("-o", "--output", type=Path, default=Path("worker.js"))
    parser.add_argument(
        "--capabilities",
        nargs="*",
        default=["fs.bundle.read", "fs.tmp.read", "fs.tmp.write", "http.fetch"],
    )
    parser.add_argument("--tmp-quota-mb", type=int, default=32)
    parser.add_argument("--wasm-filename", default="worker_linked.wasm")
    args = parser.parse_args()
    generate_worker(
        args.output, args.capabilities, args.tmp_quota_mb, args.wasm_filename
    )
    print(f"Generated {args.output}", file=__import__("sys").stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
