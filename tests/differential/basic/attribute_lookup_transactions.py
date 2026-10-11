"""Attribute lookups retain their semantic selection across callback mutation."""



def metaclass_snapshot():
    events = []

    class Old:
        def __get__(self, obj, owner):
            events.append("old_get")
            return "original"

    class Replacement:
        def __get__(self, obj, owner):
            events.append("replacement_get")
            return "replacement"

    class Meta(type):
        pass

    replacement = Replacement()
    Meta.x = Old()

    class Key:
        def __hash__(self):
            return hash("x")

        def __eq__(self, other):
            if isinstance(other, str) and other == "x":
                events.append("class_eq")
                type.__setattr__(Meta, "x", replacement)
            return False

    subject = Meta("Subject", (), {Key(): None})
    events.clear()
    result = type.__getattribute__(subject, "x")
    assert result == "original", (result, events)
    assert events[-1:] == ["old_get"] and events[:-1], events
    assert all(event == "class_eq" for event in events[:-1]), events
    assert Meta.__dict__["x"] is replacement
    return result, events


def getattribute_retirement():
    events = []

    def fail(name):
        events.append("call")
        raise AttributeError(name)

    class Gate:
        def __get__(self, obj, owner):
            events.append("bind")
            type.__delattr__(owner, "__getattribute__")
            return fail

        def __del__(self):
            events.append("retire_getattribute")

    class Subject:
        __getattribute__ = Gate()

        def __getattr__(self, name):
            events.append("fallback")
            return "fallback-result"

    result = Subject().missing
    assert result == "fallback-result", (result, events)
    assert events == ["bind", "call", "retire_getattribute", "fallback"], events
    return result, events


def method_snapshot():
    results = []
    for warm in (False, True):
        events = []
        armed = False

        class Subject:
            def method(self):
                events.append("old_method")
                return "original"

        class Key:
            def __hash__(self):
                return hash("method")

            def __eq__(self, other):
                nonlocal armed
                if armed and other == "method":
                    armed = False
                    events.append("instance_eq")
                    type.__delattr__(Subject, "method")
                return False

        subject = Subject()
        subject.__dict__[Key()] = None

        def invoke():
            return subject.method()

        if warm:
            for _ in range(100):
                assert invoke() == "original"
        events.clear()
        armed = True
        result = invoke()
        assert result == "original", (warm, result, events)
        assert events == ["instance_eq", "old_method"], (warm, events)
        try:
            invoke()
        except AttributeError:
            pass
        else:
            raise AssertionError("next lookup must observe the removed method")
        results.append((warm, result, events.copy()))
    return results


def mro_error_policy():
    class Key:
        def __init__(self, armed=True):
            self.armed = armed

        def __hash__(self):
            return hash("x")

        def __eq__(self, other):
            if self.armed:
                raise RuntimeError("class namespace comparison")
            return False

    subject_type = type("Subject", (), {Key(): None})
    try:
        type.__getattribute__(subject_type, "x")
    except AttributeError:
        pass
    else:
        raise AssertionError("failed type lookup must be reported as absence")
    subject = subject_type()
    subject.__dict__["x"] = 17
    assert object.__getattribute__(subject, "x") == 17
    subject.__dict__.clear()
    instance_key = Key(armed=False)
    subject.__dict__[instance_key] = None
    instance_key.armed = True
    try:
        object.__getattribute__(subject, "x")
    except RuntimeError as error:
        assert str(error) == "class namespace comparison"
    else:
        raise AssertionError("instance dictionary failure must propagate")
    return "type-miss", "instance-hit", "instance-error"


def subclass_name_cache():
    events = []

    class Subject:
        x = 17

    class Name(str):
        def __hash__(self):
            events.append("name_hash")
            return hash("a-different-name")

    subject = Subject()
    assert getattr(subject, "x") == 17
    try:
        getattr(subject, Name("x"))
    except AttributeError:
        pass
    else:
        raise AssertionError("same bytes must not erase subclass name hashing")
    assert events, events
    return "subclass-miss", events



def getterless_descriptor_precedence():
    class Descriptor:
        def __set__(self, obj, value):
            raise AssertionError("getter lookup must not execute setter")

    class Subject:
        x = Descriptor()

    subject = Subject()
    subject.__dict__["x"] = 17
    assert object.__getattribute__(subject, "x") == 17

    class Meta(type):
        x = Descriptor()

    class ClassSubject(metaclass=Meta):
        x = 23

    assert type.__getattribute__(ClassSubject, "x") == 23
    return 17, 23


CASES = {
    "metaclass_snapshot": metaclass_snapshot,
    "getattribute_retirement": getattribute_retirement,
    "method_snapshot": method_snapshot,
    "mro_error_policy": mro_error_policy,
    "subclass_name_cache": subclass_name_cache,
    "getterless_descriptor_precedence": getterless_descriptor_precedence,
}

def descriptor_type_precedence_changes():
    class Descriptor:
        def __get__(self, instance, owner):
            return 41

    class Subject:
        x = Descriptor()

    subject = Subject()
    subject.__dict__["x"] = 17
    assert subject.x == 17
    Descriptor.__set__ = lambda self, instance, value: None
    assert subject.x == 41
    del Descriptor.__set__
    assert subject.x == 17
    return 17, 41, 17


def custom_action_does_not_probe_counterpart():
    results = []
    for action in ("set", "delete"):
        events = []
        armed = False
        counterpart = "__delattr__" if action == "set" else "__setattr__"

        class Key:
            def __hash__(self):
                return hash(counterpart)

            def __eq__(self, other):
                if armed and other == counterpart:
                    events.append(("unneeded_probe", counterpart))
                return False

        def setter(owner, name, value):
            events.append(("set", name, value))
            type.__setattr__(owner, name, value)

        def deleter(owner, name):
            events.append(("delete", name))
            type.__delattr__(owner, name)

        namespace = {Key(): None}
        namespace["__setattr__" if action == "set" else "__delattr__"] = (
            setter if action == "set" else deleter
        )
        Meta = type("Meta", (type,), namespace)
        Subject = Meta("Subject", (), {"x": 11})
        events.clear()
        armed = True
        if action == "set":
            Subject.x = 17
            assert events == [("set", "x", 17)], events
            assert Subject.x == 17
        else:
            del Subject.x
            assert events == [("delete", "x")], events
            assert "x" not in Subject.__dict__
        results.append((action, events))
    return results



def paired_object_defaults_keep_generic_admission():
    class Meta(type):
        __setattr__ = object.__setattr__
        __delattr__ = object.__delattr__

    Subject = Meta("Subject", (), {})
    Subject.x = 17
    assert Subject.__dict__["x"] == 17
    del Subject.x
    assert "x" not in Subject.__dict__
    object.__setattr__(Subject, "x", 23)
    assert Subject.__dict__["x"] == 23
    object.__delattr__(Subject, "x")
    assert "x" not in Subject.__dict__


CASES["custom_action_does_not_probe_counterpart"] = custom_action_does_not_probe_counterpart
CASES["paired_object_defaults_keep_generic_admission"] = paired_object_defaults_keep_generic_admission


for name, case in CASES.items():
    case()
    print(name, "ok")
print("descriptor-type-precedence", descriptor_type_precedence_changes())
