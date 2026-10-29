# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-297 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: `FileFinder` honors caller-supplied loader details (HF-297).

`FileFinder(path, (Loader, suffixes))` builds a spec whose loader is
`Loader(fullname, path)` for a file with one of the suffixes, and finds
nothing for an absent name. The custom loader runs no compiled body.
"""

import importlib.machinery
import importlib.util
import os
import tempfile


class CustomLoader:
    def __init__(self, fullname, path):
        self.fullname = fullname
        self.path = path

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.loaded_from = os.path.basename(self.path)


root = tempfile.mkdtemp()
with open(os.path.join(root, "thing.custom"), "w") as handle:
    handle.write("data")
finder = importlib.machinery.FileFinder(root, (CustomLoader, [".custom"]))
spec = finder.find_spec("thing")
print(type(spec.loader).__name__, spec.loader.fullname, os.path.basename(spec.origin))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
print(module.__name__, module.loaded_from)
print(finder.find_spec("absent"))
