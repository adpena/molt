import asyncio
import builtins

import pytest

_PENDING = 0x7FFD_0000_0000_0000


def _lookup_magic(name: str):
    return getattr(builtins, name, globals().get(name))


def test_magic_concurrency():
    funcs = {
        "molt_chan_send": _lookup_magic("molt_chan_send"),
        "molt_chan_new": _lookup_magic("molt_chan_new"),
        "molt_spawn": _lookup_magic("molt_spawn"),
        "molt_chan_recv": _lookup_magic("molt_chan_recv"),
    }
    missing = [name for name, func in funcs.items() if func is None]
    if missing:
        pytest.skip("molt magic concurrency intrinsics are not available in CPython")

    async def worker(chan):
        funcs["molt_chan_send"](chan, 42)

    async def main():
        chan = funcs["molt_chan_new"]()
        funcs["molt_spawn"](worker(chan))
        while True:
            res = funcs["molt_chan_recv"](chan)
            if res != _PENDING:
                assert res == 42
                return

    asyncio.run(main())
