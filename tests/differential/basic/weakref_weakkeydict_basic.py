"""Purpose: differential coverage for weakref.WeakKeyDictionary."""

import gc
import weakref


class Key:
    def __init__(self, name):
        self.name = name


k1 = Key("a")
store = weakref.WeakKeyDictionary()
store[k1] = 1
print(list(store.items())[0][1])

k1_ref = weakref.ref(k1)

# Drop the key and force collection.

del k1

gc.collect()

print(k1_ref() is None)
print(list(store.items()))
