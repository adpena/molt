"""Purpose: globals() as a first-class callable."""

g = globals
print(g is globals)
print(g() is globals())
print("__name__" in g())

# Reference acquisition must not manufacture a module-local Python function.
print(type(g).__name__, g.__name__, g.__qualname__, g.__module__)


def captured(fn=globals):
    return fn is g, fn() is globals()


print(captured())


def cross_module():
    import builtins
    import globals_callable_support as support

    print(g is builtins.globals, g is support.alias)
    # The caller's frame, not the module that acquired the callable, owns globals.
    print(support.alias() is globals())
    print(support.invoke(g) is support.__dict__)
    print(support.invoke(support.alias) is support.__dict__)
    print(support.direct() is support.__dict__)
    print(support.invoke.__globals__ is support.__dict__)

    # Source locations and module metadata are not execution namespaces.
    original_file = support.__file__
    support.__file__ = __file__
    print(support.invoke(g) is support.__dict__)
    del support.__file__
    print(support.invoke(g) is support.__dict__)

    # Suspension and delayed traceback materialization retain the same owner.
    suspended = support.suspended(g)
    print(next(suspended) is support.__dict__)
    print(next(suspended) is support.__dict__)
    try:
        support.fail()
    except RuntimeError as error:
        traceback = error.__traceback__
        while traceback.tb_next is not None:
            traceback = traceback.tb_next
        print(traceback.tb_frame.f_globals is support.__dict__)
    support.__file__ = original_file


cross_module()


def rebound_builtin_shapes():
    import builtins
    from types import FunctionType
    import globals_callable_support as support

    def replacement(*args):
        return "replacement", args

    for name in (
        "bool",
        "int",
        "float",
        "complex",
        "str",
        "bytes",
        "bytearray",
        "tuple",
        "list",
        "set",
        "frozenset",
        "dict",
        "range",
        "len",
    ):
        original = getattr(support, "shape_" + name)
        # Builtin fallback and a shadowing global are independent lookup paths.
        for namespace in (
            {"__builtins__": {name: replacement}},
            {"__builtins__": builtins.__dict__, name: replacement},
        ):
            clone = FunctionType(original.__code__, namespace)
            print("rebound-shape", name, clone())

    class Namespace(dict):
        def pop(self, key, default=None):
            return "namespace-subclass", key, default

    namespace = Namespace(__builtins__=builtins.__dict__)
    clone = FunctionType(support.namespace_pop.__code__, namespace)
    print("rebound-namespace", clone())

    class GlobalNamespace(dict):
        def __getitem__(self, name):
            if name == "marker":
                return "global-subclass"
            raise KeyError(name)

        def __setitem__(self, name, value):
            raise AssertionError("STORE_GLOBAL must use dictionary storage")

        def __delitem__(self, name):
            raise AssertionError("DELETE_GLOBAL must use dictionary storage")

    class BuiltinNamespace(dict):
        def __getitem__(self, name):
            if name == "namespace_probe":
                return "builtin-subclass"
            raise KeyError(name)

    namespace = GlobalNamespace(__builtins__=BuiltinNamespace())
    for original in (
        support.read_marker,
        support.write_then_read_marker,
        support.read_builtin_probe,
    ):
        clone = FunctionType(original.__code__, namespace)
        print("rebound-lookup", clone())
        assert clone.__globals__ is namespace
    assert namespace.get("marker") == "stored"
    FunctionType(support.delete_marker.__code__, namespace)()
    assert "marker" not in namespace

    namespace = GlobalNamespace(
        __builtins__=builtins.__dict__, __package__=support.__name__
    )
    clone = FunctionType(support.relative_import_probe.__code__, namespace)
    assert clone() == 0
    print("rebound-relative-import", clone())

    original = support.make_closed_builtin_probe()

    def closure_cell(value):
        def retain():
            return value

        return retain.__closure__[0]

    clone = FunctionType(
        original.__code__, globals(), closure=(closure_cell(lambda value: 99),)
    )
    assert original() == 0
    assert clone() == 99
    print("rebound-closure-builtin", original(), clone())

    def empty_closure_cell():
        value = None

        def retain():
            return value  # noqa: F821 - intentionally emptied closure cell below

        del value
        return retain.__closure__[0]

    empty = FunctionType(original.__code__, globals(), closure=(empty_closure_cell(),))
    try:
        empty()
    except NameError:
        print("rebound-closure-empty", "NameError")
    else:
        raise AssertionError("an empty closure cell must not use builtin fallback")

    original_generator = support.make_closed_generator_probe()
    generator_code = original_generator.gi_code
    clone_factory = FunctionType(
        generator_code, globals(), closure=(closure_cell((1,)),)
    )
    clone_generator = clone_factory(iter((None,)))
    original_value, clone_value = next(original_generator), next(clone_generator)
    assert original_value is True and clone_value is False
    print("rebound-generator-closure", original_value, clone_value)
    original_generator.close()
    clone_generator.close()
    empty_factory = FunctionType(
        generator_code, globals(), closure=(empty_closure_cell(),)
    )
    empty_generator = empty_factory(iter((None,)))
    try:
        next(empty_generator)
    except NameError:
        print("rebound-generator-empty", "NameError")
    else:
        raise AssertionError("an empty generator cell must not use builtin fallback")
    finally:
        empty_generator.close()

    class FailingNamespace(dict):
        def __getitem__(self, name):
            raise ValueError("namespace lookup")

    for namespace in (
        FailingNamespace(__builtins__={"namespace_probe": "must-not-fall-through"}),
        {"__builtins__": FailingNamespace()},
    ):
        clone = FunctionType(support.read_builtin_probe.__code__, namespace)
        try:
            clone()
        except ValueError as error:
            print("rebound-lookup-error", str(error))

    class BuiltinMapping:
        def __getitem__(self, name):
            if name == "namespace_probe":
                return 73
            raise KeyError(name)

    # FunctionType normalizes a module to its dictionary, but retains every
    # other explicit builtins value. Its usability is checked only at lookup.
    mapping = BuiltinMapping()
    namespace = {"__builtins__": mapping}
    direct = FunctionType(support.read_builtin_probe.__code__, namespace)
    suspended = FunctionType(support.suspended_builtin_probe.__code__, namespace)
    namespace["__builtins__"] = {"namespace_probe": 99}
    iterator = suspended()
    assert direct() == next(iterator) == next(iterator) == 73
    iterator.close()
    print("rebound-builtins-mapping", direct())
    for invalid in (None, 17):
        clone = FunctionType(
            support.read_builtin_probe.__code__, {"__builtins__": invalid}
        )
        try:
            clone()
        except TypeError:
            print("rebound-builtins-invalid", type(invalid).__name__)
        else:
            raise AssertionError("explicit nonmapping builtins must not fall back")

    clone = FunctionType(support.shape_int.__code__, {"__builtins__": builtins})
    assert clone() == 0
    print("rebound-builtins-module", clone())


