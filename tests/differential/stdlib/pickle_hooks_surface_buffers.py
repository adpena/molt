"""Differential coverage for pickle reducer/object-hook and class-surface edges."""

from __future__ import annotations

import io
import pickle


class WithGetState:
    def __init__(self, x: int) -> None:
        self.x = x

    def __getstate__(self):
        return {"x": self.x + 1}


class WithGetStateNone:
    def __init__(self) -> None:
        self.x = 1

    def __getstate__(self):
        return None


class BadReduceScalar:
    def __reduce__(self):
        return 1


class BadReduceCallable:
    def __reduce__(self):
        return (1, ())


class BadReduceArgs:
    def __reduce__(self):
        return (len, 1)


class BadReduceSetter:
    def __reduce__(self):
        return (len, (), None, None, None, 1)


def main() -> None:
    for proto in (2, 4, 5):
        out = pickle.loads(pickle.dumps(WithGetState(3), protocol=proto))
        print("getstate", proto, out.x)

    for proto in (2, 4, 5):
        out = pickle.loads(pickle.dumps(WithGetStateNone(), protocol=proto))
        print("getstate_none", proto, hasattr(out, "x"))

    for cls in (
        BadReduceScalar,
        BadReduceCallable,
        BadReduceArgs,
        BadReduceSetter,
    ):
        try:
            pickle.dumps(cls(), protocol=5)
        except Exception as exc:
            print("bad", cls.__name__, type(exc).__name__, str(exc))

    pickler = pickle.Pickler(io.BytesIO())
    print("pickler_fast_bin", pickler.fast, pickler.bin)
    print("pickler_dispatch_table_attr", hasattr(pickler, "dispatch_table"))

    pb = pickle.PickleBuffer(bytearray(b"abc"))
    payload = [pb, pb]
    captured: list[bytes] = []

    def callback(view) -> None:
        captured.append(bytes(view))
        return None

    blob = pickle.dumps(payload, protocol=5, buffer_callback=callback)
    print("captured", len(captured), blob.count(b"\x97"), blob.count(b"\x98"))
    restored = pickle.loads(blob, buffers=[memoryview(raw) for raw in captured])
    print(
        "loaded",
        restored[0] is restored[1],
        type(restored[0]).__name__,
        bytes(restored[0]),
    )

    pb_readonly = pickle.PickleBuffer(b"xyz")
    captured_readonly: list[bytes] = []

    def callback_readonly(view) -> None:
        captured_readonly.append(bytes(view))
        return None

    readonly_blob = pickle.dumps(
        pb_readonly, protocol=5, buffer_callback=callback_readonly
    )
    readonly = pickle.loads(readonly_blob, buffers=[memoryview(captured_readonly[0])])
    print("readonly", type(readonly).__name__, readonly.readonly, bytes(readonly))


if __name__ == "__main__":
    main()


# Reducer iterator slots use a distinct owned collector from NEXT_BUFFER.
class WithListItems:
    def __reduce__(self):
        return list, (), None, iter([11, 12])


class WithDictItems:
    def __reduce__(self):
        return dict, (), None, None, iter([("left", 11), ("right", 12)])


assert pickle.loads(pickle.dumps(WithListItems(), protocol=4)) == [11, 12]
assert pickle.loads(pickle.dumps(WithDictItems(), protocol=4)) == {
    "left": 11,
    "right": 12,
}


class ReducerFailure(RuntimeError):
    pass


reducer_failure = ReducerFailure("reducer iterator failed")


class FailingItems:
    def __init__(self, dictionary):
        self.dictionary = dictionary
        self.position = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.position:
            raise reducer_failure
        self.position += 1
        return ("left", 11) if self.dictionary else 11


class WithFailingItems:
    def __init__(self, dictionary):
        self.dictionary = dictionary

    def __reduce__(self):
        if self.dictionary:
            return dict, (), None, None, FailingItems(True)
        return list, (), None, FailingItems(False)


for dictionary in (False, True):
    try:
        pickle.dumps(WithFailingItems(dictionary), protocol=4)
    except ReducerFailure as caught:
        assert caught is reducer_failure
    else:
        raise AssertionError("pickle swallowed reducer iterator failure")
print("reducer iterator slots", True, True)
