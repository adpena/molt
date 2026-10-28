# MOLT_META: backends=native,wasm
"""Purpose: warnings carry CPython's location: stacklevel, file, line, module, registry."""

import os
import warnings

import warnings_location_support as support


def show(records):
    for record in records:
        print(
            os.path.basename(record.filename),
            record.lineno,
            record.category.__name__,
            str(record.message),
        )


def registry_keys(namespace):
    registry = namespace.get("__warningregistry__", {})
    return sorted(
        (key[0], key[1].__name__, key[2]) for key in registry if key != "version"
    )


def level_one():
    warnings.warn("level one", UserWarning)


def level_two():
    warnings.warn("level two", UserWarning, stacklevel=2)


def level_three_inner():
    warnings.warn("level three", UserWarning, stacklevel=3)


def level_three_outer():
    level_three_inner()


print("stacklevel")
with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("always")
    level_one()
    level_two()
    level_three_outer()
    warnings.warn("module level", UserWarning)
    support.warn_here()
    support.warn_caller()
show(records)

print("module filter")
with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("always")
    warnings.filterwarnings(
        "ignore", category=DeprecationWarning, module="warnings_location_support"
    )
    support.deprecated_here()
    support.deprecated_caller()
    warnings.warn("main deprecated", DeprecationWarning)
show(records)

print("main filter")
with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("always")
    warnings.filterwarnings("ignore", category=DeprecationWarning, module="__main__")
    support.deprecated_here()
    support.deprecated_caller()
show(records)


def emit_twice():
    warnings.warn("from a function", UserWarning, stacklevel=2)


def call_once_site():
    emit_twice()


print("default dedup")
with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("default")
    call_once_site()
    call_once_site()
    emit_twice()
    emit_twice()
    support.warn_here()
    support.warn_here()
show(records)
print(registry_keys(globals()))
print(registry_keys(vars(support)))

print("module action")
with warnings.catch_warnings(record=True) as records:
    warnings.simplefilter("module")
    emit_twice()
    emit_twice()
show(records)
