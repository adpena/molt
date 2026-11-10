use super::*;
use crate::{
    TAG_BOOL, TAG_INT, TAG_NONE, TAG_PENDING, TAG_PTR, box_bool_bits, box_int_bits,
    fits_inline_int, is_bool_bits, is_int_bits, tag_bits, unbox_bool_bits, unbox_inline_int_bits,
};

/// Evaluates the primitives on concrete 64-bit words.
struct Evaluate;

impl WordBuilder for Evaluate {
    type Word = u64;
    type Predicate = bool;
    fn constant(&mut self, value: u64) -> u64 {
        value
    }
    fn and(&mut self, value: u64, mask: u64) -> u64 {
        value & mask
    }
    fn or(&mut self, value: u64, bits: u64) -> u64 {
        value | bits
    }
    fn shift_left(&mut self, value: u64, amount: u32) -> u64 {
        value << amount
    }
    fn shift_right_arithmetic(&mut self, value: u64, amount: u32) -> u64 {
        ((value as i64) >> amount) as u64
    }
    fn add_immediate(&mut self, value: u64, delta: i64) -> u64 {
        value.wrapping_add_signed(delta)
    }
    fn compare(&mut self, comparison: WordComparison, left: u64, right: u64) -> bool {
        match comparison {
            WordComparison::Equal => left == right,
            WordComparison::NotEqual => left != right,
            WordComparison::UnsignedLessThan => left < right,
        }
    }
    fn both(&mut self, left: bool, right: bool) -> bool {
        left && right
    }
}

// An independent restatement of the encoding: an inline int holds a value in
// [-2^46, 2^46) as the low 47 bits under QNAN | TAG_INT.
const INLINE_MIN: i128 = -(1 << 46);
const INLINE_MAX: i128 = (1 << 46) - 1;

fn spec_box(value: i128) -> u64 {
    assert!((INLINE_MIN..=INLINE_MAX).contains(&value));
    QNAN | TAG_INT | ((value as u64) & ((1 << 47) - 1))
}

const EDGES: [i128; 11] = [
    INLINE_MIN,
    INLINE_MIN + 1,
    -2,
    -1,
    0,
    1,
    2,
    INLINE_MAX - 1,
    INLINE_MAX,
    12_345,
    -98_765_432_101,
];

struct Xorshift(u64);

impl Xorshift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Words of every kind, including ones no producer should make (a negative
/// NaN carrying a tag, bit 47 set under the int tag), so each generator is
/// compared with its authority beyond the values it is meant for.
fn sweep_words() -> std::vec::Vec<u64> {
    let mut words: std::vec::Vec<u64> = EDGES.iter().map(|&v| spec_box(v)).collect();
    for tag in [TAG_INT, TAG_BOOL, TAG_NONE, TAG_PTR, TAG_PENDING] {
        words.extend([
            QNAN | tag,
            QNAN | tag | 1,
            QNAN | tag | (1 << 47),
            (1 << 63) | QNAN | tag | 5,
        ]);
    }
    words.extend([
        0,
        1,
        1.5_f64.to_bits(),
        (-0.0_f64).to_bits(),
        f64::INFINITY.to_bits(),
        QNAN,
        u64::MAX,
    ]);
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let word = rng.next();
        words.push(word);
        // Half of the random words carry a tag, so tagged payloads are swept too.
        let tag = [TAG_INT, TAG_BOOL, TAG_PTR][(word % 3) as usize];
        words.push(QNAN | tag | (word & ((1 << 48) - 1)));
    }
    words
}

#[test]
fn has_tag_is_the_runtime_tag_test_for_every_tag() {
    let tags = [
        BoxTag::Int,
        BoxTag::Bool,
        BoxTag::None,
        BoxTag::Ptr,
        BoxTag::Pending,
    ];
    for word in sweep_words() {
        for tag in tags {
            let expected = tag_bits(word) == QNAN | tag.bits();
            assert_eq!(
                generate_has_tag(&mut Evaluate, word, tag),
                expected,
                "{word:#x} {tag:?}"
            );
        }
        assert_eq!(
            generate_has_tag(&mut Evaluate, word, BoxTag::Int),
            is_int_bits(word),
            "{word:#x}"
        );
    }
}

