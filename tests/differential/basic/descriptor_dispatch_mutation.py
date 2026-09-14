"""Descriptor binding preserves receiver bits, live lookup, and callback errors."""


def late_method_mutation(preferred, delete):
    events = []

    def fresh(self, other):
        events.append("fresh")
        return 41

    class Left:
        def __add__(self, other):
            events.append("left")
            if delete:
                del Right.__radd__
            else:
                Right.__radd__ = fresh
            return NotImplemented

    if preferred:

        class Right(Left):
            def __radd__(self, other):
                events.append("right")
                if delete:
                    del Left.__add__
                else:
                    Left.__add__ = fresh
                return NotImplemented
    else:

        class Right:
            def __radd__(self, other):
                events.append("stale")
                return -1

    try:
        result = Left() + Right()
    except TypeError:
        result = "TypeError"
    print("late-method", preferred, delete, result, events)


def same_type_reflected():
    events = []

    class Value:
        def __add__(self, other):
            events.append("forward")
            return NotImplemented

        def __radd__(self, other):
            events.append("reflected")
            return -1

    try:
        Value() + Value()
    except TypeError:
        print("same-type", "TypeError", events)


def inherited_reflected_priority():
    events = []

    class Left:
        def __add__(self, other):
            events.append("forward")
            return 45

        def __radd__(self, other):
            events.append("inherited-reflected")
            return -1

    class Right(Left):
        pass

    print("inherited-priority", Left() + Right(), events)


def late_receiver_class_mutation():
    events = []

    class Updated:
        def __radd__(self, other):
            events.append("updated")
            return 42

    class Original:
        def __radd__(self, other):
            events.append("stale")
            return -1

    class Left:
        def __add__(self, other):
            events.append("left")
            other.__class__ = Updated
            return NotImplemented

    print("late-class", Left() + Original(), events)


def descriptor_callback_lifetime():
    events = []

    def replacement(self, instance, owner):
        events.append("replacement")
        return 44

    class Descriptor:
        def __get__(self, instance, owner):
            events.append("initial")
            Descriptor.__get__ = replacement
            return 43

    class Owner:
        value = Descriptor()

    value = Owner()
    print("get-lifetime", value.value, value.value, events)


def descriptor_callback_error(error_type):
    events = []

    class Descriptor:
        def __get__(self, instance, owner):
            events.append("get")
            raise error_type("descriptor-error")

    class Owner:
        __add__ = Descriptor()

    try:
        Owner() + 1
    except Exception as error:
        print(
            "get-error",
            "requested=" + error_type.__name__,
            "actual=" + type(error).__name__,
            events,
        )


def operator_binding_and_body_errors():
    for error_type in (AttributeError, RuntimeError):
        for error_site in ("binding", "body"):
            if error_site == "binding":

                class RaisingMethodDescriptor:
                    def __get__(self, instance, owner):
                        raise error_type("operator binding failure")

                hook = RaisingMethodDescriptor()

                class Value:
                    __add__ = hook
                    __eq__ = hook
                    __lt__ = hook

            else:

                class Value:
                    def __add__(self, other):
                        raise error_type("operator body failure")

                    def __eq__(self, other):
                        raise error_type("operator body failure")

                    def __lt__(self, other):
                        raise error_type("operator body failure")

            left = Value()
            right = Value()
            for operator_name, operation in (
                ("+", lambda: left + right),
                ("==", lambda: left == right),
                ("<", lambda: left < right),
            ):
                try:
                    outcome = ("value", repr(operation()))
                except Exception as error:
                    outcome = ("error", type(error).__name__)
                print(
                    "operator-error",
                    operator_name,
                    error_site,
                    "requested=" + error_type.__name__,
                    outcome,
                )


