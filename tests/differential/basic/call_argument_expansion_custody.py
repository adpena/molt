"""Callback order and ownership boundaries for CALL_FUNCTION_EX-style expansion."""

import sys

events = []


def target(*args, **kwargs):
    events.append(("call", args, tuple(kwargs.items())))


class Values:
    def __init__(self, values, label):
        self.values = values
        self.label = label
        self.index = 0

    def __iter__(self):
        events.append((self.label, "iter"))
        return self

    def __length_hint__(self):
        events.append((self.label, "hint"))
        return len(self.values)

    def __next__(self):
        events.append((self.label, "next", self.index))
        if self.index == len(self.values):
            raise StopIteration
        value = self.values[self.index]
        self.index += 1
        return value


class Mapping:
    def __init__(self, keys, label):
        self.key_values = keys
        self.label = label

    def keys(self):
        events.append((self.label, "keys"))
        return Values(self.key_values, self.label + ".keys")

    def __getitem__(self, key):
        events.append((self.label, "get", key))
        return self.label


def operand(label, value):
    events.append(("operand", label))
    return value


def run(label, callback):
    events.clear()
    try:
        callback()
    except Exception as error:
        events.append(("error", type(error).__name__))
    print(label, events)


run("star", lambda: target(*Values([1, 2], "star")))
run("keys-first", lambda: target(**Mapping(["a", "b"], "map")))
run("duplicate", lambda: target(a=0, **Mapping(["a", "b"], "map")))
run("duplicate-in-keys", lambda: target(**Mapping(["a", "a"], "map")))
run(
    "defer-nonstring",
    lambda: target(**Mapping([1, "a"], "bad"), **operand("later", {"b": 2})),
)
run("noncallable", lambda: operand("callee", 42)(**Mapping([1], "bad")))


class HintError(Values):
    def __length_hint__(self):
        events.append((self.label, "hint-error"))
        raise ValueError("hint")


class NextError(Values):
    def __next__(self):
        events.append((self.label, "next-error"))
        raise RuntimeError("next")


run("hint-error", lambda: target(*HintError([], "star")))
run("next-error", lambda: target(*NextError([], "star")))


class HintNotImplemented(Values):
    def __length_hint__(self):
        events.append((self.label, "hint-notimplemented"))
        return NotImplemented


run("hint-notimplemented", lambda: target(*HintNotImplemented([3], "star")))


class HintFloat(Values):
    def __length_hint__(self):
        events.append((self.label, "hint-float"))
        return 1.0


class HintNegative(Values):
    def __length_hint__(self):
        events.append((self.label, "hint-negative"))
        return -1


class ListSubclass(list):
    def __iter__(self):
        events.append("subclass.iter")
        return iter([8, 9])

    def __len__(self):
        events.append("subclass.len")
        return 2


run("hint-float", lambda: target(*HintFloat([3], "star")))
run("hint-negative", lambda: target(*HintNegative([3], "star")))
run("subclass-star", lambda: target(*ListSubclass([1])))
run("subclass-list", lambda: target(list(ListSubclass([1]))))
run("subclass-tuple", lambda: target(tuple(ListSubclass([1]))))


def partial_extend():
    destination = [0]
    try:
        destination.extend(NextError([], "extend"))
    finally:
        events.append(("destination", destination))


run("extend-error", partial_extend)


class ExactListKeys:
    def __init__(self):
        self.names = ["a"]

    def keys(self):
        events.append("keys-list")
        return self.names

    def __getitem__(self, key):
        events.append(("get", key))
        if key == "a":
            self.names.append("b")
        return key


run("live-exact-keys-list", lambda: target(**ExactListKeys()))


def instance_special_shadow():
    mapping = Mapping(["a"], "map")
    mapping.__getitem__ = lambda key: events.append("wrong-instance-get")
    target(**mapping)


run("type-only-getitem", instance_special_shadow)


class Truth:
    def __init__(self, value, raises=False):
        self.value = value
        self.raises = raises

    def __bool__(self):
        events.append(("truth", self.value))
        if self.raises:
            raise ValueError("truth")
        return self.value


class Key(str):
    def __hash__(self):
        events.append(("hash", str(self)))
        return 11

    def __eq__(self, other):
        events.append(("eq", str(self), str(other)))
        return Truth(str(self) == str(other))


def key_mapping_call():
    target(**Mapping([Key("a"), Key("b")], "custom"))


