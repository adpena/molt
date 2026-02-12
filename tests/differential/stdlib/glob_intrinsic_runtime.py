from __future__ import annotations

import glob
import os
import tempfile


def _rm_tree(path: str) -> None:
    for name in os.listdir(path):
        child = os.path.join(path, name)
        if os.path.isdir(child):
            _rm_tree(child)
            os.rmdir(child)
        else:
            os.unlink(child)


root = tempfile.mkdtemp(prefix="molt_glob_intrinsic_")
try:
    with open(os.path.join(root, "alpha.py"), "w", encoding="utf-8") as handle:
        handle.write("alpha")
    with open(os.path.join(root, "beta.txt"), "w", encoding="utf-8") as handle:
        handle.write("beta")
    os.mkdir(os.path.join(root, "pkg"))
    with open(os.path.join(root, "pkg", "gamma.py"), "w", encoding="utf-8") as handle:
        handle.write("gamma")

    top_pat = os.path.join(root, "*.py")
    nested_pat = os.path.join(root, "pkg", "*.py")

    print("has_magic", glob.has_magic(top_pat))
    print(
        "top",
        sorted(os.path.basename(match) for match in glob.glob(top_pat)),
    )
    print(
        "top_i",
        sorted(os.path.basename(match) for match in glob.iglob(top_pat)),
    )
    print(
        "nested",
        sorted(os.path.relpath(match, root) for match in glob.glob(nested_pat)),
    )
finally:
    _rm_tree(root)
    os.rmdir(root)
