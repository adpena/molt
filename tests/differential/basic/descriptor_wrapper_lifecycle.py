"""Descriptor wrapper allocation, reinitialization, metadata, and cloning."""

import sys
import weakref


VERSION = sys.version_info[:2]
print("target-version", VERSION)


def expect_error(label, error_types, action):
    try:
        action()
    except error_types as error:
        print(label, "error", type(error).__name__)
    else:
        print(label, "no-error")


def static_first(value=0, *, scale=1):
    """static-first-doc"""
    return ("static-first", value, scale)


def static_second(value=0, *, scale=1):
    """static-second-doc"""
    return ("static-second", value, scale)


def class_first(cls, value=0):
    """class-first-doc"""
    return ("class-first", cls.__name__, value)


def class_second(cls, value=0):
    """class-second-doc"""
    return ("class-second", cls.__name__, value)


for function, label in (
    (static_first, "static-first"),
    (static_second, "static-second"),
    (class_first, "class-first"),
    (class_second, "class-second"),
):
    function.__module__ = "descriptor_wrapper_lifecycle"
    function.__qualname__ = label + "-qualname"
    function.__annotations__ = {"value": label}
    function.payload_tag = label


class StaticChild(staticmethod):
    pass


class StaticGrandchild(StaticChild):
    pass


class ClassChild(classmethod):
    pass


class ClassGrandchild(ClassChild):
    pass


class PropertyChild(property):
    pass


class PropertyGrandchild(PropertyChild):
    pass


def allocation_matrix():
    for label, base, child, grandchild, payload in (
        ("static", staticmethod, StaticChild, StaticGrandchild, static_first),
        ("class", classmethod, ClassChild, ClassGrandchild, class_first),
        ("property", property, PropertyChild, PropertyGrandchild, static_first),
    ):
        exact = base(payload)
        direct_subclass = child(payload)
        inherited_subclass = grandchild(payload)
        raw_exact = base.__new__(base, "ignored", ignored=True)
        raw_direct = child.__new__(child, "ignored", ignored=True)
        raw_inherited = grandchild.__new__(grandchild, "ignored", ignored=True)
        print(
            "allocation",
            label,
            type(exact).__name__,
            type(direct_subclass).__name__,
            type(inherited_subclass).__name__,
            type(raw_exact).__name__,
            type(raw_direct).__name__,
            type(raw_inherited).__name__,
        )


def uninitialized_objects():
    class Owner:
        pass

    raw_static = staticmethod.__new__(staticmethod)
    print(
        "uninitialized-static-members",
        raw_static.__func__,
        raw_static.__wrapped__,
        raw_static.__dict__,
    )
    expect_error(
        "uninitialized-static-get-class",
        RuntimeError,
        lambda: raw_static.__get__(None, Owner),
    )
    expect_error(
        "uninitialized-static-get-instance",
        RuntimeError,
        lambda: raw_static.__get__(Owner(), Owner),
    )
    # CPython 3.12.13 dereferences the null payload and crashes here. This is
    # deliberately outside the safe in-process differential oracle.
    print("uninitialized-static-call", "skipped-upstream-crash")

    raw_class = classmethod.__new__(classmethod)
    print(
        "uninitialized-class-members",
        raw_class.__func__,
        raw_class.__wrapped__,
        raw_class.__dict__,
        callable(raw_class),
    )
    expect_error(
        "uninitialized-class-get-class",
        RuntimeError,
        lambda: raw_class.__get__(None, Owner),
    )
    expect_error(
        "uninitialized-class-get-instance",
        RuntimeError,
        lambda: raw_class.__get__(Owner(), Owner),
    )

    raw_property = property.__new__(property)
    print(
        "uninitialized-property-members",
        raw_property.fget,
        raw_property.fset,
        raw_property.fdel,
        raw_property.__doc__,
        raw_property.__get__(None, Owner) is raw_property,
    )
    expect_error(
        "uninitialized-property-get",
        AttributeError,
        lambda: raw_property.__get__(Owner(), Owner),
    )
    expect_error(
        "uninitialized-property-set",
        AttributeError,
        lambda: raw_property.__set__(Owner(), 1),
    )
    expect_error(
        "uninitialized-property-delete",
        AttributeError,
        lambda: raw_property.__delete__(Owner()),
    )


