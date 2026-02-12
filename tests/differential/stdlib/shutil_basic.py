# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read,env.write
"""Purpose: differential coverage for shutil basics."""

import os
import shutil
import tempfile


with tempfile.TemporaryDirectory() as root:
    src = os.path.join(root, "src.txt")
    dst = os.path.join(root, "dst.txt")
    with open(src, "w", encoding="utf-8") as handle:
        handle.write("data")

    shutil.copyfile(src, dst)
    with open(dst, "r", encoding="utf-8") as handle:
        print(handle.read())

    bindir = os.path.join(root, "bin")
    os.mkdir(bindir)
    name = "tool.exe" if os.name == "nt" else "tool"
    tool = os.path.join(bindir, name)
    with open(tool, "w", encoding="utf-8") as handle:
        handle.write("echo hi")
    if os.name != "nt":
        os.chmod(tool, 0o755)

    old_path = os.environ.get("PATH", "")
    os.environ["PATH"] = bindir + os.pathsep + old_path
    found = shutil.which(name)
    print(os.path.basename(found) if found else None)
