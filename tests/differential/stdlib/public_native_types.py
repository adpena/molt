"""Native public types: constructor identity, subclasses and physical state."""
import copy
import functools
import itertools
import operator
import sys

cases = (
    ("chain", ([1, 2], [3]), {}),
    ("islice", (range(8), 1, 7, 2), {}),
    ("repeat", ("x", 3), {}),
    ("count", (), {"start": 2, "step": 3}),
    ("cycle", ([1, 2],), {}),
    ("accumulate", ([1, 2, 3],), {"initial": 10}),
    ("batched", ([1, 2, 3], 2), {}),
    ("combinations", ([1, 2, 3], 2), {}),
    ("combinations_with_replacement", ([1, 2], 2), {}),
    ("compress", ([1, 2, 3], [True, False, True]), {}),
    ("dropwhile", (bool, [1, 0, 2]), {}),
    ("filterfalse", (bool, [1, 0, 2]), {}),
    ("pairwise", ([1, 2, 3],), {}),
    ("groupby", ([1, 1, 2],), {}),
    ("product", ([1, 2],), {"repeat": 2}),
    ("permutations", ([1, 2, 3],), {"r": 2}),
    ("starmap", (operator.add, [(1, 2), (3, 4)]), {}),
    ("takewhile", (bool, [1, 2, 0, 3]), {}),
    ("zip_longest", ([1, 2], [3]), {"fillvalue": 9}),
)


def consume(name, iterator):
    if name == "groupby":
        return [(key, list(group)) for key, group in itertools.islice(iterator, 4)]
    return list(itertools.islice(iterator, 4))


for name, args, keywords in cases:
    constructor = getattr(itertools, name)
    value = constructor(*args, **keywords)
    print("type", name, type(constructor).__name__, constructor.__name__, constructor.__qualname__, constructor.__module__)
    print("instance", type(value) is constructor, isinstance(value, constructor), consume(name, value))
    derived = type("Derived", (constructor,), {})
    value = derived(*args, **keywords)
    value.note = "subclass"
    print("subclass", type(value) is derived, isinstance(value, constructor), value.note, consume(name, value))

print("chain classmethod", list(itertools.chain.from_iterable([[1], [2, 3]])))
derived_chain = type("DerivedChain", (itertools.chain,), {})
value = derived_chain.from_iterable([[4], [5]])
print("chain subclass classmethod", type(value) is derived_chain, list(value))
print("iterator repr", repr(itertools.count(2, 3)), repr(itertools.repeat("x", 2)))
print("iterator keyword defaults", list(itertools.product([1, 2])),
      list(itertools.accumulate([1, 2])), list(itertools.zip_longest([1], [2, 3])))

partial = functools.partial(operator.add, 7)
print("partial type", type(functools.partial).__name__, functools.partial.__name__, functools.partial.__qualname__, functools.partial.__module__)
print("partial instance", type(partial) is functools.partial, isinstance(partial, functools.partial), partial(5), partial.func is operator.add, partial.args, partial.keywords)
print("partial repr", repr(partial))
recipe = partial.__reduce__()
print("partial reduction", len(recipe), recipe[0] is functools.partial, recipe[1][0] is operator.add, len(recipe[2]), recipe[2][0] is partial.func, recipe[2][1] is partial.args, recipe[2][2] is partial.keywords, recipe[2][3])
partial.note = {"values": [3]}
shallow = copy.copy(partial)
deep = copy.deepcopy(partial)
print("partial copy", shallow is not partial, shallow.note is partial.note, shallow(1))
print("partial deepcopy", deep is not partial, deep.note == partial.note, deep.note is not partial.note, deep(1))


class PartialSubclass(functools.partial):
    pass


subclass = PartialSubclass(operator.add, 8)
print("partial subclass", type(subclass) is PartialSubclass, isinstance(subclass, functools.partial), subclass(2), repr(subclass))
flattened = functools.partial(functools.partial(operator.add, 2), 3)
print("partial flatten", flattened.func is operator.add, flattened.args, flattened())
for attribute in ("func", "args", "keywords"):
    try:
        setattr(partial, attribute, None)
    except Exception as error:
        print("readonly", attribute, type(error).__name__)