def wrapper_init_arguments():
    for label, wrapper_type, payload in (
        ("static", staticmethod, static_first),
        ("class", classmethod, class_first),
    ):
        expect_error(
            label + "-init-missing",
            TypeError,
            lambda wrapper_type=wrapper_type: wrapper_type.__init__(
                wrapper_type.__new__(wrapper_type)
            ),
        )
        expect_error(
            label + "-init-extra",
            TypeError,
            lambda wrapper_type=wrapper_type, payload=payload: wrapper_type.__init__(
                wrapper_type.__new__(wrapper_type), payload, payload
            ),
        )
        expect_error(
            label + "-init-keyword",
            TypeError,
            lambda wrapper_type=wrapper_type, payload=payload: wrapper_type.__init__(
                wrapper_type.__new__(wrapper_type), callable=payload
            ),
        )
        expect_error(
            label + "-constructor-keyword",
            TypeError,
            lambda wrapper_type=wrapper_type, payload=payload: wrapper_type(
                callable=payload
            ),
        )

    property_args = (static_first, static_second, class_first, "positional-doc")
    for count in range(5):
        positional = property.__new__(property)
        result = property.__init__(positional, *property_args[:count])
        print(
            "property-init-positional",
            count,
            result,
            positional.fget is static_first,
            positional.fset is static_second,
            positional.fdel is class_first,
            positional.__doc__,
        )
    keyword = property.__new__(property)
    result = property.__init__(
        keyword,
        fget=static_first,
        fset=None,
        fdel=None,
        doc="keyword-doc",
    )
    print(
        "property-init-keyword",
        result,
        keyword.fget is static_first,
        keyword.fset,
        keyword.fdel,
        keyword.__doc__,
    )
    expect_error(
        "property-init-extra",
        TypeError,
        lambda: property.__init__(
            property.__new__(property), None, None, None, None, None
        ),
    )
    expect_error(
        "property-init-unknown-keyword",
        TypeError,
        lambda: property.__init__(property.__new__(property), unknown=True),
    )

    reset = property(static_first, doc="explicit-doc")
    property.__init__(reset)
    print(
        "property-reinit-empty",
        reset.fget,
        reset.fset,
        reset.fdel,
        reset.__doc__,
    )


def wrapper_metadata_and_replacement():
    for label, wrapper_type, first, second in (
        ("static", staticmethod, static_first, static_second),
        ("class", classmethod, class_first, class_second),
    ):
        wrapper = wrapper_type(first)
        print(
            "metadata-before",
            label,
            wrapper.__func__ is first,
            wrapper.__wrapped__ is first,
            wrapper.__name__,
            wrapper.__qualname__,
            wrapper.__doc__,
            wrapper.__module__,
            wrapper.__annotations__,
            sorted(wrapper.__dict__),
        )
        wrapper.local_marker = label + "-local"
        wrapper.__dict__["__func__"] = "shadow-func"
        wrapper.__dict__["__wrapped__"] = "shadow-wrapped"
        print(
            "metadata-data-precedence",
            label,
            wrapper.__func__ is first,
            wrapper.__wrapped__ is first,
            wrapper.__dict__["__func__"],
            wrapper.__dict__["__wrapped__"],
        )
        expect_error(
            "metadata-readonly-func-" + label,
            AttributeError,
            lambda wrapper=wrapper: setattr(wrapper, "__func__", second),
        )
        expect_error(
            "metadata-readonly-wrapped-" + label,
            AttributeError,
            lambda wrapper=wrapper: setattr(wrapper, "__wrapped__", second),
        )
        result = wrapper_type.__init__(wrapper, second)
        print(
            "metadata-after",
            label,
            result,
            wrapper.__func__ is second,
            wrapper.__wrapped__ is second,
            wrapper.__name__,
            wrapper.__qualname__,
            wrapper.__doc__,
            wrapper.__module__,
            wrapper.__annotations__,
            wrapper.__dict__.get("payload_tag"),
            wrapper.local_marker,
            sorted(wrapper.__dict__),
        )


