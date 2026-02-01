# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for filecmp basic."""

import os
import tempfile
import filecmp

root = tempfile.mkdtemp()
path_a = os.path.join(root, 'a.txt')
path_b = os.path.join(root, 'b.txt')
with open(path_a, 'w', encoding='utf-8') as handle:
    handle.write('same
')
with open(path_b, 'w', encoding='utf-8') as handle:
    handle.write('same
')

print(filecmp.cmp(path_a, path_b, shallow=False))
