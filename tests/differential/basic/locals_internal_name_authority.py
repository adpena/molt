"""Legal Python names must never collide with compiler-only binding storage."""

import sys


__molt_globals_builtin__ = "user-global"
__molt_loop = 0
for __molt_loop in range(2):
    __molt_branch = __molt_loop
if __molt_branch:
    __molt_control = "live"

module_view = globals()
print(
    "module",
    module_view["__molt_globals_builtin__"],
    module_view["__molt_loop"],
    module_view["__molt_branch"],
    module_view["__molt_control"],
)


def outer(__molt_closure__, v0):
    __molt_user = v0
    first = locals()
    del __molt_user
    __molt_user = "rebound"

    def inner():
        return __molt_closure__, __molt_user

    second = locals()
    return (
        first is second,
        first.get("__molt_user"),
        second["__molt_user"],
        inner(),
    )


def collision_factory(captured):
    def collision(__molt_closure__):
        return locals()["__molt_closure__"], captured

    return collision


async def tick():
    return None


async def async_probe(self, __molt_user):
    first = locals()
    await tick()
    del __molt_user
    __molt_user = "async-rebound"
    second = locals()
    return first is second, second["self"], second["__molt_user"]


def generator_probe(__molt_user):
    first = locals()
    yield first.get("__molt_user")
    del __molt_user
    __molt_user = "generator-rebound"
    second = locals()
    yield first is second, second["__molt_user"]


def run_coro(coro):
    while True:
        try:
            coro.send(None)
        except StopIteration as stop:
            return stop.value


print("sync", sys.version_info[:2], outer("user-closure", "user-v0"))
print("collision", collision_factory("captured")("public-param"))
print("async", run_coro(async_probe("user-self", "user-async")))
probe = generator_probe("user-generator")
print("generator", next(probe), next(probe))
