import _thread


print("imported")
lock = _thread.RLock()
context_value = lock.__enter__()
lock.__exit__(None, None, None)
print("context", context_value, context_value is True)
print("created", lock._is_owned())
print("acquire", lock.acquire(), lock.acquire(), lock._is_owned())
state = lock._release_save()
print("saved", type(state).__name__, state[0], lock._is_owned())
lock._acquire_restore(state)
print("restored", lock._is_owned())
lock.release()
lock.release()
print("released", lock._is_owned())
