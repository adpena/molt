"""Purpose: differential coverage for asyncio taskgroup error order."""

import asyncio


async def boom(label: str) -> None:
    await asyncio.sleep(0)
    raise ValueError(label)


async def main() -> None:
    errors: list[str] = []
    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(boom("a"))
            tg.create_task(boom("b"))
    except* ValueError as exc:
        errors.extend(sorted(str(e) for e in exc.exceptions))
    print(errors)


asyncio.run(main())
