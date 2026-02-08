"""Purpose: validate importlib.resources zip namespace multipath aggregation."""

import importlib.resources
import os
import sys
import tempfile
import zipfile


root = tempfile.mkdtemp(prefix="molt_resources_zip_multi_")
left_archive = os.path.join(root, "left.zip")
right_archive = os.path.join(root, "right.zip")

with zipfile.ZipFile(left_archive, "w") as zf:
    zf.writestr("nszip/pkg/left.txt", "left\n")

with zipfile.ZipFile(right_archive, "w") as zf:
    zf.writestr("nszip/pkg/right.txt", "right\n")

orig_path = list(sys.path)
orig_modules = {name: sys.modules.get(name) for name in ("nszip", "nszip.pkg")}
try:
    sys.path[:] = [left_archive, right_archive]
    traversable = importlib.resources.files("nszip.pkg")
    names = sorted(entry.name for entry in traversable.iterdir())
    left_payload = importlib.resources.read_text("nszip.pkg", "left.txt").strip()
    right_payload = importlib.resources.read_text("nszip.pkg", "right.txt").strip()
finally:
    sys.path[:] = orig_path
    for name, previous in orig_modules.items():
        if previous is None:
            sys.modules.pop(name, None)
        else:
            sys.modules[name] = previous

print("left.txt" in names)
print("right.txt" in names)
print(left_payload == "left")
print(right_payload == "right")
