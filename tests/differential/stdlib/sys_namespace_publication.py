"""Sys exports are real dictionary entries and never materialize on a miss."""
import sys


namespace = vars(sys)
for name in (
    "flags", "implementation", "version_info", "float_info", "int_info", "hash_info",
    "thread_info", "stdlib_module_names", "builtin_module_names", "orig_argv",
    "stdin", "stdout", "stderr", "__stdin__", "__stdout__", "__stderr__",
    "getrecursionlimit", "setrecursionlimit", "exc_info", "_getframe", "get_asyncgen_hooks",
    "set_asyncgen_hooks", "displayhook", "__displayhook__", "excepthook", "__excepthook__",
    "unraisablehook", "__unraisablehook__", "addaudithook", "audit",
):
    print("published", name, name in namespace, namespace.get(name) is getattr(sys, name))

print("shapes", hasattr(sys.version_info, "major"), hasattr(sys.implementation, "name"),
      hasattr(sys.float_info, "max"), isinstance(sys.stdlib_module_names, frozenset))
saved_flags = sys.flags
saved_limit = sys.getrecursionlimit
saved_hook = sys.displayhook
original_path = sys.path
try:
    marker = object()
    sys.path = ["user-path"]
    sys.displayhook = marker
    del sys.flags
    del sys.getrecursionlimit
    print("user-values", sys.path == ["user-path"], sys.displayhook is marker,
          hasattr(sys.version_info, "major"))
    print("deletion", hasattr(sys, "flags"), hasattr(sys, "getrecursionlimit"),
          "flags" in namespace, "getrecursionlimit" in namespace)
finally:
    sys.flags = saved_flags
    sys.getrecursionlimit = saved_limit
    sys.displayhook = saved_hook
    sys.path = original_path
