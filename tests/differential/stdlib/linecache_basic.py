# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for linecache basic."""

import os
import sys
import tempfile
import linecache

root = tempfile.mkdtemp()
name = 'molt_linecache_join_fixture.py'
path = os.path.join(root, name)
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('a = 1\n')

line = linecache.getline(path, 1)
print(line.strip())
sys.path.insert(0, root)
try:
    # A relative source name exercises linecache's public join consumer.
    print(linecache.getline(name, 1).strip())
finally:
    sys.path.pop(0)
    os.unlink(path)
    os.rmdir(root)
