# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-530 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
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
