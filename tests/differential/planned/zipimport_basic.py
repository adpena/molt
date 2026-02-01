# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipimport basic."""

import os
import tempfile
import zipfile
import zipimport

root = tempfile.mkdtemp()
zip_path = os.path.join(root, 'pkg.zip')
with zipfile.ZipFile(zip_path, 'w') as zf:
    zf.writestr('m.py', 'value = 42')

importer = zipimport.zipimporter(zip_path)
mod = importer.load_module('m')
print(mod.value)
