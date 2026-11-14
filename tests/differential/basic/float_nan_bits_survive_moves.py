import math
import struct


def bits(x: float) -> str:
    return struct.pack("<d", x).hex()


def identity(x: float) -> float:
    return x


nan = float("nan")
print(bits(nan), bits(-nan), math.isnan(nan))
payload = struct.unpack("<d", bytes.fromhex("9a07000000f8ff7f"))[0]
print(bits(payload), math.isnan(payload))
box = [payload]
print(bits(box[0]), bits(identity(payload)), bits({"k": payload}["k"]))
signaling = struct.unpack("<d", bytes.fromhex("0100000000f0ff7f"))[0]
print(bits(signaling))
print(repr(nan), nan != nan, payload != payload)

import array


class Holder:
    def __init__(self, value: float) -> None:
        self.value = value


negative = -nan
print(math.copysign(1.0, negative), math.copysign(1.0, nan), bits(negative))
print(bits(float("inf") - float("inf")), bits(payload + 0.0) == bits(payload + 0.0))
print(bits(Holder(payload).value), bits((payload, 1.0)[0]))
values = array.array("d", [payload, nan, -0.0])
print(bits(values[0]), bits(values[1]), bits(values[2]))
view = memoryview(struct.pack("<d", payload)).cast("d")
print(bits(view[0]))
print(nan in [nan], payload is payload)
