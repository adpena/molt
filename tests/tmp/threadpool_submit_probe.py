from __future__ import annotations

import builtins as _builtins

molt_thread_submit = getattr(_builtins, "molt_thread_submit", None)
molt_block_on = getattr(_builtins, "molt_block_on", None)
if molt_thread_submit is None or molt_block_on is None:
    raise RuntimeError("molt threadpool intrinsics are unavailable")


def add(x: int, y: int) -> int:
    return x + y


future = molt_thread_submit(add, (1, 2), {})
print("future", future)
print("result", molt_block_on(future))