def explicit_binding_and_calls():
    class Host:
        pass

    instance = Host()
    static = staticmethod(static_first)
    static_from_class = static.__get__(None, Host)
    static_from_instance = static.__get__(instance, Host)
    print(
        "explicit-static",
        static_from_class is static_first,
        static_from_instance is static_first,
        static(7, scale=3),
        static_from_class(8, scale=4),
    )

    classed = classmethod(class_first)
    class_from_class = classed.__get__(None, Host)
    class_from_instance = classed.__get__(instance, Host)
    print(
        "explicit-class",
        class_from_class.__func__ is class_first,
        class_from_class.__self__ is Host,
        class_from_instance.__self__ is Host,
        class_from_class(9),
        class_from_instance(10),
        callable(classed),
    )
    expect_error("explicit-class-direct-call", TypeError, lambda: classed())

    noncallable = staticmethod(17)
    print(
        "static-noncallable",
        callable(noncallable),
        noncallable.__get__(None, Host),
        callable(noncallable.__get__(instance, Host)),
    )
    expect_error("static-noncallable-call", TypeError, lambda: noncallable())


def explicit_argument_diagnostics():
    # Only initialized wrappers are used for protocol calls. In particular,
    # never invoke an uninitialized staticmethod, even through __call__.
    def diagnostic(label, action):
        try:
            action()
        except (TypeError, AttributeError, RuntimeError) as error:
            print("argument-diagnostic", label, type(error).__name__, str(error))
        else:
            print("argument-diagnostic", label, "no-error")

    diagnostic("property-duplicate-fget", lambda: property(static_first, fget=static_second))
    diagnostic("property-unknown-keyword", lambda: property(unknown=True))
    diagnostic("property-too-many-positional", lambda: property(None, None, None, None, None))
    diagnostic(
        "property-too-many-total",
        lambda: property(None, None, None, None, fget=static_first),
    )
    for wrapper_type in (staticmethod, classmethod, property):
        wrapper = wrapper_type(static_first)
        label = wrapper_type.__name__
        diagnostic(label + "-get-no-args", lambda wrapper=wrapper: wrapper.__get__())
        diagnostic(
            label + "-get-too-many",
            lambda wrapper=wrapper: wrapper.__get__(None, object, object),
        )
        diagnostic(
            label + "-get-keyword",
            lambda wrapper=wrapper: wrapper.__get__(None, owner=object),
        )
        diagnostic(
            label + "-new-wrong-type",
            lambda wrapper_type=wrapper_type: wrapper_type.__new__(object),
        )


def subclass_overrides():
    static_events = []

    class StaticOverride(staticmethod):
        def __get__(self, instance, owner=None):
            static_events.append(("get", instance is None, owner.__name__))
            return "static-override-get"

        def __call__(self, *args, **kwargs):
            static_events.append(("call", args, sorted(kwargs.items())))
            return "static-override-call"

    static = StaticOverride(static_first)

    class StaticHost:
        value = static

    print(
        "static-override",
        static(),
        StaticHost.value,
        StaticHost().value,
        static_events,
    )

    class_events = []

    class ClassOverride(classmethod):
        def __get__(self, instance, owner=None):
            class_events.append(("get", instance is None, owner.__name__))
            return "class-override-get"

        def __call__(self, *args, **kwargs):
            class_events.append(("call", args, sorted(kwargs.items())))
            return "class-override-call"

    classed = ClassOverride(class_first)

    class ClassHost:
        value = classed

    print(
        "class-override",
        classed(),
        ClassHost.value,
        ClassHost().value,
        class_events,
    )

    property_events = []

    class PropertyOverride(property):
        def __get__(self, instance, owner=None):
            property_events.append(("get", instance is None, owner.__name__))
            return "property-override-get"

        def __set__(self, instance, value):
            property_events.append(("set", value))

        def __delete__(self, instance):
            property_events.append(("delete",))

    overridden = PropertyOverride(lambda self: "base")

    class PropertyHost:
        value = overridden

    host = PropertyHost()
    class_value = PropertyHost.value
    instance_value = host.value
    host.value = 31
    del host.value
    print(
        "property-override",
        class_value,
        instance_value,
        property_events,
    )


