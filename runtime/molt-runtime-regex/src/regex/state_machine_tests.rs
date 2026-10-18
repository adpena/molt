use super::{
    CompiledPattern, MatchMode, MatchResult, execute_match, execute_match_bytes,
    parse_pattern_typed,
};

type Span = (usize, usize);
type ExpectedMatch<'a> = (Span, &'a [Option<Span>], Option<u32>);
type CpythonCase<'a> = (&'a str, &'a str, &'a str, Option<ExpectedMatch<'a>>);

/// Literal observations retained from CPython 3.12.15, 3.13.12 and 3.14.4t in
/// re_parity.py continuation/scanner groups. CPython sre_lib.h MARK
/// closing and LASTMARK_RESTORE own lastindex; SUCCESS owns fullmatch and
/// must_advance. These are execution oracles, independent of instruction shape.
fn assert_cpython_case(
    pattern: &str,
    text: &str,
    mode: &str,
    window: Span,
    expected: Option<ExpectedMatch<'_>>,
) {
    for is_bytes in [false, true] {
        let compiled = parse_pattern_typed(pattern, 0, is_bytes)
            .unwrap()
            .compile()
            .unwrap();
        let found = if is_bytes {
            execute_match_bytes(
                &compiled,
                text.as_bytes(),
                window.0,
                window.1,
                MatchMode::try_from(mode).unwrap(),
            )
        } else {
            execute_match(
                &compiled,
                text,
                window.0,
                window.1,
                MatchMode::try_from(mode).unwrap(),
            )
        }
        .unwrap();
        let context = format!("{pattern:?} {text:?} {mode} {window:?} bytes={is_bytes}");
        if let Some((span, groups, lastindex)) = expected {
            let found = found.unwrap_or_else(|| panic!("missing match: {context}"));
            assert_eq!((found.start, found.end), span, "{context}");
            let (unused, actual_groups) = found.groups.split_first().expect("group-zero slot");
            assert_eq!(*unused, None, "group zero is not a capture: {context}");
            assert_eq!(actual_groups, groups, "{context}");
            assert_eq!(found.lastindex, lastindex, "{context}");
        } else {
            assert!(found.is_none(), "unexpected match: {context}");
        }
    }
}

#[test]
fn cpython_continuations_restore_captures_and_closing_order() {
    let cases: &[CpythonCase<'_>] = &[
        (
            "(a|ab)",
            "ab",
            "match",
            Some(((0, 1), &[Some((0, 1))], Some(1))),
        ),
        (
            "(a|ab)",
            "ab",
            "fullmatch",
            Some(((0, 2), &[Some((0, 2))], Some(1))),
        ),
        ("(?i:a*)a", "Aaa", "fullmatch", Some(((0, 3), &[], None))),
        ("(?i:a*)a", "AA", "fullmatch", None),
        (
            "((a){1,3}a)",
            "aaa",
            "fullmatch",
            Some(((0, 3), &[Some((0, 3)), Some((1, 2))], Some(1))),
        ),
        (
            "((a)|(ab))c",
            "abc",
            "fullmatch",
            Some(((0, 3), &[Some((0, 2)), None, Some((0, 2))], Some(1))),
        ),
        (
            "((a|ab)+)b",
            "aabb",
            "fullmatch",
            Some(((0, 4), &[Some((0, 3)), Some((1, 3))], Some(1))),
        ),
        (
            "(a(b)?)+c",
            "abac",
            "fullmatch",
            Some(((0, 4), &[Some((2, 3)), Some((1, 2))], Some(1))),
        ),
        (
            r"((a){1,3}?)(\2)",
            "aaa",
            "fullmatch",
            Some(((0, 3), &[Some((0, 2)), Some((1, 2)), Some((2, 3))], Some(3))),
        ),
        (
            "(a?)*",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((1, 1))], Some(1))),
        ),
        (
            "(?P<outer>(?P<inner>a))",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 1)), Some((0, 1))], Some(1))),
        ),
        (
            r"(?=(a+))a*\1",
            "aaaa",
            "fullmatch",
            Some(((0, 4), &[Some((0, 4))], Some(1))),
        ),
        (r"(?=(a|ab))\1c", "abc", "fullmatch", None),
        (
            r"(?=(ab|a))\1c",
            "abc",
            "fullmatch",
            Some(((0, 3), &[Some((0, 2))], Some(1))),
        ),
        (
            "(?:(?!(a))b|(a))",
            "a",
            "fullmatch",
            Some(((0, 1), &[None, Some((0, 1))], Some(2))),
        ),
        (
            "(?:(a)c|a)(?(1)b|d)",
            "ad",
            "fullmatch",
            Some(((0, 2), &[None], None)),
        ),
        (
            r"(a|ab)\1",
            "abab",
            "fullmatch",
            Some(((0, 4), &[Some((0, 2))], Some(1))),
        ),
        (
            "(?<=(a))b",
            "ab",
            "search",
            Some(((1, 2), &[Some((0, 1))], Some(1))),
        ),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}

