# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for fileinput basic."""

import os
import tempfile
import fileinput

root = tempfile.mkdtemp()
path = os.path.join(root, 'data.txt')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('a
')
    handle.write('b
')

lines = [line.strip() for line in fileinput.input([path])]
print(lines)
