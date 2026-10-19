# MOLT_ENV: MOLT_CAPABILITIES=fs,env,time,random,os.getpid
"""The shared filter protocol must reach every logger/handler consumer."""

import io
import logging
import sys

stream = io.StringIO()
handler = logging.StreamHandler(stream)


class Dropper(logging.Filter):
    def filter(self, record):
        return record.msg != "drop"

logger = logging.getLogger("filter_demo")
logger.handlers[:] = [handler]
logger.setLevel(logging.INFO)
logger.propagate = False
logger.addFilter(Dropper())

logger.info("keep")
logger.info("drop")

print("out", stream.getvalue().strip())


def record(message):
    return logging.LogRecord("protocol", logging.INFO, "fixture", 1, message, (), None)


original = record("original")
replacement = record("replacement")
filterer = logging.Filterer()
print("empty identity", filterer.filter(original) is original)
events = []


class Both:
    def filter(self, value):
        events.append(("method", value is original))
        return replacement

    def __call__(self, value):
        raise AssertionError("filter method must take precedence")


def mutate(value):
    events.append(("callable", value is replacement))
    value.msg = "changed"
    return "truthy, not a replacement"


filterer.addFilter(Both())
filterer.addFilter(mutate)
print("chain identity", filterer.filter(original) is replacement, original.msg)
print("chain events", events, replacement.msg)

for false_value in (False, None, 0, "", []):
    filterer = logging.Filterer()
    filterer.addFilter(lambda value: false_value)
    filterer.addFilter(lambda value: events.append("unreachable"))
    print("false veto", filterer.filter(original) is False)


class FalseRecord(logging.LogRecord):
    def __bool__(self):
        return False


false_record = FalseRecord("protocol", logging.INFO, "fixture", 1, "false", (), None)
filterer = logging.Filterer()
filterer.addFilter(lambda value: false_record)
print("false record veto", filterer.filter(original) is False)

# The filter list is live. Adding a filter during a callback participates in
# this pass; taking a snapshot would lose it. Duplicate registration stays unique.
events = []
filterer = logging.Filterer()


def appended(value):
    events.append("appended")
    return True


def append_filter(value):
    events.append("append")
    filterer.addFilter(appended)
    return True


filterer.addFilter(append_filter)
filterer.addFilter(append_filter)
print("live registration", filterer.filter(original) is original, events)
filterer.removeFilter(appended)
filterer.removeFilter(appended)
print("removed once", len(filterer.filters))


class Capture(logging.Handler):
    def __init__(self):
        super().__init__()
        self.records = []

    def emit(self, value):
        self.records.append(value)


capture = Capture()
capture.addFilter(lambda value: replacement)
print("handler identity", capture.handle(original) is replacement,
      capture.records[0] is replacement)
capture.filters[:] = [lambda value: False]
print("handler veto", capture.handle(original) is False, len(capture.records))

# An overridden filter can return any truthy value. Handler.handle returns that
# exact value while emitting the original record unless it is a LogRecord.
truthy = object()
capture.filter = lambda value: truthy
print("override truthy", capture.handle(original) is truthy,
      capture.records[-1] is original)
capture.close()

# Logger replacements reach all handlers, but a handler replacement stays local.
# Propagation visits ancestor handlers without invoking ancestor logger filters.
parent = logging.getLogger("protocol.parent")
child = logging.getLogger("protocol.parent.child")
parent.propagate = False
child.propagate = True
parent.handlers[:] = []
child.handlers[:] = []
first, sibling, ancestor = Capture(), Capture(), Capture()
local = record("handler local")
child.addFilter(lambda value: replacement)
first.addFilter(lambda value: local)
child.addHandler(first)
child.addHandler(sibling)
parent.addHandler(ancestor)


def ancestor_filter(value):
    raise AssertionError("propagation must not run ancestor logger filters")


parent.addFilter(ancestor_filter)
print("logger return", child.handle(original) is None)
print("replacement scope", first.records[0] is local,
      sibling.records[0] is replacement, ancestor.records[0] is replacement)
child.disabled = True
print("disabled", child.handle(original) is None, len(first.records))
for sink in (first, sibling, ancestor):
    sink.close()

# Both Filterer and its consumers must expose the same exception, without
# swallowing callback, attribute lookup or truth-value failures.
failure = ValueError("filter failure")


def fail(value):
    raise failure


class BadLookup:
    @property
    def filter(self):
        raise failure


class BadTruth:
    def __bool__(self):
        raise failure


for filt in (fail, BadLookup(), lambda value: BadTruth()):
    for owner in (logging.Filterer(), Capture(), logging.Logger("failure")):
        owner.addFilter(filt)
        try:
            if isinstance(owner, logging.Filterer) and not isinstance(owner, (logging.Handler, logging.Logger)):
                owner.filter(original)
            else:
                owner.handle(original)
        except ValueError as exc:
            print("exception identity", exc is failure)
        else:
            print("exception swallowed")
        if isinstance(owner, logging.Handler):
            print("exception did not emit", not owner.records)
            owner.close()

# TypeError from a present non-callable method or non-callable filter is public.
class BadMethod:
    filter = None


for filt in (BadMethod(), object()):
    filterer = logging.Filterer()
    filterer.addFilter(filt)
    try:
        filterer.filter(original)
    except TypeError:
        print("noncallable refused")
    else:
        print("noncallable swallowed")

# StreamHandler inherits the same replacement boundary used by custom handlers.
stream = io.StringIO()
handler = logging.StreamHandler(stream)
handler.addFilter(lambda value: replacement)
print("stream identity", handler.handle(original) is replacement)
print("stream replacement", stream.getvalue().strip())
handler.close()

# CPython 3.12 invokes the public acquire/release hooks; 3.13+ enters the lock.
# In either policy emission failure must release exactly the acquired lock.
events = []


class Lock:
    def acquire(self):
        events.append("lock acquire")

    def release(self):
        events.append("lock release")

    def __enter__(self):
        events.append("lock enter")

    def __exit__(self, kind, exc, tb):
        events.append(("lock exit", exc is failure))


class Locked(Capture):
    def acquire(self):
        events.append("handler acquire")
        super().acquire()

    def release(self):
        events.append("handler release")
        super().release()

    def emit(self, value):
        events.append(("emit replacement", value is replacement))
        raise failure


handler = Locked()
handler.lock = Lock()
handler.addFilter(lambda value: replacement)
try:
    handler.handle(original)
except ValueError as exc:
    print("emit exception identity", exc is failure)
if sys.version_info >= (3, 13):
    expected = ["lock enter", ("emit replacement", True), ("lock exit", True)]
else:
    expected = ["handler acquire", "lock acquire", ("emit replacement", True),
                "handler release", "lock release"]
print("versioned lock order", events == expected)
handler.close()
