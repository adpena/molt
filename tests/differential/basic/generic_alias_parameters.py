import types
from typing import TypeVar, TypeVarTuple

T = TypeVar("T")
U = TypeVar("U")
Ts = TypeVarTuple("Ts")

alias_list = list[T]
alias_dict = dict[str, T]
alias_tuple_dup = tuple[T, T]
alias_tuple_var = tuple[Ts]

print(isinstance(alias_list, types.GenericAlias))
print(alias_list.__parameters__)
print(alias_dict.__parameters__)
print(alias_tuple_dup.__parameters__)
print(alias_tuple_var.__parameters__)


# Parameter identity and ordering are independent of names and equality/hash.
from typing import Annotated, Callable, Concatenate, Literal, ParamSpec, Union
import sys

P = ParamSpec("P")
same_name = TypeVar("T")


def check_parameters(label, alias, expected):
    actual = alias.__parameters__
    print(label, len(actual) == len(expected) and all(
        value is expected[index] for index, value in enumerate(actual)
    ))


check_parameters("same-name identity", tuple[T, same_name, T], (T, same_name))
check_parameters("first appearance", tuple[U, list[T], U, dict[T, Ts]], (U, T, Ts))
check_parameters("direct paramspec", tuple[P, T, P, Ts], (P, T, Ts))
check_parameters("paramspec attributes", tuple[P.args, P.kwargs], ())
check_parameters("typing generic", list[Union[U, T]], (U, T))
check_parameters("callable parameters", list[Callable[P, T]], (P, T))
check_parameters("callable list", list[Callable[[U, T], U]], (U, T))
check_parameters("concatenate parameters", list[Concatenate[T, P]], (T, P))
check_parameters("annotated origin", list[Annotated[T, U]], (T,))
check_parameters("literal argument", list[Literal[T]], (T,))
check_parameters("native union", list[list[T] | dict[U, T]], (T, U))
check_parameters("raw nested tuple", types.GenericAlias(tuple, ((T, U),)),
                 (T, U) if sys.version_info >= (3, 14) else ())
check_parameters("raw nested list", types.GenericAlias(tuple, ([T, U],)),
                 (T, U) if sys.version_info >= (3, 14) else ())


class IteratedList(list):
    def __iter__(self):
        return iter((U,))


class IteratedTuple(tuple):
    def __iter__(self):
        raise AssertionError("native tuple storage must not call __iter__")


class MetadataList(list):
    __parameters__ = None


check_parameters("list subclass iterator", types.GenericAlias(tuple, (IteratedList([T]),)),
                 (U,) if sys.version_info >= (3, 14) else ())
check_parameters("tuple subclass storage", types.GenericAlias(tuple, (IteratedTuple((T,)),)),
                 (T,) if sys.version_info >= (3, 14) else ())
check_parameters("explicit metadata blocks descent", types.GenericAlias(tuple, (MetadataList([T]),)), ())


class MutatingProvider:
    @property
    def __parameters__(self):
        mutation_source.clear()
        return (U,)


mutation_source = [MutatingProvider(), T]
check_parameters("list snapshot survives callback mutation", types.GenericAlias(tuple, (mutation_source,)),
                 (U, T) if sys.version_info >= (3, 14) else ())
print("mutation follows target policy", len(mutation_source) == (0 if sys.version_info >= (3, 14) else 2))
if sys.version_info >= (3, 14):
    cycle = []
    cycle.append(cycle)
    try:
        types.GenericAlias(tuple, (cycle,)).__parameters__
    except RecursionError:
        print("raw cycle recursion refused", True)


class _TypeVar:
    pass


check_parameters("class name is not parameter identity", list[_TypeVar()], ())


class EqualityBomb:
    def __eq__(self, other):
        raise AssertionError("parameter collection called equality")

    def __hash__(self):
        raise AssertionError("parameter collection called hash")


first = EqualityBomb()
second = EqualityBomb()


class ParameterProvider:
    __parameters__ = (first, second, first)


check_parameters("provider identity", tuple[ParameterProvider(), ParameterProvider()],
                 (first, second))


class BareParameterMeta(type):
    @property
    def __parameters__(cls):
        raise AssertionError("bare class parameter descriptor must not run")


class BareParameterClass(metaclass=BareParameterMeta):
    pass


check_parameters("bare class ignored", list[BareParameterClass], ())


class ProtocolParameter:
    # CPython tests presence, not truthiness or callability, during collection.
    __typing_subst__ = None


protocol_parameter = ProtocolParameter()
check_parameters("substitution protocol presence", list[protocol_parameter],
                 (protocol_parameter,))


class FailingProvider:
    def __init__(self):
        self.calls = 0

    @property
    def __parameters__(self):
        self.calls += 1
        if self.calls == 1:
            raise ValueError("parameter getter failed")
        return (T,)


provider = FailingProvider()
cached_alias = list[provider]
try:
    cached_alias.__parameters__
except ValueError as error:
    print("getter error", str(error))
initial_parameters = cached_alias.__parameters__
print("cached tuple identity", cached_alias.__parameters__ is initial_parameters)
print("getter calls", provider.calls)


class AttributeErrorProvider:
    @property
    def __parameters__(self):
        raise AttributeError("no parameters here")


class InvalidProvider:
    __parameters__ = [T]


check_parameters("attribute error is missing", list[AttributeErrorProvider()], ())
check_parameters("non-tuple metadata ignored", list[InvalidProvider()], ())


alias_events = []


def alias_value():
    alias_events.append("value")
    return int


type LazyAlias[V] = alias_value()
check_parameters("declared alias parameters", list[LazyAlias], LazyAlias.__type_params__)
check_parameters("applied alias parameters", list[LazyAlias[T]], (T,))
print("collection preserves alias laziness", alias_events == [])