def decorated_descriptor_hooks(kind):
    events = []

    if kind == "static":

        class Descriptor:
            @staticmethod
            def __get__(*args):
                events.append(("get", len(args)))
                return "static-get"

            @staticmethod
            def __set__(*args):
                events.append(("set", len(args)))

            @staticmethod
            def __delete__(*args):
                events.append(("delete", len(args)))

    else:

        class Descriptor:
            @classmethod
            def __get__(cls, *args):
                events.append(("get", cls.__name__, len(args)))
                return "class-get"

            @classmethod
            def __set__(cls, *args):
                events.append(("set", cls.__name__, len(args)))

            @classmethod
            def __delete__(cls, *args):
                events.append(("delete", cls.__name__, len(args)))

    class Owner:
        value = Descriptor()

    owner = Owner()
    try:
        get_result = owner.value
    except Exception as error:
        get_result = (type(error).__name__, str(error))
    print("decorated-get", kind, get_result)
    owner.value = 5
    del owner.value
    print("decorated-events", kind, events)


def standalone_staticmethod_calls():
    events = []

    def call0():
        events.append(("call0",))
        return 200

    def call1(first):
        events.append(("call1", first))
        return 201

    def call2(first, second):
        events.append(("call2", first, second))
        return 202

    def call3(first, second, third):
        events.append(("call3", first, second, third))
        return 203

    def defaulted(first, second=72):
        events.append(("defaulted", first, second))
        return first + second

    def variadic(*args, **kwargs):
        normalized = tuple(sorted(kwargs.items()))
        events.append(("variadic", args, normalized))
        return (args, normalized)

    wrappers = (
        staticmethod(call0),
        staticmethod(call1),
        staticmethod(call2),
        staticmethod(call3),
    )
    class_wrapper = classmethod(call0)
    noncallable_wrapper = staticmethod(9)
    nested = staticmethod(staticmethod(call1))
    wrapped_classmethod = staticmethod(class_wrapper)
    print(
        "standalone-callable",
        tuple(callable(wrapper) for wrapper in wrappers),
        callable(noncallable_wrapper),
        callable(class_wrapper),
        callable(nested),
        callable(wrapped_classmethod),
    )
    print(
        "standalone-positional",
        wrappers[0](),
        wrappers[1](11),
        wrappers[2](21, 22),
        wrappers[3](31, 32, 33),
    )
    print(
        "standalone-builder",
        wrappers[2](first=41, second=42),
        wrappers[2](43, second=44),
        staticmethod(defaulted)(71),
        staticmethod(variadic)(81, 82, tail=83),
    )
    print("standalone-nested", nested(51))

    def error_name(action):
        try:
            action()
        except Exception as error:
            return type(error).__name__
        return "ok"

    print(
        "standalone-errors",
        error_name(noncallable_wrapper),
        error_name(class_wrapper),
        error_name(wrapped_classmethod),
    )

    holder = {}

    def rebind_owner(value):
        events.append(("owner-rebind", value))
        holder["wrapped"] = None
        return 204

    holder["wrapped"] = staticmethod(rebind_owner)
    print("standalone-lifetime", holder["wrapped"](61), holder["wrapped"] is None)
    print("standalone-events", events)


def descriptor_hook_values():
    events = []

    def hook(label, result=None):
        def invoke(*args):
            events.append((label, "call", len(args)))
            return result

        return invoke

    class PropertyHooks:
        @property
        def __get__(self):
            events.append(("property-get", "bind"))
            return hook("property-get", "property-value")

        @property
        def __set__(self):
            events.append(("property-set", "bind"))
            return hook("property-set")

        @property
        def __delete__(self):
            events.append(("property-delete", "bind"))
            return hook("property-delete")

    class HookValue:
        def __init__(self, label, result=None):
            self.label = label
            self.result = result

        def __get__(self, instance, owner):
            events.append((self.label, "bind", type(instance).__name__, owner.__name__))
            return hook(self.label, self.result)

    class CustomHooks:
        __get__ = HookValue("custom-get", "custom-value")
        __set__ = HookValue("custom-set")
        __delete__ = HookValue("custom-delete")

    for label, descriptor in (
        ("property", PropertyHooks()),
        ("custom", CustomHooks()),
    ):

        class Owner:
            value = descriptor

        owner = Owner()
        try:
            value = owner.value
        except Exception as error:
            value = (type(error).__name__, str(error))
        try:
            owner.value = 7
            set_result = "ok"
        except Exception as error:
            set_result = (type(error).__name__, str(error))
        try:
            del owner.value
            delete_result = "ok"
        except Exception as error:
            delete_result = (type(error).__name__, str(error))
        print("hook-values", label, value, set_result, delete_result)
    print("hook-value-events", events)


