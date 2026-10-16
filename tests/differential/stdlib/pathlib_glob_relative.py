# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for pathlib glob relative."""

from pathlib import Path
import os
import sys
import tempfile


tmpdir = Path(tempfile.gettempdir()) / "molt_pathlib"
tmpdir.mkdir(exist_ok=True)

(tmpdir / "a.txt").write_text("a")
(tmpdir / "b.log").write_text("b")

files = sorted(p.name for p in tmpdir.glob("*.txt"))
print(files)

print((tmpdir / "a.txt").relative_to(tmpdir))

for child in tmpdir.iterdir():
    child.unlink()
tmpdir.rmdir()

# Both selectors share traversal but retain their distinct empty/root rules.
with tempfile.TemporaryDirectory(prefix="molt_pathlib_selector_") as root:
    base = Path(root)
    (base / "nested").mkdir()
    (base / "nested" / "leaf.txt").write_text(
        "caf\xe9\n", encoding="latin-1", newline="\r\n"
    )
    (base / ".hidden").write_text("hidden", encoding="utf-8")
    print(
        "selected-encoding",
        (base / "nested" / "leaf.txt").read_text(encoding="latin-1"),
    )
    print(
        "parent-selector",
        sorted(str(p.relative_to(base)) for p in (base / "nested").glob("../.hidden")),
    )
    print(
        "recursive-parent",
        sorted(str(p.relative_to(base)) for p in base.rglob("../.hidden")),
    )
    print("recursive-root", base in list(base.glob("**")))
    print("recursive-empty", sorted(str(p.relative_to(base)) for p in base.rglob("")))
    print(
        "trailing-directory", sorted(str(p.relative_to(base)) for p in base.glob("*/"))
    )
    print("recursive-unique", len(list(base.rglob("**"))) == len(set(base.rglob("**"))))
    for method, pattern in (
        ("glob", ""),
        ("glob", "/"),
        ("rglob", "/"),
        ("rglob", "a**b"),
    ):
        try:
            list(getattr(base, method)(pattern))
        except Exception as error:
            print("pattern-refusal", method, pattern, type(error).__name__)

    class Override(Path):
        def glob(self, pattern, **kwargs):
            raise RuntimeError("rglob must not dispatch through this override")

    try:
        print("rglob-override", len(list(Override(root).rglob("*.txt"))))
    except RuntimeError as error:
        print("rglob-override", type(error).__name__)

# CPython 3.12-3.14 rejects drive OR root before selector validation and I/O.
# A nonexistent parent prevents accidental directory contents from being the oracle.
with tempfile.TemporaryDirectory(prefix="molt_glob_anchor_") as root:
    missing = Path(root) / "missing"
    anchored = ["/", "//server/share", "/a**b"]
    windows_shapes = ["\\leaf", "C:leaf", "C:/leaf", "é:leaf", "😀:leaf"]
    if os.name == "nt":
        anchored.extend(windows_shapes[:-1])
        if sys.version_info < (3, 13):
            anchored.append(windows_shapes[-1])
    for method in ("glob", "rglob"):
        for pattern in anchored:
            try:
                list(getattr(missing, method)(pattern))
            except NotImplementedError as error:
                assert str(error) == "Non-relative patterns are unsupported"
            else:
                raise AssertionError((method, pattern, "accepted non-relative pattern"))
        # On POSIX those spellings are relative filenames, not Windows anchors.
        relative = ["leaf", "sub/leaf"]
        if os.name != "nt":
            relative.extend(windows_shapes)
        elif sys.version_info >= (3, 13):
            relative.append(windows_shapes[-1])
        for pattern in relative:
            assert list(getattr(missing, method)(pattern)) == []
        print("anchor-contract", method, "ok")