#[test]
fn cpython_nonempty_retry_and_bounds_are_engine_success_constraints() {
    let pattern = "(?P<empty>)|(?P<run>a+)";
    assert_cpython_case(
        pattern,
        "aa",
        "search",
        (0, 2),
        Some(((0, 0), &[Some((0, 0)), None], Some(1))),
    );
    assert_cpython_case(
        pattern,
        "aa",
        "search_nonempty",
        (0, 2),
        Some(((0, 2), &[None, Some((0, 2))], Some(2))),
    );
    assert_cpython_case(
        pattern,
        "aa",
        "search",
        (2, 2),
        Some(((2, 2), &[Some((2, 2)), None], Some(1))),
    );
    assert_cpython_case(pattern, "aa", "search_nonempty", (2, 2), None);
    assert_cpython_case("a*?", "aa", "search", (0, 2), Some(((0, 0), &[], None)));
    assert_cpython_case(
        "a*?",
        "aa",
        "search_nonempty",
        (0, 2),
        Some(((0, 1), &[], None)),
    );
    assert_cpython_case(
        "(?=(a))|a",
        "a",
        "search",
        (0, 1),
        Some(((0, 0), &[Some((0, 1))], Some(1))),
    );
    assert_cpython_case(
        "(?=(a))|a",
        "a",
        "search_nonempty",
        (0, 1),
        Some(((0, 1), &[None], None)),
    );
    assert_cpython_case(
        "(|a)",
        "zaaz",
        "search_nonempty",
        (1, 3),
        Some(((1, 2), &[Some((1, 2))], Some(1))),
    );
    assert_cpython_case(
        "(|a)",
        "zaaz",
        "match",
        (3, 1),
        Some(((3, 3), &[Some((3, 3))], Some(1))),
    );
    for mode in ["fullmatch", "search", "search_nonempty"] {
        assert_cpython_case("(|a)", "zaaz", mode, (3, 1), None);
    }
}

