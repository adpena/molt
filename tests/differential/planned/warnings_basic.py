"""Purpose: differential coverage for warnings basic."""

import warnings


def warn_once():
    warnings.warn("hello", UserWarning)


with warnings.catch_warnings(record=True) as rec:
    warnings.simplefilter("always")
    warn_once()
    warn_once()
    print(len(rec))
    print(type(rec[0].message).__name__)
