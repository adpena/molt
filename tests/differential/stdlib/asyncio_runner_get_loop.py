"""Purpose: differential coverage for asyncio.Runner loop access."""

import asyncio


async def main(loop_id: int) -> bool:
    return id(asyncio.get_running_loop()) == loop_id


with asyncio.Runner() as runner:
    loop = runner.get_loop()
    same = runner.run(main(id(loop)))
    closed_inside = loop.is_closed()

closed_after = loop.is_closed()
print(same, closed_inside, closed_after)
