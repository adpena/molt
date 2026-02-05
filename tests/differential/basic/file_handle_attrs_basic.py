"""Purpose: verify file handle closefd/buffer attributes parity."""

import os
import tempfile


tmp = tempfile.NamedTemporaryFile(delete=False)
path = tmp.name
tmp.close()

try:
    raw = open(path, "rb", buffering=0)
    print("raw-closefd", hasattr(raw, "closefd"))
    print("raw-closefd-value", raw.closefd)
    print("raw-buffer", hasattr(raw, "buffer"))
    raw.close()

    buf = open(path, "rb")
    print("buf-closefd", hasattr(buf, "closefd"))
    try:
        _ = buf.closefd
        print("buf-closefd-value")
    except AttributeError:
        print("buf-closefd-attrerr")
    print("buf-buffer", hasattr(buf, "buffer"))
    buf.close()

    text = open(path, "r")
    print("text-buffer", hasattr(text, "buffer"))
    print("text-buffer-not-none", text.buffer is not None)
    print("text-closefd", hasattr(text, "closefd"))
    text.close()
finally:
    os.unlink(path)
