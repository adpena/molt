# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,process.exec,os.kill,os.waitpid
"""Purpose: differential coverage for asyncio subprocess cancellation during I/O.

The child is this program started again with a `child` argument. A compiled
program is its own executable and runs no `-c` source (dynamic execution is
outside the verified subset): started as `sys.executable -c ...`, it would run
this program again, and that copy would start another.
"""

import asyncio
import sys
import time


def child() -> None:
    print("ready", flush=True)
    time.sleep(2)


async def main() -> None:
    program = sys.argv[0]
    # CPython runs the script through its interpreter; a compiled program is
    # its own executable.
    command = [sys.executable, program] if program.endswith(".py") else [program]
    proc = await asyncio.create_subprocess_exec(
        *command,
        "child",
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


if sys.argv[1:2] == ["child"]:
    child()
else:
    asyncio.run(main())
