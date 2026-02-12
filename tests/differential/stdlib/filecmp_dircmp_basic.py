# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for filecmp dircmp basic."""

import os
import tempfile
import filecmp

root = tempfile.mkdtemp()
left = os.path.join(root, 'left')
right = os.path.join(root, 'right')
os.makedirs(left, exist_ok=True)
os.makedirs(right, exist_ok=True)

with open(os.path.join(left, 'a.txt'), 'w', encoding='utf-8') as handle:
    handle.write('same
')
with open(os.path.join(right, 'a.txt'), 'w', encoding='utf-8') as handle:
    handle.write('same
')

cmp = filecmp.dircmp(left, right)
print(sorted(cmp.common))
