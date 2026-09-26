"""Exercise runtime-resolved intrinsic results, not direct BUILTIN_FUNC lowering."""

from _intrinsics import require_intrinsic


def resolve(name, loader=require_intrinsic):
    return loader(name)


new_event = resolve("molt_asyncio_event_new")
drop_event = resolve("molt_asyncio_event_drop")
event = new_event()
assert drop_event(event) is None
# Dropping an absent registry handle must still return canonical None.
assert drop_event(event) is None

new_stream = resolve("molt_stream_new")
close_stream = resolve("molt_stream_close")
drop_stream = resolve("molt_stream_drop")
stream = new_stream(1)
assert close_stream(stream) is None
assert close_stream(stream) is None
assert drop_stream(stream) is None

spawn = resolve("molt_spawn")
try:
    spawn(None)
except TypeError as exc:
    assert str(exc) == "object is not awaitable"
else:
    raise AssertionError("invalid task did not raise TypeError")
# A caught provider exception must not poison the next indirect call.
assert drop_event(new_event()) is None
print("boxed-none-ok")
