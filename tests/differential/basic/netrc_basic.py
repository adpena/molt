# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,env.write
"""Purpose: differential coverage for netrc basic."""

import os
import tempfile
import netrc

root = tempfile.mkdtemp()
path = os.path.join(root, 'netrc')
with open(path, 'w', encoding='utf-8') as handle:
    handle.write('machine example.com login user password pass\n')

os.environ['NETRC'] = path

auth = netrc.netrc().authenticators('example.com')
print(auth[0], auth[2])
