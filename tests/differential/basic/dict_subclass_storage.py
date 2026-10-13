"""Purpose: dict subclass __dict__ should not expose mapping entries."""


class D(dict):
    pass


d = D()
d["a"] = 1
d.b = 2

print(d["a"])
print(d.b)
print("a" in d.__dict__)
print(sorted(d.__dict__.items()))

try:
    print(d["b"])
    print("mapping-attr-ok")
except KeyError:
    print("mapping-attr-keyerror")


# Native descriptors share the dictionary-subclass storage owner. Attribute
# storage remains separate, and explicit base calls bypass subclass overrides.
class ClearOverride(D):
    def clear(self):
        raise AssertionError("override")

    def __delitem__(self, key):
        raise AssertionError("item deletion override")

    def clear_base(self):
        return super().clear()


for clear_mode in ("inherited", "descriptor", "super"):
    mapping = D(a=1, b=2) if clear_mode == "inherited" else ClearOverride(a=1, b=2)
    mapping.note = "attribute survives"
    view = mapping.items()
    if clear_mode == "inherited":
        result = mapping.clear()
    elif clear_mode == "descriptor":
        result = dict.clear(mapping)
    else:
        result = mapping.clear_base()
    print("clear-subclass", clear_mode, result, list(view), mapping.note)
    mapping["fresh"] = 3
    print("clear-live-view", list(view), dict.copy(mapping))
    dict.clear(mapping)
    dict.clear(mapping)
    print("clear-empty", len(mapping))


class MerelyHasDict:
    pass


foreign = MerelyHasDict()
foreign.saved = 7
for value in (None, [], foreign):
    try:
        dict.clear(value)
    except TypeError:
        print("clear-refusal", type(value).__name__)
print("clear-foreign-unchanged", foreign.saved)

# Child retirement sees the complete empty publication, and a reentrant write
# survives clearing. The dictionary owns the value's last reference.
clear_events = []
owner = D()


class RetiredValue:
    def __del__(self):
        clear_events.append((len(owner), list(owner.items())))
        owner["reborn"] = 42


owner["old"] = RetiredValue()
print("clear-retirement-return", owner.clear())
print("clear-retirement", clear_events, list(owner.items()))


# Equality slots use the same backing admission on both operands, including
# reflected fallback when a Python override declines. Explicit descriptors
# bypass outer overrides but still invoke element comparison normally.
class DecliningDict(D):
    def __eq__(self, other):
        return NotImplemented

    def __ne__(self, other):
        return NotImplemented


for left, right in (
    (D(a=1), {"a": 1}),
    ({"a": 1}, D(a=1)),
    (D(a=1), D(a=1)),
    (DecliningDict(a=1), {"a": 1}),
    ({"a": 1}, DecliningDict(a=1)),
):
    print(
        "dict-subclass-equality",
        left == right,
        left != right,
        dict.__eq__(left, right),
        dict.__ne__(left, right),
    )
    right["extra"] = 2
    print("dict-subclass-unequal", left == right, left != right)


class OverrideDict(D):
    def __eq__(self, other):
        return "custom equality"

    def __ne__(self, other):
        return "custom inequality"

    def __iter__(self):
        raise AssertionError("iteration override")

    def __getitem__(self, key):
        raise AssertionError("lookup override")


overridden = OverrideDict(a=1)
print("dict-outer-override", overridden == {"a": 1}, {"a": 1} != overridden)
print(
    "dict-base-equality",
    dict.__eq__(overridden, {"a": 1}),
    dict.__ne__({"a": 1}, overridden),
)
print(
    "dict-equality-foreign",
    dict.__eq__(D(), foreign) is NotImplemented,
    dict.__lt__(D(), {}) is NotImplemented,
)
try:
    dict.__eq__(foreign, {})
except TypeError:
    print("dict-equality-receiver-refusal", foreign.saved)

comparison_error = RuntimeError("element equality")


class BadEquality:
    def __eq__(self, other):
        raise comparison_error


try:
    dict.__eq__(D(key=BadEquality()), {"key": object()})
except RuntimeError as error:
    print("dict-equality-error", error is comparison_error)
