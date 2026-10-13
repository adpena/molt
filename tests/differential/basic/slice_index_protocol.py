"""Purpose: differential coverage for slice indices using __index__."""

log = []


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        log.append(self.value)
        return self.value


if __name__ == "__main__":
    data = [0, 1, 2, 3, 4, 5]
    print("slice", data[Index(1) : Index(5) : Index(2)])
    print("log", log)


# Every specialized sequence layout must preserve the Python slot protocol.
# Omitted and explicit steps take different compiler/runtime paths.
for parent, value in [
    (str, "abcd"),
    (bytes, b"abcd"),
    (bytearray, b"abcd"),
    (tuple, (1, 2, 3, 4)),
    (list, (1, 2, 3, 4)),
]:

    class Sequence(parent):
        def __getitem__(self, key):
            print("override", parent.__name__, key.start, key.stop, key.step)
            return "observed"

    sequence = Sequence(value)
    print("slice-result", sequence[1:3], sequence[1:3:1])
    sequence[:0]  # Discarding the result must preserve its callback.

    # A live slot mutation must invalidate any previous fast-path assumption.
    def reject(self, key):
        raise LookupError("slice callback")

    Sequence.__getitem__ = reject
    for explicit_step in (False, True):
        try:
            if explicit_step:
                sequence[1:3:1]
            else:
                sequence[1:3]
        except LookupError as error:
            print("slice-error", parent.__name__, str(error))
