"""Purpose: differential coverage for print keywords."""


class Sink:
    def __init__(self):
        self.parts = []
        self.flushes = 0

    def write(self, value):
        self.parts.append(value)
        return len(value)

    def flush(self):
        self.flushes += 1


class SinkNoFlush:
    def write(self, value):
        return len(value)


sink = Sink()
print("a", "b", sep=":", end="!", file=sink, flush=True)
print("sink1", repr("".join(sink.parts)), sink.flushes)

sink2 = Sink()
print("x", "y", sep=None, end=None, file=sink2)
print("sink2", repr("".join(sink2.parts)), sink2.flushes)

print("end-empty", end="")
print("tail")


def show_err(label, **kwargs):
    try:
        print("err", **kwargs)
    except Exception as exc:
        print(label, type(exc).__name__, exc)


show_err("sep-int", sep=1)
show_err("end-int", end=1)
show_err("file-object", file=object())

try:
    print("flush-missing", file=SinkNoFlush(), flush=True)
except Exception as exc:
    print("flush-missing", type(exc).__name__, exc)
