// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

use super::*;

fn parse_pattern(pattern: &str, flags: i64) -> Result<ParsedPattern, String> {
    parse_pattern_typed(pattern, flags, false)
}
use std::sync::Arc;

fn scanner_spans_text(pattern: &str, text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let compiled = Arc::new(PatternHandle::new(
        parse_pattern_typed(pattern, 0, false)
            .unwrap()
            .compile()
            .unwrap(),
    ));
    let mut scanner = ScannerHandle::new(Arc::clone(&compiled), 0, chars.len(), None);
    let mut spans = Vec::new();
    while let Some(found) = advance_scanner(
        &compiled.compiled,
        &mut scanner,
        SubjectStorage::Text(&chars),
    )
    .unwrap()
    {
        spans.push((found.start, found.end));
    }
    spans
}

fn scanner_spans_bytes(pattern: &str, subject: &[u8]) -> Vec<(usize, usize)> {
    let compiled = Arc::new(PatternHandle::new(
        parse_pattern_typed(pattern, 0, true)
            .unwrap()
            .compile()
            .unwrap(),
    ));
    let mut scanner = ScannerHandle::new(Arc::clone(&compiled), 0, subject.len(), None);
    let mut spans = Vec::new();
    while let Some(found) = advance_scanner(
        &compiled.compiled,
        &mut scanner,
        SubjectStorage::Bytes(subject),
    )
    .unwrap()
    {
        spans.push((found.start, found.end));
    }
    spans
}

#[test]
fn test_strip_verbose_empty() {
    assert_eq!(re_strip_verbose_impl("", 64), "");
}

#[test]
fn test_strip_verbose_no_whitespace() {
    assert_eq!(re_strip_verbose_impl("abc", 64), "abc");
}

#[test]
fn test_strip_verbose_strips_spaces() {
    assert_eq!(re_strip_verbose_impl("a b c", 64), "abc");
}

#[test]
fn test_strip_verbose_strips_comments() {
    let pat = "a  # match a\nb  # match b\n";
    assert_eq!(re_strip_verbose_impl(pat, 64), "ab");
}

#[test]
fn test_strip_verbose_escaped_space() {
    // `\ ` should survive
    assert_eq!(re_strip_verbose_impl("\\ x", 64), "\\ x");
}

#[test]
fn test_strip_verbose_escaped_hash() {
    assert_eq!(re_strip_verbose_impl("\\#", 64), "\\#");
}

#[test]
fn test_strip_verbose_class_preserved() {
    // Whitespace and # inside [...] must be kept verbatim.
    assert_eq!(re_strip_verbose_impl("[ # ]", 64), "[ # ]");
}

#[test]
fn test_strip_verbose_class_escape() {
    // `\]` inside a class should not close it.
    assert_eq!(re_strip_verbose_impl("[\\]]", 64), "[\\]]");
}

#[test]
fn test_strip_verbose_no_flag_unchanged() {
    // flags=0 means NOT verbose — pattern should come back unchanged
    let pat = "a b # comment\n";
    assert_eq!(re_strip_verbose_impl(pat, 0), pat);
}

// -----------------------------------------------------------------------
// Phase-1 parser tests
// -----------------------------------------------------------------------

#[test]
fn test_parse_empty_pattern() {
    let cp = parse_pattern("", 0).unwrap();
    assert!(matches!(cp.root, ReNode::Empty));
    assert_eq!(cp.group_count, 0);
}

#[test]
fn test_parse_literal() {
    let cp = parse_pattern("hello", 0).unwrap();
    match &cp.root {
        ReNode::Literal(s) => assert_eq!(s, "hello"),
        other => panic!("expected Literal, got {other:?}"),
    }
}

#[test]
fn test_parse_any() {
    let cp = parse_pattern(".", 0).unwrap();
    assert!(matches!(cp.root, ReNode::Any));
}

#[test]
fn test_parse_anchor_start() {
    let cp = parse_pattern("^", 0).unwrap();
    match &cp.root {
        ReNode::Anchor(k) => assert_eq!(*k, Anchor::Start),
        other => panic!("expected Anchor, got {other:?}"),
    }
}

#[test]
fn test_parse_anchor_end() {
    let cp = parse_pattern("$", 0).unwrap();
    match &cp.root {
        ReNode::Anchor(k) => assert_eq!(*k, Anchor::End),
        other => panic!("expected Anchor, got {other:?}"),
    }
}

#[test]
fn test_parse_alternation() {
    let cp = parse_pattern("a|b", 0).unwrap();
    match &cp.root {
        ReNode::Alt(opts) => assert_eq!(opts.len(), 2),
        other => panic!("expected Alt, got {other:?}"),
    }
}

#[test]
fn test_parse_capturing_group() {
    let cp = parse_pattern("(abc)", 0).unwrap();
    assert_eq!(cp.group_count, 1);
    match &cp.root {
        ReNode::Group { index, .. } => assert_eq!(*index, 1),
        other => panic!("expected Group, got {other:?}"),
    }
}

#[test]
fn test_parse_named_group() {
    let cp = parse_pattern("(?P<word>\\w+)", 0).unwrap();
    assert_eq!(cp.group_count, 1);
    assert_eq!(cp.group_names.get("word"), Some(&1u32));
}

#[test]
fn test_parse_non_capturing_group() {
    let cp = parse_pattern("(?:abc)", 0).unwrap();
    assert_eq!(cp.group_count, 0);
    match &cp.root {
        ReNode::Literal(s) => assert_eq!(s, "abc"),
        other => panic!("expected Literal (collapsed non-capturing group), got {other:?}"),
    }
}