def class_object_descriptor_hooks():
    events = []

    class DescriptorMeta(type):
        def __get__(cls, instance, owner):
            events.append(("meta-get", cls.__name__, owner.__name__))
            return "metaclass-value"

        def __set__(cls, instance, value):
            events.append(("meta-set", cls.__name__, value))

        def __delete__(cls, instance):
            events.append(("meta-delete", cls.__name__))

    class ClassDescriptor(metaclass=DescriptorMeta):
        pass

    class Owner:
        value = ClassDescriptor

    owner = Owner()
    print("metaclass-get", owner.value)
    owner.value = 11
    del owner.value
    print("metaclass-events", events)

    class ClassAccessValue:
        def __get__(self, instance, owner):
            events.append(("class-access", instance is None, owner.__name__))
            return "class-access-value"

    class ClassAccessOwner:
        controlled = ClassAccessValue()

    print("class-access-get", ClassAccessOwner.controlled)

    misleading_events = []

    class ClassLocalOnly:
        def __get__(*args):
            misleading_events.append("wrong-get")
            return "wrong"

        def __set__(*args):
            misleading_events.append("wrong-set")

    class PlainOwner:
        value = ClassLocalOnly

    plain = PlainOwner()
    first = plain.value is ClassLocalOnly
    plain.value = "instance-value"
    print("class-local-ignored", first, plain.value, misleading_events)

    managed_events = []

    class ManagedValue:
        def __get__(self, instance, owner):
            managed_events.append(("get", instance.__name__, owner.__name__))
            return "metaclass-descriptor"

        def __set__(self, instance, value):
            managed_events.append(("set", instance.__name__, value))

        def __delete__(self, instance):
            managed_events.append(("delete", instance.__name__))

    class ManagedMeta(type):
        controlled = ManagedValue()

    class Managed(metaclass=ManagedMeta):
        controlled = "class-local-shadow"

    print("class-metadata-get", Managed.controlled)
    Managed.controlled = 13
    del Managed.controlled
    print("class-metadata-events", managed_events)

    getattribute_events = []

    class GetattributeMeta(type):
        def __getattribute__(cls, name):
            if name == "controlled":
                getattribute_events.append(("override", name))
                return "metaclass-getattribute"
            if name in ("__getattribute__", "__getattr__"):
                getattribute_events.append(("literal", name))
                return "metaclass-literal:" + name
            return super().__getattribute__(name)

    class GetattributeManaged(metaclass=GetattributeMeta):
        controlled = "class-local-shadow"

    print("class-getattribute", GetattributeManaged.controlled, getattribute_events)
    print(
        "class-getattribute-literal",
        GetattributeManaged.__getattribute__,
        GetattributeManaged.__getattr__,
        getattribute_events,
    )


def getattr_snapshot_and_literal_names():
    events = []

    def fresh(self, name):
        events.append(("fresh", name))
        return "fresh-value"

    class Snapshot:
        def __getattribute__(self, name):
            events.append(("primary", name))
            Snapshot.__getattr__ = fresh
            raise AttributeError("primary miss")

        def __getattr__(self, name):
            events.append(("captured", name))
            return "captured-value"

    print("getattr-snapshot", Snapshot().missing, events)

    literal_events = []

    class Literal:
        def __getattribute__(self, name):
            if name in ("__getattribute__", "__getattr__"):
                literal_events.append(name)
                return "instance-literal:" + name
            return object.__getattribute__(self, name)

        def __getattr__(self, name):
            return "fallback:" + name

    literal = Literal()
    print(
        "getattribute-literal",
        literal.__getattribute__,
        literal.__getattr__,
        literal_events,
    )


