"""Frame-reading builtins reached through their live read (HF-776).

Storing the same objects back is a `builtins` member write, so no read below
is proved: the frontend calls the live binding instead of its frame-aware
lowering. The runtime callables must still read the caller's frame in every
function scope kind and at module scope, as CPython's do. A class body is not
covered: Molt's runtime has no class-body frame, so a dynamic call of `locals`,
`vars` or `dir` there reads the enclosing frame on main as well.
"""

import builtins

builtins.globals = builtins.globals
builtins.locals = builtins.locals
builtins.vars = builtins.vars
builtins.dir = builtins.dir
builtins.super = builtins.super

module_value = 1
print(
    "module",
    "module_value" in globals(),
    "module_value" in locals(),
    "module_value" in vars(),
    "module_value" in dir(),
)


class Base:
    def who(self):
        return "base"


class Child(Base):
    def method(self, arg):
        local = 2
        return (
            sorted(locals()),
            sorted(vars()),
            sorted(dir()),
            super().who(),
            "module_value" in globals(),
            local,
        )


print("method", Child().method(3))
print(
    "comprehension",
    [("module_value" in globals(), vars(Base)["who"].__name__) for _ in range(2)],
)


def generator():
    local = 5
    yield sorted(locals())
    yield sorted(dir())
    yield "module_value" in globals(), local


print("generator", list(generator()))


async def coroutine():
    local = 6
    return sorted(locals()), sorted(vars()), local


running = coroutine()
try:
    running.send(None)
except StopIteration as stop:
    print("coroutine", stop.value)


def outer():
    captured = 7

    def inner():
        mine = captured
        return sorted(locals()), sorted(dir()), mine

    return inner()


print("closure", outer())
