"""Purpose: differential coverage for contextvars propagation into tasks."""

import asyncio
import contextvars


var = contextvars.ContextVar("var", default="unset")


async def main() -> None:
    var.set("task-value")

    async def read_var() -> str:
        return var.get()

    task = asyncio.create_task(read_var())
    print(await task)


asyncio.run(main())


async def context_interleaving() -> None:
    events = []
    explicit = contextvars.Context()
    explicit.run(var.set, "selected")

    async def child(label):
        events.append((label, "before", var.get()))
        var.set(label)
        await asyncio.sleep(0)
        events.append((label, "after", var.get()))

    parent_value = var.get()
    task = asyncio.create_task(child("owned"), context=explicit)
    await asyncio.sleep(0)
    events.append(("parent", "during", var.get() == parent_value))
    await task
    print("interleaving", events, explicit[var])
    print("selected-identity", task.get_context() is explicit)

    # Two sequential Tasks may select the same Context; lifetime-long entry
    # would leave it entered and reject the second selection.
    await asyncio.create_task(child("second"), context=explicit)
    print("sequential-task", explicit[var])

asyncio.run(context_interleaving())
