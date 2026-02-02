# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib util exec module."""

import importlib.util
import os
import tempfile

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod_c.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 12\n')

spec = importlib.util.spec_from_file_location('mod_c', path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print(module.value)
