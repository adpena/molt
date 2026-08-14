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
obsolete_ref = replacements.valuerefs()[0]
replacements[new_key] = replacement_value
replacement_ref = replacements.valuerefs()[0]
print(
    "wvd-keyed-ref-surface",
    type(replacement_ref) is weakref.KeyedRef,
    replacement_ref.key is new_key,
    not hasattr(replacement_ref, "__dict__"),
)
obsolete_ref.__callback__(obsolete_ref)
replacement_ref.__callback__(replacement_ref)
del old_value
gc.collect()
print(
    "wvd-replacement-cookie",
    obsolete_ref is not replacement_ref,
    replacements[new_key] is replacement_value,
)
del old_key, new_key
gc.collect()
replacement_keys = list(replacements)
print(
    "wvd-equal-key-identity",
    replacement_keys[0] is old_key_ref(),
    old_key_ref() is not None,
    new_key_ref() is not None,
)
del replacement_keys, replacement_ref, obsolete_ref, replacement_value
gc.collect()
print("wvd-equal-key-release", old_key_ref() is None, new_key_ref() is None)

class CountingHash:
    def __init__(self):
        self.calls = 0

    def __hash__(self):
        self.calls += 1
        return 313


hash_key = CountingHash()
hash_keys = weakref.WeakKeyDictionary()
hash_keys[hash_key] = "value"
hash_ref = hash_keys.keyrefs()[0]
print("wkd-native-hash-insert", hash_key.calls, hasattr(hash_ref, "_hash"))
del hash_key
gc.collect()
print("wkd-native-hash-dead-first", hash(hash_ref))

same_old_key = EqualKey("same-value")
same_new_key = EqualKey("same-value")
same_old_ref = weakref.ref(same_old_key)
same_new_ref = weakref.ref(same_new_key)
same_value = Value()
same_values = weakref.WeakValueDictionary()
same_values[same_old_key] = same_value
same_old_value_ref = same_values.valuerefs()[0]
same_values[same_new_key] = same_value
same_new_value_ref = same_values.valuerefs()[0]
del same_old_key, same_new_key
gc.collect()
same_keys = list(same_values)
print(
    "wvd-same-value-key-identity",
    same_keys[0] is same_old_ref(),
    same_old_ref() is not None,
    same_new_ref() is not None,
    same_old_value_ref is not same_new_value_ref,
    same_new_value_ref.key is same_new_ref(),
)
del same_keys, same_old_value_ref, same_new_value_ref, same_value
gc.collect()
print("wvd-same-value-key-release", same_old_ref() is None, same_new_ref() is None)

late_key = EqualKey("late-hash")
late_value = Value()
late_values = weakref.WeakValueDictionary({late_key: late_value})
late_ref = late_values.valuerefs()[0]
del late_value
gc.collect()
try:
    hash(late_ref)
except TypeError as exc:
    print("wvd-value-hash-not-preseeded", type(exc).__name__)

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

equal_first = Node(4)
equal_second = Node(4)
equal_first_ref = weakref.ref(equal_first)
equal_second_ref = weakref.ref(equal_second)
equal_set = weakref.WeakSet([equal_first])
equal_container_ref = next(
    ref for ref in weakref.getweakrefs(equal_first) if ref.__callback__ is not None
)
equal_set.add(equal_second)
print(
    "ws-equal-retains-original",
    len(equal_set),
    equal_container_ref()
    is equal_first,
)
del equal_second
gc.collect()
print("ws-equal-new-unretained", equal_second_ref() is None, len(equal_set))
del equal_first
gc.collect()
print("ws-equal-original-death", equal_first_ref() is None, len(equal_set))

hash_item = CountingHash()
hash_set = weakref.WeakSet()
hash_set.add(hash_item)
hash_item_ref = weakref.getweakrefs(hash_item)[0]
print("ws-native-hash-insert", hash_item.calls, hasattr(hash_item_ref, "_hash"))
del hash_item
gc.collect()
print("ws-native-hash-dead-first", hash(hash_item_ref))