# The 3.12 tail lookup is observable, after anchor rejection but before selector
# validation and filesystem traversal. Modern rglob deliberately dispatches glob.
if sys.version_info < (3, 13):
    with tempfile.TemporaryDirectory(prefix="molt_glob_callbacks_") as root:
        base = Path(root)
        (base / "directory").mkdir()
        (base / "file").write_text("x")
        events = []

        class PathPattern:
            def __init__(self, value):
                self.value = value

            def __fspath__(self):
                events.append("fspath")
                return self.value

            def replace(self, old, new):
                events.append("replace")
                return self.value.replace(old, new)

            def __getitem__(self, key):
                assert key == -1
                events.append("tail")
                raise LookupError("tail sentinel")

        for method in ("glob", "rglob"):
            events.clear()
            try:
                list(getattr(base, method)(PathPattern("/a**b")))
            except NotImplementedError as error:
                assert str(error) == "Non-relative patterns are unsupported"
            else:
                raise AssertionError("anchored PathLike accepted")
            assert events == (["replace"] if os.name == "nt" else ["fspath"])
            events.clear()
            try:
                list(getattr(base, method)(PathPattern("a**b")))
            except LookupError as error:
                assert str(error) == "tail sentinel"
            else:
                raise AssertionError("selector validation preceded original tail callback")
            assert events == (["replace", "tail"] if os.name == "nt" else ["fspath", "tail"])
            print("pathlike-anchor-order", method, "ok")

        class Tail:
            def __eq__(self, other):
                events.append(("eq", other))
                return other == os.altsep

        class Pattern(str):
            def __getitem__(self, key):
                if key == -1:
                    events.append("tail")
                    return Tail()
                return super().__getitem__(key)

        events.clear()
        selected = sorted(p.name for p in base.glob(Pattern("*")))
        assert selected == ["directory"]
        assert events == ["tail", ("eq", os.sep), ("eq", os.altsep)]
        print("subclass-tail-equality", selected)

        class CreateDuringTail(str):
            def __getitem__(self, key):
                if key == -1:
                    (base / "created").write_text("x")
                return super().__getitem__(key)

        assert "created" in [p.name for p in base.glob(CreateDuringTail("*"))]
        print("tail-before-traversal", "ok")

# Independent drive/root coordinates: Python code points before 3.13, the native
# Windows UTF-16 provider thereafter. PureWindowsPath on POSIX uses Python's
# ntpath implementation, even when the Python version is modern.
from pathlib import PureWindowsPath, PurePosixPath

assert PurePosixPath("//server/share").anchor == "//"
for spelling, drive in [("é:leaf", "é:"), ("C:/leaf", "C:")]:
    path = PureWindowsPath(spelling)
    assert path.drive == drive
    assert path.parts[-1] == "leaf"
modern_windows = os.name == "nt" and sys.version_info >= (3, 13)
path = PureWindowsPath("😀:leaf")
assert path.drive == ("" if modern_windows else "😀:")
assert path.parts == (("😀:leaf",) if modern_windows else ("😀:", "leaf"))
if os.name == "nt":
    # These exercise the shared provider through actual public consumers. No
    # filesystem entry is needed for lexical join/commonpath/escape.
    import glob
    assert os.path.join("base", "😀:leaf") == ("base\\😀:leaf" if modern_windows else "😀:leaf")
    assert os.path.commonpath(["😀:one", "😀:two"]) == ("" if modern_windows else "😀:")
    assert glob.escape("😀:a*b") == "😀:a[*]b"
    if modern_windows:
        # CPython's bytes provider decodes/re-encodes through the frozen FS codec.
        # Compare independently with explicit Unicode inputs for lexical roots;
        # preserve byte matching/escaping after the root has been determined.
        encoded = os.fsencode("é:leaf")
        assert os.path.join(b"base", encoded) == os.fsencode("é:leaf")
        assert os.path.commonpath([encoded, os.fsencode("é:other")]) == os.fsencode("é:")
        assert glob.escape(os.fsencode("é:a*b")) == os.fsencode("é:a[*]b")
print("root-coordinate-contract", "ok")

# 3.13 retains raw str-subclass tail callbacks; 3.14's native splitroot result
# is an exact str, so neither an original replace nor index override is used.
with tempfile.TemporaryDirectory(prefix="molt_glob_raw_tail_") as root:
    base = Path(root)
    (base / "leaf").write_text("x")
    events = []

    class RawPattern(str):
        def replace(self, old, new):
            events.append("replace")
            return super().replace(old, new)

        def __getitem__(self, key):
            if key == -1:
                events.append("tail")
                raise LookupError("raw tail")
            return super().__getitem__(key)

    if sys.version_info < (3, 14):
        try:
            list(base.glob(RawPattern("*")))
        except LookupError as error:
            assert str(error) == "raw tail"
        else:
            raise AssertionError("raw pattern tail callback was discarded")
        assert events == (["replace", "tail"] if os.name == "nt" else ["tail"])
    else:
        assert [path.name for path in base.glob(RawPattern("*"))] == ["leaf"]
        assert events == []
    events.clear()
    try:
        list(base.glob(RawPattern(".")))
    except (IndexError, ValueError):
        pass
    except LookupError:
        assert sys.version_info < (3, 13)
    else:
        raise AssertionError("empty parsed pattern accepted")
    if sys.version_info >= (3, 13):
        assert "tail" not in events
    print("versioned-raw-tail", "ok")

