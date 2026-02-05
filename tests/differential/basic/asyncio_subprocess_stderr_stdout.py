"""Purpose: differential coverage for asyncio stderr=STDOUT redirection."""

import asyncio
import sys


async def main() -> None:
    proc = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        "import sys; sys.stderr.write('err\\n'); sys.stderr.flush(); sys.stdout.write('out\\n'); sys.stdout.flush()",
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,
    )
    out, err = await proc.communicate()
    out_text = out.decode("utf-8").replace("\n", "\\n") if out is not None else "None"
    err_text = "None" if err is None else err.decode("utf-8").replace("\n", "\\n")
    print(out_text, err_text)


asyncio.run(main())
