from __future__ import annotations

import pytest

from molt.rust_source_scan import mask_rust_comments_and_strings, rust_comment_segments


@pytest.mark.parametrize(
    "literal",
    [
        '"ordinary, => { pattern }"',
        r'"escaped \" quote and \\ slash"',
        'r###"raw "## quote, => { /* text */ }"###',
        'br#"raw bytes, => { // text }"#',
        'cr##"raw C \\ "# quote"##',
        'r"raw\\"',
        'b"byte string"',
        'c"C string"',
        "'\"'",
        r"'\u{1f980}'",
        "b'}'",
    ],
)
def test_rust_source_tokens_keep_every_literal_as_one_verbatim_atom(literal):
    from molt.rust_source_scan import rust_source_tokens, rust_token_range

    source = "/* opening */ let item: &'a str = " + literal + "; // closing\r\n"
    tokens = rust_source_tokens(source)
    values = [token.text for token in tokens]
    assert values == ["let", "item", ":", "&", "'", "a", "str", "=", literal, ";"]
    assert all(source[token.start : token.end] == token.text for token in tokens)
    start = source.index(literal)
    assert rust_token_range(source, literal) == (start, start + len(literal))


def test_rust_match_arms_separate_literal_trivia_and_structural_punctuation():
    from molt.rust_source_scan import rust_literal_pattern_names, rust_match_arms

    source = (
        "match op.kind.as_str() {\r\n"
        ' /* pre */ "live" /* => { } */ | "_alias" => /* body */ "literal, => {}",\r\n'
        ' r##"raw " => {}"## => r#"RHS, /* content */ { }"#,\n'
        ' "block" => { self.emit("}, =>"); }\n'
        ' kind if pred("=>") => self.guarded(op),\n'
        " _ => self.fallback(op),\n}"
    )
    arms = rust_match_arms(source, "op.kind.as_str()")
    assert arms is not None and len(arms) == 5
    assert rust_literal_pattern_names(arms[0].pattern) == frozenset({"live", "_alias"})
    assert arms[0].body.strip() == '"literal, => {}"'
    assert arms[1].pattern.strip() == 'r##"raw " => {}"##'
    assert arms[1].body.strip() == 'r#"RHS, /* content */ { }"#'
    assert rust_literal_pattern_names(arms[1].pattern) is None
    assert rust_literal_pattern_names(arms[2].pattern) == frozenset({"block"})
    assert rust_literal_pattern_names(arms[3].pattern) is None
    assert arms[4].pattern.strip() == "_"
    for arm in arms:
        assert source[arm.body_start : arm.end] == arm.body
        assert source[arm.start : arm.start + len(arm.pattern)] == arm.pattern
    assert source[arms[0].start] == '"'
    assert source[arms[0].body_start] == '"'


def test_rust_lexical_comparison_does_not_normalize_raw_literal_whitespace():
    from molt.rust_source_scan import (
        rust_literal_pattern_names,
        rust_match_arms,
        rust_token_range,
    )

    raw = 'r#"a " b"#'
    assert rust_token_range(raw, 'r#"a "b"#') is None
    assert rust_literal_pattern_names('"n op"') is None
    assert rust_literal_pattern_names('"live" |') is None
    assert rust_match_arms('match kind { "live" => }', "kind") is None
    # Unsupported comma-free expression-with-block syntax cannot swallow a
    # neighboring wildcard into a preceding arm's exclusion range.
    assert (
        rust_match_arms(
            'match kind { "first" => match other { _ => 1 } _ => 2 }', "kind"
        )
        is None
    )
    assert rust_match_arms('match kind { "live" => r#"unterminated }', "kind") is None


def test_rust_block_scope_filters_candidates_before_requiring_uniqueness():
    from molt.rust_source_scan import rust_block_region, rust_token_range

    header = "for (index, op) in function.ops.iter().enumerate()"
    nested = "if context { " + header + " { check_context(op); } } "
    mandatory = header + " { validate_required_fields(op)?; }"
    source = nested + mandatory
    region = rust_block_region(source, header, depth=0)
    assert region is not None
    assert source[slice(*region)].strip() == "validate_required_fields(op)?;"
    assert rust_token_range(source, header, depth=0)[0] == len(nested)
    assert rust_block_region(source, header) is None
    # A nested check is not a substitute for the removed dominating check.
    assert rust_block_region(nested, header, depth=0) is None
    # Two actual siblings remain ambiguous; never choose the first one.
    assert rust_block_region(mandatory + mandatory, header, depth=0) is None
    nested_region = rust_block_region(source, header, depth=1)
    assert nested_region is not None
    assert source[slice(*nested_region)].strip() == "check_context(op);"


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