def replace_during_call(value):
    changing.__setstate__((operator.neg, (), {}, None))
    return value


changing = functools.partial(replace_during_call, [1, 2])
print("partial reentrant state", changing(), changing(3), changing.func is operator.neg)
with_keywords = functools.partial(dict, a=1)
print("partial keyword merge", with_keywords(b=2), with_keywords.keywords)
print("partial keyword override", with_keywords(a=3), with_keywords.keywords)


def recipe_shape(value):
    if isinstance(value, tuple):
        return tuple(recipe_shape(item) for item in value)
    if isinstance(value, list):
        return [recipe_shape(item) for item in value]
    if isinstance(value, type):
        return ("type", value.__module__, value.__qualname__)
    if hasattr(value, "__next__"):
        return ("iterator", type(value).__name__)
    if callable(value):
        return ("callable", value.__module__, value.__qualname__)
    return value


for name, args, keywords in cases:
    for advances in (0, 1, 5):
        value = getattr(itertools, name)(*args, **keywords)
        for unused in range(advances):
            try:
                next(value)
            except StopIteration:
                break
        try:
            recipe = value.__reduce__()
        except TypeError as error:
            print("iterator reduction", name, advances, type(error).__name__)
        else:
            print("iterator reduction", name, advances, recipe_shape(recipe))
            rebuilt = recipe[0](*recipe[1])
            if len(recipe) == 3:
                rebuilt.__setstate__(recipe[2])
            print("iterator restored", name, advances, consume(name, rebuilt))

events = []


def cycle_source():
    for item in (1, 2):
        events.append(item)
        yield item


lazy_cycle = itertools.cycle(cycle_source())
print("cycle lazy", events, next(lazy_cycle), events)


class NotIterator:
    def __iter__(self):
        events.append("iter called")
        return iter(())


if sys.version_info < (3, 14):
    try:
        itertools.chain().__setstate__((NotIterator(),))
    except TypeError as error:
        print("chain state boundary", type(error).__name__, events)


class StateTuple(tuple):
    def __iter__(self):
        events.append("state tuple iteration")
        return iter((9, 10))


class StateDict(dict):
    def keys(self):
        raise AssertionError("state normalization must copy physical dictionary storage")


events.clear()
normalized = functools.partial(tuple)
state_keywords = StateDict(x=3)
normalized.__setstate__((tuple, StateTuple((1, 2)), state_keywords, None))
print("partial state normalization", normalized.args, type(normalized.args) is tuple,
      normalized.keywords, type(normalized.keywords) is dict,
      normalized.keywords is state_keywords, events)
for dictionary in ([], 42):
    normalized = functools.partial(abs)
    try:
        functools.partial.__setstate__(normalized, (abs, (-2,), {}, dictionary))
    except TypeError as error:
        print("partial legacy dictionary rejected", type(error).__name__, normalized.args)
    else:
        print("partial legacy dictionary", normalized.__dict__ is dictionary,
              functools.partial.__reduce__(normalized)[2][3] is dictionary)
        try:
            normalized.missing
        except Exception as error:
            print("partial legacy lookup", type(error).__name__)
    functools.partial.__setstate__(normalized, (abs, (-3,), {}, {"restored": True}))
    print("partial state repaired", normalized(), normalized.__dict__)
    normalized.__dict__ = {"replaced": True}
    print("partial dictionary replaced", normalized.__dict__)
    try:
        del normalized.__dict__
    except TypeError as error:
        print("partial dictionary deletion", type(error).__name__)


class RetiredDefault:
    def __del__(self):
        events.append((retiring.func is abs, retiring.args, dict(retiring.keywords)))


old_function = lambda unused=RetiredDefault(): None
retiring = functools.partial(old_function, 1)
del old_function
events.clear()
retiring.__setstate__((abs, (-2,), {"changed": 3}, None))
print("partial field retirement", events)
