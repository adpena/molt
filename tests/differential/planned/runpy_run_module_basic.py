# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for runpy run module basic."""

import os
import sys
import tempfile
import runpy

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod_a.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 11\n')

sys.path.insert(0, root)
try:
    ns = runpy.run_module('mod_a')
    print(ns.get('value'))
finally:
    sys.path.pop(0)
