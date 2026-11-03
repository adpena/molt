from __future__ import annotations

from molt.host_python import unsupported_frontend_python

# An unsupported interpreter cannot import the guard; the `molt` entry names
# the interpreter and the fix instead of a startup ImportError (HF-759).
if unsupported_frontend_python() is None:
    from molt.pytest_memory_guard_bootstrap import (
        ensure_python_test_memory_guard,
    )

    ensure_python_test_memory_guard()
