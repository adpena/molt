# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipfile write/read roundtrip."""

from __future__ import annotations

import tempfile
import zipfile
from pathlib import Path


with tempfile.TemporaryDirectory() as root:
    archive = Path(root) / "bundle.zip"
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as handle:
        handle.writestr("pkg/data.txt", "hello zipfile")
        handle.writestr("bin/raw.bin", b"\x00\x01\x02")

    with zipfile.ZipFile(archive, "r") as handle:
        print(handle.namelist())
        print(handle.read("pkg/data.txt").decode("utf-8"))
        print(list(handle.read("bin/raw.bin")))
