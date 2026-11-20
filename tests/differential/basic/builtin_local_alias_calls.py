"""A local bound to a builtin calls that builtin, and only while it is one.

`call_dir = dir` binds the builtin itself; a call through the alias must
behave exactly as a call by name: same results, same errors, same defaults.
A rebinding the program can reach (a branch, a loop, a closure write, a
shadowed module global) must keep the live value.
"""


class Box:
    pass


def direct_aliases():
    call_len = len
    call_delattr = delattr
    call_hasattr = hasattr
    call_getattr = getattr
    box = Box()
    box.value = 3
    call_delattr(box, "value")
    print("len", call_len([1, 2, 3]), call_len("abcd"))
    print("hasattr", call_hasattr(box, "value"))
    print("getattr default", call_getattr(box, "value", "missing"))
    try:
        call_delattr(box, "value")
    except AttributeError as exc:
        print("delattr error", type(exc).__name__)
    try:
        call_len(5)
    except TypeError as exc:
        print("len error", exc)


def conditional_alias(flag):
    call = len
    if flag:
        call = lambda value: -1
    return call([1, 2])


def loop_rebinding(items):
    call = len
    out = []
    for item in items:
        out.append(call(item))
        call = repr
    return out


def closure_alias():
    call = len

    def rebind():
        nonlocal call
        call = str

    before = call([1, 2, 3])
    rebind()
    return before, call([1, 2, 3])


dir = lambda *args: ["shadowed"]


def shadowed_global_alias():
    call_dir = dir
    return call_dir()


def import_alias():
    call_import = __import__
    module = call_import("math")
    return module.__name__


direct_aliases()
print("conditional", conditional_alias(False), conditional_alias(True))
print("loop", loop_rebinding(["ab", "cde", "f"]))
print("closure", closure_alias())
print("shadowed", shadowed_global_alias())
print("import", import_alias())
