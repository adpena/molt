"""Purpose: differential coverage for instancecheck subclasscheck."""


class Meta(type):
    def __instancecheck__(cls, instance):
        return getattr(instance, "flag", False)

    def __subclasscheck__(cls, subclass):
        return getattr(subclass, "marker", False)


class Base(metaclass=Meta):
    pass


class Child:
    marker = True


obj = Child()
obj.flag = True

print(isinstance(obj, Base))
print(issubclass(Child, Base))


# Abstract class objects are accepted by their observable __bases__ protocol.
# Equality, metaclass checks and physical storage must not supply ancestry.
events = []
original_failure = LookupError("classinfo callback")


class AbstractClass:
    def __init__(self, label, bases=()):
        self.label = label
        self.bases = bases

    @property
    def __bases__(self):
        events.append(self.label)
        return self.bases

    def __eq__(self, other):
        raise AssertionError("class ancestry must compare identity")


class ApparentInstance:
    def __init__(self, apparent):
        self.apparent = apparent

    @property
    def __class__(self):
        events.append("__class__")
        return self.apparent


class MissingBases:
    @property
    def __bases__(self):
        raise AttributeError("missing bases")


class FailingBases:
    @property
    def __bases__(self):
        raise original_failure


class FailingClass:
    @property
    def __class__(self):
        raise original_failure


class MissingClass:
    @property
    def __class__(self):
        raise AttributeError("missing class")


def original_error(label, operation):
    try:
        operation()
    except LookupError as error:
        assert error is original_failure
        print(label, "original")
    else:
        raise AssertionError(label)


def type_error(label, operation):
    try:
        operation()
    except TypeError as error:
        print(label, str(error))
    else:
        raise AssertionError(label)


root = AbstractClass("root")
child = AbstractClass("child", (root,))
instance = ApparentInstance(child)
assert issubclass(child, root)
assert events == ["child", "root", "child"]
events.clear()
assert isinstance(instance, root)
assert events == ["root", "__class__", "child"]
events.clear()
assert issubclass(root, root)
assert events == ["root", "root"]
print("abstract identity and inheritance", True)

original_error("derived bases error", lambda: issubclass(FailingBases(), object))
original_error("derived before invalid target", lambda: issubclass(FailingBases(), None))
original_error("target bases error", lambda: issubclass(child, FailingBases()))
original_error("instance target bases error", lambda: isinstance(instance, FailingBases()))
original_error("real apparent class error", lambda: isinstance(FailingClass(), int))
original_error("abstract apparent class error", lambda: isinstance(FailingClass(), root))
original_error("default subclass error", lambda: type.__subclasscheck__(object, FailingBases()))
original_error("default instance error", lambda: type.__instancecheck__(int, FailingClass()))
assert not isinstance(MissingClass(), root)
assert not isinstance(MissingClass(), int)
assert not isinstance(ApparentInstance(root), int)
assert isinstance(ApparentInstance(int), int)
assert type.__instancecheck__(int, ApparentInstance(int))
assert type.__subclasscheck__(int, AbstractClass("int child", (int,)))
assert not isinstance(42, root)
assert not isinstance(None, root)
print("apparent class and defaults", True)

type_error("missing derived bases", lambda: issubclass(MissingBases(), object))
type_error("missing target bases", lambda: isinstance(instance, MissingBases()))
bad_bases = AbstractClass("bad", [root])
type_error("malformed derived bases", lambda: issubclass(bad_bases, root))
type_error("malformed target bases", lambda: isinstance(instance, bad_bases))
# A malformed intermediate base is a nonmatch, not a top-level admission error.
assert not issubclass(AbstractClass("outer", (bad_bases,)), root)
assert not issubclass(AbstractClass("outer", (MissingBases(),)), root)
# Identity matches before even an intermediate base's failing __bases__ lookup.
flaky = AbstractClass("flaky")
flaky_child = AbstractClass("flaky child", (flaky,))
assert issubclass(flaky_child, flaky)
print("malformed and missing bases", True)


class StorageTuple(tuple):
    def __iter__(self):
        raise AssertionError("must not iterate tuple subclass")

    def __getitem__(self, key):
        raise AssertionError("must not index tuple subclass")

    def __len__(self):
        raise AssertionError("must not call tuple subclass length")


assert issubclass(AbstractClass("tuple bases", StorageTuple((root,))), root)
assert isinstance(instance, StorageTuple((root, None)))
assert issubclass(child, StorageTuple((root, None)))
assert not issubclass(FailingBases(), ())
assert not isinstance(FailingClass(), ())
original_error("tuple error before match", lambda: issubclass(child, (FailingBases(), root)))
assert issubclass(AbstractClass("branch", (root, FailingBases())), root)
original_error("bases branch error before match", lambda: issubclass(
    AbstractClass("branch", (FailingBases(), root)), root
))
print("tuple storage and short circuit", True)


class AcceptMeta(type):
    def __instancecheck__(cls, instance):
        events.append("accept instance")
        return True

    def __subclasscheck__(cls, derived):
        events.append("accept subclass")
        return True


class RejectMeta(type):
    def __instancecheck__(cls, instance):
        events.append("reject instance")
        raise original_failure

    def __subclasscheck__(cls, derived):
        events.append("reject subclass")
        raise original_failure


class Accepted(metaclass=AcceptMeta):
    pass


class Rejected(metaclass=RejectMeta):
    pass


events.clear()
exact = Rejected()
assert isinstance(exact, Rejected)
assert events == []
original_error("subclass identity still calls override", lambda: issubclass(Rejected, Rejected))
assert type.__subclasscheck__(Rejected, Rejected)
assert type.__instancecheck__(Rejected, exact)
events.clear()
assert isinstance(FailingClass(), Accepted | Rejected)
assert issubclass(FailingBases(), Accepted | Rejected)
assert events == ["accept instance", "accept subclass"]
original_error("union error before match", lambda: isinstance(instance, Rejected | Accepted))
assert isinstance([], list | list[int])
assert issubclass(list, list | list[int])
type_error("union invalid after nonmatch", lambda: isinstance(1, list | list[int]))
print("metaclass ordering and union", True)

# Single inheritance must not spend recursive-call budget; branching must.
chain = root
for _ in range(1200):
    chain = AbstractClass("chain", (chain,))
assert issubclass(chain, root)
recursive = AbstractClass("recursive")
recursive.bases = (recursive, root)
try:
    issubclass(recursive, AbstractClass("unrelated"))
except RecursionError:
    print("abstract recursion", True)
else:
    raise AssertionError("branching cycle must raise RecursionError")
recursive.bases = ()


# __bases__ may return the only tuple owning the next abstract class. The next
# lookup must finish before that tuple releases the temporary class.
class TemporaryBase:
    @property
    def __bases__(self):
        events.append("temporary bases")
        return (root,)

    def __del__(self):
        events.append("temporary retired")


class TemporaryDerived:
    @property
    def __bases__(self):
        events.append("derived bases")
        return (TemporaryBase(),)


events.clear()
assert issubclass(TemporaryDerived(), root)
assert events == [
    "derived bases", "temporary retired", "root",
    "derived bases", "temporary bases", "temporary retired",
]
print("abstract tail owner order", True)
type_error("default subclass receiver", lambda: type.__subclasscheck__(root, child))
type_error("default instance receiver", lambda: type.__instancecheck__(root, instance))
