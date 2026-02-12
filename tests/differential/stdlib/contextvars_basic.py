"""Purpose: differential coverage for contextvars basic."""

import contextvars

var = contextvars.ContextVar("var", default="default")
print(var.get())
token = var.set("value")
print(var.get())
var.reset(token)
print(var.get())

var2 = contextvars.ContextVar("var2")
try:
    var2.get()
except LookupError as exc:
    print(type(exc).__name__)

var.set("main")
ctx = contextvars.copy_context()
var.set("mutated")


def show() -> None:
    print(var.get())


ctx.run(show)
print(var.get())
