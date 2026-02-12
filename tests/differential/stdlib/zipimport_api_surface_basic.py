# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipimport API surface helpers."""

from __future__ import annotations

import tempfile
import zipfile
import zipimport
from pathlib import Path


with tempfile.TemporaryDirectory() as root:
    archive = Path(root) / "pkg.zip"
    with zipfile.ZipFile(archive, "w") as handle:
        handle.writestr("pkg/__init__.py", "FLAG = True\n")
        handle.writestr("pkg/mod.py", "VALUE = 42\n")

    importer = zipimport.zipimporter(str(archive))
    print(importer.is_package("pkg"))
    print(importer.get_filename("pkg").endswith("pkg/__init__.py"))
    source = importer.get_source("pkg")
    print(isinstance(source, str) and "FLAG = True" in source)
    try:
        importer.get_source("pkg.missing")
    except zipimport.ZipImportError:
        print("missing")
