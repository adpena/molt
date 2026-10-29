"""Purpose: a package `__init__` reaches its submodules only from function bodies.

`function_import_lazy_pkg` imports `.helper` and `.json` only inside its
functions. CPython runs a submodule's body at the first call and then binds it
as a package attribute. The package's own `.json` submodule must not be
confused with the standard library `json` that this program also imports.
"""

import json
import sys

import function_import_lazy_pkg as pkg

print(pkg.__name__ + ".helper" in sys.modules, hasattr(pkg, "helper"))
print(pkg.value())
with pkg.scoped() as got:
    print(got)
print(pkg.__name__ + ".helper" in sys.modules, hasattr(pkg, "helper"))
print(pkg.shadowed_json([1]))
print(json.dumps([1]))
