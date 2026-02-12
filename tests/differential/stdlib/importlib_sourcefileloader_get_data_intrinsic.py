"""Purpose: validate SourceFileLoader.get_data uses intrinsic-backed file reads."""

import importlib.machinery
import os
import tempfile


def show(label, fn):
    try:
        print(label, fn())
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__)


with tempfile.TemporaryDirectory() as tmp:
    path = os.path.join(tmp, "payload.bin")
    with open(path, "wb") as handle:
        handle.write(b"\x00molt\xff")

    loader = importlib.machinery.SourceFileLoader("demo_payload", path)
    print("read_ok", loader.get_data(path) == b"\x00molt\xff")

    missing = os.path.join(tmp, "missing.bin")
    show("missing", lambda: loader.get_data(missing))
