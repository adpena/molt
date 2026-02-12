"""Purpose: invalid subprocess stdio options should not surface NotImplementedError."""

import asyncio


class _BadStdio:
    pass


async def main() -> None:
    try:
        await asyncio.create_subprocess_exec("sh", "-c", "true", stdin=_BadStdio())
    except Exception as exc:
        print("is_notimplemented", isinstance(exc, NotImplementedError))
    else:
        print("is_notimplemented", False)


asyncio.run(main())
