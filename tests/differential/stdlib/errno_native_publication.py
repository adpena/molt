# MOLT_META: backends=llvm,native
"""Exact target-CPython public errno namespace and reverse insertion policy.

This cell covers native C-header publication. Molt's synthetic wasm32 host-import
namespace has a distinct contract and retains errno_basic as its public witness.
"""
import errno

names = sorted(
    name for name in dir(errno)
    if (name.startswith("E") or name.startswith("WSA"))
    and isinstance(getattr(errno, name), int)
)
print([(name, getattr(errno, name)) for name in names])
print(list(errno.errorcode.items()))
# Introspection above and the facade's explicit import surface share values.
from errno import *

namespace = globals()
print([(name, namespace[name]) for name in names])