#[test]
fn unbox_matches_the_runtime_on_every_int_word() {
    for word in sweep_words().into_iter().filter(|&w| is_int_bits(w)) {
        assert_eq!(
            generate_unbox_inline_int(&mut Evaluate, word) as i64,
            unbox_inline_int_bits(word),
            "{word:#x}"
        );
    }
    for &value in &EDGES {
        assert_eq!(
            generate_unbox_inline_int(&mut Evaluate, spec_box(value)) as i64 as i128,
            value
        );
    }
}

#[test]
fn fits_and_box_match_the_runtime() {
    let mut values: std::vec::Vec<i64> = EDGES.iter().map(|&v| v as i64).collect();
    values.extend([
        INLINE_MIN as i64 - 1,
        INLINE_MAX as i64 + 1,
        i64::MIN,
        i64::MAX,
        i64::MIN + 1,
        i64::MAX - 1,
    ]);
    let mut rng = Xorshift(0xD1B5_4A32_D192_ED03);
    values.extend((0..20_000).map(|_| (rng.next() as i64) >> (rng.next() % 20)));
    for value in values {
        let fits = generate_inline_int_fits(&mut Evaluate, value as u64);
        assert_eq!(fits, fits_inline_int(value), "{value}");
        if fits {
            let word = generate_box_inline_int(&mut Evaluate, value as u64);
            assert_eq!(word, box_int_bits(value) as u64, "{value}");
            assert_eq!(word, spec_box(value as i128), "{value}");
        }
    }
}

#[test]
fn the_composed_add_and_sub_lanes_match_exact_arithmetic() {
    // The late TIR stage composes the primitives around a raw i64 add or sub;
    // this replays that composition against i128 arithmetic.
    let mut rng = Xorshift(0x2545_F491_4F6C_DD1D);
    let mut pairs: std::vec::Vec<(i128, i128)> = EDGES
        .iter()
        .flat_map(|&l| EDGES.iter().map(move |&r| (l, r)))
        .collect();
    for _ in 0..20_000 {
        let l = ((rng.next() as i64) >> 17) as i128;
        let r = ((rng.next() as i64) >> 17) as i128;
        pairs.push((
            l.clamp(INLINE_MIN, INLINE_MAX),
            r.clamp(INLINE_MIN, INLINE_MAX),
        ));
    }
    for (l, r) in pairs {
        let (lw, rw) = (spec_box(l), spec_box(r));
        assert!(generate_has_tag(&mut Evaluate, lw, BoxTag::Int));
        assert!(generate_has_tag(&mut Evaluate, rw, BoxTag::Int));
        let x = generate_unbox_inline_int(&mut Evaluate, lw) as i64;
        let y = generate_unbox_inline_int(&mut Evaluate, rw) as i64;
        for (raw, exact) in [(x.wrapping_add(y), l + r), (x.wrapping_sub(y), l - r)] {
            // Two 47-bit operands never overflow i64: the raw result is exact.
            assert_eq!(raw as i128, exact, "{l} {r}");
            let fits = generate_inline_int_fits(&mut Evaluate, raw as u64);
            assert_eq!(fits, (INLINE_MIN..=INLINE_MAX).contains(&exact), "{l} {r}");
            if fits {
                assert_eq!(
                    generate_box_inline_int(&mut Evaluate, raw as u64),
                    spec_box(exact)
                );
            }
        }
    }
}

#[test]
fn bool_words_unbox_to_their_runtime_truth_value() {
    for value in [0_i64, 1] {
        let word = box_bool_bits(value) as u64;
        assert!(generate_has_tag(&mut Evaluate, word, BoxTag::Bool));
        assert_eq!(
            generate_unbox_bool(&mut Evaluate, word),
            unbox_bool_bits(word) != 0
        );
    }
    for word in sweep_words().into_iter().filter(|&w| is_bool_bits(w)) {
        assert_eq!(
            generate_unbox_bool(&mut Evaluate, word),
            unbox_bool_bits(word) != 0,
            "{word:#x}"
        );
    }
}
