import asyncio


async def ok(val: int) -> int:
    await asyncio.sleep(0)
    return val


async def boom() -> int:
    await asyncio.sleep(0)
    raise ValueError("boom")


async def main() -> None:
    res = await asyncio.gather(ok(1), ok(2))
    print(f"gather_ok:{res}")
    try:
        await asyncio.gather(boom(), ok(3))
    except Exception as exc:
        print(f"gather_err:{type(exc).__name__}:{exc}")
    res = await asyncio.gather(boom(), ok(4), return_exceptions=True)
    print(f"gather_exc:{type(res[0]).__name__}:{res[1]}")


asyncio.run(main())
