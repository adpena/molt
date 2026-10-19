use super::{
    GIB_BYTES, default_process_max_rss_gb_from_physical_mem_bytes, parse_nonnegative_gb,
    process_max_rss_bytes_from,
};

const GIB: u64 = 1024 * 1024 * 1024;

#[test]
fn fractional_gb_from_the_memory_guard_becomes_the_cap() {
    assert_eq!(
        process_max_rss_bytes_from(Some("1.500000"), 64),
        3 * GIB / 2
    );
    assert_eq!(process_max_rss_bytes_from(Some(" 12 "), 64), 12 * GIB);
}

#[test]
fn zero_or_invalid_values_fall_back_to_the_physical_default() {
    for raw in [
        None,
        Some("0"),
        Some("0.0"),
        Some("-1"),
        Some("not-a-number"),
        Some("inf"),
    ] {
        assert_eq!(process_max_rss_bytes_from(raw, 64), 64 * GIB, "{raw:?}");
    }
}
#[test]
fn fractional_guard_values_parse_to_bytes() {
    assert_eq!(parse_nonnegative_gb("1.500000"), Some(3 * GIB_BYTES / 2));
    assert_eq!(parse_nonnegative_gb(" 12 "), Some(12 * GIB_BYTES));
    assert_eq!(parse_nonnegative_gb("0"), Some(0));
    assert_eq!(parse_nonnegative_gb("1e30"), Some(u64::MAX));
}

#[test]
fn negative_non_finite_and_malformed_values_are_absent() {
    for raw in ["-1", "inf", "NaN", "", "2G", "not-a-number"] {
        assert_eq!(parse_nonnegative_gb(raw), None, "{raw:?}");
    }
}
#[test]
fn backend_rss_default_scales_with_host_memory() {
    assert_eq!(
        default_process_max_rss_gb_from_physical_mem_bytes(Some(8 * GIB)),
        4
    );
    assert_eq!(
        default_process_max_rss_gb_from_physical_mem_bytes(Some(16 * GIB)),
        8
    );
    assert_eq!(
        default_process_max_rss_gb_from_physical_mem_bytes(Some(32 * GIB)),
        12
    );
    assert_eq!(
        default_process_max_rss_gb_from_physical_mem_bytes(Some(64 * GIB)),
        16
    );
}
