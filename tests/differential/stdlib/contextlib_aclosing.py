"""Purpose: differential coverage for contextlib.aclosing async semantics."""

import asyncio
import contextlib


class ACloseable:
    def __init__(self) -> None:
        self.closed = 0

    async def aclose(self) -> None:
        self.closed += 1


class RaisesOnClose:
    async def aclose(self) -> None:
        raise RuntimeError("close boom")


class MissingClose:
    pass


async def main() -> None:
    thing = ACloseable()
    async with contextlib.aclosing(thing) as got:
        print("same", got is thing)
    print("closed", thing.closed)

    try:
        async with contextlib.aclosing(RaisesOnClose()):
            pass
    except Exception as exc:
        print("raises", type(exc).__name__, str(exc))

    try:
        async with contextlib.aclosing(MissingClose()):
            pass
    except Exception as exc:
        print("missing", type(exc).__name__)


asyncio.run(main())
