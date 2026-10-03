"""Purpose: differential coverage for sys asyncgen hooks API."""

import sys


hooks = sys.get_asyncgen_hooks()
print(
    "orig",
    hooks.firstiter is None,
    hooks.finalizer is None,
    type(hooks).__name__,
)


def first(agen):
    return None


def final(agen):
    return None


ret = sys.set_asyncgen_hooks(firstiter=first, finalizer=final)
print("ret", ret)

cur = sys.get_asyncgen_hooks()
print("cur", cur.firstiter is first, cur.finalizer is final)

try:
    sys.set_asyncgen_hooks(firstiter=1, finalizer=final)
except Exception as exc:
    print("bad_first", type(exc).__name__, str(exc))

try:
    sys.set_asyncgen_hooks(firstiter=first, finalizer=1)
except Exception as exc:
    print("bad_final", type(exc).__name__, str(exc))

sys.set_asyncgen_hooks(firstiter=hooks.firstiter, finalizer=hooks.finalizer)


# CPython 3.12 commits arguments independently; 3.13+ prevalidates both
# and audits a finalizer rollback if firstiter rejects the update.
transactional = sys.version_info >= (3, 13)
events = []
reject_event = None
reject_rollback = False


def audit_hook(event, args):
    if event.startswith("sys.set_asyncgen_hook_"):
        events.append((event, args))
        if reject_rollback and event == final_event and len(events) > 1:
            raise LookupError("rollback rejected")
        if event == reject_event:
            raise RuntimeError("hook update rejected")


def other(agen):
    return None


sys.addaudithook(audit_hook)
first_event = "sys.set_asyncgen_hook_firstiter"
final_event = "sys.set_asyncgen_hook_finalizer"

