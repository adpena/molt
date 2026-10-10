"""Public codecs.StreamWriter protocol; CPython is the differential oracle."""


def run_protocol(codecs):
    class Failure(Exception):
        pass

    failure = Failure("original stream failure")

    def raises_same(call, expected):
        try:
            call()
        except BaseException as actual:
            assert actual is expected
        else:
            raise AssertionError("operation did not propagate the original exception")

    class Stream:
        def __init__(self):
            self.events = []
            self.result = object()
            self.fail_seek = False
            self.fail_close = False

        def write(self, data):
            self.events.append(("write", data))
            return self.result

        def seek(self, offset, whence=0):
            self.events.append(("seek", offset, whence))
            if self.fail_seek:
                raise failure
            return self.result

        def close(self):
            self.events.append(("close",))
            if self.fail_close:
                raise failure
            return self.result

    for method, value in ((codecs.Codec().encode, "data"),
                          (codecs.Codec().decode, b"data")):
        try:
            method(value)
        except NotImplementedError:
            pass
        else:
            raise AssertionError("base Codec must be abstract")
    stream = Stream()
    try:
        codecs.StreamWriter(stream).write("data")
    except NotImplementedError:
        pass
    else:
        raise AssertionError("base StreamWriter needs an encode implementation")
    assert stream.events == []

    # Use actual public encoding selection, including distinct byte layouts.
    for encoding, text, errors, expected in (
        ("utf-8", "café", "strict", b"caf\xc3\xa9"),
        ("latin-1", "café", "strict", b"caf\xe9"),
        ("ascii", "aéz", "replace", b"a?z"),
        ("utf-8", "a\udc80\udcffz", "surrogateescape", b"a\x80\xffz"),
    ):
        stream = Stream()
        writer = codecs.getwriter(encoding)(stream, errors)
        assert writer.write(text) is None
        assert stream.events == [("write", expected)]
        assert writer.reset() is None
        assert stream.events == [("write", expected)]

    class LookupFailure:
        @property
        def write(self):
            raise failure

    class WriteFailure:
        def write(self, data):
            assert data == b"data"
            raise failure

    for stream in (LookupFailure(), WriteFailure()):
        writer = codecs.getwriter("utf-8")(stream)
        raises_same(lambda: writer.write("data"), failure)

    # Encoder failure must precede even lookup of the stream's write member.
    stream = Stream()
    writer = codecs.getwriter("ascii")(stream)
    try:
        writer.write("é")
    except UnicodeEncodeError as exc:
        assert (exc.object, exc.start, exc.end) == ("é", 0, 1)
    else:
        raise AssertionError("strict encoding accepted non-ASCII text")
    assert stream.events == []

    # Dynamic encode overrides and stream lookup occur in public protocol order.
    class Writer(codecs.StreamWriter):
        def encode(self, obj, errors="strict"):
            self.calls.append(("encode", obj, errors))
            self.errors = "replace"
            self.stream = self.destination
            return b"custom", 0  # consumed does not change the write payload.

    first, second = Stream(), Stream()
    writer = Writer(first, "ignore")
    writer.calls, writer.destination = [], second
    assert writer.write("input") is None
    assert writer.calls == [("encode", "input", "ignore")]
    assert first.events == [] and second.events == [("write", b"custom")]
    third = Stream()
    writer.destination = third
    assert writer.write("next") is None
    assert writer.calls == [("encode", "input", "ignore"),
                            ("encode", "next", "replace")]
    assert second.events == [("write", b"custom")]
    assert third.events == [("write", b"custom")]

    class WriteOverride(codecs.StreamWriter):
        def write(self, obj):
            self.calls.append(obj)
            return object()

    writer = WriteOverride(Stream())
    writer.calls = []
    assert writer.writelines(iter(["one", "two", "three"])) is None
    assert writer.calls == ["onetwothree"]
    assert writer.writelines([]) is None
    assert writer.calls == ["onetwothree", ""]
    try:
        writer.writelines(["valid", 1])
    except TypeError:
        pass
    else:
        raise AssertionError("writelines accepted a non-string")
    assert writer.calls == ["onetwothree", ""]

    class ResetWriter(codecs.StreamWriter):
        def reset(self):
            self.stream.events.append(("reset",))
            if self.fail_reset:
                raise failure
            return object()

    stream = Stream()
    writer = ResetWriter(stream)
    writer.fail_reset = False
    for offset, whence in ((3, 0), (0, 1), (0, 2), (0, 0)):
        assert writer.seek(offset, whence) is None
    assert stream.events == [
        ("seek", 3, 0), ("seek", 0, 1), ("seek", 0, 2),
        ("seek", 0, 0), ("reset",),
    ]
    stream.events.clear()
    stream.fail_seek = True
    raises_same(lambda: writer.seek(0), failure)
    assert stream.events == [("seek", 0, 0)]
    stream.events.clear()
    stream.fail_seek = False
    writer.fail_reset = True
    raises_same(lambda: writer.seek(0), failure)
    assert stream.events == [("seek", 0, 0), ("reset",)]

    stream = Stream()
    writer = codecs.getwriter("utf-8")(stream)
    with writer as entered:
        assert entered is writer
    assert stream.events == [("close",)]
    stream.events.clear()

    def body_failure():
        with writer:
            raise failure

    raises_same(body_failure, failure)
    assert stream.events == [("close",)]
    stream.events.clear()
    stream.fail_close = True
    raises_same(lambda: writer.__exit__(None, None, None), failure)
    assert stream.events == [("close",)]

    # Sequential transfer to a real native thread must not bind a writer to
    # its construction thread. Join establishes deterministic observation.
    import threading

    stream = Stream()
    writer = codecs.getwriter("utf-8")(stream, "surrogateescape")
    outcomes = []

    def child():
        try:
            outcomes.append(writer.write("child\udc80"))
        except BaseException as exc:
            outcomes.append(exc)

    thread = threading.Thread(target=child)
    thread.start()
    thread.join()
    assert outcomes == [None]
    assert writer.write("parent") is None
    assert stream.events == [("write", b"child\x80"), ("write", b"parent")]


if __name__ == "__main__":
    import codecs

    run_protocol(codecs)
    print("stream-writer-protocol-ok")
