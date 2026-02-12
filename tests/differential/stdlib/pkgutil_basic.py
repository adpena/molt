# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for pkgutil basic."""

import os
import tempfile
import pkgutil

root = tempfile.mkdtemp()
with open(os.path.join(root, 'mod_a.py'), 'w', encoding='utf-8') as handle:
    handle.write('x = 1\n')

modules = [info.name for info in pkgutil.iter_modules([root])]
print(modules)