def test_preserve_literals_masks_only_real_comments_and_preserves_offsets():
    source = (
        'let url = "https://x\\"//escaped"; // trailing\n'
        'let raw = br##"// /* fake */"##; /* outer\n /* nested */ */\n'
        "let slash = '/'; let escaped = '\\\\''; let value: &'a str = url; // final\n"
    )
    masked = mask_rust_comments_and_strings(source, preserve_literals=True)
    assert len(masked) == len(source)
    assert [i for i, c in enumerate(masked) if c in "\r\n"] == [
        i for i, c in enumerate(source) if c in "\r\n"
    ]
    assert "https://x" in masked
    assert 'br##"// /* fake */"##' in masked
    assert "&'a str" in masked
    assert "trailing" not in masked and "nested" not in masked and "final" not in masked
    assert "https://x" not in mask_rust_comments_and_strings(source)


def test_preserved_rust_quote_character_literals_do_not_start_strings():
    source = (
        r"""let quote = '"'; let apostrophe = '\''; let value: &'a str = "https://x"; // trailing"""
        + "\n"
    )
    preserved = mask_rust_comments_and_strings(source, preserve_literals=True)
    masked = mask_rust_comments_and_strings(source)
    assert r"""'"'""" in preserved and r"""'\''""" in preserved
    assert "&'a str" in preserved and "&'a str" in masked
    assert "trailing" not in preserved and "https://x" in preserved
    assert len(preserved) == len(masked) == len(source)


def test_test_item_mask_keeps_mixed_line_production_and_coordinates():
    from molt.rust_source_scan import mask_rust_test_items

    source = '#[cfg (test)] fn oracle() { let s = r#"} mod fake;"#; } fn live() {}\n'
    masked = mask_rust_test_items(source)
    assert len(masked) == len(source)
    assert masked.index("fn live") == source.index("fn live")
    assert "oracle" not in masked
    assert masked.count("\n") == source.count("\n")


def test_declared_test_module_graph_preserves_shared_production(tmp_path):
    from molt.rust_source_scan import rust_test_only_source_files

    (tmp_path / "lib.rs").write_text(
        '#[cfg(test)] #[path = "specs/mod.rs"] mod validation;\n'
        '#[path = "shared.rs"] mod live;\n',
        encoding="utf-8",
    )
    (tmp_path / "specs").mkdir()
    (tmp_path / "specs/mod.rs").write_text(
        'mod oracle; #[path = "../shared.rs"] mod shared;\n', encoding="utf-8"
    )
    (tmp_path / "specs/oracle.rs").write_text("fn fixture() {}", encoding="utf-8")
    (tmp_path / "shared.rs").write_text("fn live() {}", encoding="utf-8")
    found = rust_test_only_source_files(tmp_path.rglob("*.rs"))
    assert found == {
        (tmp_path / "specs/mod.rs").resolve(),
        (tmp_path / "specs/oracle.rs").resolve(),
    }


def test_test_attributes_own_fields_variants_and_declaration_headers():
    from molt.rust_source_scan import mask_rust_test_items

    source = """struct State {
        #[cfg(test)] #[allow(dead_code)] oracle: Map<u32, Vec<u8>>,
        live: usize,
    }
    enum Mode { #[cfg(test)] Oracle(Map<u32, u32>), Live }
    #[cfg(test)] const fn oracle<T, U>() where T: Copy, U: Copy { }
    fn live() {}
    """
    masked = mask_rust_test_items(source)
    assert "oracle:" not in masked and "Oracle(" not in masked
    assert "fn oracle" not in masked
    assert "live: usize" in masked and "Live }" in masked and "fn live" in masked
    assert len(masked) == len(source)


