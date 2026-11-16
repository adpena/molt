"""Purpose: `del sys.modules[name]` then `import name` makes a second module.

CPython executes the body again for a new module object; the old object keeps
its own globals. A static-cell lowering that shares one set of cells between
the two objects would print the second body's values through the first object
(HF-877). Any program write of `sys.modules[name]` makes that row a redirect
writer, so the module keeps its dictionary namespace.
"""

import sys

import import_redirect_support.counter as old

old.bump()
old.bump()
del sys.modules["import_redirect_support.counter"]
import import_redirect_support.counter as new  # noqa: E402

print(old is new, old.counter, new.counter)
new.bump()
print(old.counter, new.counter, old.bump(), new.counter)
