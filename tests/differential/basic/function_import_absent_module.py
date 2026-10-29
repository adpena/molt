"""Purpose: an import inside a function runs only when that function runs.

CPython executes `import` and `from ... import` in a function body when the
body runs. `hf557_absent_module` does not exist: a function that never runs
cannot stop the program, and a function that runs raises ModuleNotFoundError
at its import statement, which the caller can catch.
"""

import contextlib


def unreached_call():
    from hf557_absent_module import helper

    return helper(1)


@contextlib.contextmanager
def absent_context():
    from hf557_absent_module import scope

    with scope() as value:
        yield value


async def unreached_coroutine():
    from hf557_absent_module import helper

    return helper(2)


def reached_import():
    try:
        from hf557_absent_module import helper
    except ModuleNotFoundError as exc:
        return type(exc).__name__, exc.name, str(exc)
    return helper(3)


def reached_context():
    try:
        with absent_context() as value:
            return value
    except ImportError as exc:
        return type(exc).__name__, exc.name


try:
    from hf557_absent_module import fallback
except ImportError:
    fallback = None


def call_fallback():
    if fallback is None:
        return "no fallback"
    return fallback()


print(callable(unreached_call), callable(absent_context), callable(unreached_coroutine))
print(reached_import())
print(reached_context())
print(call_fallback())
