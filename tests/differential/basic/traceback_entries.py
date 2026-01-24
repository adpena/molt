"""Purpose: differential coverage for traceback entries."""


def _def_line_for(func):
    code = getattr(func, "__code__", None)
    if code is None:
        return 0
    return int(getattr(code, "co_firstlineno", 0) or 0)


def _basename(path):
    if not path:
        return ""
    text = str(path)
    sep = "/" if "/" in text or "\\" not in text else "\\"
    parts = text.split(sep)
    return parts[-1] if parts else ""


def _entries_from_traceback(tb):
    entries = []
    if isinstance(tb, (tuple, list)):
        items = tb
    else:
        items = []
        while tb is not None:
            frame = tb.tb_frame
            code = frame.f_code
            items.append((code.co_filename, tb.tb_lineno, code.co_name))
            tb = tb.tb_next
    for entry in items:
        if isinstance(entry, dict):
            filename = entry.get("filename")
            lineno = entry.get("lineno")
            name = entry.get("name", "<module>")
        elif isinstance(entry, (tuple, list)) and len(entry) >= 3:
            filename, lineno, name = entry[0], entry[1], entry[2]
        else:
            continue
        try:
            lineno = int(lineno)
        except Exception:
            lineno = 0
        entries.append((str(filename), lineno, str(name)))
    return entries


def _find_entry(entries, func_name):
    for filename, lineno, name in entries:
        if name == func_name or name.split(".")[-1] == func_name:
            return filename, lineno, name
    return "<missing>", 0, "<missing>"


def boom():
    raise ValueError("boom")


def direct():
    boom()


def wrapper():
    fn = boom
    fn()


def setitem_boom():
    [].__setitem__(0, 1)


def main():
    def report(label, exc, func, func_name):
        def_line = _def_line_for(func)
        tb = exc.__traceback__
        assert not isinstance(tb, (tuple, list))
        entries = _entries_from_traceback(tb)
        filename, lineno, name = _find_entry(entries, func_name)
        raise_line = def_line + 1 if def_line else 0
        if raise_line:
            assert lineno == raise_line
        basename = _basename(filename)
        print(f"{label}:{basename}:{lineno}:{name.split('.')[-1]}")

    try:
        direct()
    except Exception as exc:
        report("direct", exc, boom, "boom")

    try:
        fn = wrapper
        fn()
    except Exception as exc:
        report("indirect", exc, boom, "boom")

    try:
        setitem_boom()
    except Exception as exc:
        report("setitem", exc, setitem_boom, "setitem_boom")


if __name__ == "__main__":
    main()
