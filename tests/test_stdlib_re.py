from __future__ import annotations

import re as cpython_re
import weakref

import pytest

from tests.stdlib_intrinsic_registry import intrinsic_registry


@pytest.fixture
def source_re():
    compiled: dict[int, cpython_re.Pattern] = {}
    next_handle = 1

    def compile_pattern(pattern, flags):
        nonlocal next_handle
        try:
            compiled_pattern = cpython_re.compile(pattern, flags)
        except cpython_re.error as exc:
            return None, None, str(exc)
        handle = next_handle
        next_handle += 1
        compiled[handle] = compiled_pattern
        return handle

    def pattern_info(handle):
        pattern = compiled[handle]
        return pattern.groups, pattern.groupindex, pattern.flags, None

    def result(match):
        if match is None:
            return None
        spans = tuple(
            match.span(i) if match.start(i) >= 0 else None
            for i in range(1, match.re.groups + 1)
        )
        return match.start(), match.end(), spans

    def execute(handle, subject, pos, end, mode):
        pattern = compiled[handle]
        return result(getattr(pattern, mode)(subject, pos, end))

    class Scanner:
        def __init__(self, handle, subject, pos, end):
            self.matches = iter(compiled[handle].finditer(subject, pos, end))

    def scanner_new(handle, subject, pos, end):
        return Scanner(handle, subject, pos, end)

    def scanner_next(scanner, _subject):
        return result(next(scanner.matches, None))

    def split(handle, subject, maxsplit):
        return compiled[handle].split(subject, maxsplit)

    def sub(handle, repl, subject, count):
        try:
            return compiled[handle].subn(repl, subject, count)
        except cpython_re.error as exc:
            return None, None, str(exc)

    def group_values(subject, spans):
        return tuple(
            None if span is None else _physical_slice(subject, span[0], span[1])
            for span in spans
        )

    def expand_replacement(repl, values, names):
        for pattern in compiled.values():
            if pattern.groupindex != names:
                continue
            match = pattern.match(values[0])
            if match is None:
                continue
            actual = (match.group(0),) + match.groups()
            if actual == values:
                try:
                    return match.expand(repl)
                except cpython_re.error as exc:
                    return None, None, str(exc)
        raise AssertionError(
            "source fixture could not resolve the host CPython oracle match"
        )

    def _physical_slice(subject, start, end):
        if isinstance(subject, str):
            return str.__getitem__(subject, slice(start, end))
        if type(subject) is bytes:
            return bytes.__getitem__(subject, slice(start, end))
        return memoryview(subject)[start:end].tobytes()

    def subject_snapshot(value):
        if isinstance(value, str) or type(value) is bytes:
            return value
        return memoryview(value).tobytes()

    behavior = {
        "molt_re_compile": compile_pattern,
        "molt_re_pattern_info": pattern_info,
        "molt_re_execute": execute,
        "molt_re_scanner_new": scanner_new,
        "molt_re_finditer_next": scanner_next,
        "molt_re_split": split,
        "molt_re_sub": sub,
        "molt_re_group_values": group_values,
        "molt_re_expand_replacement": expand_replacement,
        "molt_re_subject_snapshot": subject_snapshot,
        "molt_memoryview_from_simple_buffer": lambda value: memoryview(value),
        "molt_re_strip_verbose": lambda pattern, _flags: pattern,
    }
    with intrinsic_registry(behavior):
        from molt.stdlib import re as module

        yield module


def test_bytes_pattern_subject_admission_and_live_match_string(source_re):
    module = source_re
    pattern = module.compile(b"(?P<hex>[0-9A-F]+)")
    subject = bytearray(b"xxA1!")
    match = pattern.search(subject)
    assert match is not None
    assert match.string is subject
    assert match.group() == b"A1"
    assert match.group("hex") == b"A1"
    subject[2:4] = b"B2"
    assert match.group() == b"B2"
    assert pattern.findall(memoryview(b"A1,FF")) == [b"A1", b"FF"]


