# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for plistlib basic."""

import os
import plistlib
import tempfile

payload = {'a': 1, 'b': True}
blob = plistlib.dumps(payload)
print(isinstance(blob, (bytes, bytearray)))
print(plistlib.loads(blob))

root = tempfile.mkdtemp()
path = os.path.join(root, "data.plist")
with open(path, "wb") as handle:
    plistlib.dump(payload, handle)
with open(path, "rb") as handle:
    print(plistlib.load(handle))