assert PureWindowsPath("//server/share").anchor == "\\\\server\\share\\"
assert PureWindowsPath("//server/share").parts == ("\\\\server\\share\\",)
assert PureWindowsPath("//?/UNC/server/share").anchor == "\\\\?\\UNC\\server\\share\\"
assert PurePosixPath("//server/share").parts == ("//", "server", "share")
print("root-projection-contract", "ok")

# Bytes are an error-order input, never a successful glob pattern. In 3.14
# glob, native splitroot rejects anchors before the str-separated tail fails.
# Earlier glob and every rglob reject bytes before that anchor outcome.
with tempfile.TemporaryDirectory(prefix="molt_glob_bytes_") as root:
    missing = Path(root) / "missing"
    for method in ("glob", "rglob"):
        for raw in (b"/", b"leaf"):
            for wrapped in (False, True):
                events = []

                class BytesPattern:
                    def __fspath__(self):
                        events.append("fspath")
                        return raw

                    def replace(self, old, new):
                        events.append("replace")
                        return raw

                    def __getitem__(self, key):
                        events.append("tail")
                        raise AssertionError("bytes pattern reached tail callback")

                pattern = BytesPattern() if wrapped else raw
                expected = (
                    NotImplementedError
                    if sys.version_info >= (3, 14) and method == "glob" and raw == b"/"
                    else TypeError
                )
                try:
                    list(getattr(missing, method)(pattern))
                except expected as error:
                    if expected is NotImplementedError:
                        assert str(error) == "Non-relative patterns are unsupported"
                else:
                    raise AssertionError((method, raw, wrapped, "accepted bytes pattern"))
                assert "tail" not in events
                assert events == (
                    ["replace"]
                    if wrapped and os.name == "nt" and sys.version_info < (3, 13)
                    else ["fspath"] if wrapped else []
                ), (method, raw, wrapped, events)
    print("bytes-pattern-error-order", "ok")

# Pattern validation precedes base filesystem encoding, including invalid bases.
for method in ("glob", "rglob"):
    try:
        list(getattr(Path("\ud800"), method)("/"))
    except NotImplementedError as error:
        assert str(error) == "Non-relative patterns are unsupported"
    else:
        raise AssertionError((method, "base encoding preceded anchor refusal"))
print("pattern-before-base-encoding", "ok")

# 3.13 PurePath parsing observes converted-str truth even on POSIX; 3.14
# native splitroot deliberately does not invoke this subclass callback.
class TruthPattern(str):
    def __bool__(self):
        raise LookupError("pattern truth")

for method in ("glob", "rglob"):
    expected = LookupError if sys.version_info < (3, 14) else NotImplementedError
    try:
        list(getattr(Path("missing"), method)(TruthPattern("/")))
    except expected:
        pass
    else:
        raise AssertionError((method, "pattern truth phase"))
print("pattern-truth-phase", "ok")

# commonpath first resolves every PathLike, then starts native filesystem
# decoding. These inputs distinguish a per-item decode loop and eager global
# mixed-type prechecks from CPython's ordered provider phase.
if os.name == "nt" and sys.version_info >= (3, 13):
    if sys.getfilesystemencoding() == "utf-8" and sys.getfilesystemencodeerrors() == "surrogatepass":
        for first, later, expected in (
            (b"\xff:one", b"C:two", UnicodeDecodeError),
            ("C:one", b"\xff:two", TypeError),
            (b"\xff:one", "C:two", UnicodeDecodeError),
        ):
            events = []

            class LaterPath:
                def __fspath__(self):
                    events.append("later-fspath")
                    return later

            try:
                os.path.commonpath([first, LaterPath()])
            except expected:
                pass
            else:
                raise AssertionError((first, later, "commonpath phase"))
            assert events == ["later-fspath"]
        print("commonpath-fspath-before-native-decode", "ok")
    else:
        print("commonpath-fspath-before-native-decode", "different-filesystem-codec")

