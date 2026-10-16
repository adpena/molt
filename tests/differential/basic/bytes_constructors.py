"""Purpose: differential coverage for bytes constructors."""

print(bytes("abc", "utf-8"))
print(bytes("abc", encoding="utf-8"))
print(bytearray("abc", "utf-8"))
print(bytearray("abc", encoding="utf-8"))

try:
    bytes("abc")
except TypeError as exc:
    print(f"bytes-missing-encoding:{exc}")

try:
    bytearray("abc")
except TypeError as exc:
    print(f"bytearray-missing-encoding:{exc}")

try:
    bytes(encoding="utf-8")
except TypeError as exc:
    print(f"bytes-encoding-nosrc:{exc}")

try:
    bytes(errors="strict")
except TypeError as exc:
    print(f"bytes-errors-nosrc:{exc}")

print(bytes(3))
print(bytearray(3))
print(bytes([1, 2, 3]))
print(bytearray([1, 2, 3]))

try:
    bytes([256])
except ValueError as exc:
    print(f"bytes-range:{exc}")

try:
    bytearray([256])
except ValueError as exc:
    print(f"bytearray-range:{exc}")

try:
    bytes([1, "a"])
except TypeError as exc:
    print(f"bytes-type:{exc}")

try:
    bytearray([1, "a"])
except TypeError as exc:
    print(f"bytearray-type:{exc}")

try:
    bytes(1.2)
except TypeError as exc:
    print(f"bytes-float:{exc}")

try:
    bytearray(1.2)
except TypeError as exc:
    print(f"bytearray-float:{exc}")


# Join uses semantic result classes and fully consumes iterables before it
# validates or borrows any item. These are public CPython protocol oracles.
class BytesSubclass(bytes):
    pass


class StringSubclass(str):
    pass


for separator, exact, subtype in (
    (b"-", b"one", BytesSubclass(b"one")),
    ("-", "one", StringSubclass("one")),
):
    for source in ([exact], (exact,), iter([exact])):
        print("join-exact-singleton", separator.join(source) is exact)
    result = separator.join([subtype])
    print("join-subtype-result", type(result) is type(exact), result == exact)
    print("join-repeated", separator.join([exact] * 4))

for source in ([b"one"], (b"one",), iter([b"one"])):
    result = bytearray(b"-").join(source)
    print("join-mutable-result", type(result) is bytearray, result == b"one")


class JoinList(list):
    def __iter__(self):
        return iter([b"override"])


print("join-list-subclass", b"-".join(JoinList([b"storage"])))


def mutate_item(item):
    yield item
    item.extend(b"longer")
    yield b"tail"


for separator in (b"/", bytearray(b"/")):
    item = bytearray(b"head")
    print("join-mutated-item", separator.join(mutate_item(item)))
    # Temporary export leases must have retired on successful completion.
    item.extend(b"!")
    print("join-released-item", item)


class JoinIterator:
    def __init__(self, values, events, failure=None):
        self.values = values
        self.events = events
        self.index = 0
        self.failure = failure

    def __iter__(self):
        self.events.append("iter")
        return self

    def __length_hint__(self):
        self.events.append("hint")
        return len(self.values)

    def __next__(self):
        self.events.append("next")
        if self.index == len(self.values):
            if self.failure is not None:
                raise self.failure
            raise StopIteration
        value = self.values[self.index]
        self.index += 1
        return value


for separator, good in ((b"/", b"ok"), (bytearray(b"/"), b"ok"), ("/", "ok")):
    events = []
    try:
        separator.join(JoinIterator([1, good], events))
    except TypeError as error:
        print("join-deferred-type", str(error), events)
    events = []
    sentinel = RuntimeError("iterator wins")
    try:
        separator.join(JoinIterator([1], events, sentinel))
    except Exception as error:
        print("join-iterator-priority", error is sentinel, events)

# Separator export begins before iterator callbacks. Its in-place edits are
# visible, but resizing must fail; all completion paths release the export.
for resize in (False, True):
    separator = bytearray(b"-")
    events = []

    def mutate_separator():
        yield b"a"
        if resize:
            try:
                separator.extend(b"+")
            except BufferError:
                events.append("pinned")
        else:
            separator[0] = ord("+")
        yield b"b"

    print("join-separator", separator.join(mutate_separator()), events)
    separator.extend(b"!")
    print("join-separator-released", separator)

separator = bytearray(b"-")
try:
    separator.join([bytearray(b"valid"), 7])
except TypeError:
    separator.extend(b"!")
    print("join-error-release", separator)

view = memoryview(bytearray(b"live"))


def release_item():
    yield view
    view.release()


try:
    b"".join(release_item())
