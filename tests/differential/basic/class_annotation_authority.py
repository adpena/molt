"""Class annotation namespaces, descriptor binding, and evaluator lifetime."""
import sys
import weakref


def outcome(label, thunk):
    try:
        print(label, thunk())
    except Exception as error:
        print(label, type(error).__name__, str(error))


events = []


class StoredDescriptor:
    def __init__(self, label, result):
        self.label = label
        self.result = result

    def __get__(self, instance, owner):
        events.append((self.label, instance is None, owner.__name__))
        return self.result


explicit_annotations = {"explicit": int}


class ExplicitAnnotations:
    __annotations__ = StoredDescriptor("annotations", explicit_annotations)


first_annotations = ExplicitAnnotations.__annotations__
second_annotations = ExplicitAnnotations.__annotations__
print(
    "descriptor-annotations",
    first_annotations is explicit_annotations,
    second_annotations is explicit_annotations,
    events,
)
events.clear()


def stored_descriptor_contract(attribute):
    for replace, fail in ((False, False), (False, True), (True, False), (True, True)):
        observed = []
        result = []  # Stored descriptors may return a non-dictionary unchanged.
        replacement = {"replacement": int}
        failure = ValueError("stored annotation descriptor sentinel")

        class Probe:
            def __get__(self, instance, owner):
                observed.append(("get", instance is None, owner is Owner))
                if replace:
                    # On 3.14 this also removes an explicit __annotate__ entry;
                    # assigning __annotate__ itself would leave that entry intact.
                    owner.__annotations__ = replacement
                    observed.append(("retained", finalizer.alive))
                if fail:
                    raise failure
                return result

        descriptor = Probe()
        if attribute == "__annotations__":
            class Owner:
                __annotations__ = descriptor
        else:
            class Owner:
                __annotate__ = descriptor

        finalizer = weakref.finalize(descriptor, observed.append, ("finalized",))
        del descriptor  # The class namespace now owns the only descriptor edge.
        for read in range(1 if replace else 2):
            try:
                value = getattr(Owner, attribute)
            except Exception as error:
                observed.append(("error", error is failure))
                # The exception's traceback otherwise owns the __get__ frame/self.
                error.__traceback__ = None
            else:
                observed.append(("result", value is result))
        if replace:
            expected = replacement if attribute == "__annotations__" else None
            observed.append(("replacement", getattr(Owner, attribute) is expected))
        else:
            observed.append(("still-owned", finalizer.alive))
            Owner.__annotations__ = replacement
        print("stored-descriptor", attribute, replace, fail, observed, finalizer.alive)


stored_descriptor_contract("__annotations__")


class Empty:
    pass


outcome("missing-delete", lambda: delattr(Empty, "__annotations__"))
print("empty", Empty.__annotations__, Empty.__annotations__ is Empty.__annotations__)
assigned_annotations = {"assigned": str}
Empty.__annotations__ = assigned_annotations
print("assigned", Empty.__annotations__, Empty.__annotations__ is assigned_annotations)
del Empty.__annotations__
print("regenerated-empty", Empty.__annotations__)
for builtin in (object, int, type):
    print("builtin-annotations", builtin.__name__, hasattr(builtin, "__annotations__"))

if sys.version_info >= (3, 14):
    stored_descriptor_contract("__annotate__")

    class Generated:
        marker = int
        value: marker

    print("generated-evaluator", callable(Generated.__annotate__))
    print("generated-namespace", "__annotate_func__" in Generated.__dict__, "__annotate__" in Generated.__dict__)
    print("generated", Generated.__annotations__)
    print("cache-namespace", "__annotations_cache__" in Generated.__dict__, "__annotations__" in Generated.__dict__)
    print("cache-dir", "__annotations_cache__" in dir(Generated))
    print("empty-annotate", Empty.__annotate__)
    outcome("invalid-annotate", lambda: setattr(Empty, "__annotate__", 42))
    outcome("delete-annotate", lambda: delattr(Empty, "__annotate__"))

    def evaluate(format):
        events.append(("evaluate", format))
        return {"evaluated": int}

    class AnnotateDescriptor:
        __annotate__ = StoredDescriptor("annotate", evaluate)

    print("annotate-descriptor", AnnotateDescriptor.__annotations__, events)
    events.clear()

    class ExplicitWins:
        __annotate__ = 42
        annotated: str

    print("explicit-noncallable", ExplicitWins.__annotate__, ExplicitWins.__annotations__)
    print("explicit-generated-coexist", "__annotate_func__" in ExplicitWins.__dict__)
    ExplicitWins.__annotate__ = evaluate
    print("explicit-still-wins", ExplicitWins.__annotate__, ExplicitWins.__annotations__)
    ExplicitWins.__annotations__ = {"assigned": bool}
    print("annotations-remove-evaluators", ExplicitWins.__annotations__, "__annotate__" in ExplicitWins.__dict__, "__annotate_func__" in ExplicitWins.__dict__)

    class ExplicitCache:
        __annotations__ = {"explicit": str}
        ignored: bool

    print("explicit-keeps-generated", "__annotate_func__" in ExplicitCache.__dict__)
    ExplicitCache.__annotate__ = evaluate
    print("annotate-preserves-explicit", ExplicitCache.__annotations__, events)
    Generated.__annotate__ = evaluate
    print("annotate-clears-cache", "__annotations_cache__" in Generated.__dict__, Generated.__annotations__, events)
    events.clear()
    Generated.__annotate__ = None
    print("none-preserves-cache", Generated.__annotations__, events)

    class SuppliedInternal:
        __annotate_func__ = evaluate
        __annotations_cache__ = {"supplied": str}

    print("user-internal-values", SuppliedInternal.__annotate__ is evaluate, SuppliedInternal.__annotations__)

    class Meta(type):
        @property
        def __annotate__(cls):
            events.append(("metaclass", cls.__name__))
            return evaluate

    class Overridden(metaclass=Meta):
        pass

    print("metaclass-evaluator", Overridden.__annotations__, events)
    events.clear()

    class SelfReplacing:
        pass

    class Evaluator:
        def __init__(self, fail):
            self.fail = fail

        def __call__(self, format):
            events.append(("enter", format))
            SelfReplacing.__annotate__ = None
            events.append(("inside", "finalized" in events))
            if self.fail:
                raise ValueError("annotation callback retained")
            events.append("return")
            return {"alive": int}

    for fail in (False, True):
        ann = Evaluator(fail)
        finalizer = weakref.finalize(ann, events.append, "finalized")
        SelfReplacing.__annotate__ = ann
        del ann
        outcome("self-replacing", lambda: SelfReplacing.__annotations__)
        print("lifetime", events, finalizer.alive)
        events.clear()

    # Function annotation storage remains independent of class namespace keys.
    def function(value: int) -> str:
        return str(value)

    print("function", function.__annotations__, callable(function.__annotate__))

    from dataclasses import dataclass
    from typing import Protocol, TypedDict, runtime_checkable

    def empty_evaluator(format):
        events.append("dataclass-evaluate")
        return {}

    class EmptyData:
        pass

    EmptyData.__annotate__ = empty_evaluator
    dataclass(EmptyData)
    print("dataclass-once", events)
    events.clear()

    class Record(TypedDict):
        count: int

    print("typed-dict", sorted(Record.__required_keys__), Record.__annotations__)

    @runtime_checkable
    class HasCount(Protocol):
        count: int

    class Counted:
        count = 1

    print("protocol", isinstance(Counted(), HasCount))
