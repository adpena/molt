"""Purpose: a nested or method `async def` binds only its own scope.

A function-local `async def inner` once published `inner` as a module global
hint, so every later function that called a free `inner` from a coroutine or
generator body loaded the missing global and raised NameError. The binders
come first here, so each later user would read the leaked hint.
"""


def drive(coro):
    try:
        coro.send(None)
    except StopIteration as stop:
        return stop.value
    raise AssertionError("suspended")


def async_binder_sync_user(n):
    async def inner():
        return n

    def user():
        return inner

    return drive(user()())


def async_generator_binder(n):
    async def inner():
        yield n

    async def user():
        return [value async for value in inner()]

    return drive(user())


class Service:
    async def close(self):
        return "service closed"


def sync_binder_async_user(n):
    def inner():
        return n

    async def user():
        return inner()

    return drive(user())


def async_binder_async_user(n):
    async def inner():
        return n

    async def user():
        return await inner()

    return drive(user())


def lambda_binder_async_user(n):
    inner = lambda: n  # noqa: E731

    async def user():
        return inner()

    return drive(user())


def generator_user(n):
    def inner():
        return n

    def user():
        yield inner()

    return next(user())


def method_name_stays_a_method(n):
    def close():
        return f"local close {n}"

    async def user():
        return close()

    return drive(user())


def module_global_named_like_a_method():
    try:
        return close()  # noqa: F821
    except NameError as exc:
        return type(exc).__name__


for case in (
    async_binder_sync_user,
    async_generator_binder,
    sync_binder_async_user,
    async_binder_async_user,
    lambda_binder_async_user,
    generator_user,
    method_name_stays_a_method,
):
    print(case.__name__, case(3))
print("service", drive(Service().close()))
print("global close", module_global_named_like_a_method())