def key_duplicate():
    target(**Mapping([Key("a"), Key("a")], "duplicate"))


def key_cached_hashes():
    source = {Key("a"): 1}
    events.clear()
    target(**source)


def named(a):
    events.append(("named", a))


def positional_only(a, /, **kwargs):
    events.append(("posonly", a, tuple(kwargs.items())))


run("key-callbacks", key_mapping_call)
run("key-duplicate", key_duplicate)
run("key-cached-hash", key_cached_hashes)
run("key-parameter-match", lambda: named(**{Key("a"): 4}))
run("posonly-varkw", lambda: positional_only(1, a=2))


class ReentrantKey:
    def __init__(self, label, owner=None):
        self.label = label
        self.owner = owner

    def __hash__(self):
        return 7

    def __eq__(self, other):
        events.append(("reentrant-eq", self.label, other.label))
        if self.owner is not None:
            owner = self.owner
            self.owner = None
            owner.clear()
            owner["after"] = 3
        return Truth(True)


def reentrant_lookup():
    table = {}
    stored = ReentrantKey("stored", table)
    table[stored] = 1
    events.append(("lookup", table.get(ReentrantKey("probe"), "missing")))
    events.append(("after", tuple(table.items())))


def reentrant_insert():
    table = {}
    stored = ReentrantKey("stored", table)
    table[stored] = 1
    table[ReentrantKey("probe")] = 9
    events.append(("insert", len(table), table["after"]))


run("reentrant-lookup", reentrant_lookup)
run("reentrant-insert", reentrant_insert)


class FloatHash:
    def __hash__(self):
        return 1.0


full_width_hash = (1 << (sys.hash_info.width - 2)) + 5


class LargeHash:
    def __hash__(self):
        return full_width_hash


def full_width_hash_result():
    # A fitting Py_hash_t is not reduced modulo the integer-hash modulus.
    # Use the target's hash width, not the compiler host's pointer width.
    result = hash(LargeHash())
    assert result == full_width_hash
    events.append(("hash-full-width-preserved", result == full_width_hash))


run("hash-rejects-float", lambda: hash(FloatHash()))
run("hash-full-width", full_width_hash_result)


def mapping_update():
    destination = {}
    destination.update(Mapping(["a", "b"], "update"))
    events.append(("updated", tuple(destination.items())))


def pairs_update():
    destination = {}
    destination.update([Values(["a", 4], "pair")])
    events.append(("pairs", tuple(destination.items())))


run("mapping-update", mapping_update)
run("pairs-update", pairs_update)


class PairTypeError:
    def __init__(self, stage):
        self.stage = stage
        self.acquisitions = 0

    def __iter__(self):
        self.acquisitions += 1
        events.append(("pair-acquire", self.acquisitions))
        if self.stage == self.acquisitions:
            raise TypeError("pair-acquisition-sentinel")
        return self

    def __length_hint__(self):
        return 0

    def __next__(self):
        raise TypeError("pair-next-sentinel")


def pair_type_error(stage):
    try:
        dict([PairTypeError(stage)])
    except TypeError as error:
        message = str(error)
        expected = (
            "cannot convert dictionary update sequence element"
            if sys.version_info < (3, 14)
            else "object is not iterable"
            if stage == 1
            else "pair-acquisition-sentinel"
            if stage == 2
            else "pair-next-sentinel"
        )
        assert message.startswith(expected), message
        events.append(("pair-error-preserved", stage))
    else:
        raise AssertionError("missing pair TypeError")


run("pair-initial-error", lambda: pair_type_error(1))
run("pair-second-error", lambda: pair_type_error(2))
run("pair-next-error", lambda: pair_type_error(3))


class HashInteger(int):
    pass


class SubclassHash:
    def __init__(self, value):
        self.value = value

    def __hash__(self):
        return HashInteger(self.value)


def subclass_hash_result():
    fitting = full_width_hash
    overflowing = 1 << (sys.hash_info.width + 2)
    assert hash(SubclassHash(fitting)) == fitting
    assert hash(SubclassHash(overflowing)) == hash(overflowing)
    events.append("hash-int-subclass-preserved")


run("hash-int-subclass", subclass_hash_result)