def call_descriptor_binding_errors():
    for descriptor_kind in ("property", "custom"):
        for error_type in (AttributeError, RuntimeError):
            if descriptor_kind == "property":

                def bind_failure(self):
                    raise error_type("property special-method bind failure")

                binding_descriptor = property(bind_failure)

            else:

                class RaisingBindingDescriptor:
                    def __get__(self, instance, owner):
                        raise error_type("descriptor special-method bind failure")

                binding_descriptor = RaisingBindingDescriptor()

            class CallableValue:
                __call__ = binding_descriptor

            class SetattrValue:
                __setattr__ = binding_descriptor
                target = 88

            class DelattrValue:
                __delattr__ = binding_descriptor

            callable_value = CallableValue()
            outcomes = []
            for lane, action in (
                ("direct", lambda: callable_value()),
                ("builder", lambda: callable_value(probe=1)),
            ):
                try:
                    action()
                    outcome = "ok"
                except Exception as error:
                    outcome = (type(error).__name__, str(error))
                outcomes.append((lane, outcome))

            setattr_value = SetattrValue()
            try:
                setattr_value.target = 99
                setattr_outcome = "ok"
            except Exception as error:
                setattr_outcome = (type(error).__name__, str(error))

            delattr_value = DelattrValue()
            delattr_value.target = 77
            try:
                del delattr_value.target
                delattr_outcome = "ok"
            except Exception as error:
                delattr_outcome = (type(error).__name__, str(error))
            print(
                "call-descriptor-error",
                descriptor_kind,
                error_type.__name__,
                callable(callable_value),
                outcomes,
                ("setattr", setattr_outcome, setattr_value.target),
                ("delattr", delattr_outcome, delattr_value.target),
            )


def descriptor_precedence_and_missing_hooks():
    events = []

    class Data:
        def __get__(self, instance, owner):
            events.append("data-get")
            return "descriptor-value"

        def __set__(self, instance, value):
            events.append(("data-set", value))

    class SetOnly:
        def __get__(self, instance, owner):
            return "set-only"

        def __set__(self, instance, value):
            events.append(("set-only", value))

    class DeleteOnly:
        def __get__(self, instance, owner):
            return "delete-only"

        def __delete__(self, instance):
            events.append("delete-only")

    class Errors:
        def __get__(self, instance, owner):
            raise RuntimeError("get-failure")

        def __set__(self, instance, value):
            raise RuntimeError("set-failure")

        def __delete__(self, instance):
            raise RuntimeError("delete-failure")

    class Owner:
        data = Data()
        set_only = SetOnly()
        delete_only = DeleteOnly()
        errors = Errors()

    owner = Owner()
    owner.__dict__["data"] = "instance-shadow"
    print("data-precedence", owner.data, events)
    for label, action in (
        ("missing-delete", lambda: delattr(owner, "set_only")),
        ("missing-set", lambda: setattr(owner, "delete_only", 1)),
        ("error-get", lambda: owner.errors),
        ("error-set", lambda: setattr(owner, "errors", 2)),
        ("error-delete", lambda: delattr(owner, "errors")),
    ):
        try:
            action()
            outcome = "ok"
        except Exception as error:
            outcome = (
                type(error).__name__,
                str(error),
                getattr(error, "name", None),
                getattr(error, "obj", None) is None,
            )
        print("descriptor-outcome", label, outcome)


def self_mutating_descriptor_hooks():
    events = []

    def replacement_set(self, instance, value):
        events.append(("new-set", value))

    def replacement_delete(self, instance):
        events.append("new-delete")

    class Descriptor:
        def __set__(self, instance, value):
            events.append(("old-set", value))
            Descriptor.__set__ = replacement_set

        def __delete__(self, instance):
            events.append("old-delete")
            Descriptor.__delete__ = replacement_delete

    class Owner:
        value = Descriptor()

    owner = Owner()
    owner.value = 1
    owner.value = 2
    del owner.value
    del owner.value
    print("mutation-lifetime", events)


for preferred in (False, True):
    for delete in (False, True):
        late_method_mutation(preferred, delete)
late_receiver_class_mutation()
inherited_reflected_priority()
same_type_reflected()
descriptor_callback_lifetime()
descriptor_callback_error(RuntimeError)
descriptor_callback_error(AttributeError)
operator_binding_and_body_errors()
decorated_descriptor_hooks("static")
decorated_descriptor_hooks("class")
standalone_staticmethod_calls()
descriptor_hook_values()
class_object_descriptor_hooks()
getattr_snapshot_and_literal_names()
call_descriptor_binding_errors()
descriptor_precedence_and_missing_hooks()
self_mutating_descriptor_hooks()
