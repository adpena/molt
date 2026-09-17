"""Cross-module acquisition and invocation for the globals callable capsule."""

alias = globals
marker = "lexical"


def invoke(fn):
    return fn()


def direct():
    return globals()


def suspended(fn):
    yield fn()
    yield fn()


def fail():
    raise RuntimeError("namespace custody")


def read_marker():
    return marker


def write_marker(value):
    global marker
    marker = value


def delete_marker():
    global marker
    del marker


def make_nested():
    def child():
        return globals(), marker

    return child


def read_builtin_probe():
    return namespace_probe  # noqa: F821 - supplied by the captured builtins mapping


def make_builtin_reader():
    def child():
        return namespace_probe  # noqa: F821 - supplied by the captured builtins mapping

    return child


def rebind_without_builtins(make, code):
    return make(code, {})


# Each body is separately callable: the first unknown invocation in a combined
# body must not accidentally make the remaining probes conservative.
def shape_bool():
    return bool()


def shape_int():
    return int()


def shape_float():
    return float()


def shape_complex():
    return complex()


def shape_str():
    return str()


def shape_bytes():
    return bytes()


def shape_bytearray():
    return bytearray()


def shape_tuple():
    return tuple()


def shape_list():
    return list()


def shape_set():
    return set()


def shape_frozenset():
    return frozenset()


def shape_dict():
    return dict()


def shape_range():
    return range(3)


def shape_len():
    return len(())


def namespace_pop():
    return globals().pop("missing", None)


def write_then_read_marker():
    global marker
    marker = "stored"
    return marker


def suspended_builtin_probe():
    yield namespace_probe  # noqa: F821 - supplied by captured builtins
    yield namespace_probe  # noqa: F821 - supplied by captured builtins


def relative_import_probe():
    from . import shape_len

    return shape_len()


def make_closed_builtin_probe():
    from builtins import len as measure

    def probe():
        return measure(())

    return probe


def make_closed_generator_probe():
    value = ()
    return (not value for _ in (None,))
