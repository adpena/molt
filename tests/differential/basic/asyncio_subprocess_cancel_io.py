"""Purpose: differential coverage for asyncio subprocess cancellation during I/O."""

import asyncio
import sys


async def main() -> None:
    proc = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import time; print('ready'); time.sleep(2)",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    assert proc.stdout is not None
    await proc.stdout.readline()

    task = asyncio.create_task(proc.communicate())
    await asyncio.sleep(0)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        cancelled = True
    else:
        cancelled = False

    proc.kill()
    await proc.wait()
    print(cancelled, proc.returncode is not None)


asyncio.run(main())
