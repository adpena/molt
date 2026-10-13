"""Request injection refusal, legal folding, and atomic header publication."""

import http.client


class NoTransport(http.client.HTTPConnection):
    sent = 0

    def send(self, data):
        self.sent += 1
        raise AssertionError("invalid request reached transport")


for label, name, values in (
    ("lf", "X-Test", ("bad\nvalue",)),
    ("cr", "X-Test", (b"bad\rvalue",)),
    ("crlf", "X-Test", ("bad\r\nInjected: yes",)),
    ("terminal", "X-Test", ("bad\n",)),
    ("later", "X-Test", ("good", "bad\nvalue")),
    ("empty-name", "", ("v",)),
    ("leading-space", " X", ("v",)),
    ("leading-vtab", b"\x0bX", (b"v",)),
    ("colon", "X:Y", ("v",)),
    ("name-lf", b"X\nY", (b"v",)),
    ("latin-name-str", "X-\xff", ("v",)),
    ("wide-value", "X-Test", ("\u0100",)),
    ("latin-value", "X-Test", ("\xff",)),
    ("latin-bytes", b"X-\xff", (b"\x80\xff",)),
    ("fold-crlf", "X-Test", (b"a\r\n\tb",)),
    ("fold-lf", "X-Test", ("a\n b",)),
    ("fold-cr", "X-Test", ("a\r b",)),
    ("multi", "X-Test", ("a", b"b", 7)),
    ("empty-values", "X-Test", ()),
):
    connection = NoTransport("localhost")
    connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)
    before = tuple(connection._buffer)
    try:
        connection.putheader(name, *values)
    except Exception as exc:
        print(label, type(exc).__name__, tuple(connection._buffer) == before)
    else:
        print(label, "accepted")
    connection.close()

for label, method, path in (
    ("method-cr", "GET\rX", "/"),
    ("method-lf", "GET\nX", "/"),
    ("method-tab", "GET\tX", "/"),
    ("path-cr", "GET", "/a\rb"),
    ("path-lf", "GET", "/a\nb"),
    ("path-space", "GET", "/a b"),
    ("path-del", "GET", "/a\x7fb"),
):
    connection = NoTransport("localhost")
    try:
        connection.request(method, path)
    except Exception as exc:
        print(label, type(exc).__name__, connection.sent)
    connection.close()

for host in ("host\rInjected", "host\nInjected", "host bad", "host\x7f"):
    try:
        NoTransport(host)
    except Exception as exc:
        print("host", type(exc).__name__)

for name, value in (("X", "bad\nvalue"), ("X:Injected", "v")):
    connection = NoTransport("localhost")
    try:
        connection.request("GET", "/", headers={name: value})
    except Exception as exc:
        print("direct", type(exc).__name__, connection.sent)
    connection.close()

sentinel = ValueError("encode sentinel")


class BadValue:
    def encode(self, encoding):
        raise sentinel


connection = NoTransport("localhost")
connection.putrequest("GET", "/")
before = tuple(connection._buffer)
try:
    connection.putheader("X", BadValue())
except ValueError as exc:
    print("callback", exc is sentinel, tuple(connection._buffer) == before, connection.sent)
connection.close()


calls = []


class CountedValue:
    def encode(self, encoding):
        calls.append(encoding)
        return b"ok"


for name, values in (("bad:name", (CountedValue(),)), ("X", ("bad\n", CountedValue()))):
    connection = NoTransport("localhost")
    connection.putrequest("GET", "/")
    calls.clear()
    try:
        connection.putheader(name, *values)
    except ValueError:
        print("ordered-header", calls, connection.sent)
    connection.close()

for method, path in (("GET\n", "/"), ("GET", "/bad\n")):
    connection = NoTransport("localhost")
    calls.clear()
    try:
        connection.request(method, path, headers={"X": CountedValue()})
    except (ValueError, http.client.InvalidURL) as exc:
        print("ordered-request", type(exc).__name__, calls, connection.sent)
    connection.close()


class IndexOnly:
    def __index__(self):
        raise AssertionError("header coercion must not call __index__")


class NamedInt(int):
    def __str__(self):
        return "int-subclass"


for value in (True, 1 << 100, NamedInt(3), 1.0, IndexOnly()):
    connection = NoTransport("localhost")
    connection.putrequest("GET", "/")
    try:
        connection.putheader("X", value)
    except Exception as exc:
        print("integer-protocol", type(value).__name__, type(exc).__name__)
    else:
        print("integer-protocol", type(value).__name__, "accepted")
    connection.close()

import urllib.request

request = urllib.request.Request("http://localhost/", headers={"X": "bad\nvalue"})
print("deferred-container", request.get_header("X") == "bad\nvalue")


import gc
import urllib.parse

retired = []


