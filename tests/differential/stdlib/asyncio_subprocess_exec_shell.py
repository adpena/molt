"""Purpose: differential coverage for asyncio subprocess exec/shell."""

import asyncio
import os
import shlex
import sys


async def main() -> None:
    proc = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "print('exec')",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out, err = await proc.communicate()

    if os.name == "nt":
        cmd = f'"{sys.executable}" -c "print(\\"shell\\")"'
    else:
        cmd = f"{shlex.quote(sys.executable)} -c {shlex.quote('print("shell")')}"

    proc_shell = await asyncio.create_subprocess_shell(
        cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out_shell, err_shell = await proc_shell.communicate()

    print(out.strip(), err.strip(), out_shell.strip(), err_shell.strip())


asyncio.run(main())
