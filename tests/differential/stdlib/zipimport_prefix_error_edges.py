# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: exercise zipimport prefix handling and error-message shape edges."""

from __future__ import annotations

from pathlib import Path
import tempfile
import zipfile
import zipimport

with tempfile.TemporaryDirectory() as root:
    archive = Path(root) / "pkg.zip"
    with zipfile.ZipFile(archive, "w") as handle:
        handle.writestr("nested/m.py", "X = 7\n")

    importer = zipimport.zipimporter(f"{archive}/nested")
    print(importer.prefix == "nested/")
    print(importer.get_filename("m").endswith("nested/m.py"))
    print(importer.get_data("nested/m.py").strip() == b"X = 7")

    try:
        importer.get_data("m.py")
    except OSError as exc:
        text = str(exc)
        print(isinstance(exc, OSError))
        print(("No such" in text) or ("not found" in text) or ("can't" in text))

    try:
        importer.load_module("missing")
    except zipimport.ZipImportError as exc:
        print("can't find module" in str(exc))

    print(importer.find_spec("missing") is None)

try:
    zipimport.zipimporter("")
except zipimport.ZipImportError as exc:
    print(isinstance(exc, zipimport.ZipImportError))