@pytest.mark.parametrize("whitespace", [" ", "\t\n ", "\r\n\t ", "\u2003\u00a0"])
def test_test_items_own_complete_prefixes_without_neighboring_production(whitespace):
    from molt.rust_source_scan import mask_rust_test_items, rust_test_item_spans

    attributes = (
        whitespace
        + "/// Test-only documentation\r\n"
        + '#[fixture([r##"[ ] { } #[cfg(test)]"##], nested({ mod fake; }))]'
        + whitespace
        + "#[allow(dead_code)] /* between attributes */ #[cfg(/* gate */test)]"
        + whitespace
        + "#[allow(unused)]"
        + whitespace
    )
    parts = [
        ('#![allow(dead_code)]\nfn before() { let s = r#"#[cfg(test)]"#; }', False),
        (attributes + "const fn oracle<T, U>() where T: Copy, U: Copy {}", True),
        (
            whitespace
            + "#[allow(dead_code)] fn after() {}\nstruct State { live_before: usize,",
            False,
        ),
        (attributes + "oracle: Map<u32, Vec<u8>>,", True),
        (whitespace + "live_after: usize }\nenum Mode { Before,", False),
        (attributes + "Oracle(Map<u32, u32>),", True),
        (whitespace + "After }", False),
        (
            attributes
            + "mod tests { #[allow(dead_code)] #[cfg(test)] fn nested_oracle() {} }",
            True,
        ),
        (
            whitespace + '#[cfg(any(test, feature = "production"))] mod retained {}\n',
            False,
        ),
    ]
    source = "".join(part for part, _ in parts)
    expected = "".join(
        "".join(char if char in "\r\n" else " " for char in part) if test else part
        for part, test in parts
    )
    expected_spans = []
    cursor = 0
    for part, test in parts:
        if test:
            expected_spans.append((cursor, cursor + len(part)))
        cursor += len(part)
    assert rust_test_item_spans(source) == expected_spans
    assert mask_rust_test_items(source) == expected


def test_consecutive_test_prefixes_preserve_siblings_and_enclosing_delimiters():
    from molt.rust_source_scan import mask_rust_test_items, rust_test_item_spans

    first = "\t#[allow(dead_code)] #[cfg(test)] fn first() {}"
    second = "\r\n#[cfg(test)] #[cfg(test)] fn second() {}"
    before = "fn retained() {}"
    after = "\nstruct Last {"
    field = "\t#[allow(dead_code)] #[cfg(test)] oracle: usize"
    tail = "}\nfn final_item() {}"
    source = before + first + second + after + field + tail
    boundaries = [
        (len(before), len(before + first)),
        (len(before + first), len(before + first + second)),
        (len(before + first + second + after), len(source) - len(tail)),
    ]
    assert rust_test_item_spans(source) == boundaries

    def blank(item):
        return "".join(char if char in "\r\n" else " " for char in item)

    assert mask_rust_test_items(source) == (
        before + blank(first) + blank(second) + after + blank(field) + tail
    )


def test_module_edges_share_balanced_prefixes_and_literal_path_boundaries(tmp_path):
    from molt.rust_source_scan import (
        read_rust_module_cluster,
        rust_file_module_declarations,
    )

    root = tmp_path / "lib.rs"
    retained = (
        "#![fixture(#[cfg(test)] mod missing_inner;)]\n"
        "// #[cfg(test)] mod missing_comment;\n"
        '#[doc = r##"#[path = "missing_literal.rs"] #[cfg(test)]"##]\n'
        "#[fixture([a, b], { mod missing_attribute; })]\n"
        '#[path /* directory */ = "nested"] mod production {\n'
        '#[allow(dead_code)] #[path = "kept.rs"] pub mod child;\n}'
    )
    excluded = (
        "\t#[fixture([a, b], { mod missing_attribute; })]\n"
        '#[allow(dead_code)] #[cfg(test)] #[path = "oracle.rs"] mod oracle;'
    )
    source = retained + excluded + "\n"
    root.write_text(source, encoding="utf-8", newline="")
    nested = tmp_path / "nested"
    nested.mkdir()
    child = nested / "kept.rs"
    child_text = "pub fn live_authority() {}\n"
    child.write_text(child_text, encoding="utf-8", newline="")
    oracle = tmp_path / "oracle.rs"
    oracle.write_text("fn test_oracle() {}\n", encoding="utf-8", newline="")

    assert rust_file_module_declarations(root, source, include_tests=False) == [
        (child.resolve(), False)
    ]
    assert rust_file_module_declarations(root, source) == [
        (child.resolve(), False),
        (oracle.resolve(), True),
    ]
    blanked = "".join(char if char in "\r\n" else " " for char in excluded)
    assert (
        read_rust_module_cluster(root) == child_text + "\n" + retained + blanked + "\n"
    )
