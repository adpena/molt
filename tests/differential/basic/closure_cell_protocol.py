"""Real lexical cells: public protocol, retained identity and code replacement."""

from types import CellType, FunctionType
import sys


def raises(expected, operation):
    try:
        operation()
    except expected:
        return
    raise AssertionError(expected.__name__)


def capture(value):
    def read():
        return value

    return read


empty = CellType()
none = CellType(None)
assert type(empty) is CellType and type(none) is CellType
raises(ValueError, lambda: empty.cell_contents)
assert none.cell_contents is None
raises(TypeError, lambda: CellType(1, 2))
raises(TypeError, lambda: CellType(value=1))
raises(AttributeError, lambda: len.__code__)
raises(AttributeError, lambda: len.__closure__)

owner = capture(41)
cell = owner.__closure__[0]
assert type(cell) is CellType
assert owner.__code__.co_freevars == ("value",)
assert capture.__code__.co_cellvars == ("value",)
assert cell.cell_contents == 41
cell.cell_contents = 43
assert owner() == 43
del cell.cell_contents
raises(ValueError, lambda: cell.cell_contents)
raises(NameError, owner)
del cell.cell_contents
cell.cell_contents = 47
assert owner() == 47

for operation in (
    lambda: len(cell),
    lambda: iter(cell),
    lambda: cell[0],
    lambda: hash(cell),
):
    raises(TypeError, operation)
raises(AttributeError, lambda: cell.append(1))
raises(AttributeError, lambda: setattr(cell, "arbitrary", 1))
assert bool(empty) and bool(none)
assert CellType() == CellType()
assert CellType(3) == CellType(3)
assert CellType(3) != CellType(5)
assert CellType() < CellType(0)
assert CellType(3) < CellType(5)
assert CellType(3) <= CellType(3)
assert CellType(5) > CellType(3)
assert CellType(5) >= CellType(5)

nan = float("nan")
assert not (CellType(nan) == CellType(nan))
assert CellType(nan) != CellType(nan)


class Compared:
    def __eq__(self, other):
        return "equal-result"

    def __ne__(self, other):
        return "unequal-result"

    def __lt__(self, other):
        return "less-result"

    def __le__(self, other):
        return "less-equal-result"

    def __gt__(self, other):
        return "greater-result"

    def __ge__(self, other):
        return "greater-equal-result"


left = CellType(Compared())
right = CellType(Compared())
assert (left == right) == "equal-result"
assert (left != right) == "unequal-result"
assert (left < right) == "less-result"
assert (left <= right) == "less-equal-result"
assert (left > right) == "greater-result"
assert (left >= right) == "greater-equal-result"
assert CellType.__eq__(left, right) == "equal-result"
assert CellType.__eq__(left, []) is NotImplemented
assert CellType.__lt__(left, []) is NotImplemented
raises(TypeError, lambda: CellType.__eq__([], left))
assert not (CellType(1) == [])
raises(TypeError, lambda: CellType(1) < [])

cyclic_left = CellType()
cyclic_right = CellType()
cyclic_left.cell_contents = cyclic_left
cyclic_right.cell_contents = cyclic_right
for operation in (
    lambda: cyclic_left == cyclic_right,
    lambda: cyclic_left != cyclic_right,
    lambda: cyclic_left < cyclic_right,
    lambda: cyclic_left <= cyclic_right,
    lambda: cyclic_left > cyclic_right,
    lambda: cyclic_left >= cyclic_right,
    lambda: CellType.__eq__(cyclic_left, cyclic_left),
):
    raises(RecursionError, operation)
cyclic_left.cell_contents = None
cyclic_right.cell_contents = None

descriptor = CellType.cell_contents
descriptor.__set__(empty, 59)
assert descriptor.__get__(empty, CellType) == 59
descriptor.__delete__(empty)
raises(ValueError, lambda: descriptor.__get__(empty, CellType))
raises(TypeError, lambda: descriptor.__get__([], list))

supplied = (cell,)
clone = FunctionType(owner.__code__, globals(), closure=supplied)
assert clone.__closure__ is supplied
assert clone.__closure__[0] is cell
assert clone() == 47
cell.cell_contents = 53
assert owner() == clone() == 53
raises(TypeError, lambda: FunctionType(owner.__code__, globals()))
raises(ValueError, lambda: FunctionType(owner.__code__, globals(), closure=()))
raises(TypeError, lambda: FunctionType(owner.__code__, globals(), closure=([],)))


def direct():
    return "direct"


