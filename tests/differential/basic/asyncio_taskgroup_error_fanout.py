import asyncio


async def _boom(label: str) -> None:
    await asyncio.sleep(0.0)
    raise ValueError(label)


async def _main() -> None:
    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(_boom("one"))
            tg.create_task(_boom("two"))
    except* ValueError as group:
        print(sorted(str(exc) for exc in group.exceptions))


asyncio.run(_main())
