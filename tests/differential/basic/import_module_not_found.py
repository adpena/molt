"""CPython parity: a module-not-found import raises ModuleNotFoundError.

ModuleNotFoundError is the ImportError subclass CPython has raised for
"No module named ..." since 3.6. molt previously raised plain ImportError for
the not-found fallback paths (the known-absent path already used
ModuleNotFoundError); this asymmetry is the bug. Output must be byte-identical
to CPython, and `except ImportError` must still catch it (subclass).
"""


def kind(fn):
    try:
        fn()
        return "NO-RAISE"
    except ModuleNotFoundError as e:
        return ("ModuleNotFoundError", str(e))
    except ImportError as e:
        return ("ImportError-only", str(e))


def imp_static():
    import nonexistent_qwerty_module  # noqa: F401


def imp_from():
    from nonexistent_qwerty_pkg import thing  # noqa: F401


print(kind(imp_static))
print(kind(imp_from))
# ModuleNotFoundError must be a subclass of ImportError.
print(issubclass(ModuleNotFoundError, ImportError))
try:
    import another_missing_zzz  # noqa: F401
except ImportError as e:  # subclass catch must work
    print("subclass_catch", type(e).__name__)
