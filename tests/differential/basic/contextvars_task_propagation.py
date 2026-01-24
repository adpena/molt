"""Purpose: differential coverage for contextvars task propagation."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def child(label: str) -> tuple[str, str]:
    await asyncio.sleep(0)
    return label, var.get()


async def main() -> None:
    out: list[tuple[str, str]] = []

    var.set("main")
    t1 = asyncio.create_task(child("t1"))

    var.set("mutated")
    t2 = asyncio.create_task(child("t2"))

    out.extend(await asyncio.gather(t1, t2))

    ctx = contextvars.copy_context()
    var.set("changed")
    seen: list[str] = []

    def show() -> None:
        seen.append(var.get())

    ctx.run(show)
    out.append(("ctx", seen[0]))

    token = var.set("token")
    var.reset(token)
    try:
        var.reset(token)
    except Exception as exc:
        out.append(("reset_twice", type(exc).__name__))

    print(sorted(out))


asyncio.run(main())
