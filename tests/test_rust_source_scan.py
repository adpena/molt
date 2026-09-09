from __future__ import annotations

import pytest

from molt.rust_source_scan import mask_rust_comments_and_strings, rust_comment_segments


@pytest.mark.parametrize(
    "parts",
    [
        [("fn main() { ", False), ('"// ; () {}"', True), ("; }\n", False)],
        [("a", False), ("/* outer /* inner */ ; } */", True), ("b", False)],
        [("a", False), ("// ; () }\r\n", True), ("b", False)],
        [("a ", False), ('r###"quotes "## /* ; */"###', True), (" b", False)],
        [("a ", False), ('br#"// bytes; ("#', True), (" b", False)],
        [("a ", False), ('cr#"/* c; */"#', True), (" b", False)],
        [("a ", False), ('r"raw \\"', True), (" b", False)],
        [("a ", False), ('b"escaped \\" // \n"', True), (" b", False)],
        [("a ", False), ('c"c-string; }"', True), (" b", False)],
        [("a ", False), ("';'", True), (" b ", False), ("b'/'", True)],
        [("a ", False), (r"'\u{1f980}'", True), (" b ", False), (r"'\x2f'", True)],
        [("a ", False), ("'🦀'", True), (" b", False)],
        [("fn f<'a>(x: &'a str) { 'outer: loop { break 'outer; } }", False)],
        [("let r#type = 1;", False)],
        [("a ", False), ("/* unterminated /* nested */", True)],
        [("a ", False), ('r##"unterminated "#', True)],
        [("a ", False), ('"unterminated\\', True)],
    ],
)
def test_mask_preserves_code_offsets_and_line_endings(
    parts: list[tuple[str, bool]],
) -> None:
    source = "".join(text for text, _ in parts)
    expected = "".join(
        "".join(char if char in "\r\n" else " " for char in text) if masked else text
        for text, masked in parts
    )
    actual = mask_rust_comments_and_strings(source)
    assert actual == expected
    assert len(actual) == len(source)
    assert [(index, char) for index, char in enumerate(actual) if char in "\r\n"] == [
        (index, char) for index, char in enumerate(source) if char in "\r\n"
    ]


def test_comment_projection_uses_the_same_literal_and_nested_comment_boundaries() -> (
    None
):
    source = (
        'let x = "/* hidden */\\\n// still string";\n'
        'let y = br#"// hidden\n/* hidden */"#;\n'
        "// visible\n"
        "/* outer\n/* inner */\nend */\n"
        "let c = '/'; // tail"
    )
    assert rust_comment_segments(source) == [
        (5, "// visible"),
        (6, "/* outer\n/* inner */\nend */"),
        (9, "// tail"),
    ]
    masked = mask_rust_comments_and_strings(source)
    assert "hidden" not in masked
    assert "visible" not in masked
    assert "inner" not in masked
    assert "let c" in masked


def test_abi_macro_punctuation_in_comments_and_literals_cannot_change_structure() -> (
    None
):
    source = "exc_singletons! { /* itself; ABI (nested) */ PyExc_TypeError; }\n"
    source += 'const TEXT: &str = r#"exc_singletons! { Fake; }"#;\n'
    masked = mask_rust_comments_and_strings(source)
    assert masked.count("exc_singletons!") == 1
    assert masked.count("{") == masked.count("}") == 1
    assert masked.index("PyExc_TypeError") == source.index("PyExc_TypeError")


@pytest.mark.parametrize("prefix", ["", "_", "λ", "²", "¼", "🦀", "\u0301"])
def test_literal_prefix_boundaries_preserve_unicode_identifier_rules(
    prefix: str,
) -> None:
    raw = 'br##"quoted // body"##'
    boundary = not prefix or not (prefix[-1].isalnum() or prefix[-1] == "_")
    masked_raw = (
        " " * len(raw) if boundary else "br##" + " " * len('"quoted // body"') + "##"
    )
    assert mask_rust_comments_and_strings(prefix + raw) == prefix + masked_raw
    char = "b'/'"
    assert mask_rust_comments_and_strings(prefix + char) == (
        prefix + (" " * len(char) if boundary else "b" + " " * 3)
    )


def test_span_search_skips_long_code_and_literal_runs_without_losing_delimiters() -> (
    None
):
    code = "let ordinary_identifier = other_identifier + 123; " * 1024
    comment = "/*" + " no delimiter " * 1024 + "/* inner */\r\nend */"
    literal = '"' + "ordinary text " * 1024 + r"\" // still literal" + '"'
    source = code + comment + literal + code
    masked = mask_rust_comments_and_strings(source)
    expected_noncode = "".join(
        char if char in "\r\n" else " " for char in comment + literal
    )
    assert masked == code + expected_noncode + code
    assert rust_comment_segments(source) == [(1, comment)]


def test_combined_projection_matches_single_outputs_with_one_scan(monkeypatch) -> None:
    from molt import rust_source_scan

    source = 'fn f() { r#"// hidden"#; }\r\n/* outer /* nested */ */\n// tail'
    expected_code = mask_rust_comments_and_strings(source)
    expected_comments = rust_comment_segments(source)
    scan = rust_source_scan._non_code_spans
    calls = 0

    def counted_scan(text: str):
        nonlocal calls
        calls += 1
        return scan(text)

    monkeypatch.setattr(rust_source_scan, "_non_code_spans", counted_scan)
    projection = rust_source_scan.project_rust_source(source)
    assert projection.masked_code == expected_code
    assert projection.comments == expected_comments
    assert calls == 1
