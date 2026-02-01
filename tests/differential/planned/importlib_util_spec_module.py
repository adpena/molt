# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib util spec module."""

import importlib.util
import os
import tempfile

root = tempfile.mkdtemp()
path = os.path.join(root, 'mod_b.py')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('value = 3
')

spec = importlib.util.spec_from_file_location('mod_b', path)
module = importlib.util.module_from_spec(spec)
print(module.__name__)
