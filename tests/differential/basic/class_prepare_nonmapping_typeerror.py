"""Purpose: differential coverage for metaclass __prepare__ returning a non-mapping.

CPython 3.12+ validates the object returned by ``metaclass.__prepare__`` before
executing the class body: if it is not a mapping (``PyMapping_Check`` —
``tp_as_mapping->mp_subscript``), ``__build_class__`` raises

    TypeError: <metaclass>.__prepare__() must return a mapping, not <type>

A value whose type has no read-subscript (``int``, ``object``, ``set``,
``frozenset``, ``slice``, the ``dict`` views, ...) is rejected with that exact,
metaclass-qualified message.  A value whose type *does* provide a read-subscript
— a real mapping (``dict``, ``dict`` subclass, a custom class with
``__getitem__``) — is accepted and the class body executes into it; that path is
covered by ``class_prepare_mapping.py`` / ``class_prepare_order.py``.
"""


class MetaInt(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return 42


class MetaObj(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return object()


class MetaSet(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return {1, 2, 3}


for meta in (MetaInt, MetaObj, MetaSet):
    try:

        class Body(metaclass=meta):
            field = 1

    except TypeError as exc:
        print(meta.__name__, "->", str(exc))


# A valid mapping (dict subclass with a logging __setitem__) is accepted and
# every class-body binding flows through the namespace's mapping protocol.
log = []


class LogDict(dict):
    def __setitem__(self, key, value):
        log.append(key)
        super().__setitem__(key, value)


class MetaOk(type):
    @classmethod
    def __prepare__(mcls, name, bases, **kwargs):
        return LogDict()


class Accepted(metaclass=MetaOk):
    alpha = 1
    beta = 2


print("accepted_alpha", Accepted.alpha)
print("accepted_beta", Accepted.beta)
print("logged", [k for k in log if k in ("alpha", "beta")])
