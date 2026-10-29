"""Purpose: a reached function imports a sibling module of its own package.

`function_import_sibling_pkg.runner` imports `.sibling` only inside function
bodies (a plain function, a context-manager generator, a coroutine and a
`from . import` of the module itself), as CPython's `test.support` imports
`.os_helper`. CPython executes the sibling's body at the first call, once.
"""

import sys

from function_import_sibling_pkg import runner

SIBLING = "function_import_sibling_pkg.sibling"

print("before", SIBLING in sys.modules)
print(runner.compute_twice(1))
with runner.sibling_scope("x") as value:
    print(value)
coroutine = runner.sibling_coroutine(2)
try:
    coroutine.send(None)
except StopIteration as stop:
    print(stop.value)
print(runner.sibling_module_name())
print("after", SIBLING in sys.modules)