# Legacy root parsing consumes subclass results, rather than merely invoking
# callbacks and reparsing the original spelling. 3.13+ imports native splitroot
# on both POSIX and Windows; its Python fallback is not the selected provider.
legacy_root = sys.version_info < (3, 13)
if legacy_root:
    events = []

    class SlicePattern(str):
        def replace(self, old, new):
            return self

        def __getitem__(self, key):
            if isinstance(key, slice):
                events.append("slice")
                raise LookupError("root slice")
            return super().__getitem__(key)

    try:
        list(Path("missing").glob(SlicePattern("/leaf")))
    except LookupError as error:
        assert str(error) == "root slice"
    else:
        raise AssertionError("raw anchor parse bypassed legacy slice")
    assert events == ["slice"]

    with tempfile.TemporaryDirectory(prefix="molt_legacy_glob_parts_") as root:
        base = Path(root)
        (base / "selected.txt").write_text("selected")
        (base / "ignored.txt").write_text("ignored")
        events = []

        class Part:
            def __str__(self):
                events.append("part-str")
                return "selected.txt"

        class SplitPattern(str):
            def replace(self, old, new):
                return self

            def split(self, separator):
                events.append("split")
                return [Part()]

        assert [p.name for p in base.glob(SplitPattern("ignored.txt"))] == ["selected.txt"]
        assert events == ["split", "part-str"]
    print("legacy-produced-root-and-parts", "ok")

# 3.13 rglob joins before parsing: a relative subclass becomes an exact joined
# string and must not receive the later raw-pattern truth callback.
if (3, 13) <= sys.version_info < (3, 14):
    assert list(Path("/__molt_absent_root__").rglob(TruthPattern("leaf"))) == []
    print("rglob-join-before-pattern-truth", "ok")

# A legacy root callback can change classification, not only throw an error.
# This distinguishes consuming its result from reparsing original bytes.
if sys.version_info < (3, 13) and os.name != "nt":
    class RootPattern(str):
        def __getitem__(self, key):
            if key == slice(None, 1):
                return "/"
            return super().__getitem__(key)

    try:
        list(Path("missing").glob(RootPattern("relative")))
    except NotImplementedError as error:
        assert str(error) == "Non-relative patterns are unsupported"
    else:
        raise AssertionError("callback-produced root was discarded")
    print("legacy-produced-root-classification", "ok")

# Windows 3.13 normalization can return a PathLike; the selected native provider
# applies fspath to that result, then its bytes tail still fails before anchoring.
if os.name == "nt" and (3, 13) <= sys.version_info < (3, 14):
    for raw in ("/anchor", b"/anchor"):
        events = []

        class ReplacedPattern:
            def __fspath__(self):
                events.append("fspath")
                return raw

        class ReplacingPattern(str):
            def replace(self, old, new):
                events.append("replace")
                return ReplacedPattern()

        expected = TypeError if isinstance(raw, bytes) else NotImplementedError
        try:
            list(Path("missing").glob(ReplacingPattern("/anchor")))
        except expected:
            pass
        else:
            raise AssertionError("replace result bypassed native fspath")
        assert events == ["replace", "fspath"]
    print("native-provider-replace-result", "ok")

# Selected roots must preserve canonical spelling and the versioned constructor
# contract. An empty directory isolates result projection from traversal timing.
with tempfile.TemporaryDirectory(prefix="molt_path_factory_") as root:
    events = []

    class SelectedPath(Path):
        def __init__(self, *parts, token="selected"):
            super().__init__(*parts)
            self.token = token

        def with_segments(self, *parts):
            events.append(len(parts))
            return type(self)(*parts, token=self.token)

        def joinpath(self, *parts):
            raise AssertionError("selector dispatched through joinpath")

    for raw in (root + os.sep + "." + os.sep, os.path.relpath(root) + os.sep):
        base = SelectedPath(raw)
        for method, pattern in (("glob", "**"), ("rglob", "")):
            events.clear()
            selected = list(getattr(base, method)(pattern))
            assert len(selected) == 1
            result = selected[0]
            print(
                "selected-root-factory", method, result == base,
                str(result) == str(base), hash(result) == hash(base),
                result is base, type(result) is SelectedPath, result.token, events,
            )
