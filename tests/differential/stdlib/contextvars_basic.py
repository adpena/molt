"""Purpose: differential coverage for contextvars basic."""

import contextvars

var = contextvars.ContextVar("var", default="default")
print(var.get())
token = var.set("value")
print(var.get())
var.reset(token)
print(var.get())

var2 = contextvars.ContextVar("var2")
try:
    var2.get()
except LookupError as exc:
    print(type(exc).__name__)

var.set("main")
ctx = contextvars.copy_context()
var.set("mutated")


def show() -> None:
    print(var.get())


ctx.run(show)
print(var.get())

# Context.get is mapping lookup: it does not inherit the variable's default.
empty = contextvars.Context()
print("mapping", empty.get(var), empty.get(var, "caller"), var in empty)
try:
    empty[var]
except KeyError as exc:
    print("key", exc.args[0] is var)
print("attrs", token.var is var, token.old_value is contextvars.Token.MISSING)

# Tokens belong to a Context identity, even when a copied mapping is equal.
origin_token = var.set("origin")
copy = contextvars.copy_context()
try:
    copy.run(var.reset, origin_token)
except ValueError:
    print("wrong-context", True)
var.reset(origin_token)

# Iterator snapshots retain their original root across mutations and deletion.
snapshot = contextvars.copy_context()
keys = snapshot.keys()
values = snapshot.values()
items = snapshot.items()
snapshot.run(var.set, "later")
print("snapshot", var in list(keys), "mutated" in list(values), (var, "mutated") in list(items))
try:
    snapshot.run(snapshot.run, lambda: None)
except RuntimeError:
    print("reentry", True)
print("sequential", snapshot.run(var.get), snapshot.run(var.get))

# Distinct equal names retain identity; mapping comparison must call value eq.
a = contextvars.ContextVar("same")
b = contextvars.ContextVar("same")
other = contextvars.Context()
other.run(a.set, 1)
other.run(b.set, 2)
print("distinct", len(other), other[a], other[b], other == other.copy())
for obj, name in [(a, "name"), (origin_token, "var"), (origin_token, "old_value")]:
    try:
        setattr(obj, name, None)
    except AttributeError:
        print("readonly", name)

print("generic", contextvars.ContextVar[int].__origin__ is contextvars.ContextVar,
      contextvars.Token[int].__origin__ is contextvars.Token)
try:
    hash(origin_token)
except TypeError:
    print("token-unhashable", True)

# The retained iterator root survives replacement; collecting its old cycle
# releases it only after the iterator is gone.
import gc
import weakref
cycle_context = contextvars.Context()
cycle_var = contextvars.ContextVar("cycle")
cycle_context.run(cycle_var.set, cycle_context)
cycle_ref = weakref.ref(cycle_context)
del cycle_context
gc.collect()
print("cycle-collected", cycle_ref() is None)

# Manual coroutine resume inherits the context at each send; coroutine creation
# and cancellation identity do not supply an execution Context.
import types


@types.coroutine
def checkpoint():
    yield "pause"


async def manually_resumed():
    print("manual-before", manual_var.get())
    await checkpoint()
    print("manual-after", manual_var.get())
    return "done"


manual_var = contextvars.ContextVar("manual", default="ambient")
manual_ctx = contextvars.Context()
manual_ctx.run(manual_var.set, "override")
manual_coro = manually_resumed()
print("manual-pause", manual_ctx.run(manual_coro.send, None))
try:
    manual_coro.send(None)
except StopIteration as stopped:
    print("manual-result", stopped.value)
print("manual-context", manual_ctx.get(manual_var), manual_var.get())

# Constructor signatures and receiver admission, including direct __new__.
def constructor_case(label, call):
    try:
        result = call()
    except Exception as exc:
        print("constructor", label, type(exc).__name__)
    else:
        print("constructor", label, type(result).__name__)


