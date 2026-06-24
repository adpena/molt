"""Purpose: differential coverage for the implicit exit-time flush of binary
buffered stdout.

Regression for the P0 truncation bug where `sys.stdout.buffer.write(...)` of a
sub-blocksize payload (smaller than the 8 KiB BufferedWriter block) was never
flushed at interpreter exit, so molt emitted 0 bytes (or, for a payload that
crossed one block boundary, only the filled block and never the trailing
partial block). CPython guarantees every buffered stream is flushed at exit;
the binary `sys.stdout.buffer` path must match.

This test deliberately uses NO explicit `.flush()` and NO `.close()` so the
only thing that can emit the bytes is the runtime's exit-flush sequence.
"""

import sys

cout = sys.stdout.buffer.write

# 1) sub-blocksize payload: was lost entirely before the fix.
cout(b"alpha-")  # 6 bytes
# 2) a payload that crosses exactly one 8 KiB block boundary plus a partial
#    tail: before the fix only the first filled block survived, the tail was
#    truncated.
cout(b"x" * 8192)  # fills one block
cout(b"omega")  # 5-byte partial tail, must survive exit flush

# No flush(), no close(): rely solely on the interpreter exit flush.