def nested_and_cyclic_wrappers():
    inner = staticmethod(static_first)
    outer = staticmethod(inner)
    print(
        "nested-static",
        outer.__func__ is inner,
        outer.__wrapped__ is inner,
        outer(51, scale=2),
    )

    class StaticRelay:
        wrapper = None

        def __call__(self, depth):
            if depth == 0:
                return "static-cycle-done"
            return self.wrapper(depth - 1)

    static_relay = StaticRelay()
    static_cycle = staticmethod(static_relay)
    static_relay.wrapper = static_cycle
    print("cyclic-static", static_cycle(4))

    class ClassRelay:
        wrapper = None

        def __call__(self, owner, depth):
            if depth == 0:
                return ("class-cycle-done", owner.__name__)
            return self.wrapper.__get__(None, owner)(depth - 1)

    class CycleHost:
        pass

    class_relay = ClassRelay()
    class_cycle = classmethod(class_relay)
    class_relay.wrapper = class_cycle
    print("cyclic-class", class_cycle.__get__(None, CycleHost)(4))


def replacement_releases_old_payloads():
    class Payload:
        def __init__(self, label):
            self.label = label

        def __call__(self, *args):
            return (self.label, len(args))

    static_old = Payload("static-old")
    static_old_ref = weakref.ref(static_old)
    static = staticmethod(static_old)
    del static_old
    print("release-static-retained", static_old_ref() is not None)
    staticmethod.__init__(static, static_second)
    print("release-static-replaced", static_old_ref() is None)

    class_old = Payload("class-old")
    class_old_ref = weakref.ref(class_old)
    classed = classmethod(class_old)
    del class_old
    print("release-class-retained", class_old_ref() is not None)
    classmethod.__init__(classed, class_second)
    print("release-class-replaced", class_old_ref() is None)

    old_get = Payload("property-get-old")
    old_set = Payload("property-set-old")
    old_delete = Payload("property-delete-old")
    old_refs = tuple(weakref.ref(value) for value in (old_get, old_set, old_delete))
    prop = property(old_get, old_set, old_delete, "old-doc")
    del old_get, old_set, old_delete
    print("release-property-retained", tuple(ref() is not None for ref in old_refs))
    property.__init__(prop, static_first, static_second, class_first, "new-doc")
    print(
        "release-property-replaced",
        tuple(ref() is None for ref in old_refs),
        prop.fget is static_first,
        prop.fset is static_second,
        prop.fdel is class_first,
        prop.__doc__,
    )


