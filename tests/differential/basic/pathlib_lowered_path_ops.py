# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: lock runtime-owned pathlib path-shaping intrinsics."""

from pathlib import Path
import tempfile


with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    nested = root.joinpath("alpha", "beta", "archive.tar.gz")
    print(nested.name)
    print(nested.stem)
    print(nested.suffix)
    print(nested.suffixes)
    print(nested.relative_to(root, "alpha", "beta"))

    out_dir = root.joinpath("made", "via", "parents")
    out_dir.mkdir(parents=True, exist_ok=True)
    print(out_dir.is_dir())

    print(nested.as_uri().startswith("file://"))
