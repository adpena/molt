import builtins
import concurrent.futures as cf
import sys

print("sys.implementation", getattr(sys, "implementation", None))
print("sys.platform", sys.platform)

submit = getattr(builtins, "molt_thread_submit", None)
print("submit", submit)
print("callable", callable(submit))
print("has_code", hasattr(submit, "__code__"))
print("type", type(submit))

spawn_builtin = getattr(builtins, "molt_thread_spawn", None)
print("builtins.molt_thread_spawn", spawn_builtin)
print("builtins.molt_thread_spawn callable", callable(spawn_builtin))
print("builtins.molt_thread_spawn has_code", hasattr(spawn_builtin, "__code__"))
print("builtins.molt_thread_spawn type", type(spawn_builtin))

registry = getattr(builtins, "_molt_intrinsics", None)
if registry is None:
    print("intrinsics_registry", None)
else:
    print("intrinsics_has_submit", "molt_thread_submit" in registry)
    print("intrinsics_has_spawn", "molt_thread_spawn" in registry)

spawn = getattr(cf, "_MOLT_THREAD_SPAWN", None)
print("cf._MOLT_THREAD_SPAWN", spawn)
print("cf._MOLT_THREAD_SPAWN callable", callable(spawn))
print("cf._MOLT_THREAD_SPAWN has_code", hasattr(spawn, "__code__"))

print("cf._MOLT_THREADPOOL", getattr(cf, "_MOLT_THREADPOOL", None))
