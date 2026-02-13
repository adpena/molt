# MOLT_ENV: MOLT_CAPABILITIES=fs.read
# MOLT_META: wasm=only
"""Wasm-only contract coverage for glob dir_fd host split (server vs browser)."""

from __future__ import annotations

import glob

bad_fd = 1_000_000

try:
    print("rel_bad", glob.glob("*.txt", dir_fd=bad_fd))
except Exception as exc:
    print("rel_bad_exc", type(exc).__name__, str(exc))

try:
    print("recursive_bad", glob.glob("**/*.txt", dir_fd=bad_fd, recursive=True))
except Exception as exc:
    print("recursive_bad_exc", type(exc).__name__, str(exc))