# Builtin specialization must not interpret a starred operand as one argument
# or reject ** syntax before the mapping's effects reach the common binder.
run("builtin-list-star", lambda: target(list(*Values([[1, 2]], "arguments"))))
run("builtin-tuple-star", lambda: target(tuple(*Values([[1, 2]], "arguments"))))
run("builtin-pow-star", lambda: target(pow(*Values([2, 3], "arguments"))))
run("builtin-complex-star", lambda: target(complex(*Values([2, 3], "arguments"))))
run(
    "builtin-bytes-keywords",
    lambda: target(bytes(**{"source": "abc", "encoding": "utf8"})),
)


class SourceMutationKey:
    def __init__(self, callback=None):
        self.callback = callback

    def __hash__(self):
        return 1

    def __eq__(self, other):
        if self.callback is not None:
            callback, self.callback = self.callback, None
            callback()
        return False


def source_mutation(mode):
    # Integer keys avoid salted-string collision variability. The delete case
    # deliberately retains the unclosed CPython 3.12 entry-history boundary;
    # it must remain visible to differential runs, not live only in a WIP log.
    source = {SourceMutationKey(): 1, 2: 2}

    def mutate():
        if mode == "replace":
            source[2] = 3
        elif mode == "clear":
            source.clear()
            source.update({3: 1, 4: 2})
        elif mode == "delete":
            del source[2]
            source[3] = 3
        else:
            source[3] = 3

    destination = {SourceMutationKey(mutate): 0}
    try:
        destination.update(source)
    except RuntimeError as error:
        events.append(("source-update-error", str(error)))
    events.append(
        ("source-update-values", tuple(destination.values()), tuple(source.values()))
    )


for mutation in ("replace", "clear", "delete", "add"):
    run("source-" + mutation, lambda: source_mutation(mutation))


