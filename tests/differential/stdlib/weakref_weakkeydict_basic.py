"""Purpose: differential coverage for weakref.WeakKeyDictionary."""

import gc
import weakref


class Key:
    def __init__(self, name):
        self.name = name

    def __hash__(self):
        return hash(self.name)

    def __eq__(self, other):
        return isinstance(other, Key) and self.name == other.name


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

original = Key("equal")
equal = Key("equal")
original_ref = weakref.ref(original)
equal_ref = weakref.ref(equal)
equal_store = weakref.WeakKeyDictionary()
equal_store[original] = "old"
stored_ref = equal_store.keyrefs()[0]
equal_store[equal] = "new"
print(
    "equal-update",
    equal_store[equal],
    equal_store.keyrefs()[0] is stored_ref,
    list(equal_store)[0] is original,
)
del equal
gc.collect()
print("equal-new-unretained", equal_ref() is None, original_ref() is original)
del original
gc.collect()
print("equal-original-death", original_ref() is None, len(equal_store))