for label, call in [
    ("positional", lambda: contextvars.ContextVar("x")),
    ("default-none", lambda: contextvars.ContextVar("x", default=None)),
    ("keyword-name", lambda: contextvars.ContextVar(name="x")),
    ("two-positional", lambda: contextvars.ContextVar("x", None)),
    ("duplicate-name", lambda: contextvars.ContextVar("x", name="x")),
    ("unknown", lambda: contextvars.ContextVar("x", unknown=None)),
    ("missing", lambda: contextvars.ContextVar(default=None)),
    ("invalid-name", lambda: contextvars.ContextVar(None)),
    ("context-new", lambda: contextvars.Context.__new__(contextvars.Context)),
    ("context-unrelated", lambda: contextvars.Context.__new__(int)),
    ("context-nontype", lambda: contextvars.Context.__new__(1)),
    ("context-missing", lambda: contextvars.Context.__new__()),
    ("var-new", lambda: contextvars.ContextVar.__new__(contextvars.ContextVar, "x")),
    ("var-unrelated", lambda: contextvars.ContextVar.__new__(int, None)),
    ("var-nontype", lambda: contextvars.ContextVar.__new__(1)),
    ("var-missing", lambda: contextvars.ContextVar.__new__()),
]:
    constructor_case(label, call)
for cls in [contextvars.Token, type(contextvars.Token.MISSING),
            type(empty.keys()), type(empty.values()), type(empty.items())]:
    for receiver in [cls, int, 1]:
        constructor_case(cls.__name__, lambda cls=cls, receiver=receiver: cls.__new__(receiver))
    constructor_case(cls.__name__ + "-missing", lambda cls=cls: cls.__new__())

# Distinct roots force value equality and keep both roots alive across callbacks.
equality_calls = []
equality_key = contextvars.ContextVar("equality")
left = contextvars.Context()
right = contextvars.Context()


class EqualValue:
    def __init__(self, action=None):
        self.action = action

    def __eq__(self, other):
        equality_calls.append("eq")
        if self.action is not None:
            self.action()
        return isinstance(other, EqualValue)


left.run(equality_key.set, EqualValue())
right.run(equality_key.set, EqualValue())
print("distinct-root-equality", left == right, equality_calls)
equality_calls.clear()
left.run(equality_key.set, EqualValue(lambda: left.run(equality_key.set, "replaced")))
print("reentrant-equality", left == right, left[equality_key], equality_calls)


def fail_equality():
    raise ValueError("equality callback")


left.run(equality_key.set, EqualValue(fail_equality))
try:
    left == right
except ValueError as exc:
    print("raising-equality", str(exc))

# Snapshot retention, shared descendant last-owner release, and token cycles.
class Payload:
    pass


payload_key = contextvars.ContextVar("payload")
holder = contextvars.Context()
payload = Payload()
payload_ref = weakref.ref(payload)
holder.run(payload_key.set, payload)
shared_holder = holder.copy()
iterator = holder.values()
del payload
holder.run(payload_key.set, None)
del shared_holder
gc.collect()
print("iterator-retains", payload_ref() is not None)
del iterator
gc.collect()
print("iterator-releases", payload_ref() is None)

cycle_holder = contextvars.Context()
cycle_value = Payload()
cycle_ref = weakref.ref(cycle_value)
cycle_token = cycle_holder.run(payload_key.set, cycle_value)
cycle_value.token = cycle_token
cycle_value.context = cycle_holder
del cycle_holder, cycle_value, cycle_token
gc.collect()
print("token-cycle-collected", cycle_ref() is None)

# ContextVar's name and default are owned edges even when a str subclass has
# an instance dictionary pointing back into the Context/variable graph.
class Name(str):
    pass


owned_name = Name("owned-name")
owned_default = Payload()
name_ref = weakref.ref(owned_name)
default_ref = weakref.ref(owned_default)
owned_variable = contextvars.ContextVar(owned_name, default=owned_default)
owned_name.variable = owned_variable
owned_default.variable = owned_variable
del owned_name, owned_default, owned_variable
gc.collect()
print("variable-cycle-collected", name_ref() is None, default_ref() is None)
