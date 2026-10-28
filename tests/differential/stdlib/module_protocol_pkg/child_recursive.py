"""A source initializer observes deletion before importing itself again."""

import importlib
import sys
import module_protocol_pkg as package

package.recursive_runs += 1
generation = package.recursive_runs
package.events.append(("recursive-body", generation))

if package.recursive_mode == "private":
    if generation == 1:
        package.private_nested = importlib.import_module("module_protocol_pkg.child_recursive")
    else:
        value = 42
elif package.recursive_mode in ("cleanup-key-error", "cleanup-runtime-error"):
    original = sys.modules[__name__]
    del sys.modules[__name__]
    sys.modules[package.cleanup_key] = original
    package.cleanup_armed = True
    raise package.recursive_failure

elif generation == 1:
    package.recursive_outer = sys.modules[__name__]
    if package.recursive_mode != "present":
        del sys.modules[__name__]
    try:
        nested = importlib.import_module("module_protocol_pkg.child_recursive")
    except ValueError as error:
        package.recursive_error = error
        if package.recursive_mode != "inner-caught":
            raise
    else:
        package.recursive_nested = nested
        if package.recursive_mode == "outer-replace":
            sys.modules[__name__] = 42
        if package.recursive_mode == "outer-none":
            sys.modules[__name__] = None
        if package.recursive_mode in ("outer-failure", "outer-replace", "outer-none"):
            raise package.recursive_failure
elif package.recursive_mode in ("inner-caught", "inner-uncaught"):
    raise package.recursive_failure
