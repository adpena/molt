"""Purpose: a write of one canonical `sys.modules` row redirects only that row.

The program replaces `sys.modules["inspect"]`. Only `import inspect` may yield
something else, so other modules keep their closed-world facts (their globals
may become static cells). Their globals must still behave as CPython's do:
rebinding through a function, reads through the module attribute, and the
redirected import returning the replacement.
"""

import sys
import types

import import_redirect_support.counter as counter

fake = types.ModuleType("inspect")
fake.marker = "replaced"
sys.modules["inspect"] = fake

import inspect  # noqa: E402

print(inspect.marker, inspect is fake)
print(counter.bump(), counter.bump(), counter.counter)
counter.counter = 40
print(counter.bump(), counter.counter)
