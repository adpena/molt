"""Purpose: differential coverage for lowered weakref container methods."""

import gc
import sys
import weakref


class Value:
    pass


value = Value()
values = weakref.WeakValueDictionary()
values["a"] = value
print("wvd-len", len(values))
print("wvd-get", values["a"] is value)
print("wvd-refs", len(values.valuerefs()))
print("wvd-iterrefs", len(list(values.itervaluerefs())))
print("wvd-pop", values.pop("a") is value)
print("wvd-len", len(values))
values["a"] = value
del value
gc.collect()
print("wvd-len-gc", len(values))


class EqualKey:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return hash(self.value)

    def __eq__(self, other):
        return isinstance(other, EqualKey) and self.value == other.value


old_key = EqualKey("same")
new_key = EqualKey("same")
old_key_ref = weakref.ref(old_key)
new_key_ref = weakref.ref(new_key)
old_value = Value()
replacement_value = Value()
replacements = weakref.WeakValueDictionary()
replacements[old_key] = old_value
replacements[new_key] = replacement_value
del old_key, new_key
gc.collect()
replacement_keys = list(replacements)
print(
    "wvd-equal-key-identity",
    replacement_keys[0] is old_key_ref(),
    old_key_ref() is not None,
    new_key_ref() is not None,
)
del replacement_keys, replacement_value
gc.collect()
print("wvd-equal-key-release", old_key_ref() is None, new_key_ref() is None)

same_old_key = EqualKey("same-value")
same_new_key = EqualKey("same-value")
same_old_ref = weakref.ref(same_old_key)
same_new_ref = weakref.ref(same_new_key)
same_value = Value()
same_values = weakref.WeakValueDictionary()
same_values[same_old_key] = same_value
same_values[same_new_key] = same_value
del same_old_key, same_new_key
gc.collect()
same_keys = list(same_values)
print(
    "wvd-same-value-key-identity",
    same_keys[0] is same_old_ref(),
    same_old_ref() is not None,
    same_new_ref() is not None,
)
del same_keys, same_value
gc.collect()
print("wvd-same-value-key-release", same_old_ref() is None, same_new_ref() is None)

first = Value()
second = Value()
third = Value()
ordered = weakref.WeakValueDictionary()
ordered["first"] = first
ordered["second"] = second
ordered["third"] = third
print("wvd-popitem-lifo", ordered.popitem()[0])

guarded = weakref.WeakValueDictionary()
guarded["first"] = first
guarded["second"] = second
guard = iter(guarded.items())
print("wvd-iter-first", next(guard)[0])
del second
gc.collect()
remaining = list(guard)
print("wvd-iter-death", len(guarded), remaining)
guarded["third"] = third
if sys.version_info < (3, 14):
    len_first = Value()
    len_second = Value()
    len_guarded = weakref.WeakValueDictionary(
        {"first": len_first, "second": len_second}
    )
    len_iter = iter(len_guarded)
    next(len_iter)
    del len_second
    gc.collect()
    observed_len = len(len_guarded)
    try:
        next(len_iter)
    except RuntimeError as exc:
        print("wvd-iter-len-mutation", observed_len, str(exc))

    mutated = iter(guarded)
    next(mutated)
    guarded["fourth"] = Value()
    try:
        next(mutated)
    except RuntimeError as exc:
        print("wvd-iter-mutation", str(exc))


class Node:
    def __init__(self, n):
        self.n = n

    def __hash__(self):
        return hash(self.n)

    def __eq__(self, other):
        return isinstance(other, Node) and self.n == other.n


n1 = Node(1)
n2 = Node(1)
weak_set = weakref.WeakSet()
weak_set.add(n1)
weak_set.add(n2)
print("ws-len", len(weak_set))
print("ws-contains", n1 in weak_set, n2 in weak_set)
print("ws-pop", isinstance(weak_set.pop(), Node))
print("ws-len2", len(weak_set))
weak_set.add(n1)
weak_set.discard(n2)
print("ws-len3", len(weak_set))
