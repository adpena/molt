import asyncio

from molt import CancellationToken, cancelled, set_current_token


async def main() -> None:
    token = CancellationToken()
    prev = set_current_token(token)
    print(cancelled())
    token.cancel()
    await asyncio.sleep(0)
    print(cancelled())
    child = token.child()
    print(child.cancelled())
    set_current_token(prev)
    print(cancelled())


asyncio.run(main())
