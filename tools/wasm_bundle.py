#!/usr/bin/env python3
"""Package a directory into a bundle.tar for VFS /bundle mount."""
from __future__ import annotations
import argparse
import io
import json
import os
import sys
import tarfile
from pathlib import Path


def create_bundle(source_dir: Path, output: Path) -> dict:
    """Package source_dir into a tar archive with manifest."""
    manifest = {"files": [], "total_bytes": 0}

    with tarfile.open(output, "w") as tar:
        for path in sorted(source_dir.rglob("*")):
            if not path.is_file():
                continue
            # Security: reject symlinks
            if path.is_symlink():
                print(f"Warning: skipping symlink {path}", file=sys.stderr)
                continue
            arcname = str(path.relative_to(source_dir))
            # Security: reject paths with ..
            if ".." in arcname:
                print(f"Warning: skipping path with '..': {arcname}", file=sys.stderr)
                continue
            # Skip __pycache__ and .pyc files
            if "__pycache__" in arcname or arcname.endswith(".pyc"):
                continue
            tar.add(str(path), arcname=arcname)
            file_size = path.stat().st_size
            manifest["files"].append({"path": arcname, "size": file_size})
            manifest["total_bytes"] += file_size

        # Write manifest as last entry
        manifest_bytes = json.dumps(manifest, indent=2).encode("utf-8")
        info = tarfile.TarInfo("__manifest__.json")
        info.size = len(manifest_bytes)
        tar.addfile(info, io.BytesIO(manifest_bytes))

    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description="Package a directory into a VFS bundle")
    parser.add_argument("source", type=Path, help="Source directory to bundle")
    parser.add_argument("-o", "--output", type=Path, default=Path("bundle.tar"))
    parser.add_argument("--json", action="store_true", help="Output manifest as JSON")
    args = parser.parse_args()

    if not args.source.is_dir():
        print(f"Source is not a directory: {args.source}", file=sys.stderr)
        return 1

    manifest = create_bundle(args.source, args.output)

    n_files = len(manifest["files"])
    total = manifest["total_bytes"]
    print(f"Bundled {n_files} files ({total:,} bytes) → {args.output}", file=sys.stderr)

    if args.json:
        print(json.dumps(manifest, indent=2))

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
