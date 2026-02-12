"""Purpose: differential coverage for asyncio exception handler + debug."""

import asyncio


async def main() -> None:
    loop = asyncio.get_running_loop()
    recorded: list[tuple[str, str]] = []

    def handler(_loop: asyncio.AbstractEventLoop, context: dict) -> None:
        message = context.get("message", "")
        exc = context.get("exception")
        recorded.append((message, type(exc).__name__ if exc else "None"))

    loop.set_exception_handler(handler)
    loop.set_debug(True)
    loop.call_exception_handler({"message": "boom", "exception": RuntimeError("fail")})
    await asyncio.sleep(0)
    print(recorded)


asyncio.run(main())
