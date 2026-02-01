# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib spec fields basic."""

import importlib.util
import os
import tempfile

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod_d.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 4
')

spec = importlib.util.spec_from_file_location('mod_d', path)
print(spec.name)
print(spec.origin is not None)
