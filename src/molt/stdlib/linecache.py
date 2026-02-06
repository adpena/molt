"""Cache lines from Python source files.

This is intended to read lines from modules imported -- hence if a filename
is not found, it will look down the module search path for a file by
that name.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())


__all__ = ["getline", "clearcache", "checkcache", "lazycache"]


# The cache. Maps filenames to either a thunk which will provide source code,
# or a tuple (size, mtime, lines, fullname) once loaded.
cache: dict[str, tuple] = {}
_interactive_cache: dict[tuple, tuple] = {}

_BOM_UTF8 = b"\xef\xbb\xbf"


def clearcache() -> None:
    """Clear the cache entirely."""
    cache.clear()


def getline(filename: str, lineno: int, module_globals: dict | None = None) -> str:
    """Get a line for a Python source file from the cache.
    Update the cache if it doesn't contain an entry for this file already."""

    lines = getlines(filename, module_globals)
    if 1 <= lineno <= len(lines):
        return lines[lineno - 1]
    return ""


def getlines(filename: str, module_globals: dict | None = None) -> list[str]:
    """Get the lines for a Python source file from the cache.
    Update the cache if it doesn't contain an entry for this file already."""

    if filename in cache:
        entry = cache[filename]
        if len(entry) != 1:
            return entry[2]

    try:
        return updatecache(filename, module_globals)
    except MemoryError:
        clearcache()
        return []
    except Exception:
        return []


def _getline_from_code(filename: str, lineno: int) -> str:
    lines = _getlines_from_code(filename)
    if 1 <= lineno <= len(lines):
        return lines[lineno - 1]
    return ""


def _make_key(code: object) -> tuple:
    co_qualname = getattr(code, "co_qualname", None)
    if co_qualname is None:
        co_qualname = getattr(code, "co_name", None)
    return (
        getattr(code, "co_filename", None),
        co_qualname,
        getattr(code, "co_firstlineno", None),
    )


def _getlines_from_code(code: object) -> list[str]:
    code_id = _make_key(code)
    if code_id in _interactive_cache:
        entry = _interactive_cache[code_id]
        if len(entry) != 1:
            return entry[2]
    return []


def _source_unavailable(filename: str) -> bool:
    """Return True if the source code is unavailable for such file name."""
    return bool(
        not filename
        or (
            filename.startswith("<")
            and filename.endswith(">")
            and not filename.startswith("<frozen ")
        )
    )


def checkcache(filename: str | None = None) -> None:
    """Discard cache entries that are out of date.
    (This is not checked upon each call!)"""

    if filename is None:
        # get keys atomically
        filenames = cache.copy().keys()
    else:
        filenames = [filename]

    for entry_name in filenames:
        try:
            entry = cache[entry_name]
        except KeyError:
            continue

        if len(entry) == 1:
            # lazy cache entry, leave it lazy.
            continue
        size, mtime, _lines, fullname = entry
        if mtime is None:
            continue  # no-op for files loaded via a __loader__
        try:
            # This import can fail if the interpreter is shutting down
            import os
        except ImportError:
            return
        stat_fn = getattr(os, "stat", None)
        if stat_fn is None:
            return
        try:
            stat = stat_fn(fullname)
        except (OSError, ValueError):
            cache.pop(entry_name, None)
            continue
        if size != stat.st_size or mtime != stat.st_mtime:
            cache.pop(entry_name, None)


