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
    # Path uses both serial-owned implied-directory and name-set iteration helpers.
    with zipfile.ZipFile(archive, "r") as handle:
        root_path = zipfile.Path(handle)
        names = sorted(child.name for child in root_path.iterdir())
        package = root_path.joinpath("pkg")
        assert names == ["bin", "pkg"]
        assert package.is_dir()
        assert package.joinpath("data.txt").read_text() == "hello zipfile"
        print("path collectors", names, package.name)