rebound_builtin_shapes()


def rebound_namespaces():
    import builtins
    from types import FunctionType
    import globals_callable_support as support

    first = {"__name__": "first", "__builtins__": builtins.__dict__, "marker": "first"}
    second = {
        "__name__": "second",
        "__builtins__": builtins.__dict__,
        "marker": "second",
    }
    for namespace in (first, second):
        direct = FunctionType(support.direct.__code__, namespace)
        invoke = FunctionType(support.invoke.__code__, namespace)
        reader = FunctionType(support.read_marker.__code__, namespace)
        writer = FunctionType(support.write_marker.__code__, namespace)
        deleter = FunctionType(support.delete_marker.__code__, namespace)
        factory = FunctionType(support.make_nested.__code__, namespace)
        generator = FunctionType(support.suspended.__code__, namespace)
        print(direct() is namespace, invoke(g) is namespace)
        print(reader())
        writer("updated")
        print(reader(), namespace["marker"], support.marker)
        child = factory()
        child_globals, child_marker = child()
        print(child.__globals__ is namespace, child_globals is namespace, child_marker)
        suspended = generator(g)
        print(next(suspended) is namespace, next(suspended) is namespace)
        deleter()
        print("marker" not in namespace, support.marker)

    original_builtins = {"namespace_probe": 41}
    namespace = {"__builtins__": original_builtins}
    reader = FunctionType(support.read_builtin_probe.__code__, namespace)
    factory = FunctionType(support.make_builtin_reader.__code__, namespace)
    rebind = FunctionType(support.rebind_without_builtins.__code__, namespace)
    namespace["__builtins__"] = {"namespace_probe": 99}
    print(reader())
    child = factory()
    implicit = rebind(FunctionType, support.read_builtin_probe.__code__)
    print(child(), implicit())
    original_builtins["namespace_probe"] = 42
    print(reader(), child(), implicit())


rebound_namespaces()


def guarded_original(value=3):
    return "original", value


guarded_alias = guarded_original


def invoke_guarded_alias():
    # Both calls retain the original function hint. Neither its entry ABI nor
    # its default padding is authority after the live binding changes.
    return guarded_alias(), guarded_alias(7)


