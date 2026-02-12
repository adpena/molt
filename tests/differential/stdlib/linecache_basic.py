# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for linecache basic."""

import os
import tempfile
import linecache

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('a = 1\n')

line = linecache.getline(path, 1)
print(line.strip())