class ReplacingKeyword(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        self.calls = getattr(self, "calls", 0) + 1
        events.append(("keyword-equal", self.calls))
        return self.calls == 2


class ReplacedKeywordValue:
    def __del__(self):
        events.append("keyword-old-dropped")


class ReplacingKeywordMapping:
    def keys(self):
        return [ReplacingKeyword("a")]

    def __getitem__(self, key):
        return "new"


run(
    "keyword-replacement-custody",
    lambda: target(a=ReplacedKeywordValue(), **ReplacingKeywordMapping()),
)


class SequenceItemDescriptor:
    def __get__(self, instance, owner):
        events.append("getitem-bound")

        def item(index):
            events.append(("getitem", index))
            raise IndexError

        return item


class DescriptorSequence:
    __getitem__ = SequenceItemDescriptor()


def sequence_descriptor_lifetime():
    iterator = iter(DescriptorSequence())
    events.append("sequence-acquired")
    events.append(("first", next(iterator, "done")))
    events.append(("second", next(iterator, "done")))


run("sequence-descriptor-lifetime", sequence_descriptor_lifetime)


class FailingIterationDescriptor:
    def __init__(self, error):
        self.error = error

    def __get__(self, instance, owner):
        events.append("iteration-descriptor-bound")
        raise self.error("descriptor-failure")


def iteration_descriptor_error(error, sequence):
    if sequence:

        class Subject:
            __getitem__ = FailingIterationDescriptor(error)

    else:

        class Subject:
            def __iter__(self):
                return self

            __next__ = FailingIterationDescriptor(error)

    iterator = iter(Subject())
    events.append("error-iterator-acquired")
    for step in range(2):
        try:
            events.append((step, next(iterator, "done")))
        except (RuntimeError, IndexError) as raised:
            events.append((step, type(raised).__name__, str(raised)))


for descriptor_error in (IndexError, StopIteration, RuntimeError):
    for sequence_kind in (False, True):
        run(
            "iteration-descriptor-"
            + descriptor_error.__name__
            + "-"
            + str(sequence_kind),
            lambda: iteration_descriptor_error(descriptor_error, sequence_kind),
        )


# Function defaults are live binding metadata, resolved after keyword callbacks.
def binding_phase_target(x, y=1, *, z=2):
    return x, y, z


class BindingPhaseKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        binding_phase_target.__defaults__ = (11,)
        binding_phase_target.__kwdefaults__ = {"z": 22}
        return str.__eq__(self, other)


binding_phase_result = binding_phase_target(**{BindingPhaseKey("x"): 0})
assert binding_phase_result == (0, 11, 22)
print("binding-live-defaults", binding_phase_result)


def binding_kwdefault_phase_target(*, first, second):
    return first, second


class BindingKwdefaultSwitchKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        binding_kwdefault_phase_target.__kwdefaults__ = {"second": 22}
        return str.__eq__(self, other)


# Keep this dictionary owned explicitly while its __eq__ replaces the function
# attribute; the next parameter must nevertheless read the NEW dictionary.
binding_kwdefault_owner = {BindingKwdefaultSwitchKey("first"): 11}
binding_kwdefault_phase_target.__kwdefaults__ = binding_kwdefault_owner
binding_kwdefault_phase_result = binding_kwdefault_phase_target()
assert binding_kwdefault_phase_result == (11, 22)
print("binding-per-key-defaults", binding_kwdefault_phase_result)


binding_default_release_events = []


class BindingDefaultVictim:
    tag = 1

    def __del__(self):
        binding_default_release_events.append("released")


def binding_default_owner_target(*, first, second):
    return first.tag, second, tuple(binding_default_release_events)


class BindingDefaultReplaceKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        binding_default_owner_target.__kwdefaults__["first"] = "replacement"
        return str.__eq__(self, other)


binding_default_owner_target.__kwdefaults__ = {
    "first": BindingDefaultVictim(),
    BindingDefaultReplaceKey("second"): 22,
}
binding_default_owner_result = binding_default_owner_target()
assert binding_default_owner_result == (1, 22, ())
assert binding_default_release_events == ["released"]
print("binding-owned-default", binding_default_owner_result)


def binding_default_suffix_target(first=0, second=0):
    return first, second


binding_default_suffix_target.__defaults__ = (1, 2, 3)
binding_default_suffix_result = binding_default_suffix_target()
assert binding_default_suffix_result == (2, 3)
print("binding-default-suffix", binding_default_suffix_result)


def binding_kwdefault_error_target(*, item):
    return item


class BindingKwdefaultErrorKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        raise ValueError("kwdefault observer")


binding_kwdefault_error_target.__kwdefaults__ = {BindingKwdefaultErrorKey("item"): 1}
try:
    binding_kwdefault_error_target()
except ValueError as error:
    assert str(error) == "kwdefault observer"
    print("binding-default-error", str(error))
else:
    raise AssertionError("keyword-default lookup exception was replaced or lost")


def binding_error_precedence_target(item):
    return item


class BindingPrecedenceErrorKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        raise ValueError("keyword-before-arity")


try:
    binding_error_precedence_target(0, 1, **{BindingPrecedenceErrorKey("other"): 2})
except ValueError as error:
    assert str(error) == "keyword-before-arity"
    print("binding-keyword-before-arity", str(error))
else:
    raise AssertionError("positional arity bypassed keyword binding callbacks")


def binding_varkw_phase_target(x, y=1, **rest):
    return y, tuple(rest.values())


class BindingVarkwNamedKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        if str.__eq__(self, other):
            binding_varkw_phase_target.__defaults__ = (11,)
            return True
        return False


class BindingVarkwCollisionKey(str):
    def __hash__(self):
        return 42

    def __eq__(self, other):
        if isinstance(other, BindingVarkwCollisionKey):
            binding_varkw_phase_target.__defaults__ = (22,)
        return str.__eq__(self, other)


binding_varkw_input = {
    BindingVarkwNamedKey("x"): 0,
    BindingVarkwCollisionKey("extra_a"): 1,
    BindingVarkwCollisionKey("extra_b"): 2,
}
binding_varkw_phase_target.__defaults__ = (1,)
binding_varkw_phase_result = binding_varkw_phase_target(**binding_varkw_input)
assert binding_varkw_phase_result == (22, (1, 2))
print("binding-varkw-before-defaults", binding_varkw_phase_result)


def binding_missing_kwonly_order_target(*, absent, observed):
    return absent, observed


class BindingMissingKwonlyOrderKey(str):
    __hash__ = str.__hash__

    def __eq__(self, other):
        raise ValueError("later-default-before-missing")


binding_missing_kwonly_order_target.__kwdefaults__ = {
    BindingMissingKwonlyOrderKey("observed"): 1,
}
try:
    binding_missing_kwonly_order_target()
except ValueError as error:
    assert str(error) == "later-default-before-missing"
    print("binding-default-before-missing", str(error))
else:
    raise AssertionError("missing parameter bypassed a later default lookup")