def updatecache(filename: str, module_globals: dict | None = None) -> list[str]:
    """Update a cache entry and return its list of lines.
    If something's wrong, print a message, discard the cache entry,
    and return an empty list."""

    # These imports are not at top level because linecache is in the critical
    # path of the interpreter startup and importing os and sys take a lot of time
    # and slows down the startup sequence.
    try:
        import os
        import sys
    except ImportError:
        # These import can fail if the interpreter is shutting down
        return []
    stat_fn = getattr(os, "stat", None)
    if stat_fn is None:
        return _updatecache_no_stat(filename, module_globals, os, sys)

    if filename in cache:
        if len(cache[filename]) != 1:
            cache.pop(filename, None)
    if _source_unavailable(filename):
        return []

    if filename.startswith("<frozen "):
        # This is a frozen module, so we need to use the filename
        # from the module globals.
        if module_globals is None:
            return []
        fullname = module_globals.get("__file__")
        if fullname is None:
            return []
    else:
        fullname = filename

    try:
        stat = stat_fn(fullname)
    except OSError:
        basename = filename

        # Realise a lazy loader based lookup if there is one
        # otherwise try to lookup right now.
        if lazycache(filename, module_globals):
            try:
                data = cache[filename][0]()
            except (ImportError, OSError):
                pass
            else:
                if data is None:
                    # No luck, the PEP302 loader cannot find the source
                    # for this module.
                    return []
                cache[filename] = (
                    len(data),
                    None,
                    [line + "\n" for line in data.splitlines()],
                    fullname,
                )
                return cache[filename][2]

        # Try looking through the module search path, which is only useful
        # when handling a relative filename.
        if os.path.isabs(filename):
            return []

        for dirname in sys.path:
            try:
                fullname = os.path.join(dirname, basename)
            except (TypeError, AttributeError):
                # Not sufficiently string-like to do anything useful with.
                continue
            try:
                stat = stat_fn(fullname)
                break
            except (OSError, ValueError):
                pass
        else:
            return []
    except ValueError:  # may be raised by os.stat()
        return []

    try:
        handle = _open_source(fullname)
        try:
            lines = handle.readlines()
        finally:
            try:
                handle.close()
            except Exception:
                pass
    except (OSError, FileNotFoundError, UnicodeDecodeError, SyntaxError, LookupError):
        return []

    if not lines:
        lines = ["\n"]
    elif not lines[-1].endswith("\n"):
        lines[-1] += "\n"
    size, mtime = stat.st_size, stat.st_mtime
    cache[filename] = size, mtime, lines, fullname
    return lines


def _updatecache_no_stat(
    filename: str,
    module_globals: dict | None,
    os_module,
    sys_module,
) -> list[str]:
    if _source_unavailable(filename):
        return []

    if filename.startswith("<frozen "):
        if module_globals is None:
            return []
        fullname = module_globals.get("__file__")
        if fullname is None:
            return []
    else:
        fullname = filename

    result = _try_open_source(fullname)
    if result is None:
        if lazycache(filename, module_globals):
            try:
                data = cache[filename][0]()
            except (ImportError, OSError):
                data = None
            if data is not None:
                cache[filename] = (
                    len(data),
                    None,
                    [line + "\n" for line in data.splitlines()],
                    fullname,
                )
                return cache[filename][2]

    if result is None:
        if os_module.path.isabs(filename):
            return []
        for dirname in sys_module.path:
            try:
                candidate = os_module.path.join(dirname, filename)
            except (TypeError, AttributeError):
                continue
            result = _try_open_source(candidate)
            if result is not None:
                fullname = candidate
                break

    if result is None:
        return []

    _path, lines = result
    if not lines:
        lines = ["\n"]
    elif not lines[-1].endswith("\n"):
        lines[-1] += "\n"
    cache[filename] = None, None, lines, fullname
    return lines


def _try_open_source(path: str):
    try:
        import os

        if not os.path.exists(path):
            return None
    except Exception:
        return None
    try:
        handle = _open_source(path)
        try:
            lines = handle.readlines()
        finally:
            try:
                handle.close()
            except Exception:
                pass
        return path, lines
    except (OSError, FileNotFoundError, UnicodeDecodeError, SyntaxError, LookupError):
        return None


