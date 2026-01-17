def show(label, value):
    print(label, value)


lst = [1, 2, 3]
popper = getattr(lst, "pop")
show("list_pop_default", popper())
show("list_after_default", lst)

lst2 = [1, 2, 3]
popper2 = getattr(lst2, "pop")
show("list_pop_zero", popper2(0))
show("list_after_zero", lst2)

d = {"a": 1}
getter = getattr(d, "get")
show("dict_get_hit", getter("a"))
show("dict_get_miss", getter("b"))
show("dict_get_default", getter("b", 9))

s = "hi"
upper = getattr(s, "upper")
show("str_upper", upper())
replace = getattr(s, "replace")
show("str_replace", replace("h", "H"))

lst3 = [1, 2, 3, 2]
indexer = getattr(lst3, "index")
show("list_index_default", indexer(2))
show("list_index_start", indexer(2, 2))

data2 = {"a": 1}
setdefault = getattr(data2, "setdefault")
show("dict_setdefault_hit", setdefault("a", 9))
show("dict_setdefault_miss", setdefault("b", 7))
show("dict_setdefault_dict", data2)

text = "banana"
replace_count = getattr(text, "replace")
show("str_replace_count", replace_count("a", "o", 1))


def deco(fn):
    def wrapper(*args, **kwargs):
        return ("wrap", fn(*args, **kwargs))

    return wrapper


@deco
def add(a, b=1):
    return a + b


show("decorated_call_default", add(2))
show("decorated_call_full", add(2, 3))


class CallProxy:
    def __init__(self, fn):
        self.fn = fn

    def __call__(self, *args, **kwargs):
        return self.fn(*args, **kwargs)


def make_proxy(fn):
    return CallProxy(fn)


@make_proxy
def mul(a, b=2):
    return a * b


show("decorated_callable_obj", mul(3))
show("decorated_callable_obj_full", mul(3, 4))


class Greeter:
    def greet(self, name="world"):
        return f"hi {name}"


g = Greeter()
show("greeter_default_direct", g.greet())
show("greeter_default_getattr", getattr(g, "greet")())
show("greeter_kw_direct", g.greet(name="molt"))
show("greeter_kw_getattr", getattr(g, "greet")(name="molt"))


class NonFuncDescriptor:
    __get__ = 123


class Host:
    bad = NonFuncDescriptor()


h = Host()
try:
    getattr(h, "bad")
except TypeError as exc:
    show("descriptor_nonfunc_get_error", type(exc).__name__)


class CallGet:
    def __call__(self, desc, inst, owner):
        inst_name = None if inst is None else inst.__class__.__name__
        owner_name = None if owner is None else owner.__name__
        return ("callget", desc.__class__.__name__, inst_name, owner_name)


class CallableDescriptor:
    __get__ = CallGet()


class Host2:
    val = CallableDescriptor()


h2 = Host2()
show("descriptor_callable_get_instance", getattr(h2, "val"))
show("descriptor_callable_get_class", getattr(Host2, "val"))


class Mix:
    @classmethod
    def cm(cls, value=1):
        return (cls.__name__, value)

    @staticmethod
    def sm(value=2):
        return ("sm", value)


class ChildMix(Mix):
    @staticmethod
    def cm():
        return ("child", "static")


m = Mix()
show("classmethod_getattr_instance", getattr(m, "cm")())
show("classmethod_getattr_class", getattr(Mix, "cm")())
show("classmethod_getattr_kw", getattr(m, "cm")(value=3))
show("staticmethod_getattr_instance", getattr(m, "sm")())
show("staticmethod_getattr_class", getattr(Mix, "sm")())
show("staticmethod_override_child", getattr(ChildMix(), "cm")())
