"""Protocol 5 out-of-band buffer parity for pickle dumps/loads."""

from __future__ import annotations

import pickle


def _as_bytes(buf) -> bytes:
    try:
        return bytes(buf)
    except TypeError:
        return bytes(buf.raw())


def main() -> None:
    payload = {
        "b": pickle.PickleBuffer(b"alpha"),
        "ba": pickle.PickleBuffer(bytearray(b"beta")),
    }

    captured: list[bytes] = []

    def callback(view) -> None:
        captured.append(_as_bytes(view))
        return None

    blob = pickle.dumps(payload, protocol=5, buffer_callback=callback)
    print("ops", blob.count(b"\x97"), blob.count(b"\x98"))
    print("captured", captured)

    restored = pickle.loads(blob, buffers=[memoryview(raw) for raw in captured])
    print("round", bytes(restored["b"]), bytes(restored["ba"]))
    print("types", type(restored["b"]).__name__, type(restored["ba"]).__name__)

    inband_seen: list[bytes] = []

    def inband_callback(view) -> bool:
        inband_seen.append(_as_bytes(view))
        return True

    inband_blob = pickle.dumps(payload, protocol=5, buffer_callback=inband_callback)
    inband_restored = pickle.loads(inband_blob)
    print("inband", len(inband_seen), inband_restored["b"], inband_restored["ba"])


if __name__ == "__main__":
    main()
