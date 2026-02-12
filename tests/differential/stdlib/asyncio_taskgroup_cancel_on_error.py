"""Purpose: differential coverage for TaskGroup cancel-on-error behavior."""

import asyncio


async def boom() -> None:
    await asyncio.sleep(0)
    raise ValueError("boom")


async def sleeper(state: dict[str, bool]) -> None:
    try:
        await asyncio.sleep(1)
    except asyncio.CancelledError:
        state["cancelled"] = True
        raise


async def main() -> None:
    state = {"cancelled": False}
    errors: list[str] = []
    try:
        async with asyncio.TaskGroup() as tg:
            tg.create_task(sleeper(state))
            tg.create_task(boom())
    except* ValueError as exc:
        errors = sorted(str(err) for err in exc.exceptions)
    print(state["cancelled"], errors)


asyncio.run(main())
