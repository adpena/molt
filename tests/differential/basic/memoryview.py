# MOLT_META: stderr=exact
"""Purpose: differential coverage for memoryview basics."""

import sys


data = bytearray(b"abcd")
mv = memoryview(data)
print(mv[1])
mv[2] = 120
print(mv.tobytes())
print(mv[1:4].tobytes())

mv[1:4] = b"XYZ"
print(mv.tobytes())

stride = mv[::2]
print(stride.tobytes())
stride[1] = 122
print(mv.tobytes())

stride[:] = b"12"
print(mv.tobytes())
try:
    stride[::2] = b"12"
except Exception as exc:
    print(str(exc))

ro = memoryview(b"hi")
print(ro[0])
print(ro.tobytes())

if sys.version_info >= (3, 14):
    count_view = memoryview(bytearray(b"abca"))
    print(count_view.count(97))
    print(count_view.index(99))
    try:
        print(count_view.index(120))
    except Exception as exc:
        print(type(exc).__name__, str(exc))
else:
    count_view = memoryview(bytearray(b"abca"))
    for name in ("count", "index"):
        try:
            getattr(count_view, name)
        except AttributeError as error:
            print("old-view-method", name, type(error).__name__)
        else:
            raise AssertionError(("unexpected pre3.14 memoryview method", name))
count_view.release()


def released(label, view):
    try:
        view.tobytes()
    except ValueError as error:
        print(label, type(error).__name__, str(error))
    else:
        raise AssertionError((label, "view remained live"))


backing = bytearray(b"abcd")
with memoryview(backing) as entered, entered.cast("B") as cast:
    print("view-context", entered.__enter__() is entered, cast.__enter__() is cast)
    cast[0] = ord("Z")
    print("view-context-write", entered.tobytes(), bytes(backing))
released("view-context-parent", entered)
released("view-context-cast", cast)
backing.extend(b"!")
print("view-context-unpinned", bytes(backing))

# Arbitrary positional exit information is ignored; no argument is coerced.
for arguments in ((), (None,), (None, None, None), (1, 2, 3, 4)):
    view = memoryview(b"value")
    print("view-exit-positional", len(arguments), view.__exit__(*arguments))
    print("view-exit-idempotent", view.__exit__())
    try:
        view.__enter__()
    except ValueError as error:
        print("view-enter-released", type(error).__name__)
    else:
        raise AssertionError("released view entered")

view = memoryview(b"value")
for method, arguments, keywords in (
    (view.__enter__, (1,), {}),
    (view.__enter__, (), {"extra": 1}),
    (view.__exit__, (), {"exc_info": None}),
):
    try:
        method(*arguments, **keywords)
    except TypeError as error:
        print("view-context-call-error", type(error).__name__, str(error))
    else:
        raise AssertionError("invalid context call accepted")
print("view-context-call-preserved", view.tobytes())
view.release()

# Reentering the same view does not count nesting. Inner exit releases it;
# outer exit remains idempotent. A derived view has an independent release.
view = memoryview(bytearray(b"alias"))
child = view[1:].cast("B")
with view as outer:
    with view as inner:
        print("view-context-alias", outer is inner is view)
    released("view-context-inner-release", outer)
print("view-context-child", child.tobytes())
child.release()


class BodyError(Exception):
    pass


view = memoryview(bytearray(b"body"))
try:
    with view:
        raise BodyError("body-error")
except BodyError as error:
    print("view-context-body-error", str(error))
else:
    raise AssertionError("context suppressed body error")
released("view-context-error-release", view)

# A true exported Py_buffer blocks release. Exit failure preserves the view,
# and chains rather than swallowing an active body exception.
view = memoryview(bytearray(b"exported"))
export = view.__buffer__(0)
try:
    with view:
        raise BodyError("exported-body")
except BufferError as error:
    print("view-context-export-error", str(error), type(error.__context__).__name__)
else:
    raise AssertionError("exported view released")
print("view-context-export-live", view.tobytes())
export.release()
print("view-context-export-retry", view.__exit__(None, None, None))
released("view-context-export-done", view)

# Exporter release can reenter the original context through retained aliases.
# The original is marked released before callbacks and cannot release twice.
events = []


class Exporter:
    def __init__(self):
        self.data = bytearray(b"hook")
        self.context = None

    def __buffer__(self, flags):
        events.append(("acquire", flags))
        return memoryview(self.data)

    def __release_buffer__(self, view):
        events.append(("release", view.tobytes()))
        events.append(("reentrant-exit", self.context.__exit__(1, 2)))
        try:
            self.context.__enter__()
        except ValueError as error:
            events.append(("reentrant-enter", type(error).__name__))
        else:
            raise AssertionError("release callback reentered released view")


exporter = Exporter()
exporter.context = memoryview(exporter)
with exporter.context as alias:
    alias[0] = ord("H")
print("view-context-release-hook", events)
exporter.data.extend(b"!")
print("view-context-release-hook-unpinned", bytes(exporter.data))