def test_bytes_pattern_flag_and_mixed_subject_errors_match_cpython(source_re):
    module = source_re
    with pytest.raises(ValueError, match="UNICODE flag with a bytes pattern"):
        module.compile(b"a", module.UNICODE)
    with pytest.raises(TypeError, match="bytes pattern on a string-like object"):
        module.search(b"a", "a")
    with pytest.raises(TypeError, match="string pattern on a bytes-like object"):
        module.search("a", b"a")
    assert module.search(b"\\w+", b"\xffA").group() == b"A"
    assert module.compile(b"a") is module.compile(b"a")
    compiled = module.compile(b"a")
    assert compiled.pattern == b"a"
    assert compiled.flags == cpython_re.compile(b"a").flags
    with pytest.raises(AttributeError):
        compiled.flags = 0
    named = module.compile(b"(?P<letter>a)")
    with pytest.raises(TypeError):
        named.groupindex["other"] = 1
    assert weakref.ref(named)() is named
    assert module.search(b"\xc4", b"\xe4", module.IGNORECASE) is None


def test_bytes_split_sub_expand_escape_and_callable_results(source_re):
    module = source_re
    pattern = module.compile(b"(?P<part>[A-Z]+)")
    assert pattern.split(b"1AB2CD3") == [b"1", b"AB", b"2", b"CD", b"3"]
    assert pattern.subn(b"<\\g<part>>", b"xAB") == (b"x<AB>", 1)
    match = pattern.search(b"AB")
    assert match is not None
    assert match.expand(b"[\\g<part>]") == b"[AB]"
    assert module.escape(b"a b-c#~&!") == cpython_re.escape(b"a b-c#~&!")
    assert (
        pattern.sub(lambda current: current.group("part").lower(), b"XABY") == b"xaby"
    )
    assert (
        pattern.sub(lambda current: current.group("part").lower(), bytearray(b"XABY"))
        == b"xaby"
    )


def test_replacement_errors_keep_cpython_error_classes(source_re):
    module = source_re
    with pytest.raises(module.error, match="unterminated character set"):
        module.compile(b"[")
    match = module.compile(b"(a)").match(b"a")
    assert match is not None
    with pytest.raises(module.error, match="bad escape"):
        match.expand(b"\\x")
    with pytest.raises(IndexError, match="unknown group name"):
        match.expand(b"\\g<nope>")
    with pytest.raises(module.error, match="invalid group reference"):
        module.sub(b"(a)", b"\\99", b"a")


def test_text_match_expand_and_subject_subclass_hooks_are_bypassed(source_re):
    class TrappingStr(str):
        def __len__(self):
            raise AssertionError("__len__ must not be called")

        def __getitem__(self, key):
            raise AssertionError("__getitem__ must not be called")

    pattern = source_re.compile("(?P<part>a)")
    match = pattern.search(TrappingStr("ba"))
    assert match is not None
    assert match.span() == (1, 2)
    assert match.expand("[\\g<part>]") == "[a]"


def test_bytes_subclass_compile_subject_replacement_and_expand_authority(source_re):
    class ExportingBytes(bytes):
        def __new__(cls, raw, exported):
            value = super().__new__(cls, raw)
            value.exported = exported
            value.buffer_calls = 0
            return value

        def __buffer__(self, flags):
            self.buffer_calls += 1
            return memoryview(self.exported)

    syntax = ExportingBytes(b"A", b"[")
    compiled = source_re.compile(syntax)
    assert compiled.fullmatch(b"A") is not None
    assert cpython_re.compile(ExportingBytes(b"A", b"[")).fullmatch(b"A") is not None

    subject = ExportingBytes(b"A", b"Z")
    assert source_re.search(b"Z", subject).group() == b"Z"
    assert cpython_re.search(b"Z", ExportingBytes(b"A", b"Z")).group() == b"Z"

    replacement = ExportingBytes(b"X", b"Z")
    assert source_re.sub(b"A", replacement, b"A") == b"Z"
    assert cpython_re.sub(b"A", ExportingBytes(b"X", b"Z"), b"A") == b"Z"

    match = source_re.compile(b"(A)").match(b"A")
    assert match is not None
    template = ExportingBytes(b"\\1", b"not a template")
    calls = template.buffer_calls
    assert match.expand(template) == b"A"
    assert template.buffer_calls == calls
