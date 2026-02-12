# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for tabnanny basic."""

import os
import tempfile
import tabnanny

root = tempfile.mkdtemp()
path = os.path.join(root, 'good.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('def f():\n    return 1\n')

result = tabnanny.check(path)
print(result is None)