def guarded_callable_rebinding():
    namespace = globals()
    print("guarded-original", invoke_guarded_alias())

    def replacement(value=11, extra=19):
        return "replacement", value, extra

    namespace["guarded_alias"] = replacement
    print("guarded-shape", invoke_guarded_alias())
    replacement.__defaults__ = (23, 29)
    print("guarded-defaults", invoke_guarded_alias())

    def make_closed(label, default):
        def closed(value=default):
            return label, value

        return closed

    left = make_closed("left", 41)
    right = make_closed("right", 43)
    print("guarded-shared-code", left.__code__ is right.__code__)
    for current in (left, right, left):
        namespace["guarded_alias"] = current
        print("guarded-cell", invoke_guarded_alias())

    class CallableReplacement:
        def __call__(self, value=47):
            return "callable-object", value

        def method(self, value=53):
            return "method", value

    instance = CallableReplacement()
    namespace["guarded_alias"] = instance
    print("guarded-object", invoke_guarded_alias())
    bound = instance.method
    CallableReplacement.method.__defaults__ = (59,)
    print("guarded-method-defaults", instance.method(), bound())

    class GuardedError(Exception):
        def __init__(self, value=61):
            self.value = value

    GuardedError.__init__.__defaults__ = (67,)
    print("guarded-init-defaults", GuardedError().value)
    namespace["guarded_alias"] = guarded_original


guarded_callable_rebinding()


def task_marker_attributes_are_not_execution_authority():
    import inspect
    from types import FunctionType

    def direct():
        return "direct"

    def generator():
        yield "generator"

    async def coroutine():
        return "coroutine"

    async def async_generator():
        yield "async-generator"

    namespace = globals()
    for definition in (direct, generator, coroutine, async_generator):
        sibling = FunctionType(definition.__code__, namespace)
        # Mutating one function must not rewrite shared code execution kind,
        # and creating another function afterward must retain the same kind.
        definition.__molt_is_generator__ = True
        definition.__molt_is_coroutine__ = True
        definition.__molt_is_async_generator__ = True
        definition.__molt_closure_size__ = 0
        sibling.__molt_is_generator__ = False
        sibling.__molt_is_coroutine__ = False
        sibling.__molt_is_async_generator__ = False
        sibling.__molt_closure_size__ = 999
        later = FunctionType(definition.__code__, namespace)
        predicates = []
        results = []
        for current in (definition, sibling, later):
            predicates.append(
                (
                    inspect.isgeneratorfunction(current),
                    inspect.iscoroutinefunction(current),
                    inspect.isasyncgenfunction(current),
                )
            )
            value = current()
            if definition is direct:
                results.append(value)
            elif definition is generator:
                results.append(next(value))
                value.close()
            elif definition is coroutine:
                try:
                    value.send(None)
                except StopIteration as finished:
                    results.append(finished.value)
            else:
                step = value.__anext__()
                try:
                    step.send(None)
                except StopIteration as finished:
                    results.append(finished.value)
                closing = value.aclose()
                try:
                    closing.send(None)
                except StopIteration:
                    pass
        print("task-marker-data", results)
        print("task-marker-inspect", predicates)

    marked = FunctionType(direct.__code__, namespace)
    unmarked = FunctionType(direct.__code__, namespace)
    marked._is_coroutine_marker = object()
    print("coroutine-marker-identity", inspect.iscoroutinefunction(marked))
    print("coroutine-mark", inspect.markcoroutinefunction(marked) is marked)
    print(
        "coroutine-mark-separate",
        inspect.iscoroutinefunction(marked),
        inspect.iscoroutinefunction(unmarked),
        inspect.isgeneratorfunction(marked),
        marked(),
    )

    from types import coroutine as iterable_coroutine

    @iterable_coroutine
    def iterable():
        yield "iterable"

    awaitable_generator = iterable()
    print(
        "iterable-coroutine-protocol",
        inspect.isgeneratorfunction(iterable),
        inspect.iscoroutinefunction(iterable),
        inspect.isawaitable(awaitable_generator),
        inspect.iscoroutine(awaitable_generator),
    )
    awaitable_generator.close()

    wrapped_generator = iterable_coroutine(lambda: generator())()
    print(
        "wrapped-generator-protocol",
        inspect.isawaitable(wrapped_generator),
        inspect.iscoroutine(wrapped_generator),
        next(wrapped_generator),
    )
    wrapped_generator.close()


task_marker_attributes_are_not_execution_authority()


# Helper spelling cannot authorize runtime-intrinsic substitution or fabricate
# callable identity. Exercise ordinary, annotated and deferred assignments.
optional_loader_events = []


def _load_optional_intrinsic(name):
    optional_loader_events.append(name)
    return lambda value: (name, value)


optional_plain = _load_optional_intrinsic("molt_gpu_linear_contiguous")
optional_annotated: object = _load_optional_intrinsic("arbitrary-user-value")


def deferred_optional_loader():
    local = _load_optional_intrinsic("deferred-user-value")
    return local(3)


assert optional_plain(1) == ("molt_gpu_linear_contiguous", 1)
assert optional_annotated(2) == ("arbitrary-user-value", 2)
assert deferred_optional_loader() == ("deferred-user-value", 3)
assert optional_loader_events == [
    "molt_gpu_linear_contiguous",
    "arbitrary-user-value",
    "deferred-user-value",
]
print("ordinary-optional-loader", optional_loader_events)


