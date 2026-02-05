"""Purpose: differential coverage for asyncio subprocess stdio fd redirection."""

import asyncio
import os
import sys
import tempfile


async def main() -> None:
    temp = tempfile.NamedTemporaryFile(mode="w+b", delete=False)
    path = temp.name
    try:
        proc = await asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            "print('fdout')",
            stdout=temp,
        )
        await proc.wait()
        temp.close()
        with open(path, "rb") as handle:
            data = handle.read()
    finally:
        try:
            temp.close()
        except Exception:
            pass
        try:
            os.unlink(path)
        except Exception:
            pass
    text = data.decode("utf-8").replace("\n", "\\n")
    print(text)


asyncio.run(main())
