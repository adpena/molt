"""Purpose: asyncio child-watcher gate is intrinsic-backed and avoids NotImplementedError."""

import asyncio


try:
    watcher = asyncio.get_child_watcher()
except Exception as exc:
    print(type(exc).__name__, isinstance(exc, NotImplementedError))
else:
    print(type(watcher).__name__, False)