class LayoutPoint:
    def __init__(self, x, y):
        self.x = x
        self.y = y


def live_constructor_layout(value):
    point = LayoutPoint(0, 0)
    alias = point
    alias.x = value
    point.y = value + 1
    alias.x += 2
    return point.x, alias.y


layout_events = []


class ObservedLayoutPoint:
    def __init__(self, x, y):
        self._x = x
        self.y = y

    @property
    def x(self):
        layout_events.append(("get", self._x))
        return self._x

    @x.setter
    def x(self, value):
        layout_events.append(("set", value))
        self._x = value


# The lexical class's fixed offsets cannot describe an arbitrary constructor
# from the active namespace, including aliases and augmented assignments.
from types import FunctionType

foreign_layout = FunctionType(
    live_constructor_layout.__code__,
    {"LayoutPoint": ObservedLayoutPoint, "__builtins__": __builtins__},
)
assert live_constructor_layout(5) == (7, 6)
assert foreign_layout(5) == (7, 6)
assert layout_events == [("set", 5), ("get", 5), ("set", 7), ("get", 7)]
print("live-constructor-layout", layout_events)


class ChangedLayoutPoint:
    @property
    def x(self):
        return self.y * 10

    @x.setter
    def x(self, value):
        self.y = value


def change_layout(point):
    point.__class__ = ChangedLayoutPoint


# A previously exact allocation can lose its class identity at a callback.
layout_mutation_target = LayoutPoint(0, 2)
change_layout(layout_mutation_target)
layout_mutation_target.x = 3
assert layout_mutation_target.x == 30
print("callback-class-mutation", layout_mutation_target.x)


def replace_layout_during_rhs(point):
    point.__class__ = ChangedLayoutPoint
    return 3


# The augmented load happens before the RHS, but its store happens after it.
# Neither the saved receiver nor its earlier exact-class fact licenses bypassing
# a descriptor installed by the intervening callback.
augmented_layout_target = LayoutPoint(4, 2)
augmented_layout_target.x += replace_layout_during_rhs(augmented_layout_target)
assert augmented_layout_target.y == 7
assert augmented_layout_target.x == 70
print("augmented-callback-class-mutation", augmented_layout_target.x)

# Builtin argument evaluation holds the receiver across a later callback too.
setattr_layout_target = LayoutPoint(4, 2)
setattr(
    setattr_layout_target,
    "x",
    replace_layout_during_rhs(setattr_layout_target),
)
assert setattr_layout_target.y == 3
assert setattr_layout_target.x == 30
print("setattr-callback-class-mutation", setattr_layout_target.x)

# getattr evaluates its default even when the attribute exists, and evaluates
# it before performing the lookup. The callback changes which descriptor wins.
getattr_layout_target = LayoutPoint(4, 2)
assert (
    getattr(
        getattr_layout_target,
        "x",
        replace_layout_during_rhs(getattr_layout_target),
    )
    == 20
)
print("getattr-default-class-mutation", getattr_layout_target.x)

try:
    getattr(getattr_layout_target, "x", 1 // 0)
except ZeroDivisionError:
    print("getattr-present-attribute-default-still-raises")
else:
    raise AssertionError("getattr skipped evaluation of its default")

# A layout guard established outside a loop cannot outlive a callback inside it.
loop_layout_target = LayoutPoint(1, 2)
loop_layout_values = []
for layout_iteration in range(2):
    if layout_iteration == 0:
        change_layout(loop_layout_target)
    loop_layout_target.x = layout_iteration + 4
    loop_layout_values.append(loop_layout_target.x)
assert loop_layout_values == [40, 50]
print("loop-callback-class-mutation", loop_layout_values)

backedge_layout_target = LayoutPoint(1, 2)
backedge_layout_values = []
for layout_iteration in range(2):
    backedge_layout_values.append(backedge_layout_target.x)
    if layout_iteration == 0:
        change_layout(backedge_layout_target)
assert backedge_layout_values == [1, 20]
print("loop-backedge-class-mutation", backedge_layout_values)


def replace_layout_and_make_point(point):
    change_layout(point)
    return LayoutPoint(8, 9)


# The held first argument is not the later binding of its source name.
rebound_layout_target = LayoutPoint(4, 2)
saved_layout_target = rebound_layout_target
setattr(
    rebound_layout_target,
    "x",
    (rebound_layout_target := replace_layout_and_make_point(rebound_layout_target)).x,
)
assert saved_layout_target.x == 80
assert rebound_layout_target.x == 8
print("held-receiver-name-rebinding", saved_layout_target.x, rebound_layout_target.x)