def property_protocol_and_cloning():
    def get_a(instance):
        """get-a-doc"""
        return instance.stored

    def get_b(instance):
        """get-b-doc"""
        return ("get-b", instance.stored)

    def set_a(instance, value):
        instance.stored = ("set-a", value)

    def set_b(instance, value):
        instance.stored = ("set-b", value)

    def delete_a(instance):
        instance.stored = "delete-a"

    def delete_b(instance):
        instance.stored = "delete-b"

    prop = property(get_a, set_a, delete_a, "explicit-property-doc")

    class Host:
        value = prop

        def __init__(self):
            self.stored = "initial"

    host = Host()
    print(
        "property-explicit-get",
        prop.__get__(None, Host) is prop,
        prop.__get__(host, Host),
    )
    print("property-explicit-set", prop.__set__(host, 61), host.stored)
    print("property-explicit-delete", prop.__delete__(host), host.stored)

    clone_events = []

    def accessor_name(accessor):
        return None if accessor is None else accessor.__name__

    class TrackingProperty(property):
        def __init__(self, fget=None, fset=None, fdel=None, doc=None):
            clone_events.append(
                (
                    accessor_name(fget),
                    accessor_name(fset),
                    accessor_name(fdel),
                    doc,
                )
            )
            super().__init__(fget, fset, fdel, doc)

    tracked = TrackingProperty(get_a, set_a, delete_a, "tracked-explicit-doc")
    clone_events.clear()
    getter_clone = tracked.getter(get_b)
    print(
        "property-clone-getter",
        type(getter_clone).__name__,
        clone_events,
        getter_clone.fget is get_b,
        getter_clone.fset is set_a,
        getter_clone.fdel is delete_a,
        getter_clone.__doc__,
    )
    clone_events.clear()
    setter_clone = tracked.setter(set_b)
    print(
        "property-clone-setter",
        type(setter_clone).__name__,
        clone_events,
        setter_clone.fget is get_a,
        setter_clone.fset is set_b,
        setter_clone.fdel is delete_a,
        setter_clone.__doc__,
    )
    clone_events.clear()
    deleter_clone = tracked.deleter(delete_b)
    print(
        "property-clone-deleter",
        type(deleter_clone).__name__,
        clone_events,
        deleter_clone.fget is get_a,
        deleter_clone.fset is set_a,
        deleter_clone.fdel is delete_b,
        deleter_clone.__doc__,
    )
    clone_events.clear()
    none_getter_clone = tracked.getter(None)
    print(
        "property-clone-getter-none",
        type(none_getter_clone).__name__,
        clone_events,
        none_getter_clone.fget is get_a,
        none_getter_clone.fset is set_a,
        none_getter_clone.fdel is delete_a,
        none_getter_clone.__doc__,
    )

    exact_none_getter = prop.getter(None)
    print(
        "property-exact-getter-none",
        type(exact_none_getter).__name__,
        exact_none_getter.fget is get_a,
        exact_none_getter.fset is set_a,
        exact_none_getter.fdel is delete_a,
        exact_none_getter.__doc__,
    )


def versioned_descriptor_metadata_and_chaining():
    named = property(static_first)
    if VERSION >= (3, 13):
        print("property-name", "available", named.__name__)
        named.__name__ = "renamed-property"
        print("property-name-write", named.__name__)
    else:
        print("property-name", "unavailable", hasattr(named, "__name__"))

    class ChainedHost:
        chained_property = classmethod(
            property(lambda cls: ("chained-property", cls.__name__))
        )
        chained_static = classmethod(staticmethod(lambda: "chained-static"))

    if VERSION < (3, 13):
        print(
            "classmethod-descriptor-chain",
            "legacy",
            ChainedHost.chained_property,
            ChainedHost.chained_static(),
        )
    else:
        property_bound = ChainedHost.chained_property
        static_bound = ChainedHost.chained_static
        print(
            "classmethod-descriptor-chain",
            "removed",
            type(property_bound).__name__,
            callable(property_bound),
            type(static_bound).__name__,
            callable(static_bound),
        )
        expect_error(
            "classmethod-property-chain-call",
            TypeError,
            lambda: property_bound(),
        )
        expect_error(
            "classmethod-static-chain-call",
            TypeError,
            lambda: static_bound(),
        )


allocation_matrix()
uninitialized_objects()
wrapper_init_arguments()
wrapper_metadata_and_replacement()
explicit_binding_and_calls()
explicit_argument_diagnostics()
subclass_overrides()
nested_and_cyclic_wrappers()
replacement_releases_old_payloads()
property_protocol_and_cloning()
versioned_descriptor_metadata_and_chaining()
