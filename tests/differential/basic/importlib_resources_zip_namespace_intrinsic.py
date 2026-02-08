"""Purpose: validate importlib.resources zip namespace/resource intrinsic parity."""

import importlib.resources
import os
import sys
import tempfile
import zipfile


root = tempfile.mkdtemp(prefix="molt_resources_zip_ns_")
archive = os.path.join(root, "mods.zip")
with zipfile.ZipFile(archive, "w") as zf:
    zf.writestr("nszip/pkg/data.txt", "payload\n")
    zf.writestr("nszip/pkg/sub/item.txt", "inner\n")

orig_path = list(sys.path)
try:
    sys.path[:] = [archive]
    traversable = importlib.resources.files("nszip.pkg")
    names = sorted(entry.name for entry in traversable.iterdir())
    payload = importlib.resources.read_text("nszip.pkg", "data.txt").strip()
    is_file = importlib.resources.is_resource("nszip.pkg", "data.txt")
finally:
    sys.path[:] = orig_path

print(traversable.is_dir())
print("data.txt" in names)
print("sub" in names)
print(payload == "payload")
print(is_file)
