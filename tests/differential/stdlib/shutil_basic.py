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

    # Exercise the actual facade with explicit text and byte search inputs.
    print("which explicit text:", shutil.which(name, path=bindir) == tool)
    print(
        "which explicit bytes:",
        shutil.which(os.fsencode(name), path=os.fsencode(bindir)) == os.fsencode(tool),
    )
    print("which absent:", shutil.which("absent-molt-which-probe", path=bindir))
    directory_name = "directory.exe" if os.name == "nt" else "directory"
    os.mkdir(os.path.join(bindir, directory_name))
    print(
        "which rejects directory:",
        shutil.which(directory_name, mode=os.F_OK, path=bindir),
    )
    if os.name != "nt":
        nonexecuting = os.path.join(bindir, "nonexecuting")
        with open(nonexecuting, "wb") as handle:
            handle.write(b"not executable")
        os.chmod(nonexecuting, 0o600)
        print(
            "which existence mode:",
            shutil.which("nonexecuting", mode=os.F_OK, path=bindir) == nonexecuting,
        )
        print("which execute mode:", shutil.which("nonexecuting", path=bindir))
    else:
        # A path-qualified command isolates PATHEXT ordering from ambient CWD
        # search while still exercising the real public environment policy.
        stem = os.path.join(bindir, "molt-which-suffix")
        for suffix in (".CMD", ".EXE"):
            with open(stem + suffix, "wb") as handle:
                handle.write(b"suffix candidate")
        old_pathext = os.environ.get("PATHEXT")
        try:
            os.environ["PATHEXT"] = ".CMD;.EXE"
            print("which PATHEXT cmd first:", shutil.which(stem) == stem + ".CMD")
            os.environ["PATHEXT"] = ".EXE;.CMD"
            print("which PATHEXT exe first:", shutil.which(stem) == stem + ".EXE")
        finally:
            if old_pathext is None:
                del os.environ["PATHEXT"]
            else:
                os.environ["PATHEXT"] = old_pathext
