# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for zipapp basic."""

import os
import tempfile
import zipapp

root = tempfile.mkdtemp()
app_dir = os.path.join(root, 'app')
os.makedirs(app_dir, exist_ok=True)
with open(os.path.join(app_dir, '__main__.py'), 'w', encoding='utf-8') as handle:
    handle.write('print(123)')

archive = os.path.join(root, 'app.pyz')
zipapp.create_archive(app_dir, archive)
print(os.path.exists(archive))
