# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-530 expect_molt_returncode=0 expect_molt_stdout_sha256=c02cf8f9e940c08b786b8af894ad84c830b764b971075d6c8bfd323d3c784369
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