class TrackedIterator:
    def __init__(self, rows, label):
        self.rows = rows
        self.index = 0
        self.label = label

    def __iter__(self):
        return self

    def __next__(self):
        if self.index == len(self.rows):
            raise StopIteration
        row = self.rows[self.index]
        self.index += 1
        return row

    def __del__(self):
        retired.append(self.label)


class TrackedHeaders(dict):
    def items(self):
        return TrackedIterator(list(super().items()), "headers")


for invalid in (False, True):
    retired.clear()
    headers = TrackedHeaders({"X": "ok", "Y": "bad\nvalue" if invalid else "ok"})
    connection = NoTransport("localhost")
    try:
        connection.request("GET", "/", headers=headers)
    except ValueError:
        assert invalid
    except AssertionError:
        assert not invalid
    connection.close()
    gc.collect()
    print("header-iterator-retired", invalid, retired)


class TrackedRows(list):
    def __iter__(self):
        return TrackedIterator(self, "query")


for rows in ([("a", "b")], [("a", "b"), ("bad",)]):
    retired.clear()
    try:
        print("query-result", urllib.parse.urlencode(TrackedRows(rows)))
    except ValueError:
        print("query-result", "ValueError")
    gc.collect()
    print("query-iterator-retired", retired)

# A later encoder sees and may mutate an earlier converted buffer.
shared_value = bytearray(b"a")
events = []


class MutatingValue:
    def encode(self, encoding):
        shared_value[0] = ord("z")
        return b"b"


class EncodedString(str):
    def encode(self, encoding):
        events.append(("str-result-encode", encoding))
        return b"custom"


class StringResultInt(int):
    def __str__(self):
        return EncodedString("override")


class CaptureWire(http.client.HTTPConnection):
    def send(self, data):
        self.wire = bytes(data)


for label, values in (
    ("mutated-value", (shared_value, MutatingValue())),
    ("str-result", (StringResultInt(1),)),
):
    events.clear()
    connection = CaptureWire("localhost")
    connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)
    connection.putheader("X-Test", *values)
    connection.endheaders()
    print(label, connection.wire, events)
    connection.close()

# Encoding temporaries remain alive until the current output callback runs.
retirement_events = []
connection = NoTransport("localhost")
connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)


class EncodedBytes(bytes):
    def __del__(self):
        retirement_events.append(connection._buffer[-1] == b"X: owned")


class TemporaryValue:
    def encode(self, encoding):
        return EncodedBytes(b"owned")


connection.putheader("X", TemporaryValue())
gc.collect()
print("encoding-retirement-after-output", retirement_events)
connection.close()

for replace_output in (False, True):
    connection = NoTransport("localhost")
    connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)
    published = []

    class ReplacingValue:
        def encode(self, encoding):
            connection._buffer = []
            if replace_output:
                connection._output = lambda line: published.append(line)
            return b"value"

    connection.putheader("X", ReplacingValue())
    print("late-output-owner", replace_output, connection._buffer, published)
    connection.close()

mutable_line = None


class HeaderPrefix:
    def __add__(self, value):
        global mutable_line
        mutable_line = bytearray(b"X: " + value)
        return mutable_line


class Header(bytes):
    def __add__(self, colon):
        return HeaderPrefix()


connection = CaptureWire("localhost")
connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)
connection.putheader(Header(b"X"), b"a")
mutable_line[-1] = ord("z")
connection.endheaders()
print("mutable-add-result", connection.wire)
connection.close()

calls.clear()
connection = NoTransport("localhost")
connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)


class ObservedValue:
    def encode(self, encoding):
        calls.append("value")
        return b"a"


try:
    connection.putheader(memoryview(b"X"), ObservedValue())
except TypeError:
    print("name-concatenation-after-values", calls)
connection.close()

# Original inputs and converted outputs have distinct single-frame lifetimes.
lifetime_events = []


class LifetimeEncoded(bytes):
    def __del__(self):
        lifetime_events.append(("encoded-del", bytes(self).decode("ascii")))


class LifetimeInput:
    def __init__(self, text):
        self.text = text

    def encode(self, encoding):
        lifetime_events.append(("encode", self.text, encoding))
        return LifetimeEncoded(self.text.encode(encoding))

    def __del__(self):
        lifetime_events.append(("input-del", self.text))


class LifetimeConnection(NoTransport):
    def _output(self, line):
        lifetime_events.append(("emit", bytes(line).decode("ascii")))
        super()._output(line)


connection = LifetimeConnection("localhost")
connection.putrequest("GET", "/", skip_host=True, skip_accept_encoding=True)
lifetime_events.clear()
connection.putheader(LifetimeInput("X"), LifetimeInput("one"), LifetimeInput("two"))
lifetime_events.append(("returned",))
print("single-frame-input-retirement", lifetime_events)
connection.close()
