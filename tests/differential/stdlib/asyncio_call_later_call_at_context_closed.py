"""Purpose: differential coverage for asyncio call_later/call_at context + closed loop semantics."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def _check_context() -> None:
    loop = asyncio.get_running_loop()
    done: asyncio.Future[None] = loop.create_future()
    seen: dict[str, str] = {}

    var.set("outer")
    ctx = contextvars.copy_context()
    ctx.run(var.set, "override")

    def callback(tag: str) -> None:
        seen[tag] = var.get()
        if len(seen) == 2 and not done.done():
            done.set_result(None)

    loop.call_later(0.0, callback, "later", context=ctx)
    loop.call_at(loop.time(), callback, "at", context=ctx)
    await asyncio.wait_for(done, timeout=1.0)
    print(sorted(seen.items()))


def _check_closed_loop() -> None:
    loop = asyncio.new_event_loop()
    loop.close()
    errors: list[tuple[str, str]] = []
    for name, args in (
        ("call_later", (0.0, lambda: None)),
        ("call_at", (0.0, lambda: None)),
    ):
        try:
            getattr(loop, name)(*args)
        except Exception as exc:
            errors.append((name, type(exc).__name__))
    print(errors)


async def main() -> None:
    await _check_context()
    _check_closed_loop()


asyncio.run(main())
