"""Call identity, argument lifetime and constructor-result boundary capsule."""

from builtins import float as imported_float

events = []


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
