import builtins
import threading

print("have_intrinsics", getattr(threading, "_HAVE_INTRINSICS", None))
print("molt_thread_spawn", callable(getattr(builtins, "molt_thread_spawn", None)))
print("molt_lock_new", callable(getattr(builtins, "molt_lock_new", None)))