#[test]
fn test_parse_repeat_star() {
    let cp = parse_pattern("a*", 0).unwrap();
    match &cp.root {
        ReNode::Repeat {
            min_count,
            max_count,
            greedy,
            ..
        } => {
            assert_eq!(*min_count, 0);
            assert!(max_count.is_none());
            assert!(*greedy);
        }
        other => panic!("expected Repeat, got {other:?}"),
    }
}

#[test]
fn test_parse_repeat_lazy() {
    let cp = parse_pattern("a+?", 0).unwrap();
    match &cp.root {
        ReNode::Repeat {
            min_count,
            max_count,
            greedy,
            ..
        } => {
            assert_eq!(*min_count, 1);
            assert!(max_count.is_none());
            assert!(!(*greedy));
        }
        other => panic!("expected Repeat, got {other:?}"),
    }
}

#[test]
fn test_parse_repeat_counted() {
    let cp = parse_pattern("a{2,5}", 0).unwrap();
    match &cp.root {
        ReNode::Repeat {
            min_count,
            max_count,
            ..
        } => {
            assert_eq!(*min_count, 2);
            assert_eq!(*max_count, Some(5));
        }
        other => panic!("expected Repeat, got {other:?}"),
    }
}

#[test]
fn test_parse_charclass() {
    let cp = parse_pattern("[abc]", 0).unwrap();
    match &cp.root {
        ReNode::CharClass { negated, chars, .. } => {
            assert!(!negated);
            assert!(chars.contains(&"a".to_string()));
        }
        other => panic!("expected CharClass, got {other:?}"),
    }
}

#[test]
fn test_parse_negated_charclass() {
    let cp = parse_pattern("[^abc]", 0).unwrap();
    match &cp.root {
        ReNode::CharClass { negated, .. } => {
            assert!(*negated);
        }
        other => panic!("expected CharClass, got {other:?}"),
    }
}

#[test]
fn test_parse_charclass_range() {
    let cp = parse_pattern("[a-z]", 0).unwrap();
    match &cp.root {
        ReNode::CharClass { ranges, .. } => {
            assert!(!ranges.is_empty());
            let (s, e) = &ranges[0];
            assert_eq!(s, "a");
            assert_eq!(e, "z");
        }
        other => panic!("expected CharClass with range, got {other:?}"),
    }
}

#[test]
fn test_parse_backslash_d() {
    let cp = parse_pattern("\\d", 0).unwrap();
    match &cp.root {
        ReNode::CharClass {
            negated,
            categories,
            ..
        } => {
            assert!(!negated);
            assert!(categories.contains(&Category::Digit));
        }
        other => panic!("expected CharClass, got {other:?}"),
    }
}

#[test]
#[allow(non_snake_case)]
fn test_parse_backslash_D() {
    let cp = parse_pattern("\\D", 0).unwrap();
    match &cp.root {
        ReNode::CharClass {
            negated,
            categories,
            ..
        } => {
            assert!(!*negated);
            assert!(categories.contains(&Category::NotDigit));
        }
        other => panic!("expected CharClass, got {other:?}"),
    }
}

#[test]
fn test_parse_lookahead_positive() {
    let cp = parse_pattern("(?=abc)", 0).unwrap();
    match &cp.root {
        ReNode::Look {
            behind, positive, ..
        } => {
            assert!(!behind);
            assert!(*positive);
        }
        other => panic!("expected Look, got {other:?}"),
    }
}

#[test]
fn test_parse_lookahead_negative() {
    let cp = parse_pattern("(?!abc)", 0).unwrap();
    match &cp.root {
        ReNode::Look {
            behind, positive, ..
        } => {
            assert!(!behind);
            assert!(!positive);
        }
        other => panic!("expected Look, got {other:?}"),
    }
}

#[test]
fn test_parse_lookbehind_positive() {
    let cp = parse_pattern("(?<=ab)", 0).unwrap();
    match &cp.root {
        ReNode::Look {
            behind,
            positive,
            width,
            ..
        } => {
            assert!(*behind);
            assert!(*positive);
            assert_eq!(*width, Some(2));
        }
        other => panic!("expected Look, got {other:?}"),
    }
}

#[test]
fn test_parse_lookbehind_negative() {
    let cp = parse_pattern("(?<!ab)", 0).unwrap();
    match &cp.root {
        ReNode::Look {
            behind,
            positive,
            width,
            ..
        } => {
            assert!(*behind);
            assert!(!positive);
            assert_eq!(*width, Some(2));
        }
        other => panic!("expected Look, got {other:?}"),
    }
}

#[test]
fn test_parse_scoped_flags() {
    let cp = parse_pattern("(?i:abc)", 0).unwrap();
    match &cp.root {
        ReNode::ScopedFlags {
            add_flags,
            clear_flags,
            ..
        } => {
            assert_eq!(*add_flags & RE_IGNORECASE, RE_IGNORECASE);
            assert_eq!(*clear_flags, 0);
        }
        other => panic!("expected ScopedFlags, got {other:?}"),
    }
}

#[test]
fn test_parse_inline_flags_global() {
    let cp = parse_pattern("(?i)abc", 0).unwrap();
    assert_eq!(cp.flags & RE_IGNORECASE, RE_IGNORECASE);
}

#[test]
fn test_parse_backref() {
    let cp = parse_pattern("(a)\\1", 0).unwrap();
    match &cp.root {
        ReNode::Concat(nodes) => {
            assert_eq!(nodes.len(), 2);
            assert!(matches!(&nodes[0], ReNode::Group { index: 1, .. }));
            assert!(matches!(&nodes[1], ReNode::Backref(1)));
        }
        other => panic!("expected Concat, got {other:?}"),
    }
}