try:
    sys.set_asyncgen_hooks(first, final)
    events.clear()
    sys.set_asyncgen_hooks()
    assert events == []
    assert sys.get_asyncgen_hooks() == (first, final)

    sys.set_asyncgen_hooks(None)
    assert events == [(first_event, ())]
    assert sys.get_asyncgen_hooks() == (None, final)
    events.clear()
    sys.set_asyncgen_hooks(finalizer=None)
    assert events == [(final_event, ())]
    assert sys.get_asyncgen_hooks() == (None, None)

    events.clear()
    sys.set_asyncgen_hooks(first, finalizer=final)
    assert events == [(final_event, ()), (first_event, ())]
    events.clear()
    try:
        sys.set_asyncgen_hooks(other, 1)
    except TypeError as error:
        assert str(error) == "callable finalizer expected, got int"
    else:
        raise AssertionError("invalid finalizer accepted")
    assert events == []
    assert sys.get_asyncgen_hooks() == (first, final)

    try:
        sys.set_asyncgen_hooks(1, other)
    except TypeError as error:
        assert str(error) == "callable firstiter expected, got int"
    else:
        raise AssertionError("invalid firstiter accepted")
    assert events == ([] if transactional else [(final_event, ())])
    assert sys.get_asyncgen_hooks() == (first, final if transactional else other)

    sys.set_asyncgen_hooks(first, other)
    events.clear()
    reject_event = final_event
    try:
        sys.set_asyncgen_hooks(other, final)
    except RuntimeError as error:
        assert str(error) == "hook update rejected"
    else:
        raise AssertionError("finalizer audit rejection ignored")
    assert events == [(final_event, ())]
    assert sys.get_asyncgen_hooks() == (first, other)

    events.clear()
    reject_event = first_event
    try:
        sys.set_asyncgen_hooks(other, final)
    except RuntimeError as error:
        assert str(error) == "hook update rejected"
    else:
        raise AssertionError("firstiter audit rejection ignored")
    assert events == (
        [(final_event, ()), (first_event, ()), (final_event, ())]
        if transactional
        else [(final_event, ()), (first_event, ())]
    )
    assert sys.get_asyncgen_hooks() == (first, other if transactional else final)
    reject_event = None

    # A rollback audit failure replaces the firstiter error, without treating
    # the detached raised error as handled __context__.
    sys.set_asyncgen_hooks(first, other)
    events.clear()
    reject_event = first_event
    reject_rollback = True
    try:
        sys.set_asyncgen_hooks(other, final)
    except BaseException as error:
        assert type(error) is (LookupError if transactional else RuntimeError)
        assert str(error) == (
            "rollback rejected" if transactional else "hook update rejected"
        )
        assert error.__context__ is None
    else:
        raise AssertionError("audit rejection ignored")
    assert events == (
        [(final_event, ()), (first_event, ()), (final_event, ())]
        if transactional
        else [(final_event, ()), (first_event, ())]
    )
    assert sys.get_asyncgen_hooks() == (first, final)
    reject_event = None
    reject_rollback = False

    events.clear()
    reject_event = first_event
    try:
        sys.set_asyncgen_hooks(other)
    except RuntimeError:
        pass
    else:
        raise AssertionError("firstiter audit rejection ignored")
    assert events == (
        [(first_event, ()), (final_event, ())] if transactional else [(first_event, ())]
    )
    assert sys.get_asyncgen_hooks() == (first, final)
    reject_event = None
    print("positional-omitted-audit-order", True)

    import weakref

    released = []

    class Hook:
        def __call__(self, agen):
            return None

    def finalizer_released(reference):
        current = sys.get_asyncgen_hooks()
        released.append((current.firstiter is first, current.finalizer is final))
        sys.set_asyncgen_hooks(firstiter=other)

    previous = Hook()
    reference = weakref.ref(previous, finalizer_released)
    sys.set_asyncgen_hooks(first, previous)
    del previous
    events.clear()
    sys.set_asyncgen_hooks(finalizer=final)
    assert reference() is None
    assert released == [(True, True)]
    assert sys.get_asyncgen_hooks() == (other, final)
    assert events == [(final_event, ()), (first_event, ())]
    print("omitted-hook-survives-finalizer-reentry", True)

    # Restoring a live or resurrected old finalizer is defined in every oracle.
    # No probe rolls back to a destroyed CPython borrowed pointer.
    resurrected = []
    destruction = []

    class ResurrectingHook:
        def __call__(self, agen):
            pass

        def __del__(self):
            destruction.append(sys.get_asyncgen_hooks().finalizer is final)
            resurrected.append(self)

    previous = ResurrectingHook()
    reference = weakref.ref(previous)
    sys.set_asyncgen_hooks(first, previous)
    del previous
    events.clear()
    reject_event = first_event
    try:
        sys.set_asyncgen_hooks(other, final)
    except RuntimeError:
        pass
    else:
        raise AssertionError("firstiter audit rejection ignored")
    reject_event = None
    assert destruction == [True]
    assert len(resurrected) == 1
    assert reference() is resurrected[0]
    assert sys.get_asyncgen_hooks().finalizer is (
        resurrected[0] if transactional else final
    )
    sys.set_asyncgen_hooks(first, final)
    resurrected.clear()
    assert reference() is None
    print("rollback-respects-prompt-resurrection", True)

    # An externally owned callable with no weakref slot must also roll back.
    class SlottedHook:
        __slots__ = ()

        def __call__(self, agen):
            pass

    previous = SlottedHook()
    sys.set_asyncgen_hooks(first, previous)
    events.clear()
    reject_event = first_event
    try:
        sys.set_asyncgen_hooks(other, final)
    except RuntimeError:
        pass
    else:
        raise AssertionError("firstiter audit rejection ignored")
    reject_event = None
    assert sys.get_asyncgen_hooks().finalizer is (previous if transactional else final)
    sys.set_asyncgen_hooks(first, final)
    del previous
    print("rollback-nonweakrefable-callable", True)

    live = []
    added = False

    def late_audit(event, args):
        if event in ("molt.hooks.live", "molt.hooks.nested"):
            live.append(("late", event, args))

    def live_audit(event, args):
        global added
        if event in ("molt.hooks.live", "molt.hooks.nested"):
            live.append(("first", event, args))
        if event == "molt.hooks.live" and not added:
            added = True
            sys.addaudithook(late_audit)
            sys.audit("molt.hooks.nested", 23)

    sys.addaudithook(live_audit)
    sys.audit("molt.hooks.live", 17)
    assert live == [
        ("first", "molt.hooks.live", (17,)),
        ("first", "molt.hooks.nested", (23,)),
        ("late", "molt.hooks.nested", (23,)),
        ("late", "molt.hooks.live", (17,)),
    ]
    print("audit-live-addition-and-reentry", True)

    registration_error = None
    registered = []

    def registration_gate(event, args):
        if event == "sys.addaudithook" and registration_error is not None:
            raise registration_error("registration rejected")

    def rejected_audit(event, args):
        if event == "molt.hooks.registration":
            registered.append(event)

    class RegistrationHalt(BaseException):
        pass

    sys.addaudithook(registration_gate)
    registration_error = ValueError
    assert sys.addaudithook(rejected_audit) is None
    registration_error = RegistrationHalt
    try:
        sys.addaudithook(rejected_audit)
    except RegistrationHalt as error:
        assert str(error) == "registration rejected"
    else:
        raise AssertionError("non-Exception audit failure suppressed")
    registration_error = None
    sys.audit("molt.hooks.registration")
    assert registered == []
    print("audit-registration-exception-identity", True)

    registration_live = []
    registration_active = False
    registration_added = False

    def registration_third(event, args):
        if registration_active and event in ("sys.addaudithook", "molt.hooks.order"):
            registration_live.append(("third", event))

    def registration_second(event, args):
        if registration_active and event in ("sys.addaudithook", "molt.hooks.order"):
            registration_live.append(("second", event))

    def registration_first(event, args):
        global registration_added
        if registration_active and event in ("sys.addaudithook", "molt.hooks.order"):
            registration_live.append(("first", event))
            if event == "sys.addaudithook" and not registration_added:
                registration_added = True
                sys.addaudithook(registration_third)

    sys.addaudithook(registration_first)
    registration_active = True
    sys.addaudithook(registration_second)
    sys.audit("molt.hooks.order")
    registration_active = False
    assert registration_live == [
        ("first", "sys.addaudithook"),
        ("first", "sys.addaudithook"),
        ("third", "sys.addaudithook"),
        ("first", "molt.hooks.order"),
        ("third", "molt.hooks.order"),
        ("second", "molt.hooks.order"),
    ]
    print("audit-registration-live-order", True)

    class AuditName(str):
        def __str__(self):
            raise AssertionError("audit event conversion invoked")

    event_object = AuditName("molt.hooks.name")
    normalized_events = []

    def event_listener(event, args):
        if event == "molt.hooks.name":
            normalized_events.append((type(event) is str, event is event_object, args))

    sys.addaudithook(event_listener)
    sys.audit(event_object, 29)
    if sys.version_info >= (3, 14):
        try:
            sys.audit("molt.hooks.name\0suffix", 31)
        except ValueError as error:
            assert str(error) == "embedded null character"
        else:
            raise AssertionError("embedded NUL audit event accepted")
        assert normalized_events == [(True, False, (29,))]
    else:
        sys.audit("molt.hooks.name\0suffix", 31)
        assert normalized_events == [(True, False, (29,)), (True, False, (31,))]
    try:
        sys.audit(1)
    except TypeError as error:
        expected = (
            "audit() argument 1 must be str, not int"
            if sys.version_info >= (3, 14)
            else "expected str for argument 'event', not int"
        )
        assert str(error) == expected
    else:
        raise AssertionError("non-string audit event accepted")
    print("audit-event-string-normalization", True)

    attribute_events = []
    attribute_mode = None

    class TraceFlag:
        def __bool__(self):
            attribute_events.append("bool")
            if attribute_mode == "bool-error":
                raise ValueError("flag truthiness")
            return True

    class AuditAttributeError(Exception):
        pass

    class AttributeHook:
        def __getattribute__(self, name):
            if name == "__cantrace__":
                if attribute_mode is None:
                    raise AttributeError(name)
                attribute_events.append("lookup")
                if attribute_mode == "missing":
                    raise AttributeError(name)
                if attribute_mode == "lookup-error":
                    raise AuditAttributeError("flag descriptor")
                return TraceFlag()
            return object.__getattribute__(self, name)

        def __call__(self, event, args):
            if event == "molt.hooks.attribute":
                attribute_events.append("call")

    sys.addaudithook(AttributeHook())
    for mode, expected in [
        ("missing", ["lookup", "call"]),
        ("truthy", ["lookup", "bool", "call"]),
        ("lookup-error", ["lookup"]),
        ("bool-error", ["lookup", "bool"]),
    ]:
        attribute_events.clear()
        attribute_mode = mode
        try:
            sys.audit("molt.hooks.attribute")
        except BaseException as error:
            assert type(error) is (
                AuditAttributeError if mode == "lookup-error" else ValueError
            )
            assert mode.endswith("error")
        else:
            assert not mode.endswith("error")
        finally:
            attribute_mode = None
        assert attribute_events == expected
    print("audit-optional-attribute-and-truthiness", True)
finally:
    reject_event = None
    reject_rollback = False
    sys.set_asyncgen_hooks(hooks.firstiter, hooks.finalizer)
