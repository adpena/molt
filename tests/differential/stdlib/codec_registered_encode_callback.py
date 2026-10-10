"""Registered codec callback custody; compiled runtime support remains open.

This independent differential case is deliberately retained without an xfail or
skip. CPython supplies the oracle; it is not part of the narrower writer-dispatch
qualification while registered encoding handlers remain unsupported in Molt.
"""


def run_protocol(codecs):
    class Stream:
        def __init__(self):
            self.events = []
            self.result = object()

        def write(self, data):
            self.events.append(("write", data))
            return self.result

    # The encoder receives the entry errors value. An actual error callback
    # changes stream/errors; write then looks up the current stream, and the
    # next write sees the changed errors value.
    original, replacement = Stream(), Stream()
    writer = codecs.getwriter("ascii")(original, "molt_writer_protocol")
    callbacks = []

    def handle_error(exc):
        callbacks.append((exc.object, exc.start, exc.end, writer.errors))
        writer.stream = replacement
        writer.errors = "ignore"
        return "?", exc.end

    codecs.register_error("molt_writer_protocol", handle_error)
    assert writer.write("aéz") is None
    assert callbacks == [("aéz", 1, 2, "molt_writer_protocol")]
    assert original.events == []
    assert replacement.events == [("write", b"a?z")]
    assert writer.write("béz") is None
    assert replacement.events[-1] == ("write", b"bz")
    assert len(callbacks) == 1


if __name__ == "__main__":
    import codecs

    run_protocol(codecs)
    print("registered-encode-callback-ok")
