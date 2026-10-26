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
        let compiled = parse_pattern_typed(pattern.chars().map(u32::from).collect(), 0, is_bytes)
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
                text.as_bytes(),
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
        let compiled = parse_pattern_typed(pattern.chars().map(u32::from).collect(), 0, false)
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
                    black_box(text.as_bytes()),
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
        let compiled = parse_pattern_typed(pattern.chars().map(u32::from).collect(), 0, is_bytes)
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
            execute_match(
                &compiled,
                text.as_bytes(),
                0,
                text.len(),
                MatchMode::FullMatch,
            )
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

// Projection of independently observed CPython 3.12.15 records.
// Existing assert_cpython_case executes each literal against text and bytes.
// Equal result tuples across modes share one vector; original records keep all modes.
#[test]
fn cpython_atomic_and_possessive_scope_contracts() {
    let cases: &[CpythonCase<'_>] = &[
        // ordinary-star-control; equivalent observed modes: fullmatch, match, search
        ("a*a", "aa", "fullmatch", Some(((0, 2), &[], None))),
        // possessive-star; equivalent observed modes: fullmatch, match, search
        ("a*+a", "aa", "fullmatch", None),
        // possessive-plus; equivalent observed modes: fullmatch, match, search
        ("a++a", "aa", "fullmatch", None),
        // possessive-optional; equivalent observed modes: fullmatch, match, search
        ("a?+a", "a", "fullmatch", None),
        // possessive-bounded; equivalent observed modes: fullmatch, match, search
        ("a{1,2}+a", "aa", "fullmatch", None),
        // possessive-tail-hit; equivalent observed modes: fullmatch, match, search
        ("a{1,2}+b", "aab", "fullmatch", Some(((0, 3), &[], None))),
        // possessive-zero-count; equivalent observed modes: fullmatch, match, search
        ("(a){0}+a", "a", "fullmatch", Some(((0, 1), &[None], None))),
        // ordinary-alternative-control; equivalent observed modes: fullmatch
        ("(?:a|ab)b", "abb", "fullmatch", Some(((0, 3), &[], None))),
        // ordinary-alternative-control; equivalent observed modes: match, search
        ("(?:a|ab)b", "abb", "match", Some(((0, 2), &[], None))),
        // atomic-alternative; equivalent observed modes: fullmatch
        ("(?>a|ab)b", "abb", "fullmatch", None),
        // atomic-alternative; equivalent observed modes: match, search
        ("(?>a|ab)b", "abb", "match", Some(((0, 2), &[], None))),
        // atomic-alternative-priority; equivalent observed modes: fullmatch, match, search
        ("(?>ab|a)b", "abb", "fullmatch", Some(((0, 3), &[], None))),
        // atomic-outer-rescue; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?>a|ab)c|ab)",
            "ab",
            "fullmatch",
            Some(((0, 2), &[], None)),
        ),
        // possessive-minimum; equivalent observed modes: fullmatch, match, search
        ("(?:a|ab){2}+", "abab", "fullmatch", None),
        // atomic-repeat-minimum; equivalent observed modes: fullmatch
        ("(?>(?:a|ab){2})", "abab", "fullmatch", None),
        // atomic-repeat-minimum; equivalent observed modes: match, search
        (
            "(?>(?:a|ab){2})",
            "abab",
            "match",
            Some(((0, 3), &[], None)),
        ),
        // atomic-tail-inside; equivalent observed modes: fullmatch, match, search
        (
            "(?>(?:a|ab){2}b)",
            "abab",
            "fullmatch",
            Some(((0, 4), &[], None)),
        ),
        // possessive-minimum-hit; equivalent observed modes: fullmatch, match, search
        (
            "(?:a|ab){2}+b",
            "aab",
            "fullmatch",
            Some(((0, 3), &[], None)),
        ),
        // possessive-empty-star; equivalent observed modes: fullmatch, match, search
        (
            "()*+a",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 0))], Some(1))),
        ),
        // possessive-empty-counted; equivalent observed modes: fullmatch, match, search
        (
            "(){2,3}+a",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 0))], Some(1))),
        ),
        // possessive-empty-assertion; equivalent observed modes: fullmatch, match, search
        (
            "(?=(a)){2,3}+a",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 1))], Some(1))),
        ),
        // possessive-nullable-minimum; equivalent observed modes: fullmatch, match
        ("(|a){1,2}+c", "ac", "fullmatch", None),
        // possessive-nullable-minimum; equivalent observed modes: search
        (
            "(|a){1,2}+c",
            "ac",
            "search",
            Some(((1, 2), &[Some((1, 1))], Some(1))),
        ),
        // atomic-nullable-tail-inside; equivalent observed modes: fullmatch, match, search
        (
            "(?>(|a){1,2}c)",
            "ac",
            "fullmatch",
            Some(((0, 2), &[Some((0, 1))], Some(1))),
        ),
        // possessive-optional-capture-undo; equivalent observed modes: fullmatch, match, search
        (
            "(a(b)?){1,3}+c",
            "abac",
            "fullmatch",
            Some(((0, 4), &[Some((2, 3)), Some((1, 2))], Some(1))),
        ),
        // possessive-closing-order; equivalent observed modes: fullmatch, match, search
        (
            "(?P<outer>(?P<inner>a))++b",
            "aab",
            "fullmatch",
            Some(((0, 3), &[Some((1, 2)), Some((1, 2))], Some(1))),
        ),
        // atomic-failed-outer-branch; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?>(a))c|(ab))",
            "ab",
            "fullmatch",
            Some(((0, 2), &[None, Some((0, 2))], Some(2))),
        ),
        // atomic-positive-assertion; equivalent observed modes: fullmatch, match, search
        ("(?=(?>(a|ab)))\\1c", "abc", "fullmatch", None),
        // atomic-positive-priority; equivalent observed modes: fullmatch, match, search
        (
            "(?=(?>(ab|a)))\\1c",
            "abc",
            "fullmatch",
            Some(((0, 3), &[Some((0, 2))], Some(1))),
        ),
        // negative-ordinary-control; equivalent observed modes: fullmatch, match, search
        ("(?!(a|ab)c)(abc)", "abc", "fullmatch", None),
        // negative-atomic-undo; equivalent observed modes: fullmatch, match, search
        (
            "(?!(?>(a|ab))c)(abc)",
            "abc",
            "fullmatch",
            Some(((0, 3), &[None, Some((0, 3))], Some(2))),
        ),
        // atomic-conditional-undo; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?>(a))c|a)(?(1)b|d)",
            "ad",
            "fullmatch",
            Some(((0, 2), &[None], None)),
        ),
        // possessive-open-group-conditional; equivalent observed modes: fullmatch, match, search
        (
            "(?:(a(?(1)b|c))x){2}+",
            "acxacx",
            "fullmatch",
            Some(((0, 6), &[Some((3, 5))], Some(1))),
        ),
        // possessive-failed-iteration-conditional; equivalent observed modes: fullmatch, match, search
        (
            "(?:(a)b)*+(?(1)c|d)",
            "d",
            "fullmatch",
            Some(((0, 1), &[None], None)),
        ),
        // ordinary-backreference-control; equivalent observed modes: fullmatch, match, search
        (
            "(a|ab)\\1",
            "abab",
            "fullmatch",
            Some(((0, 4), &[Some((0, 2))], Some(1))),
        ),
        // atomic-backreference; equivalent observed modes: fullmatch, match, search
        ("(?>(a|ab))\\1", "abab", "fullmatch", None),
        // atomic-backreference-priority; equivalent observed modes: fullmatch, match, search
        (
            "(?>(ab|a))\\1",
            "abab",
            "fullmatch",
            Some(((0, 4), &[Some((0, 2))], Some(1))),
        ),
        // nested-atomic-first-branch; equivalent observed modes: fullmatch, match, search
        ("(?>(?>(a|ab))b)c", "abbc", "fullmatch", None),
        // nested-atomic-priority; equivalent observed modes: fullmatch, match, search
        (
            "(?>(?>(ab|a))b)c",
            "abbc",
            "fullmatch",
            Some(((0, 4), &[Some((0, 2))], Some(1))),
        ),
        // atomic-lookbehind-width; equivalent observed modes: fullmatch, match
        ("(?<=(?>ab))c", "abc", "fullmatch", None),
        // atomic-lookbehind-width; equivalent observed modes: search
        ("(?<=(?>ab))c", "abc", "search", Some(((2, 3), &[], None))),
        // possessive-lookbehind-width; equivalent observed modes: fullmatch, match
        ("(?<=a{2}+)b", "aab", "fullmatch", None),
        // possessive-lookbehind-width; equivalent observed modes: search
        ("(?<=a{2}+)b", "aab", "search", Some(((2, 3), &[], None))),
        // atomic-inner-lazy; equivalent observed modes: fullmatch, match, search
        (
            "(?s:(?>.*?\\.).*)\\Z",
            "a.txt",
            "fullmatch",
            Some(((0, 5), &[], None)),
        ),
        // possessive-partial-optional-undo; equivalent observed modes: fullmatch, match, search
        (
            "(?:(a)(b)){1,3}+ac",
            "abac",
            "fullmatch",
            Some(((0, 4), &[Some((0, 1)), Some((1, 2))], Some(2))),
        ),
        // possessive-outer-choice-undo; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?:(a)(b)){1,3}+x|(abac))",
            "abac",
            "fullmatch",
            Some(((0, 4), &[None, None, Some((0, 4))], Some(3))),
        ),
        // atomic-possessive-outer-choice-undo; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?>(?:(a)(b)){1,3}+)x|(abac))",
            "abac",
            "fullmatch",
            Some(((0, 4), &[None, None, Some((0, 4))], Some(3))),
        ),
        // possessive-inside-ordinary-repeat; equivalent observed modes: fullmatch, match, search
        (
            "((a?)*+){1,2}b",
            "aab",
            "fullmatch",
            Some(((0, 3), &[Some((2, 2)), Some((2, 2))], Some(1))),
        ),
        // possessive-inside-atomic; equivalent observed modes: fullmatch, match, search
        (
            "(?>(a?)*+)b",
            "aab",
            "fullmatch",
            Some(((0, 3), &[Some((2, 2))], Some(1))),
        ),
        // possessive-inside-positive-assertion; equivalent observed modes: fullmatch, match, search
        (
            "(?=((a?)*+)b)aab",
            "aab",
            "fullmatch",
            Some(((0, 3), &[Some((0, 2)), Some((2, 2))], Some(1))),
        ),
        // possessive-inside-negative-assertion; equivalent observed modes: fullmatch, match, search
        (
            "(?!(?:(a?)*+)c)(aab)",
            "aab",
            "fullmatch",
            Some(((0, 3), &[None, Some((0, 3))], Some(2))),
        ),
        // possessive-first-optional-after-empty-minimum; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?(1)a|())){1,2}+",
            "a",
            "fullmatch",
            Some(((0, 1), &[Some((0, 0))], Some(1))),
        ),
        // possessive-first-optional-after-counted-minimum; equivalent observed modes: fullmatch, match, search
        (
            "(?:(?(1)a|())){2,3}+",
            "aa",
            "fullmatch",
            Some(((0, 2), &[Some((0, 0))], Some(1))),
        ),
    ];
    for &(pattern, text, mode, expected) in cases {
        assert_cpython_case(pattern, text, mode, (0, text.len()), expected);
    }
}