def lazycache(filename: str, module_globals: dict | None) -> bool:
    """Seed the cache for filename with module_globals.

    The module loader will be asked for the source only when getlines is
    called, not immediately.

    If there is an entry in the cache already, it is not altered.

    :return: True if a lazy load is registered in the cache,
        otherwise False. To register such a load a module loader with a
        get_source method must be found, the filename must be a cacheable
        filename, and the filename must not be already cached.
    """
    if filename in cache:
        if len(cache[filename]) == 1:
            return True
        return False
    if not filename or (filename.startswith("<") and filename.endswith(">")):
        return False
    # Try for a __loader__, if available
    if module_globals and "__name__" in module_globals:
        spec = module_globals.get("__spec__")
        name = getattr(spec, "name", None) or module_globals["__name__"]
        loader = getattr(spec, "loader", None)
        if loader is None:
            loader = module_globals.get("__loader__")
        get_source = getattr(loader, "get_source", None)

        if name and get_source:

            def get_lines(name=name, *args, **kwargs):
                return get_source(name, *args, **kwargs)

            cache[filename] = (get_lines,)
            return True
    return False


def _register_code(code: object, string: str, name: str) -> None:
    entry = (
        len(string),
        None,
        [line + "\n" for line in string.splitlines()],
        name,
    )
    stack = [code]
    while stack:
        current = stack.pop()
        for const in getattr(current, "co_consts", ()):
            if isinstance(const, type(code)):
                stack.append(const)
        _interactive_cache[_make_key(current)] = entry


def _detect_encoding(readline, filename: str | None) -> tuple[str, list[bytes]]:
    try:
        filename = readline.__self__.name
    except AttributeError:
        pass

    bom_found = False
    default = "utf-8"

    def read_or_stop() -> bytes:
        try:
            return readline()
        except StopIteration:
            return b""

    def check(line: bytes, enc: str) -> None:
        if b"\x00" in line:
            raise SyntaxError("source code cannot contain null bytes")
        if bom_found and enc.lower() != "utf-8":
            if filename is None:
                msg = "encoding problem: utf-8"
            else:
                msg = f"encoding problem for {filename!r}: utf-8"
            raise SyntaxError(msg)

    def find_cookie(line: bytes) -> str | None:
        stripped = line.lstrip(b" \t\f")
        if not stripped.startswith(b"#"):
            return None
        index = stripped.find(b"coding")
        if index < 0:
            return None
        rest = stripped[index + 6 :]
        rest = rest.lstrip(b" \t\f")
        if not rest or rest[:1] not in (b":", b"="):
            return None
        rest = rest[1:].lstrip(b" \t\f")
        if not rest:
            return None
        encoding_bytes = []
        for value in rest:
            if (
                48 <= value <= 57
                or 65 <= value <= 90
                or 97 <= value <= 122
                or value in (45, 95, 46)
            ):
                encoding_bytes.append(value)
            else:
                break
        if not encoding_bytes:
            return None
        return bytes(encoding_bytes).decode("ascii")

    def is_blank(line: bytes) -> bool:
        stripped = line.lstrip(b" \t\f")
        return not stripped or stripped.startswith((b"#", b"\r", b"\n"))

    first = read_or_stop()
    if first.startswith(_BOM_UTF8):
        bom_found = True
        first = first[3:]
        default = "utf-8-sig"
    if not first:
        return default, []

    encoding = find_cookie(first)
    if encoding:
        check(first, encoding)
        if bom_found and encoding.lower() == "utf-8":
            encoding = "utf-8-sig"
        return encoding, [first]
    if not is_blank(first):
        check(first, default)
        return default, [first]

    second = read_or_stop()
    if not second:
        check(first, default)
        return default, [first]

    encoding = find_cookie(second)
    if encoding:
        check(first + second, encoding)
        if bom_found and encoding.lower() == "utf-8":
            encoding = "utf-8-sig"
        return encoding, [first, second]

    check(first + second, default)
    return default, [first, second]


def _open_source(filename: str):
    # Keep linecache intrinsic-only by avoiding tokenize's tempfile dependency.
    return _open_with_fallback(filename)


def _open_with_fallback(filename: str):
    with open(filename, "rb") as buffer:
        encoding, _lines = _detect_encoding(buffer.readline, filename)
    return open(filename, "r", encoding=encoding)