#[test]
fn test_parse_conditional() {
    let cp = parse_pattern("(?(1)yes|no)", 0).unwrap();
    match &cp.root {
        ReNode::Conditional { group_index, .. } => assert_eq!(*group_index, 1),
        other => panic!("expected Conditional, got {other:?}"),
    }
}

#[test]
fn test_parse_named_backref() {
    let cp = parse_pattern("(?P<w>a)(?P=w)", 0).unwrap();
    assert_eq!(cp.group_count, 1);
    match &cp.root {
        ReNode::Concat(nodes) => {
            assert!(matches!(&nodes[1], ReNode::Backref(1)));
        }
        other => panic!("expected Concat with named backref, got {other:?}"),
    }
}

#[test]
fn test_parse_bad_quantifier_range() {
    let err = parse_pattern("a{5,3}", 0).unwrap_err();
    assert!(err.contains("invalid quantifier range"), "got: {err}");
}

#[test]
fn test_parse_missing_close_paren() {
    parse_pattern("(abc", 0).unwrap_err();
}

#[test]
fn test_parse_verbose_whitespace_stripped() {
    // In verbose mode, whitespace between atoms is ignored.
    let cp = parse_pattern("a b c", RE_VERBOSE).unwrap();
    match &cp.root {
        ReNode::Literal(s) => assert_eq!(s, "abc"),
        other => panic!("expected Literal 'abc', got {other:?}"),
    }
}

#[test]
fn test_parse_anchor_abs() {
    let cp = parse_pattern("\\A\\Z", 0).unwrap();
    match &cp.root {
        ReNode::Concat(nodes) => {
            assert!(matches!(&nodes[0], ReNode::Anchor(k) if *k == Anchor::StartAbsolute));
            assert!(matches!(&nodes[1], ReNode::Anchor(k) if *k == Anchor::EndAbsolute));
        }
        other => panic!("expected Concat, got {other:?}"),
    }
}

#[test]
fn test_parse_word_boundary() {
    let cp = parse_pattern("\\b\\B", 0).unwrap();
    match &cp.root {
        ReNode::Concat(nodes) => {
            assert!(matches!(&nodes[0], ReNode::Anchor(k) if *k == Anchor::Boundary));
            assert!(matches!(&nodes[1], ReNode::Anchor(k) if *k == Anchor::NotBoundary));
        }
        other => panic!("expected Concat, got {other:?}"),
    }
}

#[test]
fn test_fixed_width_literal() {
    assert_eq!(
        fixed_width(&ReNode::Literal("hello".to_string()), None),
        Some(5)
    );
}

#[test]
fn test_fixed_width_repeat_exact() {
    let node = ReNode::Repeat {
        node: Box::new(ReNode::Any),
        min_count: 3,
        max_count: Some(3),
        greedy: true,
    };
    assert_eq!(fixed_width(&node, None), Some(3));
}

#[test]
fn test_fixed_width_repeat_variable() {
    let node = ReNode::Repeat {
        node: Box::new(ReNode::Any),
        min_count: 1,
        max_count: Some(3),
        greedy: true,
    };
    assert_eq!(fixed_width(&node, None), None);
}

#[test]
fn test_parse_group_count_multiple() {
    let cp = parse_pattern("(a)(b)(c)", 0).unwrap();
    assert_eq!(cp.group_count, 3);
}

