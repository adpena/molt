# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib import module basic."""

import importlib
import os
import sys
import tempfile

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod_a.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 9\n')

sys.path.insert(0, root)
try:
    mod = importlib.import_module('mod_a')
    print(mod.value)
finally:
    sys.path.pop(0)
