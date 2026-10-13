# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for tempfile namedtempfile basic."""

import tempfile

with tempfile.NamedTemporaryFile(delete=True) as handle:
    handle.write(b"hi")
    handle.flush()
    print(handle.name is not None)
    print(
        "raw-name", handle.file.raw.name == handle.name, handle.file.name == handle.name
    )

# Codec lookup happens after secure creation; its failure must retire both owners.
import os

with tempfile.TemporaryDirectory() as directory:
    try:
        tempfile.NamedTemporaryFile(
            mode="w", encoding="molt-invalid-codec", dir=directory
        )
    except LookupError:
        print("codec-failure-cleanup", os.listdir(directory) == [])
    with tempfile.NamedTemporaryFile(
        mode="w+", encoding="utf-8", dir=directory
    ) as handle:
        handle.write("secure café")
        handle.seek(0)
        print("text-descriptor", handle.read())
        print(
            "text-raw-name",
            handle.file.buffer.raw.name == handle.name,
            handle.file.name == handle.name,
        )
        print("fd-live", os.fstat(handle.fileno()).st_size >= 0)
    print("named-cleanup", not os.path.exists(handle.name))
