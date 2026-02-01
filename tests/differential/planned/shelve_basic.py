# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for shelve basic."""

import os
import tempfile
import shelve

root = tempfile.mkdtemp()
path = os.path.join(root, 'db')
with shelve.open(path) as db:
    db['x'] = 5

with shelve.open(path) as db:
    print(db['x'])
