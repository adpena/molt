# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for runpy basic."""

import os
import tempfile
import runpy

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 7
')

ns = runpy.run_path(path)
print(ns.get('value'))
