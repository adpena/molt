#!/usr/bin/env python3
"""Package a directory into a bundle.tar for VFS /bundle mount."""

from __future__ import annotations
import argparse
import json
import sys
from pathlib import Path

SRC_ROOT = Path(__file__).resolve().parents[1] / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.wasm_bundle import BundleManifest, write_wasm_bundle  # noqa: E402


def create_bundle(source_dir: Path, output: Path) -> BundleManifest:
    """Publish one deterministic archive of a coherent source generation."""
    manifest = write_wasm_bundle(
        (source_dir,),
        output,
        include=lambda root, path: (
            "__pycache__" not in path.relative_to(root).parts
            and path.suffix not in {".pyc", ".pyo"}
        ),
    )
    assert manifest is not None
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Package a directory into a VFS bundle"
    )
    parser.add_argument("source", type=Path, help="Source directory to bundle")
    parser.add_argument("-o", "--output", type=Path, default=Path("bundle.tar"))
    parser.add_argument("--json", action="store_true", help="Output manifest as JSON")
    args = parser.parse_args()

    if not args.source.is_dir():
        print(f"Source is not a directory: {args.source}", file=sys.stderr)
        return 1

    try:
        manifest = create_bundle(args.source, args.output)
    except (OSError, ValueError) as exc:
        print(f"Bundle failed: {exc}", file=sys.stderr)
        return 1

    n_files = len(manifest["files"])
    total = manifest["total_bytes"]
    print(f"Bundled {n_files} files ({total:,} bytes) → {args.output}", file=sys.stderr)

    if args.json:
        print(json.dumps(manifest, indent=2))

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