except TypeError as error:
    print("join-released-view", type(error).__name__)

for separator in (b"/", bytearray(b"/")):
    print("join-buffer", separator.join([memoryview(b"abc"), bytearray(b"def")]))
    try:
        separator.join([memoryview(b"abcdef")[::2]])
    except TypeError as error:
        print("join-strided", type(error).__name__)


# An exact list is held, not copied: replacing a future sole-owned element
# executes its finalizer before the exporter returns and before output copying.
for separator in (b"-", bytearray(b"-")):
    events = []
    later = bytearray(b"x")

    class DisplacedJoinItem:
        def __del__(self):
            events.append("displaced")
            later[0] = ord("z")

    class ReplacingJoinExporter:
        def __buffer__(self, flags):
            events.append(("export", flags))
            items[1] = b"b"
            events.append("after-replace")
            return memoryview(b"a")

        def __release_buffer__(self, view):
            events.append("release")

    items = [ReplacingJoinExporter(), DisplacedJoinItem(), later]
    print("join-displaced-finalizer", separator.join(items), events)

# PySequence_List owns its elements and retires them last-to-first on a
# validation failure. The generator itself retains neither returned object.
for separator in (b"", bytearray(), ""):
    events = []

    class InvalidJoinItem:
        def __init__(self, name):
            self.name = name

        def __del__(self):
            events.append(self.name)

    def invalid_join_items():
        yield InvalidJoinItem("first")
        yield InvalidJoinItem("second")

    try:
        separator.join(invalid_join_items())
    except TypeError:
        print("join-materialized-retirement", events)

# An exporter MemoryError is normalized, unlike the join owner's own allocation
# failure. Exercise the actual protocol boundary, not a guessed error class.
class FailingJoinExporter:
    def __buffer__(self, flags):
        raise MemoryError("exporter failed")


for separator in (b"", bytearray()):
    try:
        separator.join([FailingJoinExporter()])
    except Exception as error:
        print("join-exporter-memory-error", type(error).__name__)


for separator in (b"", bytearray(), ""):
    events = []
    sentinel = RuntimeError("late materialization error")

    def failing_join_items():
        yield InvalidJoinItem("first")
        yield InvalidJoinItem("second")
        raise sentinel

    try:
        separator.join(failing_join_items())
    except RuntimeError as error:
        print("join-partial-retirement", error is sentinel, events)


# Actual managed classes exercise slot projection through public consumers.
# CPython 3.12+ PEP 688 supplies the independent output oracle.
class ManagedBuffer:
    def __init__(self, name, data, events):
        self.name = name
        self.data = data
        self.events = events
        self.views = []

    def __buffer__(self, flags):
        self.events.append((self.name, "get", flags))
        view = memoryview(self.data)
        self.views.append(view)
        return view

    def __release_buffer__(self, view):
        self.events.append((self.name, "release", view is self.views.pop(0)))


for operation in ("memoryview", "bytes", "bytearray", "join", "add"):
    events = []
    source = ManagedBuffer("source", bytearray(b"abc"), events)
    if operation == "memoryview":
        result = memoryview(source)
        print("managed-result", operation, result.tobytes())
        result.release()
    elif operation == "bytes":
        print("managed-result", operation, bytes(source))
    elif operation == "bytearray":
        print("managed-result", operation, bytearray(source))
    elif operation == "join":
        print("managed-result", operation, b"/".join([source, source]))
    else:
        print("managed-result", operation, b"!" + source)
    print("managed-events", operation, events)
    source.data.extend(b"!")
    print("managed-unpinned", source.data)


class ManagedChild(ManagedBuffer):
    pass


events = []
source = ManagedChild("child", b"abc", events)
view = memoryview(source)
view.release()
print("managed-inherited", events)


def replacement_buffer(self, flags):
    self.events.append(("replacement", flags))
    view = memoryview(b"xyz")
    self.views.append(view)
    return view


ManagedChild.__buffer__ = replacement_buffer
view = memoryview(source)
print("managed-overridden", view.tobytes())
view.release()
del ManagedChild.__buffer__
view = memoryview(source)
print("managed-deleted-override", view.tobytes())
view.release()
ManagedChild.__buffer__ = None
try:
    memoryview(source)
except Exception as error:
    print("managed-none-slot", type(error).__name__)
del ManagedChild.__buffer__
print("managed-mutation-events", events)


class InvalidManagedBuffer:
    def __buffer__(self, flags):
        return b"not-a-memoryview"


try:
    memoryview(InvalidManagedBuffer())
except Exception as error:
    print("managed-invalid-result", type(error).__name__)


# A field with the private array spelling never grants public buffer eligibility.
import array


