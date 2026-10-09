"""Purpose: a `with` target binds inside the protected region; reads after
a suppressing `__exit__` or a recovering handler see CPython's bindings.

`with cm as name` stores `name` after `__enter__` returns and before the body
runs, inside the region whose exceptions reach `__exit__`. A suppressing
`__exit__` continues after the `with`. A later read sees the stored value, or
the earlier binding when the store never ran. The same holds for a local store
that is the only fallible step of a `try` body.
"""

import warnings


class Suppress:
    def __init__(self, value):
        self.value = value
        self.seen = []

    def __enter__(self):
        return self.value

    def __exit__(self, kind, value, traceback):
        self.seen.append(None if kind is None else kind.__name__)
        return True


class RejectsAttribute:
    def __setattr__(self, name, value):
        raise AttributeError(f"cannot set {name}")


def read_after_suppressed_body(value):
    manager = Suppress(value)
    with manager as bound:
        raise ValueError("suppressed")
    return bound, manager.seen


def read_after_clean_body(value):
    manager = Suppress(value)
    with manager as bound:
        pass
    return bound, manager.seen


def rebind_in_loop(values):
    results = []
    for value in values:
        with Suppress(value) as bound:
            if value == 2:
                raise KeyError(value)
        results.append(bound)
    return results


def earlier_binding_survives(value):
    bound = "earlier"
    manager = Suppress(value)
    with manager as bound:
        pass
    return bound


def failing_attribute_target():
    target = RejectsAttribute()
    manager = Suppress("entered")
    with manager as target.attribute:
        raise RuntimeError("body must not run")
    return manager.seen


def nested_targets():
    with Suppress("outer") as outer, Suppress("inner") as inner:
        raise LookupError("both")
    return outer, inner


def recorded_warnings(actions):
    lines = []
    for action in actions:
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter(action, DeprecationWarning)
            warnings.warn("old", DeprecationWarning)
        lines.append((action, [str(item.message) for item in caught]))
    return lines


def try_body_local_copy(other):
    try:
        copy = other
    except MemoryError:
        pass
    return copy


def try_body_in_loop(values):
    results = []
    for value in values:
        try:
            copy = value
        except MemoryError:
            copy = None
        results.append(copy)
    return results


for value in (1, 1 << 62, 1 << 100, -7, None, "text", 2.5):
    print(read_after_suppressed_body(value))
    print(read_after_clean_body(value))
print(rebind_in_loop([1, 2, 3]))
print(earlier_binding_survives("entered"))
print(failing_attribute_target())
print(nested_targets())
print(recorded_warnings(["always", "ignore"]))
print(try_body_local_copy(1 << 62), try_body_local_copy("x"))
print(try_body_in_loop([1, 1 << 63, -(1 << 64)]))
