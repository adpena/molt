"""Update the pinned SHA-256 hash for molt_runtime.wasm in wasm_link.py.

Run after rebuilding the runtime binary so CI integrity checks pass:

    python tools/update_runtime_hash.py
"""

from __future__ import annotations

import hashlib
import re
import sys
from pathlib import Path

_HASH_RE = re.compile(
    r'("molt_runtime\.wasm":\s*")([0-9a-f]{64})(")',
)


def _find_project_root() -> Path:
    """Walk up from this file's directory until we find Cargo.toml."""
    candidate = Path(__file__).resolve().parent
    while True:
        if (candidate / "Cargo.toml").exists():
            return candidate
        parent = candidate.parent
        if parent == candidate:
            print(
                "error: could not find project root (no Cargo.toml found)",
                file=sys.stderr,
            )
            raise SystemExit(1)
        candidate = parent


def main() -> None:
    root = _find_project_root()

    wasm_path = root / "wasm" / "molt_runtime.wasm"
    if not wasm_path.exists():
        print(f"error: runtime binary not found: {wasm_path}", file=sys.stderr)
        raise SystemExit(1)

    new_hash = hashlib.sha256(wasm_path.read_bytes()).hexdigest()

    link_py = root / "tools" / "wasm_link.py"
    text = link_py.read_text()

    match = _HASH_RE.search(text)
    if match is None:
        print(
            f"error: could not find 'molt_runtime.wasm' hash entry in {link_py}",
            file=sys.stderr,
        )
        raise SystemExit(1)

    old_hash = match.group(2)

    if old_hash == new_hash:
        print(f"old: {old_hash}", file=sys.stderr)
        print(f"new: {new_hash} (unchanged)", file=sys.stderr)
        return

    updated = _HASH_RE.sub(rf"\g<1>{new_hash}\3", text)
    link_py.write_text(updated)

    print(f"old: {old_hash}", file=sys.stderr)
    print(f"new: {new_hash}", file=sys.stderr)


if __name__ == "__main__":
    main()