#[test]
fn cpython_nested_assertions_keep_the_subject_window_and_atomic_capture_state() {
    let cases: &[CpythonCase<'_>] = &[
        (
            "(?<=a(?=(b)))b",
            "ab",
            "search",
            Some(((1, 2), &[Some((1, 2))], Some(1))),
        ),
        (r"(?=(a(?=(b|bc))))a\2c", "abcc", "fullmatch", None),
        (
            r"(?=(a(?=(bc|b))))a\2c",
            "abcc",
            "fullmatch",
            Some(((0, 4), &[Some((0, 1)), Some((1, 3))], Some(1))),
        ),
        (
            "(?:(?=(a(?!(b))))ac|a(b))",
            "ab",
            "fullmatch",
            Some(((0, 2), &[None, None, Some((1, 2))], Some(3))),
        ),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}

#[test]
fn cpython_nested_repeat_minimums_and_fixed_width_capture_offsets() {
    assert_cpython_case(
        "((a?){2}){2}a",
        "a",
        "fullmatch",
        (0, 1),
        Some(((0, 1), &[Some((0, 0)), Some((0, 0))], Some(1))),
    );
    // Both retry directions must reconstruct the selected last iteration,
    // including empty captures before and after its consuming atom.
    for pattern in ["(()(a)())+a", "(()(a)())+?a"] {
        assert_cpython_case(
            pattern,
            "aaa",
            "fullmatch",
            (0, 3),
            Some((
                (0, 3),
                &[Some((1, 2)), Some((1, 1)), Some((1, 2)), Some((2, 2))],
                Some(1),
            )),
        );
    }
}

#[test]
fn cpython_absolute_end_and_word_boundaries_use_the_original_window() {
    // Unlike $, \Z cannot stop before a trailing newline, but an explicit
    // endpos still limits the subject window used by the absolute-end test.
    assert_cpython_case(r"a\Z", "a\n", "match", (0, 2), None);
    assert_cpython_case(r"a\Z", "a\n", "match", (0, 1), Some(((0, 1), &[], None)));
    assert_cpython_case(r"\ba\b", "ab", "match", (0, 1), Some(((0, 1), &[], None)));
    // pos is not a synthetic start of the string: the preceding word unit
    // remains visible even though it lies outside the matching window.
    assert_cpython_case(r"\bb", "ab", "match", (1, 2), None);
}

#[test]
fn cpython_class_category_negation_and_literal_nested_set_spelling() {
    let cases: &[CpythonCase<'_>] = &[
        (r"[\D]", "a", "fullmatch", Some(((0, 1), &[], None))),
        (r"[\S\d]", "a", "fullmatch", Some(((0, 1), &[], None))),
        (r"[\S\d]", "\n", "fullmatch", None),
        (r"[^\D]", "7", "fullmatch", Some(((0, 1), &[], None))),
        (r"[\W_]+", "!?_", "fullmatch", Some(((0, 3), &[], None))),
        // CPython warns about the nested-set spelling; it is not a POSIX
        // category extension. The first ] closes the class; the next is literal.
        ("[[:alpha:]]", "a]", "fullmatch", Some(((0, 2), &[], None))),
        ("[[:alpha:]]", "a", "fullmatch", None),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}

#[test]
#[ignore = "development component measurement; not release performance acceptance"]
fn long_subject_matching_components() {
    use std::hint::black_box;
    use std::time::Instant;

    const ITERATIONS: usize = 32;
    let text = "a".repeat(20_000);
    for (pattern, mode) in [("ab", "search"), ("a*", "fullmatch"), ("(a)+", "fullmatch")] {
        let compiled = parse_pattern_typed(pattern, 0, false)
            .unwrap()
            .compile()
            .unwrap();
        let mut matches = 0usize;
        let mut matched_ends = 0usize;
        let mut capture_slots = 0usize;
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let result = black_box(
                execute_match(
                    black_box(&compiled),
                    black_box(text.as_str()),
                    0,
                    text.len(),
                    MatchMode::try_from(mode).unwrap(),
                )
                .unwrap(),
            );
            if let Some(found) = result {
                matches += 1;
                matched_ends += found.end;
                capture_slots += found.groups.len();
            }
        }
        let elapsed = start.elapsed();
        println!(
            "regex_components pattern={pattern:?} mode={mode} subject_bytes={} \
             iterations={ITERATIONS} matches={matches} matched_ends={matched_ends} \
             capture_slots={capture_slots} wall_ns={} ns_per_iteration={:.1} \
             compiled_header_bytes={} result_header_bytes={}",
            text.len(),
            elapsed.as_nanos(),
            elapsed.as_nanos() as f64 / ITERATIONS as f64,
            std::mem::size_of::<CompiledPattern>(),
            std::mem::size_of::<MatchResult>(),
        );
    }
}

#[test]
fn long_concat_executes_without_matcher_recursion_or_ast_tail_copies() {
    let pattern = ".".repeat(6000);
    let text = "a".repeat(6000);
    for is_bytes in [false, true] {
        let compiled = parse_pattern_typed(&pattern, 0, is_bytes)
            .unwrap()
            .compile()
            .unwrap();
        let found = if is_bytes {
            execute_match_bytes(
                &compiled,
                text.as_bytes(),
                0,
                text.len(),
                MatchMode::FullMatch,
            )
        } else {
            execute_match(&compiled, &text, 0, text.len(), MatchMode::FullMatch)
        }
        .unwrap()
        .unwrap();
        assert_eq!((found.start, found.end, found.lastindex), (0, 6000, None));
    }
}

#[test]
fn cpython_optional_progress_excludes_mandatory_iterations() {
    let cases: &[CpythonCase<'_>] = &[
        (
            "(?:(|a)){1,2}c",
            "ac",
            "fullmatch",
            Some(((0, 2), &[Some((0, 1))], Some(1))),
        ),
        (
            "(?:(|a)){1,2}?c",
            "ac",
            "fullmatch",
            Some(((0, 2), &[Some((0, 1))], Some(1))),
        ),
        (
            "(a?){1,2}",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((1, 1))], Some(1))),
        ),
        (
            "(a?){1,2}?",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 1))], Some(1))),
        ),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}

#[test]
fn cpython_conditionals_read_current_start_and_previous_end_marks() {
    let cases: &[CpythonCase<'_>] = &[
        (
            "(?:(a(?(1)b|c))x)+",
            "acxacx",
            "match",
            Some(((0, 6), &[Some((3, 5))], Some(1))),
        ),
        (
            "(a(?(1)b|c))+",
            "acab",
            "fullmatch",
            Some(((0, 4), &[Some((2, 4))], Some(1))),
        ),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}
