"""Destructuring outputs must release their independent owned references."""

import gc
import weakref


class Box:
    pass


def consume_items(mapping):
    for key, value in mapping.items():
        if key is value:
            raise AssertionError("dict item key unexpectedly aliases its value")


def unpack_lifetime_refs():
    first = Box()
    second = Box()
    first_ref = weakref.ref(first)
    second_ref = weakref.ref(second)
    mapping = {"first": first, "second": second}
    consume_items(mapping)
    return first_ref, second_ref


first_ref, second_ref = unpack_lifetime_refs()
gc.collect()
print("unpack-released", first_ref() is None, second_ref() is None)
