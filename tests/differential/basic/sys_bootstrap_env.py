"""Purpose: differential coverage for sys bootstrap env wiring."""

# MOLT_ENV: PYTHONPATH=src:tests/differential/basic

import sys


def _norm(path: str) -> str:
    return path.replace("\\", "/")


paths = [_norm(entry) for entry in sys.path if isinstance(entry, str)]
print(any(entry.endswith("/src") or entry == "src" for entry in paths))
print(
    any(
        entry.endswith("/tests/differential/basic")
        or entry == "tests/differential/basic"
        for entry in paths
    )
)
print(isinstance(getattr(sys, "_molt_bootstrap_module_roots", ()), tuple))
print(isinstance(getattr(sys, "_molt_bootstrap_include_cwd", False), bool))
print(
    getattr(sys, "_molt_bootstrap_stdlib_root", None) is None
    or isinstance(getattr(sys, "_molt_bootstrap_stdlib_root", None), str)
)
