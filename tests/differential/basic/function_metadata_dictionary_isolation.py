"""Function dictionaries share the ordinary descriptor/storage authority.

CPython oracle: Objects/funcobject.c function getsets use GenericGetDict and
GenericSetDict; Objects/classobject.c methods delegate reads to their function;
Lib/functools.py update_wrapper updates the wrapper's __dict__ from the wrapped
object. The differential runner supplies matching CPython target expectations.
"""

from functools import update_wrapper, wraps
import gc
import inspect
import weakref


def dictionary_target():
    pass


first = dictionary_target.__dict__
print("lazy dictionary", first is dictionary_target.__dict__, vars(dictionary_target) is first)
dictionary_target.extra = 17
print("attribute owner", first["extra"], vars(dictionary_target)["extra"])
first["from_dict"] = 23
print("mapping owner", dictionary_target.from_dict)
del dictionary_target.extra
print("delete attribute", "extra" in first)
replacement = {"replacement": 29}
dictionary_target.__dict__ = replacement
print("replace owner", dictionary_target.__dict__ is replacement, vars(dictionary_target) is replacement)
print("retired mapping", first["from_dict"], hasattr(dictionary_target, "from_dict"))
for invalid in (None, [], 7):
    try:
        dictionary_target.__dict__ = invalid
    except TypeError:
        print("reject dictionary", dictionary_target.__dict__ is replacement)
try:
    del dictionary_target.__dict__
except TypeError:
    print("reject delete", dictionary_target.__dict__ is replacement)


class MethodOwner:
    def method(self):
        return 31


instance = MethodOwner()
bound = instance.method
MethodOwner.method.flag = 37
print("method owner", bound.__dict__ is MethodOwner.method.__dict__, vars(bound)["flag"])
bound.__dict__["shared"] = 41
print("method mutation", MethodOwner.method.shared, instance.method.shared)
try:
    bound.__dict__ = {}
except AttributeError:
    print("method readonly", bound.shared)


@wraps(dictionary_target)
def decorated():
    return 43


def wrapper():
    return 47


wrapper.retained = 53
update_wrapper(wrapper, dictionary_target)
print("wraps copies", decorated.replacement, decorated.__wrapped__ is dictionary_target)
print("update copies", wrapper.replacement, wrapper.retained, wrapper.__wrapped__ is dictionary_target)
print("wrapper ownership", decorated.__dict__ is not replacement, wrapper.__dict__ is not replacement)

events = []


class RetiredAttribute:
    def __del__(self):
        events.append(dictionary_target.__dict__.get("published"))


dictionary_target.__dict__ = {"retired": RetiredAttribute()}
dictionary_target.__dict__ = {"published": 59}
print("retirement publication", events)


def make_cycle():
    def cyclic():
        pass
    cyclic.owner = cyclic
    return weakref.ref(cyclic)


cycle = make_cycle()
gc.collect()
print("dictionary cycle", cycle() is None)

"""Typed function fields and ordinary user metadata have independent authority."""


def target(a=11, *, b=13):
    "original documentation"
    return a, b


def other():
    return "wrong code"


original_code = target.__code__
original_globals = target.__globals__
print("initial user metadata", vars(target))
target.__dict__.update(
    __defaults__=(101,),
    __kwdefaults__={"b": 103},
    __name__="forged",
    __qualname__="forged.qualname",
    __doc__="forged documentation",
    __code__=other.__code__,
    __globals__={},
    __closure__=("forged closure",),
)
print("dict writes", target(), target.__defaults__, target.__kwdefaults__)
print("names", target.__name__, target.__qualname__, target.__doc__)
print(
    "execution fields",
    target.__code__ is original_code,
    target.__globals__ is original_globals,
    target.__closure__ is None,
)
target.__defaults__ = (17,)
target.__kwdefaults__ = {"b": 19}
print("typed writes", target(), target.__dict__["__defaults__"])
target.__dict__.clear()
print("dict clear", target(), target.__name__, target.__doc__)
target.__dict__ = {"__defaults__": (107,), "__kwdefaults__": {"b": 109}}
print("dict replace", target(), target.__defaults__, target.__kwdefaults__)
del target.__defaults__
del target.__kwdefaults__
print("typed delete", target.__defaults__, target.__kwdefaults__)
print("dict remains", target.__dict__["__defaults__"], target.__dict__["__kwdefaults__"])
try:
    target()
except TypeError:
    print("required arguments")
print("explicit arguments", target(23, b=29))

# Compiler-private-looking public attributes are still user attributes. Neither
# attribute nor mapping mutation may rewrite compiled argument binding.
def isolated(value=61, *, named=67):
    return value, named

isolated.__molt_vararg__ = True
isolated.__dict__["__molt_kwonly_names__"] = ()
isolated.__dict__["__molt_arg_names__"] = ()
print("private-looking attributes", isolated(), isolated.__molt_vararg__)
print("private-looking signature", str(inspect.signature(isolated)))
isolated.__dict__.clear()
print("clear private-looking attributes", isolated())
print("cleared signature", str(inspect.signature(isolated)))
for invalid in ([], 7):
    try:
        isolated.__defaults__ = invalid
    except TypeError:
        print("defaults rejection", isolated())
    try:
        isolated.__kwdefaults__ = invalid
    except TypeError:
        print("keyword defaults rejection", isolated())

def typed_cycle():
    def value(arg=None):
        pass
    value.__defaults__ = (value,)
    return weakref.ref(value)

typed_ref = typed_cycle()
gc.collect()
print("typed field cycle", typed_ref() is None)
