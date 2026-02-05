# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipimport basic."""

import os
import tempfile
import zipfile
import zipimport

root = tempfile.mkdtemp()
zip_path = os.path.join(root, 'pkg.zip')
with zipfile.ZipFile(zip_path, 'w', compression=zipfile.ZIP_DEFLATED) as zf:
    zf.writestr('pkg/__init__.py', 'value = 1')
    zf.writestr('pkg/mod.py', 'value = 2')
    zf.writestr('nested/m.py', 'value = 3')

importer = zipimport.zipimporter(zip_path)
pkg = importer.load_module('pkg')
mod = importer.load_module('pkg.mod')
print(pkg.value)
print(mod.value)

subimporter = zipimport.zipimporter(f'{zip_path}/nested')
submod = subimporter.load_module('m')
print(submod.value)