class HandleImpostor:
    def __init__(self, value):
        self._handle = value


real_array = array.array("B", [97, 98])
for value in (HandleImpostor(real_array), HandleImpostor(0)):
    try:
        memoryview(value)
    except Exception as error:
        print("managed-handle-impostor", type(error).__name__)


class ArrayChild(array.array):
    pass


for array_type in (array.array, ArrayChild):
    value = array_type("B", [97, 98])
    view = memoryview(value)
    print("declared-array", view.tobytes(), view.format, view.itemsize)
    try:
        value.append(99)
    except BufferError:
        print("declared-array-pinned", True)
    view.release()
    value.append(99)
    print("declared-array-released", value.tolist())
    direct = array.array.__buffer__(value, 0)
    print("declared-array-direct", direct.tobytes())
    array.array.__release_buffer__(value, direct)
    direct.release()
    try:
        array.array.__release_buffer__(value, memoryview(b"wrong"))
    except Exception as error:
        print("declared-array-wrong-release", type(error).__name__)


# Indirect source exports pin the destination through extend, while direct
# self-extension uses CPython's self-copy exception.
events = []
destination = bytearray(b"abc")
source = ManagedBuffer("extend", destination, events)
try:
    destination.extend(source)
except Exception as error:
    print("managed-extend-alias", type(error).__name__, destination, events)
destination.extend(destination)
print("managed-extend-self", destination)

# Direct descriptors remain inherited, overridden and restored by deletion.
def child_array_buffer(self, flags):
    return memoryview(b"override")

ArrayChild.__buffer__ = child_array_buffer
array_child = ArrayChild("B", [1, 2])
view = memoryview(array_child)
print("declared-array-override", view.tobytes())
view.release()
del ArrayChild.__buffer__
view = memoryview(array_child)
print("declared-array-restore", view.tobytes())
view.release()

# Releasing a derived view retires the callback exactly once at final export.
events = []
source = ManagedBuffer("shared", bytearray(b"abc"), events)
view = memoryview(source)
child_view = view[1:]
view.release()
print("managed-child-live", events, child_view.tobytes())
child_view.release()
print("managed-child-release", events)
source.data.extend(b"!")

# GC visits the producer/view cycle through the existing native lease edge.
import gc
import weakref

class CycleBuffer:
    def __buffer__(self, flags):
        self.inner = memoryview(bytearray(b"cycle"))
        return self.inner

    def __release_buffer__(self, view):
        pass

cycle = CycleBuffer()
cycle.outer = memoryview(cycle)
cycle_ref = weakref.ref(cycle)
del cycle
gc.collect()
print("managed-buffer-cycle-collected", cycle_ref() is None)


import sys

class ReleaseFailureBuffer:
    def __init__(self):
        self.data = bytearray(b"failure")

    def __buffer__(self, flags):
        return memoryview(self.data)

    def __release_buffer__(self, view):
        raise RuntimeError("release-marker")

release_errors = []
def observe_release_error(record):
    release_errors.append((record.exc_type.__name__, str(record.exc_value)))

previous_hook = sys.unraisablehook
sys.unraisablehook = observe_release_error
try:
    source = ReleaseFailureBuffer()
    view = memoryview(source)
    view.release()
    source.data.extend(b"!")
    print("managed-release-failure", release_errors, source.data)
finally:
    sys.unraisablehook = previous_hook

# Reimporting the producer must bind descriptors to the live declaring class.
import importlib
array = importlib.reload(array)
reloaded_array = array.array("B", [3, 4])
reloaded_view = memoryview(reloaded_array)
print("declared-array-reload", reloaded_view.tobytes())
reloaded_view.release()
reloaded_array.append(5)
print("declared-array-reload-unpinned", reloaded_array.tolist())


# Passing type as a callable and expanding its operand must preserve the same
# native exception identity as a direct type query, without consulting __class__.
def projected_type(query, value):
    return query(*(value,))


for label, operation, expected in (
    ("invalid-buffer", lambda: memoryview(InvalidManagedBuffer()), TypeError),
    ("array-release", lambda: array.array.__release_buffer__(
        reloaded_array, memoryview(b"wrong")), ValueError),
):
    try:
        operation()
    except expected as error:
        print("callable-native-error-type", label, type(error) is expected,
              projected_type(type, error) is expected,
              projected_type(type, error).__name__, isinstance(error, expected))
    else:
        print("callable-native-error-type", label, "missing-error")


class ClaimedType:
    @property
    def __class__(self):
        raise AssertionError("type must not inspect a claimed class")


print("callable-managed-type", projected_type(type, ClaimedType()) is ClaimedType,
      projected_type(type, 17) is int, projected_type(type, None) is type(None))
