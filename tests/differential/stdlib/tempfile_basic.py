# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for tempfile basics."""

import os
import tempfile


def main():
    selected = tempfile.gettempdir()
    print("default-cache", tempfile.tempdir == selected)
    print("default-bytes", tempfile.gettempdirb() == os.fsencode(selected))
    with tempfile.TemporaryDirectory() as tmp:
        previous = tempfile.tempdir
        tempfile.tempdir = os.fsencode(tmp)
        try:
            selected_dir = tempfile.mkdtemp()
            print(
                "override-bytes",
                isinstance(selected_dir, bytes),
                selected_dir.startswith(os.fsencode(tmp) + os.fsencode(os.sep)),
            )
            os.rmdir(selected_dir)
            with tempfile.TemporaryDirectory() as nested:
                print(
                    "nested-default",
                    isinstance(nested, bytes),
                    nested.startswith(os.fsencode(tmp) + os.fsencode(os.sep)),
                )
            print("nested-cleanup", not os.path.exists(nested))
            try:
                tempfile.mkdtemp(prefix=b"bytes", dir=tmp)
            except TypeError:
                print("mixed-path", True)
        finally:
            tempfile.tempdir = previous
        selected = os.path.join(tmp, "selected")
        os.mkdir(selected)
        dotted = selected + os.sep + ".." + os.sep + "selected" + os.sep + "."
        for directory in (dotted, os.fsencode(dotted)):
            created = tempfile.mkdtemp(dir=directory)
            expected = (
                os.fsencode(selected) if isinstance(directory, bytes) else selected
            )
            separator = os.fsencode(os.sep) if isinstance(directory, bytes) else os.sep
            print(
                "normalized-dir",
                created.startswith(expected + separator),
                type(created) is type(directory),
            )
            os.rmdir(created)
        path = os.path.join(tmp, "sample.txt")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("hi")
        print("exists", os.path.exists(path))

        with tempfile.NamedTemporaryFile(dir=tmp, delete=False) as handle:
            name = handle.name
            handle.write(b"x")
        print("named", os.path.exists(name))

    print("cleanup", not os.path.exists(tmp))


if __name__ == "__main__":
    main()