/// CPython observations distinguish surrogates, replacement and supplementary
/// scalars through the real storage preparation, parser and execution pipeline.
#[test]
fn cpython_raw_codepoint_identity_and_offsets() {
    // Literal CPython 3.12.15 observations, independent of implementation.
    // Fields: pattern code points, subject code points, flags, mode, window, expected.
    // Text values include 0xD800..0xDFFF: never convert through Rust char or lossy String.
    // Data only; parent uses parse_pattern_typed(pattern.to_vec(), i64::from(flags), false)
    // and execute_match_subject with SubjectStorage::Text(subject).
    type CodepointCase<'a> = (
        &'a [u32],
        &'a [u32],
        u32,
        &'a str,
        Span,
        Option<ExpectedMatch<'a>>,
    );
    let cases: &[CodepointCase<'_>] = &[
        // high-literal; equivalent modes: fullmatch, match, search
        (
            &[0xD83D],
            &[0xD83D],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // high-is-not-replacement; equivalent modes: fullmatch, match, search
        (&[0xD83D], &[0xFFFD], 0, "fullmatch", (0, 1), None),
        // replacement-is-not-high; equivalent modes: fullmatch, match, search
        (&[0xFFFD], &[0xD83D], 0, "fullmatch", (0, 1), None),
        // low-literal; equivalent modes: fullmatch, match, search
        (
            &[0xDE00],
            &[0xDE00],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // pair-literal; equivalent modes: fullmatch, match, search
        (
            &[0xD83D, 0xDE00],
            &[0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[], None)),
        ),
        // pair-is-not-scalar; equivalent modes: fullmatch, match, search
        (&[0xD83D, 0xDE00], &[0x1F600], 0, "fullmatch", (0, 1), None),
        // scalar-is-not-pair; equivalent modes: fullmatch, match, search
        (&[0x1F600], &[0xD83D, 0xDE00], 0, "fullmatch", (0, 2), None),
        // scalar-literal; equivalent modes: fullmatch, match, search
        (
            &[0x1F600],
            &[0x1F600],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // class-high-is-not-replacement; equivalent modes: fullmatch, match, search
        (
            &[0x5B, 0xD83D, 0x5D],
            &[0xFFFD],
            0,
            "fullmatch",
            (0, 1),
            None,
        ),
        // class-high-search-offset; equivalent modes: fullmatch, match
        (
            &[0x5B, 0xD83D, 0x5D],
            &[0xFFFD, 0xD83D],
            0,
            "fullmatch",
            (0, 2),
            None,
        ),
        // class-high-search-offset; equivalent modes: search
        (
            &[0x5B, 0xD83D, 0x5D],
            &[0xFFFD, 0xD83D],
            0,
            "search",
            (0, 2),
            Some(((1, 2), &[], None)),
        ),
        // class-surrogate-range; equivalent modes: fullmatch, match, search
        (
            &[0x5B, 0xD800, 0x2D, 0xDFFF, 0x5D, 0x2B],
            &[0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[], None)),
        ),
        // class-cross-surrogate-boundary; equivalent modes: fullmatch, match, search
        (
            &[0x5B, 0xD7FF, 0x2D, 0xD801, 0x5D],
            &[0xD800],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // class-boundary-excludes-replacement; equivalent modes: fullmatch, match, search
        (
            &[0x5B, 0xD7FF, 0x2D, 0xD801, 0x5D],
            &[0xFFFD],
            0,
            "fullmatch",
            (0, 1),
            None,
        ),
        // negated-high-class; equivalent modes: fullmatch, match
        (
            &[0x5B, 0x5E, 0xD83D, 0x5D, 0x2B],
            &[0xD83D, 0xDE00, 0xFFFD],
            0,
            "fullmatch",
            (0, 3),
            None,
        ),
        // negated-high-class; equivalent modes: search
        (
            &[0x5B, 0x5E, 0xD83D, 0x5D, 0x2B],
            &[0xD83D, 0xDE00, 0xFFFD],
            0,
            "search",
            (0, 3),
            Some(((1, 3), &[], None)),
        ),
        // union-excludes-other-surrogate; equivalent modes: fullmatch, match, search
        (
            &[0x5B, 0xD83D, 0xFFFD, 0x5D],
            &[0xDE00],
            0,
            "fullmatch",
            (0, 1),
            None,
        ),
        // dot-high; equivalent modes: fullmatch, match, search
        (
            &[0x2E],
            &[0xD83D],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // two-dots-pair; equivalent modes: fullmatch, match, search
        (
            &[0x2E, 0x2E],
            &[0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[], None)),
        ),
        // two-dots-scalar; equivalent modes: fullmatch, match, search
        (&[0x2E, 0x2E], &[0x1F600], 0, "fullmatch", (0, 1), None),
        // captured-pair-units; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0x2E, 0x29, 0x28, 0x2E, 0x29],
            &[0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[Some((0, 1)), Some((1, 2))], Some(2))),
        ),
        // raw-backreference; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xD83D],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[Some((0, 1))], Some(1))),
        ),
        // raw-backreference-distinct; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xFFFD],
            0,
            "fullmatch",
            (0, 2),
            None,
        ),
        // pair-backreference; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0xD83D, 0xDE00, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xDE00, 0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 4),
            Some(((0, 4), &[Some((0, 2))], Some(1))),
        ),
        // unit-lookbehind; equivalent modes: fullmatch, match
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0xD83D, 0x29, 0xDE00],
            &[0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 2),
            None,
        ),
        // unit-lookbehind; equivalent modes: search
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0xD83D, 0x29, 0xDE00],
            &[0xD83D, 0xDE00],
            0,
            "search",
            (0, 2),
            Some(((1, 2), &[], None)),
        ),
        // pair-lookbehind; equivalent modes: fullmatch, match
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0xD83D, 0xDE00, 0x29, 0x78],
            &[0xD83D, 0xDE00, 0x78],
            0,
            "fullmatch",
            (0, 3),
            None,
        ),
        // pair-lookbehind; equivalent modes: search
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0xD83D, 0xDE00, 0x29, 0x78],
            &[0xD83D, 0xDE00, 0x78],
            0,
            "search",
            (0, 3),
            Some(((2, 3), &[], None)),
        ),
        // pair-lookbehind-not-scalar; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0xD83D, 0xDE00, 0x29, 0x78],
            &[0x1F600, 0x78],
            0,
            "fullmatch",
            (0, 2),
            None,
        ),
        // scalar-lookbehind; equivalent modes: fullmatch, match
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0x1F600, 0x29, 0x78],
            &[0x1F600, 0x78],
            0,
            "fullmatch",
            (0, 2),
            None,
        ),
        // scalar-lookbehind; equivalent modes: search
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0x1F600, 0x29, 0x78],
            &[0x1F600, 0x78],
            0,
            "search",
            (0, 2),
            Some(((1, 2), &[], None)),
        ),
        // scalar-lookbehind-not-pair; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0x3F, 0x3C, 0x3D, 0x1F600, 0x29, 0x78],
            &[0xD83D, 0xDE00, 0x78],
            0,
            "fullmatch",
            (0, 3),
            None,
        ),
        // captured-raw-repeat; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0xD83D, 0x29, 0x7B, 0x31, 0x2C, 0x32, 0x7D],
            &[0xD83D, 0xD83D],
            0,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[Some((1, 2))], Some(1))),
        ),
        // captured-raw-possessive; equivalent modes: fullmatch, match, search
        (
            &[0x28, 0xD83D, 0x29, 0x2B, 0x2B, 0x78],
            &[0xD83D, 0xD83D, 0x78],
            0,
            "fullmatch",
            (0, 3),
            Some(((0, 3), &[Some((1, 2))], Some(1))),
        ),
        // ordinary-raw-priority-control; equivalent modes: fullmatch
        (
            &[0x28, 0x3F, 0x3A, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0xDE00],
            &[0xD83D, 0xDE00, 0xDE00],
            0,
            "fullmatch",
            (0, 3),
            Some(((0, 3), &[], None)),
        ),
        // ordinary-raw-priority-control; equivalent modes: match, search
        (
            &[0x28, 0x3F, 0x3A, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0xDE00],
            &[0xD83D, 0xDE00, 0xDE00],
            0,
            "match",
            (0, 3),
            Some(((0, 2), &[], None)),
        ),
        // atomic-raw-priority; equivalent modes: fullmatch
        (
            &[0x28, 0x3F, 0x3E, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0xDE00],
            &[0xD83D, 0xDE00, 0xDE00],
            0,
            "fullmatch",
            (0, 3),
            None,
        ),
        // atomic-raw-priority; equivalent modes: match, search
        (
            &[0x28, 0x3F, 0x3E, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0xDE00],
            &[0xD83D, 0xDE00, 0xDE00],
            0,
            "match",
            (0, 3),
            Some(((0, 2), &[], None)),
        ),
        // possessive-raw-minimum; equivalent modes: fullmatch, match, search
        (
            &[
                0x28, 0x3F, 0x3A, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0x7B, 0x32, 0x7D, 0x2B,
            ],
            &[0xD83D, 0xDE00, 0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 4),
            None,
        ),
        // atomic-raw-minimum; equivalent modes: fullmatch
        (
            &[
                0x28, 0x3F, 0x3E, 0x28, 0x3F, 0x3A, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0x7B, 0x32,
                0x7D, 0x29,
            ],
            &[0xD83D, 0xDE00, 0xD83D, 0xDE00],
            0,
            "fullmatch",
            (0, 4),
            None,
        ),
        // atomic-raw-minimum; equivalent modes: match, search
        (
            &[
                0x28, 0x3F, 0x3E, 0x28, 0x3F, 0x3A, 0xD83D, 0x7C, 0xD83D, 0xDE00, 0x29, 0x7B, 0x32,
                0x7D, 0x29,
            ],
            &[0xD83D, 0xDE00, 0xD83D, 0xDE00],
            0,
            "match",
            (0, 4),
            Some(((0, 3), &[], None)),
        ),
        // surrogate-word-category; equivalent modes: fullmatch, match, search
        (&[0x5C, 0x77], &[0xD83D], 0, "fullmatch", (0, 1), None),
        // surrogate-nonword-category; equivalent modes: fullmatch, match, search
        (
            &[0x5C, 0x57],
            &[0xD83D],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // surrogate-digit-category; equivalent modes: fullmatch, match, search
        (&[0x5C, 0x64], &[0xDE00], 0, "fullmatch", (0, 1), None),
        // surrogate-nondigit-category; equivalent modes: fullmatch, match, search
        (
            &[0x5C, 0x44],
            &[0xDE00],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // surrogate-space-category; equivalent modes: fullmatch, match, search
        (&[0x5C, 0x73], &[0xD83D], 0, "fullmatch", (0, 1), None),
        // surrogate-nonspace-category; equivalent modes: fullmatch, match, search
        (
            &[0x5C, 0x53],
            &[0xD83D],
            0,
            "fullmatch",
            (0, 1),
            Some(((0, 1), &[], None)),
        ),
        // literal-high-remains-distinct; equivalent modes: fullmatch, match
        (
            &[0xD83D],
            &[0xDE00, 0xFFFD, 0xD83D],
            2,
            "fullmatch",
            (0, 3),
            None,
        ),
        // literal-high-remains-distinct; equivalent modes: search
        (
            &[0xD83D],
            &[0xDE00, 0xFFFD, 0xD83D],
            2,
            "search",
            (0, 3),
            Some(((2, 3), &[], None)),
        ),
        // raw-range-remains-distinct; equivalent modes: fullmatch, match
        (
            &[0x5B, 0xD800, 0x2D, 0xDFFF, 0x5D, 0x2B],
            &[0xFFFD, 0xD83D, 0xDE00, 0xFFFD],
            2,
            "fullmatch",
            (0, 4),
            None,
        ),
        // raw-range-remains-distinct; equivalent modes: search
        (
            &[0x5B, 0xD800, 0x2D, 0xDFFF, 0x5D, 0x2B],
            &[0xFFFD, 0xD83D, 0xDE00, 0xFFFD],
            2,
            "search",
            (0, 4),
            Some(((1, 3), &[], None)),
        ),
        // backreference-keeps-raw-equality; equivalent modes: fullmatch, match
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xDE00, 0xD83D, 0xD83D],
            2,
            "fullmatch",
            (0, 4),
            None,
        ),
        // backreference-keeps-raw-equality; equivalent modes: search
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xDE00, 0xD83D, 0xD83D],
            2,
            "search",
            (0, 4),
            Some(((2, 4), &[Some((2, 3))], Some(1))),
        ),
        // ascii-casefold-positive-control; equivalent modes: fullmatch, match, search
        (
            &[0x41, 0xD83D],
            &[0x61, 0xD83D],
            2,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[], None)),
        ),
        // literal-high-remains-distinct; equivalent modes: fullmatch, match
        (
            &[0xD83D],
            &[0xDE00, 0xFFFD, 0xD83D],
            258,
            "fullmatch",
            (0, 3),
            None,
        ),
        // literal-high-remains-distinct; equivalent modes: search
        (
            &[0xD83D],
            &[0xDE00, 0xFFFD, 0xD83D],
            258,
            "search",
            (0, 3),
            Some(((2, 3), &[], None)),
        ),
        // raw-range-remains-distinct; equivalent modes: fullmatch, match
        (
            &[0x5B, 0xD800, 0x2D, 0xDFFF, 0x5D, 0x2B],
            &[0xFFFD, 0xD83D, 0xDE00, 0xFFFD],
            258,
            "fullmatch",
            (0, 4),
            None,
        ),
        // raw-range-remains-distinct; equivalent modes: search
        (
            &[0x5B, 0xD800, 0x2D, 0xDFFF, 0x5D, 0x2B],
            &[0xFFFD, 0xD83D, 0xDE00, 0xFFFD],
            258,
            "search",
            (0, 4),
            Some(((1, 3), &[], None)),
        ),
        // backreference-keeps-raw-equality; equivalent modes: fullmatch, match
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xDE00, 0xD83D, 0xD83D],
            258,
            "fullmatch",
            (0, 4),
            None,
        ),
        // backreference-keeps-raw-equality; equivalent modes: search
        (
            &[0x28, 0xD83D, 0x29, 0x5C, 0x31],
            &[0xD83D, 0xDE00, 0xD83D, 0xD83D],
            258,
            "search",
            (0, 4),
            Some(((2, 4), &[Some((2, 3))], Some(1))),
        ),
        // ascii-casefold-positive-control; equivalent modes: fullmatch, match, search
        (
            &[0x41, 0xD83D],
            &[0x61, 0xD83D],
            258,
            "fullmatch",
            (0, 2),
            Some(((0, 2), &[], None)),
        ),
        // window-0-5; equivalent modes: fullmatch
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (0, 5),
            None,
        ),
        // window-0-5; equivalent modes: match, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "match",
            (0, 5),
            Some(((0, 0), &[Some((0, 0)), None], Some(1))),
        ),
        // window-1-3; equivalent modes: fullmatch
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (1, 3),
            None,
        ),
        // window-1-3; equivalent modes: match, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "match",
            (1, 3),
            Some(((1, 1), &[Some((1, 1)), None], Some(1))),
        ),
        // window-2-3; equivalent modes: fullmatch
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (2, 3),
            Some(((2, 3), &[None, Some((2, 3))], Some(2))),
        ),
        // window-2-3; equivalent modes: match, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "match",
            (2, 3),
            Some(((2, 2), &[Some((2, 2)), None], Some(1))),
        ),
        // window-3-5; equivalent modes: fullmatch
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (3, 5),
            None,
        ),
        // window-3-5; equivalent modes: match, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "match",
            (3, 5),
            Some(((3, 3), &[Some((3, 3)), None], Some(1))),
        ),
        // window-4-2; equivalent modes: fullmatch, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (4, 2),
            None,
        ),
        // window-4-2; equivalent modes: match
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "match",
            (4, 2),
            Some(((4, 4), &[Some((4, 4)), None], Some(1))),
        ),
        // window-9-9; equivalent modes: fullmatch, match, search
        (
            &[
                0x28, 0x3F, 0x50, 0x3C, 0x65, 0x6D, 0x70, 0x74, 0x79, 0x3E, 0x29, 0x7C, 0x28, 0x3F,
                0x50, 0x3C, 0x75, 0x6E, 0x69, 0x74, 0x3E, 0x2E, 0x29,
            ],
            &[0xD83D, 0xDE00, 0x1F600, 0xFFFD, 0x41],
            0,
            "fullmatch",
            (9, 9),
            Some(((5, 5), &[Some((5, 5)), None], Some(1))),
        ),
    ];

    for &(pattern, text, flags, mode, window, expected) in cases {
        let compiled = parse_pattern_typed(pattern.to_vec(), i64::from(flags), false)
            .unwrap()
            .compile()
            .unwrap();
        let mut storage = Vec::new();
        for &unit in text {
            molt_obj_model::text_storage::push_code_point(&mut storage, unit);
        }
        let found = execute_match(
            &compiled,
            &storage,
            window.0,
            window.1,
            MatchMode::try_from(mode).unwrap(),
        )
        .unwrap();
        let context = format!("{pattern:x?} {text:x?} flags={flags} {mode} {window:?}");
        if let Some((span, groups, lastindex)) = expected {
            let found = found.unwrap_or_else(|| panic!("missing match: {context}"));
            assert_eq!((found.start, found.end), span, "{context}");
            assert_eq!(&found.groups[1..], groups, "{context}");
            assert_eq!(found.lastindex, lastindex, "{context}");
        } else {
            assert!(found.is_none(), "unexpected match: {context}");
        }
    }
}
