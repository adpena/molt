"""CPython parity: `from M import name` for a missing *name* raises ImportError.

CPython's IMPORT_FROM is not the same as a plain ``M.name`` attribute read: a
missing imported name raises ``ImportError("cannot import name 'name' from 'M'
(origin)")`` (after a ``sys.modules`` submodule-fallback used for circular
imports), NOT the ``AttributeError`` that ``M.name`` raises. molt previously
lowered ``from M import name`` through the generic module-attribute path and so
raised ``AttributeError`` — this is the bug.

The trailing ``(origin)`` suffix is environment-specific (``(unknown location)``
for a built-in module, an absolute ``.py`` path for a file module — and it
differs between any two installations / implementations), so this test asserts
the portable, byte-stable contract: the exception *type* (``ImportError``), the
message *prefix* (``cannot import name 'name' from 'module'``), and that the
miss is catchable as ``ImportError``. A name that *does* exist must still bind.
"""


def kind(fn):
    try:
        fn()
        return "NO-RAISE"
    except ModuleNotFoundError as e:
        # A missing *name* is ImportError, never the ModuleNotFoundError
        # subclass (which is reserved for a missing *module*).
        return ("ModuleNotFoundError", str(e).split(" (")[0])
    except ImportError as e:
        return ("ImportError", str(e).split(" (")[0])
    except AttributeError as e:
        return ("AttributeError", str(e))


def from_builtin_module_missing():
    from sys import this_name_does_not_exist_qwerty  # noqa: F401


def from_file_module_missing():
    from os import another_missing_name_zzz  # noqa: F401


def from_present_name():
    # A name that exists must still bind correctly.
    from sys import maxsize

    return maxsize


print(kind(from_builtin_module_missing))
print(kind(from_file_module_missing))
print("present_ok", from_present_name() > 0)
print("import_error_is_exception", issubclass(ImportError, Exception))

# The miss must be catchable through the ImportError base class.
try:
    from sys import yet_another_missing_abc  # noqa: F401
except ImportError as e:
    print("caught_as_ImportError", type(e).__name__)
