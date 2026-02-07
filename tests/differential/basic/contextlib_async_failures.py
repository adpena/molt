"""Purpose: differential coverage for async contextlib failure semantics."""

import asyncio
import contextlib


@contextlib.asynccontextmanager
async def no_yield():
    if False:  # pragma: no cover
        yield None


@contextlib.asynccontextmanager
async def no_stop():
    yield "x"
    yield "y"


@contextlib.asynccontextmanager
async def swallow_value_error():
    try:
        yield "ok"
    except ValueError:
        return


async def callback_raises(tag: str) -> None:
    raise RuntimeError(f"callback:{tag}")


async def exit_suppress(exc_type, exc, tb) -> bool:
    return True


class BadAsyncContext:
    pass


async def main() -> None:
    for label, run in (
        ("no_yield", no_yield),
        ("no_stop", no_stop),
    ):
        try:
            async with run():
                pass
        except Exception as exc:
            print(label, type(exc).__name__, "generator" in str(exc))

    try:
        async with swallow_value_error():
            raise ValueError("boom")
        print("swallow_value_error", True)
    except Exception as exc:
        print("swallow_value_error", type(exc).__name__)

    try:
        async with contextlib.AsyncExitStack() as stack:
            stack.push_async_callback(callback_raises, "boom")
    except Exception as exc:
        print("push_async_callback", type(exc).__name__, "callback:boom" in str(exc))

    async with contextlib.AsyncExitStack() as stack:
        stack.push_async_exit(exit_suppress)
        raise RuntimeError("hidden")
    print("push_async_exit_suppress", True)

    try:
        async with contextlib.AsyncExitStack() as stack:
            await stack.enter_async_context(BadAsyncContext())
    except Exception as exc:
        print("enter_async_context_bad", type(exc).__name__)


asyncio.run(main())
