"""Print stream lookup, conversion, write and flush order across targets."""

import sys

events = []


class Sink:
    @property
    def write(self):
        events.append("lookup-write")
        return self.record

    def record(self, text):
        events.append(("write", text))

    @property
    def flush(self):
        events.append("lookup-flush")
        return self.finish

    def finish(self):
        events.append("flush")


class Value:
    def __str__(self):
        events.append("str")
        return "value"


class Flush:
    def __init__(self, fails=False):
        self.fails = fails

    def __bool__(self):
        events.append("flush-bool")
        if self.fails:
            raise ValueError("flush conversion")
        return True


def show(label):
    print(label, events)
    events.clear()


print(Value(), Value(), sep="", end="", file=Sink(), flush=Flush())
show("pieces")
print(end="", file=Sink())
show("empty")

try:
    print(Value(), sep=1, file=Sink(), flush=Flush())
except TypeError:
    events.append("TypeError")
show("invalid-separator")

try:
    print(Value(), file=Sink(), flush=Flush(True))
except ValueError:
    events.append("ValueError")
show("flush-exception")

saved_stdout = sys.stdout
try:
    sys.stdout = None
    print(Value(), sep=1, flush=Flush())
    events.append("returned")
finally:
    sys.stdout = saved_stdout
show("disabled-stdout")


class ReplacingSink:
    def write(self, text):
        events.append(("first-write", text))
        self.write = self.later

    def later(self, text):
        events.append(("later-write", text))


print("one", "two", file=ReplacingSink())
show("replaced-write")


class FailingLookup:
    @property
    def write(self):
        events.append("lookup-failed")
        raise LookupError("write lookup")


try:
    print(Value(), file=FailingLookup())
except LookupError:
    events.append("LookupError")
show("lookup-before-conversion")


class FailingWrite:
    def write(self, text):
        events.append(("write-failed", text))
        raise OSError("write failure")


try:
    print(Value(), Value(), file=FailingWrite(), flush=True)
except OSError:
    events.append("OSError")
show("write-exception")


class RedirectingFlush:
    def __bool__(self):
        events.append("redirect")
        sys.stdout = Sink()
        return False


try:
    print(Value(), flush=RedirectingFlush())
finally:
    sys.stdout = saved_stdout
show("flush-before-stream")


class WriteResult:
    def __del__(self):
        events.append("drop-result")


class OwnedWriter:
    def __call__(self, text):
        events.append(("owned-write", text))
        return WriteResult()

    def __del__(self):
        events.append("drop-writer")


class OwnedWriterSink:
    @property
    def write(self):
        events.append("new-writer")
        return OwnedWriter()


print("owned", end="", file=OwnedWriterSink())
show("writer-result-release")