#[test]
fn test_parse_octal_escape_in_class() {
    // \101 octal = 'A'
    let cp = parse_pattern("[\\101]", 0).unwrap();
    match &cp.root {
        ReNode::CharClass { chars, .. } => {
            assert!(
                chars.contains(&"A".to_string()),
                "expected 'A' in chars, got {chars:?}"
            );
        }
        other => panic!("expected CharClass, got {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Phase-1b match engine tests
// -----------------------------------------------------------------------

/// Helper: compile + execute in a given mode.
fn do_execute(pattern: &str, flags: i64, text: &str, mode: &str) -> Option<MatchResult> {
    let compiled = parse_pattern(pattern, flags).unwrap().compile().unwrap();
    let text_len = text.chars().count();
    execute_match(&compiled, text, 0, text_len, mode).unwrap()
}

/// Helper: compile + execute "match" mode.
fn do_match(pattern: &str, text: &str) -> Option<MatchResult> {
    do_execute(pattern, 0, text, "match")
}

/// Helper: compile + execute "search" mode.
fn do_search(pattern: &str, text: &str) -> Option<MatchResult> {
    do_execute(pattern, 0, text, "search")
}

/// Helper: compile + execute "fullmatch" mode.
fn do_fullmatch(pattern: &str, text: &str) -> Option<MatchResult> {
    do_execute(pattern, 0, text, "fullmatch")
}

// --- Literal matching ---

#[test]
fn test_match_literal() {
    let m = do_match("hello", "hello world").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_literal_no_match() {
    assert!(do_match("xyz", "hello").is_none());
}

#[test]
fn test_search_literal() {
    let m = do_search("world", "hello world").unwrap();
    assert_eq!(m.start, 6);
    assert_eq!(m.end, 11);
}

#[test]
fn test_fullmatch_literal() {
    let m = do_fullmatch("hello", "hello").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 5);
}

#[test]
fn test_fullmatch_literal_fail() {
    assert!(do_fullmatch("hello", "hello world").is_none());
}

// --- Any (.) ---

#[test]
fn test_match_any() {
    let m = do_match(".", "a").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_any_no_newline() {
    assert!(do_match(".", "\n").is_none());
}

#[test]
fn test_match_any_dotall() {
    let m = do_execute(".", RE_DOTALL, "\n", "match").unwrap();
    assert_eq!(m.end, 1);
}

// --- Character classes ---

#[test]
fn test_match_charclass() {
    let m = do_match("[abc]", "b").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_charclass_no_match() {
    assert!(do_match("[abc]", "d").is_none());
}

#[test]
fn test_match_charclass_range() {
    let m = do_match("[a-z]", "m").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_charclass_literal_and_escaped_range_endpoints() {
    // CPython's class grammar decodes one atom on each side of '-'. These
    // literal expected sets distinguish ranges from endpoints and literal '-'.
    for (pattern, expected) in [
        (r"[\101-\132]", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        (r"[\101-Z]", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        (r"[A-\132]", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        (r"[\t-\r]", "\t\n\x0b\x0c\r"),
        (r"[\--/]", "-./"),
        (r"[\(-\)]", "()"),
        (r"[]-a]", "]^_`a"),
        (r"[--0]", "-./0"),
        (r"[a-b-c]", "-abc"),
        (r"[\141-\142-\143]", "-abc"),
        (r"[a-b-c-e]", "-abcde"),
        (r"[a-]", "-a"),
        (r"[\d-]", "-0123456789"),
    ] {
        let compiled = parse_pattern(pattern, 0).unwrap().compile().unwrap();
        for code in 0u8..=127 {
            let ch = char::from(code);
            let text = ch.to_string();
            assert_eq!(
                execute_match(&compiled, &text, 0, 1, "fullmatch")
                    .unwrap()
                    .is_some(),
                expected.contains(ch),
                "pattern {pattern:?}, input {ch:?}"
            );
        }
    }
    assert!(do_fullmatch(r"[^\101-\132]", "M").is_none());
    assert!(do_fullmatch(r"[^\101-\132]", "m").is_some());
}

#[test]
fn test_parse_charclass_refuses_nonliteral_or_reversed_ranges() {
    for pattern in [
        r"[z-a]",
        r"[\141-\140]",
        r"[a-\-]",
        r"[\d-a]",
        r"[a-\w]",
        r"[\D-a]",
        r"[a-\S]",
        "[a-",
        "[]",
    ] {
        assert!(parse_pattern(pattern, 0).is_err(), "{pattern:?}");
    }
}

#[test]
fn test_match_email_header_ranges() {
    // The unmodified CPython FeedParser pattern: letters must occur in the
    // interior of the escaped ranges, not merely at their endpoints.
    let pattern = r"^(From |[\041-\071\073-\176]*:|[\t ])";
    for (line, end) in [
        ("Subject: Hello\n", Some(8)),
        ("Content-Type: text/plain\n", Some(13)),
        ("From sender\n", Some(5)),
        ("\tcontinued\n", Some(1)),
        (" continued\n", Some(1)),
        ("\n", None),
        ("Body\n", None),
        ("Bad Name: value\n", None),
    ] {
        assert_eq!(
            do_match(pattern, line).map(|result| result.end),
            end,
            "{line:?}"
        );
    }
}

#[test]
fn test_match_charclass_range_no_match() {
    assert!(do_match("[a-z]", "5").is_none());
}

#[test]
fn test_match_negated_charclass() {
    let m = do_match("[^abc]", "d").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_negated_charclass_no_match() {
    assert!(do_match("[^abc]", "a").is_none());
}

#[test]
fn test_match_charclass_digit() {
    let m = do_match("\\d", "5").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_charclass_word() {
    let m = do_match("\\w", "a").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_charclass_space() {
    let m = do_match("\\s", " ").unwrap();
    assert_eq!(m.end, 1);
}

// --- Anchors ---

#[test]
fn test_match_anchor_start() {
    let m = do_match("^hello", "hello").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_search_anchor_start_fails_mid() {
    assert!(do_search("^world", "hello world").is_none());
}

#[test]
fn test_match_anchor_end() {
    let m = do_fullmatch("hello$", "hello").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_anchor_end_trailing_newline() {
    // $ matches before a trailing newline.
    let m = do_match("hello$", "hello\n").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_multiline_anchor() {
    let compiled = parse_pattern("^world", RE_MULTILINE)
        .unwrap()
        .compile()
        .unwrap();
    let text = "hello\nworld";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "search")
        .unwrap()
        .unwrap();
    assert_eq!(m.start, 6);
    assert_eq!(m.end, 11);
}

#[test]
fn test_match_abs_start() {
    let m = do_match("\\Ahello", "hello").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_abs_end() {
    let m = do_fullmatch("hello\\Z", "hello").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_word_boundary() {
    let m = do_search("\\bhello\\b", "say hello there").unwrap();
    assert_eq!(m.start, 4);
    assert_eq!(m.end, 9);
}

// --- Quantifiers ---

#[test]
fn test_match_star() {
    let m = do_match("a*", "aaa").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_match_star_zero() {
    let m = do_match("a*", "bbb").unwrap();
    assert_eq!(m.end, 0);
}

#[test]
fn test_match_plus() {
    let m = do_match("a+", "aaa").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_match_plus_fail() {
    assert!(do_match("a+", "bbb").is_none());
}

#[test]
fn test_match_question() {
    let m = do_match("a?", "a").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_question_zero() {
    let m = do_match("a?", "b").unwrap();
    assert_eq!(m.end, 0);
}

#[test]
fn test_match_counted() {
    let m = do_match("a{2,4}", "aaaa").unwrap();
    assert_eq!(m.end, 4);
}

#[test]
fn test_match_counted_exact() {
    let m = do_match("a{3}", "aaaa").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_match_counted_fail() {
    assert!(do_match("a{3}", "aa").is_none());
}

// --- Greedy vs lazy ---

#[test]
fn test_match_greedy_star() {
    let m = do_match("a.*b", "aXXXb").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_greedy_backtrack() {
    let m = do_match(".*b", "aXXXb").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_match_lazy_star() {
    let m = do_match("a.*?b", "aXbXb").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 3);
}

// --- Groups ---

#[test]
fn test_match_group() {
    let m = do_match("(abc)", "abc").unwrap();
    assert_eq!(m.groups[1], Some((0, 3)));
}

#[test]
fn test_match_multiple_groups() {
    let m = do_match("(a)(b)(c)", "abc").unwrap();
    assert_eq!(m.groups[1], Some((0, 1)));
    assert_eq!(m.groups[2], Some((1, 2)));
    assert_eq!(m.groups[3], Some((2, 3)));
}

#[test]
fn test_match_nested_groups() {
    let m = do_match("((a)b)", "ab").unwrap();
    assert_eq!(m.groups[1], Some((0, 2)));
    assert_eq!(m.groups[2], Some((0, 1)));
}

#[test]
fn test_match_non_capturing_group() {
    let m = do_match("(?:abc)", "abc").unwrap();
    assert_eq!(m.end, 3);
    // No groups captured.
    assert_eq!(m.groups.len(), 1); // only slot 0
}

// --- Alternation ---

#[test]
fn test_match_alternation() {
    let m = do_match("cat|dog", "dog").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_match_alternation_first() {
    let m = do_match("cat|dog", "cat").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_match_alternation_no_match() {
    assert!(do_match("cat|dog", "fish").is_none());
}

// --- Backreferences ---

#[test]
fn test_match_backref() {
    let m = do_match("(a)\\1", "aa").unwrap();
    assert_eq!(m.end, 2);
}

#[test]
fn test_match_backref_fail() {
    assert!(do_match("(a)\\1", "ab").is_none());
}

// --- Lookahead ---

#[test]
fn test_match_positive_lookahead() {
    let m = do_match("a(?=b)", "ab").unwrap();
    assert_eq!(m.end, 1); // lookahead doesn't consume
}

#[test]
fn test_match_positive_lookahead_fail() {
    assert!(do_match("a(?=b)", "ac").is_none());
}

#[test]
fn test_match_negative_lookahead() {
    let m = do_match("a(?!b)", "ac").unwrap();
    assert_eq!(m.end, 1);
}

#[test]
fn test_match_negative_lookahead_fail() {
    assert!(do_match("a(?!b)", "ab").is_none());
}

// --- Lookbehind ---

#[test]
fn test_match_positive_lookbehind() {
    let compiled = parse_pattern("(?<=a)b", 0).unwrap().compile().unwrap();
    let text = "ab";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "search")
        .unwrap()
        .unwrap();
    assert_eq!(m.start, 1);
    assert_eq!(m.end, 2);
}

#[test]
fn test_match_negative_lookbehind() {
    let compiled = parse_pattern("(?<!a)b", 0).unwrap().compile().unwrap();
    let text = "cb";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "search")
        .unwrap()
        .unwrap();
    assert_eq!(m.start, 1);
    assert_eq!(m.end, 2);
}

#[test]
fn test_match_negative_lookbehind_fail() {
    let compiled = parse_pattern("(?<!a)b", 0).unwrap().compile().unwrap();
    let text = "ab";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "search").unwrap();
    assert!(m.is_none());
}

// --- Case insensitive ---

#[test]
fn test_match_ignorecase() {
    let m = do_execute("hello", RE_IGNORECASE, "HELLO", "match").unwrap();
    assert_eq!(m.end, 5);
    // CPython 3.12 fullmatch oracle, including non-ASCII and a character
    // whose Rust lowercase iterator contains multiple code points.
    for (pattern, text, expected) in [
        ("A", "a", true),
        ("É", "é", true),
        ("Σ", "σ", true),
        ("İ", "İ", true),
        ("🦀", "🦀", true),
        ("x", "z", false),
    ] {
        let matched = do_execute(pattern, RE_IGNORECASE, text, "fullmatch");
        assert_eq!(matched.is_some(), expected, "{pattern:?} / {text:?}");
        if let Some(matched) = matched {
            assert_eq!((matched.start, matched.end), (0, 1));
        }
    }
}

#[test]
fn test_match_charclass_ignorecase() {
    let m = do_execute("[a-z]", RE_IGNORECASE, "Z", "match").unwrap();
    assert_eq!(m.end, 1);
}

// --- Complex patterns ---

#[test]
fn test_match_email_like() {
    let m = do_search("\\w+@\\w+", "foo@bar").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 7);
}

#[test]
fn test_match_digits_in_parens() {
    let m = do_search("\\((\\d+)\\)", "call(42)").unwrap();
    assert_eq!(m.start, 4);
    assert_eq!(m.end, 8);
    assert_eq!(m.groups[1], Some((5, 7)));
}

// --- finditer_collect ---

#[test]
fn scanner_retries_nonempty_alternative_at_same_position() {
    let expected = [(0, 0), (0, 1), (1, 1)];
    assert_eq!(scanner_spans_text("|a", "a"), expected);
    assert_eq!(scanner_spans_bytes("|a", b"a"), expected);
}

#[test]
fn scanner_empty_match_advances_and_allows_end_empty() {
    assert_eq!(scanner_spans_bytes("", b"a"), [(0, 0), (1, 1)]);
    assert_eq!(scanner_spans_text("", "a"), [(0, 0), (1, 1)]);
}

#[test]
fn scanner_preserves_lazy_repeat_and_unicode_character_offsets() {
    assert_eq!(scanner_spans_text("a+?", "aa"), [(0, 1), (1, 2)]);
    assert_eq!(scanner_spans_text(".", "éx"), [(0, 1), (1, 2)]);
}

#[test]
fn scanner_observes_mutated_bytes_between_transitions() {
    let pattern = Arc::new(PatternHandle::new(
        parse_pattern_typed("a", 0, true)
            .unwrap()
            .compile()
            .unwrap(),
    ));
    let mut scanner = ScannerHandle::new(Arc::clone(&pattern), 0, 2, None);
    let mut subject = *b"ab";
    let first = advance_scanner(
        &pattern.compiled,
        &mut scanner,
        SubjectStorage::Bytes(&subject),
    )
    .unwrap()
    .unwrap();
    assert_eq!((first.start, first.end), (0, 1));
    subject[1] = b'a';
    let second = advance_scanner(
        &pattern.compiled,
        &mut scanner,
        SubjectStorage::Bytes(&subject),
    )
    .unwrap()
    .unwrap();
    assert_eq!((second.start, second.end), (1, 2));
    assert!(
        advance_scanner(
            &pattern.compiled,
            &mut scanner,
            SubjectStorage::Bytes(&subject),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn test_finditer_collect_basic() {
    let compiled = parse_pattern("\\d+", 0).unwrap().compile().unwrap();
    let text = "a1b22c333d";
    let text_len = text.chars().count();
    let mut results = Vec::new();
    let mut cur = 0;
    let end = text_len;
    while cur <= end {
        match execute_match(&compiled, text, cur, end, "search").unwrap() {
            Some(result) => {
                let match_end = result.end;
                results.push((result.start, result.end));
                if match_end == result.start {
                    cur = result.start + 1;
                } else {
                    cur = match_end;
                }
            }
            None => break,
        }
    }
    assert_eq!(results, vec![(1, 2), (3, 5), (6, 9)]);
}

#[test]
fn test_search_empty_pattern() {
    let m = do_search("", "abc").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 0);
}

#[test]
fn test_fullmatch_star() {
    let m = do_fullmatch("a*", "aaa").unwrap();
    assert_eq!(m.end, 3);
}

#[test]
fn test_fullmatch_star_empty() {
    let m = do_fullmatch("a*", "").unwrap();
    assert_eq!(m.end, 0);
}

// --- Scoped flags ---

#[test]
fn test_match_scoped_ignorecase() {
    let m = do_match("(?i:hello) world", "HELLO world").unwrap();
    assert_eq!(m.end, 11);
}

#[test]
fn test_match_scoped_ignorecase_outside() {
    assert!(do_match("(?i:hello) WORLD", "HELLO world").is_none());
}

// --- Conditional ---

#[test]
fn test_match_conditional_yes() {
    let m = do_match("(a)(?(1)b|c)", "ab").unwrap();
    assert_eq!(m.end, 2);
}

#[test]
fn test_match_conditional_no() {
    let m = do_match("(a)?(?(1)b|c)", "c").unwrap();
    assert_eq!(m.end, 1);
}

// --- Unicode ---

#[test]
fn test_match_unicode_literal() {
    let m = do_match("cafe\u{0301}", "cafe\u{0301}").unwrap();
    assert_eq!(m.end, 5);
}

#[test]
fn test_search_unicode() {
    let m = do_search("\\w+", "hello \u{4e16}\u{754c}").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 5);
}

// --- Named groups ---

#[test]
fn test_match_named_group() {
    let compiled = parse_pattern("(?P<word>\\w+)", 0)
        .unwrap()
        .compile()
        .unwrap();
    let text = "hello";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "match")
        .unwrap()
        .unwrap();
    assert_eq!(m.groups[1], Some((0, 5)));
}

// --- Named backref ---

#[test]
fn test_match_named_backref() {
    let compiled = parse_pattern("(?P<w>\\w+) (?P=w)", 0)
        .unwrap()
        .compile()
        .unwrap();
    let text = "abc abc";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "match")
        .unwrap()
        .unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 7);
    assert_eq!(m.groups[1], Some((0, 3)));
}

#[test]
fn test_match_named_backref_fail() {
    let compiled = parse_pattern("(?P<w>\\w+) (?P=w)", 0)
        .unwrap()
        .compile()
        .unwrap();
    let text = "abc def";
    let text_len = text.chars().count();
    let m = execute_match(&compiled, text, 0, text_len, "match").unwrap();
    assert!(m.is_none());
}

// --- Greedy backtracking through multiple patterns ---

#[test]
fn test_greedy_backtrack_complex() {
    let m = do_match(".*(\\d+)", "abc123").unwrap();
    assert_eq!(m.end, 6);
    // Greedy .* eats as much as possible, then \d+ needs at least 1 digit.
    assert_eq!(m.groups[1], Some((5, 6)));
}

#[test]
fn test_lazy_captures_more() {
    let m = do_match(".*?(\\d+)", "abc123").unwrap();
    assert_eq!(m.end, 6);
    // Lazy .*? matches "abc", \d+ matches "123".
    assert_eq!(m.groups[1], Some((3, 6)));
}

// --- Edge cases ---

#[test]
fn test_match_empty_string() {
    let m = do_match("", "").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 0);
}

#[test]
fn test_search_empty_string() {
    let m = do_search("", "").unwrap();
    assert_eq!(m.start, 0);
    assert_eq!(m.end, 0);
}

#[test]
fn test_match_pos_beyond_text() {
    let compiled = parse_pattern("a", 0).unwrap().compile().unwrap();
    let m = execute_match(&compiled, "a", 5, 5, "match").unwrap();
    assert!(m.is_none());
}

// -----------------------------------------------------------------------
// split / sub helper tests (internal logic)
// -----------------------------------------------------------------------

/// Test projection of the production scanner and replacement transitions.
fn do_split(pattern: &str, text: &str, maxsplit: usize) -> Vec<String> {
    let pattern = std::sync::Arc::new(PatternHandle::new(
        parse_pattern(pattern, 0).unwrap().compile().unwrap(),
    ));
    let chars: Vec<char> = text.chars().collect();
    let mut scanner = ScannerHandle::new(std::sync::Arc::clone(&pattern), 0, chars.len(), None);
    let mut parts = Vec::new();
    let mut last = 0;
    let mut count = 0;
    while maxsplit == 0 || count < maxsplit {
        let Some(result) = advance_scanner(
            &pattern.compiled,
            &mut scanner,
            SubjectStorage::Text(&chars),
        )
        .unwrap() else {
            break;
        };
        parts.push(chars[last..result.start].iter().collect());
        for span in result.groups.iter().skip(1) {
            parts.push(span.map_or_else(String::new, |(start, end)| {
                chars[start..end].iter().collect()
            }));
        }
        last = result.end;
        count += 1;
    }
    parts.push(chars[last..].iter().collect());
    parts
}

fn do_sub(pattern: &str, repl: &str, text: &str, count: usize) -> (String, usize) {
    let pattern = std::sync::Arc::new(PatternHandle::new(
        parse_pattern(pattern, 0).unwrap().compile().unwrap(),
    ));
    let chars: Vec<char> = text.chars().collect();
    let mut scanner = ScannerHandle::new(std::sync::Arc::clone(&pattern), 0, chars.len(), None);
    let mut out = String::new();
    let mut last = 0;
    let mut replaced = 0;
    while count == 0 || replaced < count {
        let Some(result) = advance_scanner(
            &pattern.compiled,
            &mut scanner,
            SubjectStorage::Text(&chars),
        )
        .unwrap() else {
            break;
        };
        out.extend(chars[last..result.start].iter());
        out.push_str(&expand_repl_with_groups(
            repl,
            &chars,
            &result,
            pattern.compiled.group_count,
            &pattern.compiled.group_names,
        ));
        last = result.end;
        replaced += 1;
    }
    out.extend(chars[last..].iter());
    (out, replaced)
}

fn expand_repl_with_groups(
    repl: &str,
    chars: &[char],
    result: &MatchResult,
    group_count: u32,
    names: &NamedGroups,
) -> String {
    let units: Vec<u32> = repl.chars().map(u32::from).collect();
    expand_match_replacement(
        &units,
        SubjectStorage::Text(chars),
        result,
        group_count,
        names,
    )
    .unwrap()
    .into_iter()
    .map(|unit| char::from_u32(unit).unwrap())
    .collect()
}

#[test]
fn test_split_basic() {
    let result = do_split("\\s+", "foo bar  baz", 0);
    assert_eq!(result, vec!["foo", "bar", "baz"]);
}

#[test]
fn test_split_with_maxsplit() {
    let result = do_split("\\s+", "foo bar  baz qux", 2);
    assert_eq!(result, vec!["foo", "bar", "baz qux"]);
}

#[test]
fn test_split_with_groups() {
    let result = do_split("(\\s+)", "foo bar", 0);
    assert_eq!(result, vec!["foo", " ", "bar"]);
}

#[test]
fn test_split_no_match() {
    let result = do_split("x", "abc", 0);
    assert_eq!(result, vec!["abc"]);
}

#[test]
fn test_sub_basic() {
    let (result, count) = do_sub("\\d+", "NUM", "abc123def456", 0);
    assert_eq!(result, "abcNUMdefNUM");
    assert_eq!(count, 2);
}

#[test]
fn test_sub_with_count() {
    let (result, count) = do_sub("\\d+", "NUM", "abc123def456ghi789", 2);
    assert_eq!(result, "abcNUMdefNUMghi789");
    assert_eq!(count, 2);
}

#[test]
fn test_sub_with_backref() {
    let (result, _count) = do_sub("(\\w+)", "[\\1]", "hello world", 0);
    assert_eq!(result, "[hello] [world]");
}

#[test]
fn test_sub_no_match() {
    let (result, count) = do_sub("xyz", "ABC", "hello world", 0);
    assert_eq!(result, "hello world");
    assert_eq!(count, 0);
}

#[test]
fn test_expand_repl_numbered_group() {
    let result = MatchResult {
        start: 0,
        end: 5,
        groups: vec![None, Some((0, 5))],
        lastindex: Some(1),
    };
    let chars: Vec<char> = "hello".chars().collect();
    let names = NamedGroups::new();
    let expanded = expand_repl_with_groups("<<\\1>>", &chars, &result, 1, &names);
    assert_eq!(expanded, "<<hello>>");
}

#[test]
fn test_expand_repl_named_group() {
    let result = MatchResult {
        start: 0,
        end: 5,
        groups: vec![None, Some((0, 5))],
        lastindex: Some(1),
    };
    let chars: Vec<char> = "hello".chars().collect();
    let mut names = NamedGroups::new();
    names.insert("word".to_string(), 1u32);
    let expanded = expand_repl_with_groups("<<\\g<word>>>", &chars, &result, 1, &names);
    assert_eq!(expanded, "<<hello>>");
}

#[test]
fn test_expand_repl_group_zero() {
    let result = MatchResult {
        start: 0,
        end: 5,
        groups: vec![None],
        lastindex: None,
    };
    let chars: Vec<char> = "hello".chars().collect();
    let names = NamedGroups::new();
    let expanded = expand_repl_with_groups("<<\\g<0>>>", &chars, &result, 0, &names);
    assert_eq!(expanded, "<<hello>>");
}

#[test]
fn test_expand_repl_escape_sequences() {
    let result = MatchResult {
        start: 0,
        end: 0,
        groups: vec![None],
        lastindex: None,
    };
    let chars: Vec<char> = "".chars().collect();
    let names = NamedGroups::new();
    let expanded = expand_repl_with_groups("a\\nb\\tc\\\\d", &chars, &result, 0, &names);
    assert_eq!(expanded, "a\nb\tc\\d");
}

// -----------------------------------------------------------------------
// re_escape tests
// -----------------------------------------------------------------------

#[test]
fn test_escape_no_special() {
    assert_eq!(re_escape_impl(b"hello world"), b"hello\\ world");
}

#[test]
fn test_escape_all_special() {
    assert_eq!(re_escape_impl(b"a.b*c?d+e"), b"a\\.b\\*c\\?d\\+e");
}

#[test]
fn test_escape_brackets_parens() {
    assert_eq!(re_escape_impl(b"[foo](bar)"), b"\\[foo\\]\\(bar\\)");
}

#[test]
fn test_escape_backslash() {
    assert_eq!(re_escape_impl(b"a\\b"), b"a\\\\b");
}

#[test]
fn test_escape_pipe_caret_dollar() {
    assert_eq!(re_escape_impl(b"^a|b$"), b"\\^a\\|b\\$");
}

#[test]
fn test_escape_braces() {
    assert_eq!(re_escape_impl(b"a{1,2}"), b"a\\{1,2\\}");
}

#[test]
fn test_escape_empty() {
    assert_eq!(re_escape_impl(b""), b"");
}

#[test]
fn test_escape_nul() {
    assert_eq!(re_escape_impl(b"a\0b"), b"a\0b");
}

#[test]
fn replacement_failure_roles_and_positions_match_cpython() {
    use super::functions_re::{ReplacementError, re_expand_typed};
    let groups = vec![Some(vec![u32::from(b'a')]), Some(vec![u32::from(b'a')])];
    // Independent CPython 3.12 parser oracles, shared by text and byte templates.
    for (template, expected) in [
        (
            r"\z",
            ReplacementError::Syntax(r"bad escape \z at position 0".to_owned()),
        ),
        (
            r"\9",
            ReplacementError::Syntax("invalid group reference 9 at position 1".to_owned()),
        ),
        (
            r"\g<missing>",
            ReplacementError::Index("unknown group name 'missing'".to_owned()),
        ),
        (
            r"\777",
            ReplacementError::Syntax(
                r"octal escape value \777 outside of range 0-0o377 at position 0".to_owned(),
            ),
        ),
    ] {
        let units: Vec<u32> = template.chars().map(u32::from).collect();
        let error = re_expand_typed(&units, &groups, &NamedGroups::new()).unwrap_err();
        assert_eq!(error, expected);
    }
}

#[test]
fn replacement_units_keep_surrogates_and_reject_surrogate_group_names() {
    use super::functions_re::{ReplacementError, re_expand_typed};

    // CPython: (?P<value>.+)(x)? has equal whole-match/group-1 values
    // and an unmatched group 2. Keep ordinal 0 distinct from the named group.
    let value = vec![0xD800, 0xDFFF, 0xD83D, 0xDE00, 0x1F600];
    let groups = vec![Some(value.clone()), Some(value), None];
    let mut names = NamedGroups::new();
    names.insert("value".to_owned(), 1);
    let template: Vec<u32> = r"<\g<value>|\1|\2>".chars().map(u32::from).collect();
    assert_eq!(
        re_expand_typed(&template, &groups, &names).unwrap(),
        vec![
            b'<' as u32,
            0xD800,
            0xDFFF,
            0xD83D,
            0xDE00,
            0x1F600,
            b'|' as u32,
            0xD800,
            0xDFFF,
            0xD83D,
            0xDE00,
            0x1F600,
            b'|' as u32,
            b'>' as u32,
        ]
    );

    let invalid_name = vec![
        u32::from(b'\\'),
        u32::from(b'g'),
        u32::from(b'<'),
        u32::from(b'u'),
        u32::from(b'n'),
        u32::from(b'k'),
        u32::from(b'n'),
        u32::from(b'o'),
        u32::from(b'w'),
        u32::from(b'n'),
        0xD800,
        u32::from(b'>'),
    ];
    let error = re_expand_typed(&invalid_name, &groups, &names).unwrap_err();
    assert_eq!(
        error,
        ReplacementError::InvalidGroupName {
            name: vec![
                u32::from(b'u'),
                u32::from(b'n'),
                u32::from(b'k'),
                u32::from(b'n'),
                u32::from(b'o'),
                u32::from(b'w'),
                u32::from(b'n'),
                0xD800,
            ],
            position: 3,
        }
    );
}
