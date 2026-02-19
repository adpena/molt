# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipimport API surface helpers."""

from __future__ import annotations

import tempfile
import zipfile
import zipimport
from types import ModuleType
from pathlib import Path


with tempfile.TemporaryDirectory() as root:
    archive = Path(root) / "pkg.zip"
    with zipfile.ZipFile(archive, "w") as handle:
        handle.writestr("pkg/__init__.py", "FLAG = True\n")
        handle.writestr("pkg/mod.py", "VALUE = 42\n")
        handle.writestr("pkg/data.bin", b"\x00\x01\xff")

    importer = zipimport.zipimporter(str(archive))
    print(importer.is_package("pkg"))
    print(importer.get_filename("pkg").endswith("pkg/__init__.py"))
    source = importer.get_source("pkg")
    print(isinstance(source, str) and "FLAG = True" in source)
    spec = importer.find_spec("pkg.mod")
    print(spec is not None and str(spec.origin).endswith("pkg/mod.py"))
    code = importer.get_code("pkg.mod")
    namespace = {}
    exec(code, namespace, namespace)
    print(namespace.get("VALUE"))
    data = importer.get_data(importer.get_filename("pkg.mod"))
    print(isinstance(data, bytes) and b"VALUE = 42" in data)
    raw_blob = importer.get_data(f"{archive}/pkg/data.bin")
    print(list(raw_blob))
    importer.invalidate_caches()
    print("invalidate")
    module = ModuleType("pkg.mod")
    importer.exec_module(module)
    print(module.VALUE)
    try:
        importer.get_source("pkg.missing")
    except zipimport.ZipImportError:
        print("missing")
