"""CPython is the oracle for shallow release order and resurrection (HF-660)."""

import weakref
import sys
import gc


events = []
refs = []
saved = []


class Item:
    def __init__(self, name, children=None, revive=False):
        self.name = name
        self.children = children
        self.revive = revive

    def __del__(self):
        events.append("del " + self.name)
        # Allocate and retire a separate object while a release is active.
        scratch = [{"name": self.name}]
        del scratch
        if self.revive:
            self.revive = False
            saved.append(self)


def item(name, children=None, revive=False):
    value = Item(name, children, revive)
    refs.append(weakref.ref(value, lambda ref: events.append("weak " + name)))
    return value


value = item("root", [item("left"), (item("middle"),), {"last": item("right")}])
del value
print("mixed", events)
print("mixed weakrefs cleared", all(ref() is None for ref in refs))
events.clear()
refs.clear()

value = item("revived", [item("child")], True)
del value
print("resurrected", events, len(saved), refs[-1]() is saved[0])
value = saved.pop()
del value
print("final death", events, len(saved))
print("resurrected weakrefs cleared", all(ref() is None for ref in refs))

# CPython's gen_dealloc clears weakrefs BEFORE closing a suspended generator,
# unlike subtype_dealloc's __del__-before-weakref order above.
events.clear()
refs.clear()


def closing_generator():
    try:
        yield 1
    finally:
        events.append("generator close")


value = closing_generator()
next(value)
refs.append(weakref.ref(value, lambda ref: events.append("generator weak")))
del value
print("generator", events, refs[0]() is None)

# Async generators share gen_dealloc, but their hook can resurrect the actual
# receiver and register a NEW weakref after the old weakrefs were cleared.
events.clear()
refs.clear()


async def closing_async_generator():
    try:
        yield 1
    finally:
        events.append("async close")


def revive_async_generator(value):
    events.append("async finalizer")
    saved.append(value)
    refs.append(weakref.ref(value, lambda ref: events.append("async new weak")))


hooks = sys.get_asyncgen_hooks()
sys.set_asyncgen_hooks(finalizer=revive_async_generator)
value = closing_async_generator()
step = value.__anext__()
try:
    step.send(None)
except StopIteration as exc:
    print("async yielded", exc.value)
del step
refs.append(weakref.ref(value, lambda ref: events.append("async old weak")))
del value
print("async resurrected", events, len(saved), refs[0]() is None, refs[1]() is saved[0])
value = saved.pop()
step = value.aclose()
try:
    step.send(None)
except StopIteration:
    pass
del step, value
print("async final death", events, len(saved), all(ref() is None for ref in refs))
sys.set_asyncgen_hooks(firstiter=hooks.firstiter, finalizer=hooks.finalizer)

# The long child queues an intact zero-count suffix, then unwinds to its
# sibling's finalizer. A collection here must not collect that queued suffix
# or detach its children before the terminal worklist resumes it.
events.clear()
refs.clear()


class CollectDuringRelease:
    def __del__(self):
        scratch = [{"scratch": 1}]
        del scratch
        events.append("collect sibling")
        gc.collect()


value = item("deferred bottom")
for _ in range(200):
    value = [value]
value = [CollectDuringRelease(), value]
del value
print("deferred gc", events, all(ref() is None for ref in refs))
