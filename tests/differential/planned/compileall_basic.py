# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for compileall basic."""

import os
import tempfile
import compileall

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('x = 1\n')

result = compileall.compile_file(path, quiet=1)
print(result)
print(os.path.exists(path))
