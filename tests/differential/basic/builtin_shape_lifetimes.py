"""Call identity, argument lifetime and constructor-result boundary capsule."""

from builtins import float as imported_float

events = []


def split_argument(label, value):
    events.append(label)
    return value


class SplitCallable:
    def __call__(self, *args, **keywords):
        events.append("split-called")
        return args, keywords

    def __del__(self):
        events.append("split-callable-released")


class DetachedSplitReceiver:
    @property
    def split(self):
        events.append("split-lookup")
        return SplitCallable()

    def __del__(self):
        events.append("split-receiver-released")


split_result = DetachedSplitReceiver().split(
    maxsplit=split_argument("split-maxsplit", 2),
    sep=split_argument("split-separator", "|"),
)
assert split_result == ((), {"maxsplit": 2, "sep": "|"})
assert events == [
    "split-lookup",
    "split-receiver-released",
    "split-maxsplit",
    "split-separator",
    "split-called",
    "split-callable-released",
]
print("split-capture", events)
events.clear()


def suspended_split():
    result = DetachedSplitReceiver().split(sep=(yield "split-suspended"))
    events.append("split-resumed")
    return result


split_generator = suspended_split()
assert next(split_generator) == "split-suspended"
assert events == ["split-lookup", "split-receiver-released"]
try:
    split_generator.send("|")
except StopIteration as stopped:
    assert stopped.value == ((), {"sep": "|"})
assert events == [
    "split-lookup",
    "split-receiver-released",
    "split-called",
    "split-callable-released",
    "split-resumed",
]
print("split-suspension", events)
events.clear()


class SplitText(str):
    def split(self, *arguments, **keywords):
        return ("override", arguments, keywords)


class SplitBytes(bytes):
    split = SplitText.split


class SplitBytearray(bytearray):
    split = SplitText.split


for split_receiver in (SplitText("a|b"), SplitBytes(b"a|b"), SplitBytearray(b"a|b")):
    assert split_receiver.split(1, 2, 3, sep="|") == (
        "override",
        (1, 2, 3),
        {"sep": "|"},
    )
    assert split_receiver.split(maxsplit=2, sep="|") == (
        "override",
        (),
        {"maxsplit": 2, "sep": "|"},
    )
print("split-subclass-overrides", True)


events.clear()


class SetIterationProbe:
    def __iter__(self):
        events.append("set-iterate")
        yield 1


def later_set_argument():
    events.append("set-later-argument")
    return {2}


for set_operation in (
    lambda: {0}.union(SetIterationProbe(), later_set_argument()),
    lambda: {0}.intersection(SetIterationProbe(), later_set_argument()),
    lambda: {0}.difference(SetIterationProbe(), later_set_argument()),
    lambda: {0}.update(SetIterationProbe(), later_set_argument()),
    lambda: {0}.intersection_update(SetIterationProbe(), later_set_argument()),
    lambda: {0}.difference_update(SetIterationProbe(), later_set_argument()),
):
    events.clear()
    set_operation()
    assert events == ["set-later-argument", "set-iterate"]
    print("set-argument-order", events)
events.clear()


class Probe:
    def __del__(self):
        events.append("argument-released")


def replacement_len(value):
    return 999


def argument():
    global len
    events.append("argument-evaluated")
    len = replacement_len
    return [Probe()]


length = len
observed = len(argument())
events.append("after-call")
print(observed, events)
assert observed == 1
assert events == ["argument-evaluated", "argument-released", "after-call"]
assert length([1, 2]) == 2
assert len([1, 2]) == 999


class Iterable:
    def __iter__(self):
        events.append("iterated")
        yield 3
        yield 5


make_list = list
result = make_list(Iterable())
assert type(result) is list
assert result == [3, 5]
print(result, events[-1])

# A callee loaded before a named-expression write remains the invoked object.
captured = length
observed = captured((captured := lambda value: 41, 0))
assert observed == 2
assert captured(()) == 41
print(observed, captured(()))

# The inner conversion must happen before a later outer-call argument.
events.clear()


class Text:
    def __str__(self):
        events.append("str")
        raise ValueError("inner conversion")


def base():
    events.append("base")
    raise RuntimeError("later argument")


try:
    int(str(Text()), base())
except ValueError as exc:
    print(type(exc).__name__, str(exc), events)
assert events == ["str"]


class TextSubclass(str):
    def __len__(self):
        return 17


class BytesSubclass(bytes):
    def __len__(self):
        return 19


class ProtocolValue:
    def __str__(self):
        return TextSubclass("x")

    def __bytes__(self):
        return BytesSubclass(b"x")


