"""Purpose: differential coverage for asyncio.Runner context semantics."""

import asyncio
import contextvars


runner_var = contextvars.ContextVar("runner_var", default="unset")


async def read_runner_var() -> str:
    return runner_var.get()


runner_var.set("runner-base")
with asyncio.Runner() as runner:
    first = runner.run(read_runner_var())

    runner_var.set("caller-mutated")
    second = runner.run(read_runner_var())

    explicit_context = contextvars.copy_context()
    explicit_context.run(runner_var.set, "explicit")
    third = runner.run(read_runner_var(), context=explicit_context)

    fourth = runner.run(read_runner_var())

print(first, second, third, fourth, runner_var.get())
