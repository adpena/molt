import builtins

names = [
    "molt_thread_spawn",
    "molt_thread_join",
    "molt_thread_is_alive",
    "molt_thread_ident",
    "molt_thread_native_id",
    "molt_thread_current_ident",
    "molt_thread_current_native_id",
    "molt_thread_drop",
    "molt_lock_new",
    "molt_lock_acquire",
    "molt_lock_release",
    "molt_lock_locked",
    "molt_lock_drop",
    "molt_rlock_new",
    "molt_rlock_acquire",
    "molt_rlock_release",
    "molt_rlock_locked",
    "molt_rlock_drop",
    "molt_module_cache_set",
]

for name in names:
    val = getattr(builtins, name, None)
    print(name, "present", bool(val), "callable", callable(val))