text_value = str(ProtocolValue())
bytes_value = bytes(ProtocolValue())
assert type(text_value) is TextSubclass
assert type(bytes_value) is BytesSubclass
assert length(text_value) == 17
assert length(bytes_value) == 19
print(length(text_value), length(bytes_value))


def deferred_conversion(value):
    return imported_float(value)


def replacement_float(value):
    return ("replacement", value)


assert imported_float(7) == 7.0
imported_float = replacement_float
assert deferred_conversion(7) == ("replacement", 7)
print(deferred_conversion(7))


class PublishedProbe:
    def __del__(self):
        events.append("published-released")


def publish_list(owner):
    owner.append(PublishedProbe())


def publish_dict(owner):
    owner["value"] = PublishedProbe()


def publish_set(owner):
    owner.add(PublishedProbe())


def publish_nested(owner):
    owner[0].append(PublishedProbe())


def published_container_release(make, publish):
    owner = make()
    publish(owner)
    events.append("before-release")
    owner = None
    events.append("after-release")


for factory, publish in (
    (lambda: [], publish_list),
    (lambda: {}, publish_dict),
    (lambda: set(), publish_set),
    (lambda: ([],), publish_nested),
):
    events.clear()
    published_container_release(factory, publish)
    assert events == ["before-release", "published-released", "after-release"]
    print("published-lifetime", events)


class RetainedNestedProbe:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(f"{self.label}-released")


def retained_nested_lifetime(label, make_owner, store):
    events.clear()
    owner = make_owner()
    alias = owner
    nested = [RetainedNestedProbe(label)]
    store(owner, nested)
    nested = None
    events.append("after-store")
    owner = None
    events.append("after-owner-drop")
    assert events == ["after-store", "after-owner-drop"]
    alias = None
    assert alias is None
    events.append("after-alias-drop")
    assert events == [
        "after-store",
        "after-owner-drop",
        f"{label}-released",
        "after-alias-drop",
    ]
    print("stored-lifetime", label, events)


for label, factory, store in (
    ("append", list, lambda owner, nested: owner.append(nested)),
    ("insert", list, lambda owner, nested: owner.insert(0, nested)),
    ("extend", list, lambda owner, nested: owner.extend([nested])),
    ("setdefault", dict, lambda owner, nested: owner.setdefault("key", nested)),
):
    retained_nested_lifetime(label, factory, store)


# A name is not a lifetime root after an intervening callback revokes it.
class RevokeArgumentOwner:
    def __del__(self):
        global retained_argument
        del retained_argument
        events.append("owner-revoked")


def unreachable_call(*arguments):
    raise AssertionError("argument evaluation must fail before invocation")


events.clear()
retained_argument = PublishedProbe()
argument_owner = RevokeArgumentOwner()
try:
    unreachable_call(
        retained_argument,
        (argument_owner := None),
        missing_retained_argument,  # noqa: F821 - intentional failed final argument
    )
except NameError:
    events.append("argument-failed")
assert events == ["owner-revoked", "published-released", "argument-failed"]
print("retained-argument-lifetime", events)


# The iterator retains its source after the body's last alias disappears.
# Breaking releases it before executing the next statement.
events.clear()
for item in (loop_owner := [0]):
    publish_list(loop_owner)
    loop_owner = None
    events.append("before-break")
    break
events.append("after-loop")
assert events == ["before-break", "published-released", "after-loop"]
print("retained-iterator-lifetime", events)


# The bound method owns the old receiver, not the name's replacement.
items = [1]
items.append((items := ["replacement"]))
assert items == ["replacement"]
print("replacement-owner", items)

# A fresh outer owner may still contain the receiver being mutated.
items = [1]
items.append((items := [items]))
assert items[0][0] == 1
assert items[0][1] is items
assert length(items[0]) == 2
print("nested-owner", length(items), length(items[0]))
# Break the intentional cycle; do not leave its collection timing in stdout.
items[0].pop()


def joined_receiver(flag):
    if flag:
        first = []
        second = []
    else:
        first = []
        second = []
    first.append((first := second))
    return length(first)


assert joined_receiver(True) == joined_receiver(False) == 0
print("joined-owner", joined_receiver(True), joined_receiver(False))


class CleanupText(str):
    def __del__(self):
        events.append("text-released")


def replace_text_owner():
    global retained_text
    retained_text = None
    events.append("text-owner-replaced")
    return "b"


events.clear()
retained_text = CleanupText("a")
text = "a".replace(retained_text, replace_text_owner())
events.append("after-replace")
assert text == "b"
assert events == ["text-owner-replaced", "text-released", "after-replace"]
print("normal-argument-lifetime", text, events)