raises(ValueError, lambda: setattr(owner, "__code__", direct.__code__))
assert owner() == 53 and owner.__closure__[0] is cell
raises(ValueError, lambda: setattr(direct, "__code__", owner.__code__))
assert direct() == "direct" and direct.__closure__ is None
raises(ValueError, lambda: FunctionType(direct.__code__, globals(), closure=(cell,)))
assert FunctionType(direct.__code__, globals(), closure=())() == "direct"

# Public tuple identity is independent of the executable hidden-argument ABI.
empty_closure = ()
for source, arguments in (
    (lambda: (), ()),
    (lambda a: (a,), (11,)),
    (lambda a, b: (a, b), (11, 13)),
    (lambda a, b, c: (a, b, c), (11, 13, 17)),
    (lambda a, b, c, d: (a, b, c, d), (11, 13, 17, 19)),
    (lambda a, b, c, d, e: (a, b, c, d, e), (11, 13, 17, 19, 23)),
    (lambda a, b, c, d, e, f: (a, b, c, d, e, f), (11, 13, 17, 19, 23, 29)),
    (
        lambda a, b, c, d, e, f, g: (a, b, c, d, e, f, g),
        (11, 13, 17, 19, 23, 29, 31),
    ),
):
    empty_clone = FunctionType(source.__code__, globals(), closure=empty_closure)
    assert empty_clone.__closure__ is empty_closure
    assert empty_clone(*arguments) == arguments
    assert empty_clone(*arguments) == arguments


def positional(first, second=71):
    return first, second


empty_clone = FunctionType(
    positional.__code__, globals(), argdefs=(71,), closure=empty_closure
)
assert empty_clone(67, 73) == (67, 73)
assert empty_clone(67) == (67, 71)
assert empty_clone(second=79, first=67) == (67, 79)
empty_clone.__code__ = positional.__code__
assert empty_clone.__closure__ is empty_closure
assert empty_clone(83, 89) == (83, 89)


def replaceable(value=97):
    return value


def replacement(first, second):
    return first, second


# __code__ is executable replacement, not merely an introspection pointer.
# Defaults and the function's namespace/closure remain owned by that function.
retained_defaults = replaceable.__defaults__
replaceable.__code__ = replacement.__code__
assert replaceable.__defaults__ is retained_defaults
assert replaceable(101) == (101, 97)
assert replaceable(second=103, first=107) == (107, 103)


def capture_add(value):
    def add(step):
        return value + step

    return add


original_code = owner.__code__
owner.__code__ = capture_add(0).__code__
assert owner.__closure__[0] is cell
assert owner(7) == 60
assert owner(step=11) == 64
assert clone() == 53
owner.__code__ = original_code
assert owner() == 53


def generator_replacement():
    yield "replaced-generator"


plain_code = direct.__code__
direct.__code__ = generator_replacement.__code__
generated = direct()
direct.__code__ = plain_code
assert next(generated) == "replaced-generator"
generated.close()
assert direct() == "direct"


class ReplacementBase:
    def method(self, value):
        return value


class ReplacementDerived(ReplacementBase):
    def positional(self):
        return super().method(1)

    def omitted(self):
        return super().method()


def replacement_keyword_only(self, *, value):
    return value


def replacement_varargs(self, *values):
    return values


replacement_receiver = ReplacementDerived()
assert replacement_receiver.positional() == 1
assert replacement_receiver.positional() == 1  # Warm the same super call site.
ReplacementBase.method.__code__ = replacement_keyword_only.__code__
raises(TypeError, replacement_receiver.positional)
ReplacementBase.method.__kwdefaults__ = {"value": 17}
assert replacement_receiver.omitted() == 17
raises(TypeError, replacement_receiver.positional)
ReplacementBase.method.__code__ = replacement_varargs.__code__
assert replacement_receiver.positional() == (1,)
assert replacement_receiver.omitted() == ()


def lexical_locals():
    value = 1

    def read():
        return value

    first = locals()
    value = 2
    second = locals()
    return first is second, first["value"], second["value"], read()


def suspended_locals():
    value = 1
    yield locals()
    value = 2
    yield locals()


reused_locals = sys.version_info < (3, 13)
assert lexical_locals() == (reused_locals, 2 if reused_locals else 1, 2, 2)
suspended = suspended_locals()
first_locals = next(suspended)
second_locals = next(suspended)
assert (first_locals is second_locals) == reused_locals
assert first_locals["value"] == (2 if reused_locals else 1)
assert second_locals["value"] == 2
suspended.close()

events = []
reentrant = CellType()


class Release:
    def __del__(self):
        events.append(reentrant.cell_contents)
        reentrant.cell_contents = "reentered"


reentrant.cell_contents = Release()
reentrant.cell_contents = "published"
assert events == ["published"]
assert reentrant.cell_contents == "reentered"
print("cell-protocol", owner(), clone(), direct(), events, reentrant.cell_contents)
