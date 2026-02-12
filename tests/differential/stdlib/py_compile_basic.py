# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for py compile basic."""

import os
import tempfile
import py_compile

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('x = 1\n')

compiled = py_compile.compile(path, cfile=path + 'c')
print(os.path.basename(compiled))
print(os.path.exists(compiled))
