use super::RustBackend;

mod callable;
mod stateful;
mod system;

/// The emitted `format_float` prelude: a faithful, self-contained port of
/// `runtime/molt-runtime/src/object/float_repr.rs` (`repr_float`), with
/// `num_bigint::BigInt` replaced by the inline base-1e9 `MoltBig` so the
/// generated crate has no external dependency. This string is the ONLY float
/// formatter the rust-backend emits; it must format `repr(float)` /
/// `str(float)` bit-for-bit identically to CPython 3.12 (validated in
/// `tests_float.rs` and the differential harness). Do not diverge from the
/// runtime authority — port changes there into here in the same arc.
const MOLT_FORMAT_FLOAT_PRELUDE: &str = r####"// --- BEGIN CPython-exact repr(float): ported from
// runtime/molt-runtime/src/object/float_repr.rs (repr_float). ---
// A minimal self-contained arbitrary-precision unsigned integer, base 1e9,
// little-endian limbs, only the ops the exact round-half-to-even formatter
// needs. Replaces num_bigint::BigInt (which the runtime authority uses) so this
// generated crate depends on no external crate.
#[derive(Clone, PartialEq, Eq)]
struct MoltBig {
    limbs: Vec<u32>,
}
const MOLT_BIG_BASE: u64 = 1_000_000_000;
impl MoltBig {
    fn zero() -> MoltBig {
        MoltBig { limbs: Vec::new() }
    }
    fn from_u64(mut v: u64) -> MoltBig {
        let mut limbs = Vec::new();
        while v > 0 {
            limbs.push((v % MOLT_BIG_BASE) as u32);
            v /= MOLT_BIG_BASE;
        }
        MoltBig { limbs }
    }
    fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }
    fn trim(&mut self) {
        while let Some(&0) = self.limbs.last() {
            self.limbs.pop();
        }
    }
    fn mul_small(&mut self, m: u32) {
        if m == 0 {
            self.limbs.clear();
            return;
        }
        let mut carry: u64 = 0;
        for limb in self.limbs.iter_mut() {
            let cur = (*limb as u64) * (m as u64) + carry;
            *limb = (cur % MOLT_BIG_BASE) as u32;
            carry = cur / MOLT_BIG_BASE;
        }
        while carry > 0 {
            self.limbs.push((carry % MOLT_BIG_BASE) as u32);
            carry /= MOLT_BIG_BASE;
        }
    }
    fn add_small(&mut self, a: u64) {
        let mut carry = a;
        let mut i = 0;
        while carry > 0 {
            if i == self.limbs.len() {
                self.limbs.push(0);
            }
            let cur = self.limbs[i] as u64 + carry;
            self.limbs[i] = (cur % MOLT_BIG_BASE) as u32;
            carry = cur / MOLT_BIG_BASE;
            i += 1;
        }
    }
    fn mul_pow2(&mut self, mut n: u32) {
        while n >= 30 {
            self.mul_small(1u32 << 30);
            n -= 30;
        }
        if n > 0 {
            self.mul_small(1u32 << n);
        }
    }
    fn mul_pow10(&mut self, mut p: u32) {
        while p >= 9 {
            self.mul_small(1_000_000_000u32);
            p -= 9;
        }
        if p > 0 {
            let mut m: u32 = 1;
            for _ in 0..p {
                m *= 10;
            }
            self.mul_small(m);
        }
    }
    fn cmp(&self, other: &MoltBig) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for i in (0..self.limbs.len()).rev() {
            match self.limbs[i].cmp(&other.limbs[i]) {
                Ordering::Equal => continue,
                ord => return ord,
            }
        }
        Ordering::Equal
    }
    fn sub_assign(&mut self, other: &MoltBig) {
        let mut borrow: i64 = 0;
        for i in 0..self.limbs.len() {
            let o = if i < other.limbs.len() { other.limbs[i] as i64 } else { 0 };
            let mut cur = self.limbs[i] as i64 - o - borrow;
            if cur < 0 {
                cur += MOLT_BIG_BASE as i64;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.limbs[i] = cur as u32;
        }
        self.trim();
    }
    fn divmod(&self, d: &MoltBig) -> (MoltBig, MoltBig) {
        if self.cmp(d) == std::cmp::Ordering::Less {
            return (MoltBig::zero(), self.clone());
        }
        let mut quotient = vec![0u32; self.limbs.len()];
        let mut rem = MoltBig::zero();
        for i in (0..self.limbs.len()).rev() {
            rem.mul_small(MOLT_BIG_BASE as u32);
            rem.add_small(self.limbs[i] as u64);
            let mut lo: u64 = 0;
            let mut hi: u64 = MOLT_BIG_BASE - 1;
            let mut qd: u64 = 0;
            while lo <= hi {
                let mid = (lo + hi) / 2;
                let mut t = d.clone();
                t.mul_small(mid as u32);
                if t.cmp(&rem) != std::cmp::Ordering::Greater {
                    qd = mid;
                    if mid == MOLT_BIG_BASE - 1 {
                        break;
                    }
                    lo = mid + 1;
                } else {
                    if mid == 0 {
                        break;
                    }
                    hi = mid - 1;
                }
            }
            quotient[i] = qd as u32;
            let mut t = d.clone();
            t.mul_small(qd as u32);
            rem.sub_assign(&t);
        }
        let mut q = MoltBig { limbs: quotient };
        q.trim();
        (q, rem)
    }
    fn to_decimal(&self) -> String {
        if self.limbs.is_empty() {
            return "0".to_string();
        }
        let mut s = String::new();
        s.push_str(&self.limbs[self.limbs.len() - 1].to_string());
        for i in (0..self.limbs.len() - 1).rev() {
            s.push_str(&format!("{:09}", self.limbs[i]));
        }
        s
    }
    fn is_odd(&self) -> bool {
        self.limbs.first().map(|l| l % 2 == 1).unwrap_or(false)
    }
}
fn molt_big_mul(a: &MoltBig, b: &MoltBig) -> MoltBig {
    if a.is_zero() || b.is_zero() {
        return MoltBig::zero();
    }
    let mut result = vec![0u64; a.limbs.len() + b.limbs.len()];
    for (i, &av) in a.limbs.iter().enumerate() {
        let mut carry: u64 = 0;
        for (j, &bv) in b.limbs.iter().enumerate() {
            let cur = result[i + j] + (av as u64) * (bv as u64) + carry;
            result[i + j] = cur % MOLT_BIG_BASE;
            carry = cur / MOLT_BIG_BASE;
        }
        result[i + b.limbs.len()] += carry;
    }
    let limbs: Vec<u32> = result.iter().map(|&x| x as u32).collect();
    let mut m = MoltBig { limbs };
    m.trim();
    m
}
fn molt_float_shortest_sig_count(abs: f64) -> usize {
    let sci = format!("{abs:e}");
    let mant = sci.split('e').next().unwrap_or("0");
    mant.chars().filter(|c| c.is_ascii_digit()).count().max(1)
}
fn molt_float_round_sig_half_even(abs: f64, k: usize) -> (String, i32) {
    let bits = abs.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i64;
    let raw_mant = (bits & 0x000f_ffff_ffff_ffff) as u64;
    let (mant, exp2): (u64, i64) = if raw_exp == 0 {
        (raw_mant, -1074)
    } else {
        (raw_mant | 0x0010_0000_0000_0000, raw_exp - 1075)
    };
    let mut num = MoltBig::from_u64(mant);
    let mut den = MoltBig::from_u64(1);
    if exp2 >= 0 {
        num.mul_pow2(exp2 as u32);
    } else {
        den.mul_pow2((-exp2) as u32);
    }
    let mut e = abs.log10().floor() as i32;
    loop {
        let lower_ok = if e >= 0 {
            let mut lhs = MoltBig::from_u64(1);
            lhs.mul_pow10(e as u32);
            let l = molt_big_mul(&lhs, &den);
            l.cmp(&num) != std::cmp::Ordering::Greater
        } else {
            let mut r = num.clone();
            r.mul_pow10((-e) as u32);
            den.cmp(&r) != std::cmp::Ordering::Greater
        };
        if !lower_ok {
            e -= 1;
            continue;
        }
        let upper_ok = if (e + 1) >= 0 {
            let mut rhs = MoltBig::from_u64(1);
            rhs.mul_pow10((e + 1) as u32);
            let r = molt_big_mul(&rhs, &den);
            num.cmp(&r) == std::cmp::Ordering::Less
        } else {
            let mut l = num.clone();
            l.mul_pow10((-(e + 1)) as u32);
            l.cmp(&den) == std::cmp::Ordering::Less
        };
        if !upper_ok {
            e += 1;
            continue;
        }
        break;
    }
    let s = (k as i32) - 1 - e;
    if s >= 0 {
        num.mul_pow10(s as u32);
    } else {
        den.mul_pow10((-s) as u32);
    }
    let (q, r) = num.divmod(&den);
    let mut twice = r.clone();
    twice.mul_small(2);
    let mut digit_int = q;
    match twice.cmp(&den) {
        std::cmp::Ordering::Greater => digit_int.add_small(1),
        std::cmp::Ordering::Equal => {
            if digit_int.is_odd() {
                digit_int.add_small(1);
            }
        }
        std::cmp::Ordering::Less => {}
    }
    let ds_full = digit_int.to_decimal();
    let mut decpt = e + 1;
    if ds_full.len() as i32 > k as i32 {
        decpt += ds_full.len() as i32 - k as i32;
    }
    let trimmed = ds_full.trim_end_matches('0');
    let ds = if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    };
    (ds, decpt)
}
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".to_string() } else { "inf".to_string() };
    }
    let sign = if f.is_sign_negative() { "-" } else { "" };
    let abs = f.abs();
    if abs == 0.0 {
        return format!("{sign}0.0");
    }
    let k = molt_float_shortest_sig_count(abs);
    let (digits, decpt) = molt_float_round_sig_half_even(abs, k);
    let dbytes = digits.as_bytes();
    let ndigits = dbytes.len() as i32;
    let use_exp = decpt <= -4 || decpt > 16;
    let mut out = String::with_capacity(sign.len() + digits.len() + 6);
    out.push_str(sign);
    if use_exp {
        let exp = decpt - 1;
        out.push(dbytes[0] as char);
        if ndigits > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let mag = exp.unsigned_abs();
        if mag < 10 {
            out.push_str(&format!("0{mag}"));
        } else {
            out.push_str(&format!("{mag}"));
        }
    } else if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decpt >= ndigits {
        out.push_str(&digits);
        for _ in 0..(decpt - ndigits) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        let cut = decpt as usize;
        out.push_str(&digits[..cut]);
        out.push('.');
        out.push_str(&digits[cut..]);
    }
    out
}
// --- END CPython-exact repr(float) ---

"####;

impl RustBackend {
    // ── File header ──────────────────────────────────────────────────────────

    pub(super) fn emit_header(&mut self) {
        self.output.push_str(concat!(
            "// Molt → Rust transpiled output\n",
            "// Auto-generated — do not edit\n",
            "#![allow(\n",
            "    unused_assignments, unused_mut, unused_variables, dead_code, non_snake_case,\n",
            "    clippy::needless_pass_by_value, clippy::clone_on_copy,\n",
            "    clippy::useless_vec,\n",
            ")]\n\n",
        ));
        self.output.push_str("use std::sync::Arc;\n\n");
    }

    pub(super) fn emit_prelude_conditional(&mut self, func_body: &str) {
        let used = |name: &str| func_body.contains(name);

        // Always emit the MoltValue enum — it is the foundation of everything.
        // Func variant uses Arc<dyn Fn>, which can't derive Debug or PartialEq,
        // so we implement them manually below.
        self.output.push_str(concat!(
            "#[derive(Clone)]\n",
            "pub enum MoltValue {\n",
            "    None,\n",
            "    Bool(bool),\n",
            "    Int(i64),\n",
            "    Float(f64),\n",
            "    Str(String),\n",
            "    List(Vec<MoltValue>),\n",
            "    Dict(Vec<(MoltValue, MoltValue)>),\n",
            "    Func(Arc<dyn Fn(&mut Vec<MoltValue>) -> MoltValue + Send + Sync>),\n",
            "}\n",
            "impl std::fmt::Debug for MoltValue {\n",
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n",
            "        match self {\n",
            "            MoltValue::None => write!(f, \"None\"),\n",
            "            MoltValue::Bool(b) => write!(f, \"{b}\"),\n",
            "            MoltValue::Int(n) => write!(f, \"{n}\"),\n",
            "            MoltValue::Float(v) => write!(f, \"{v}\"),\n",
            "            MoltValue::Str(s) => write!(f, \"{s:?}\"),\n",
            "            MoltValue::List(v) => write!(f, \"{v:?}\"),\n",
            "            MoltValue::Dict(d) => write!(f, \"{d:?}\"),\n",
            "            MoltValue::Func(_) => write!(f, \"<function>\"),\n",
            "        }\n",
            "    }\n",
            "}\n",
            "impl PartialEq for MoltValue {\n",
            "    fn eq(&self, other: &Self) -> bool {\n",
            "        match (self, other) {\n",
            "            (MoltValue::None, MoltValue::None) => true,\n",
            "            (MoltValue::Bool(a), MoltValue::Bool(b)) => a == b,\n",
            "            (MoltValue::Int(a), MoltValue::Int(b)) => a == b,\n",
            "            (MoltValue::Float(a), MoltValue::Float(b)) => a == b,\n",
            "            (MoltValue::Str(a), MoltValue::Str(b)) => a == b,\n",
            "            (MoltValue::List(a), MoltValue::List(b)) => a == b,\n",
            "            (MoltValue::Dict(a), MoltValue::Dict(b)) => a == b,\n",
            "            (MoltValue::Func(_), MoltValue::Func(_)) => false, // functions never equal\n",
            "            _ => false,\n",
            "        }\n",
            "    }\n",
            "}\n\n",
        ));

        self.output.push_str(concat!(
            "impl std::fmt::Display for MoltValue {\n",
            "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n",
            "        write!(f, \"{}\", molt_str(self))\n",
            "    }\n",
            "}\n\n",
        ));

        // Core coercion helpers — always emitted (used by all others).
        self.output.push_str(concat!(
            "fn molt_bool(x: &MoltValue) -> bool {\n",
            "    match x {\n",
            "        MoltValue::None => false,\n",
            "        MoltValue::Bool(b) => *b,\n",
            "        MoltValue::Int(n) => *n != 0,\n",
            "        MoltValue::Float(f) => *f != 0.0 && !f.is_nan(),\n",
            "        MoltValue::Str(s) => !s.is_empty(),\n",
            "        MoltValue::List(v) => !v.is_empty(),\n",
            "        MoltValue::Dict(d) => !d.is_empty(),\n",
            "        MoltValue::Func(_) => true,\n",
            "    }\n",
            "}\n\n",
            "fn molt_int(x: &MoltValue) -> i64 {\n",
            "    match x {\n",
            "        MoltValue::Int(n) => *n,\n",
            "        MoltValue::Float(f) => {\n",
            "            if f.is_nan() { panic!(\"ValueError: cannot convert float NaN to integer\") }\n",
            "            if !f.is_finite() || *f >= 9223372036854775808.0 || *f < -9223372036854775808.0 { panic!(\"OverflowError: float too large to convert to int\") }\n",
            "            f.trunc() as i64\n",
            "        }\n",
            "        MoltValue::Bool(b) => *b as i64,\n",
            "        MoltValue::Str(s) => s.trim().parse::<i64>().unwrap_or_else(|_| panic!(\"ValueError: invalid literal for int(): {s}\")),\n",
            "        _ => panic!(\"TypeError: int() argument must be a string or a number\"),\n",
            "    }\n",
            "}\n\n",
            "fn molt_float(x: &MoltValue) -> f64 {\n",
            "    match x {\n",
            "        MoltValue::Float(f) => *f,\n",
            "        MoltValue::Int(n) => *n as f64,\n",
            "        MoltValue::Bool(b) => *b as i64 as f64,\n",
            "        MoltValue::Str(s) => s.trim().parse::<f64>().unwrap_or_else(|_| panic!(\"ValueError: could not convert string to float: {s}\")),\n",
            "        _ => panic!(\"TypeError: float() argument must be a string or a number\"),\n",
            "    }\n",
            "}\n\n",
            "fn molt_int_add(a: i64, b: i64) -> i64 { a.checked_add(b).unwrap_or_else(|| panic!(\"OverflowError: Rust backend integer addition exceeds i64 representation\")) }\n",
            "fn molt_int_sub(a: i64, b: i64) -> i64 { a.checked_sub(b).unwrap_or_else(|| panic!(\"OverflowError: Rust backend integer subtraction exceeds i64 representation\")) }\n",
            "fn molt_int_mul(a: i64, b: i64) -> i64 { a.checked_mul(b).unwrap_or_else(|| panic!(\"OverflowError: Rust backend integer multiplication exceeds i64 representation\")) }\n",
            "fn molt_int_neg(a: i64) -> i64 { a.checked_neg().unwrap_or_else(|| panic!(\"OverflowError: Rust backend integer negation exceeds i64 representation\")) }\n",
            "fn molt_int_pow(a: i64, b: i64) -> i64 {\n",
            "    let exponent = u32::try_from(b).unwrap_or_else(|_| panic!(\"OverflowError: Rust backend integer exponent is not representable\"));\n",
            "    a.checked_pow(exponent).unwrap_or_else(|| panic!(\"OverflowError: Rust backend integer power exceeds i64 representation\"))\n",
            "}\n\n",
        ));

        // molt_str — always emitted (Display impl references it).
        let needs_repr = used("molt_repr(");
        self.output.push_str(concat!(
            "fn molt_str(x: &MoltValue) -> String {\n",
            "    match x {\n",
            "        MoltValue::None => \"None\".to_string(),\n",
            "        MoltValue::Bool(true) => \"True\".to_string(),\n",
            "        MoltValue::Bool(false) => \"False\".to_string(),\n",
            "        MoltValue::Int(n) => n.to_string(),\n",
            "        MoltValue::Float(f) => format_float(*f),\n",
            "        MoltValue::Str(s) => s.clone(),\n",
            "        MoltValue::List(v) => {\n",
            "            let parts: Vec<String> = v.iter().map(molt_repr_inner).collect();\n",
            "            format!(\"[{}]\", parts.join(\", \"))\n",
            "        }\n",
            "        MoltValue::Dict(d) => {\n",
            "            let parts: Vec<String> = d.iter()\n",
            "                .map(|(k, v)| format!(\"{}: {}\", molt_repr_inner(k), molt_repr_inner(v)))\n",
            "                .collect();\n",
            "            format!(\"{{{}}}\", parts.join(\", \"))\n",
            "        }\n",
            "        MoltValue::Func(_) => \"<function>\".to_string(),\n",
            "    }\n",
            "}\n\n",
        ));

        // `format_float` — the CPython-exact `repr(float)` / `str(float)`
        // authority, ported verbatim from
        // runtime/molt-runtime/src/object/float_repr.rs (`repr_float` +
        // `round_sig_half_even` + `shortest_sig_count`). The native and
        // C-ABI lanes call `repr_float` directly; the rust-backend emits a
        // STANDALONE crate that does not link molt-runtime, so the same
        // algorithm is inlined here. The one substitution is num_bigint::BigInt
        // → a self-contained base-1e9 `MoltBig`, so the generated crate needs
        // no external dependency. It must stay bit-for-bit identical to
        // `repr_float`: shortest round-tripping digit COUNT from Rust std, then
        // re-render the exact f64 rounded to that many significant digits with
        // round-half-to-even via exact big-integer arithmetic; the
        // `decpt <= -4 || decpt > 16` threshold selects scientific vs fixed;
        // the exponent is `%+.02d`; `.0` is appended to integer-valued fixed
        // forms (Py_DTSF_ADD_DOT_0); `nan`/`inf`/`-inf`/`-0.0` are exact. Any
        // divergence from the naive `{f}`/`{f:.1}` formatter that this replaces
        // was a silent wrong-answer for scientific notation, ties, and
        // non-finite values.
        self.output.push_str(MOLT_FORMAT_FLOAT_PRELUDE);

        self.output.push_str(concat!(
            "fn molt_repr_inner(x: &MoltValue) -> String {\n",
            "    match x {\n",
            "        MoltValue::Str(s) => format!(\"'{s}'\"),\n",
            "        other => molt_str(other),\n",
            "    }\n",
            "}\n\n",
        ));

        if needs_repr {
            self.output.push_str(concat!(
                "fn molt_repr(x: &MoltValue) -> MoltValue {\n",
                "    MoltValue::Str(molt_repr_inner(x))\n",
                "}\n\n",
            ));
        }
        if used("molt_str_from_obj(") {
            self.output.push_str(concat!(
                "fn molt_str_from_obj(x: &MoltValue) -> MoltValue {\n",
                "    MoltValue::Str(molt_str(x))\n",
                "}\n\n",
            ));
        }
        if used("molt_ascii_from_obj(") {
            self.output.push_str(concat!(
                "fn molt_ascii_from_obj(x: &MoltValue) -> MoltValue {\n",
                "    MoltValue::Str(molt_escape_non_ascii(&molt_repr_inner(x)))\n",
                "}\n\n",
                "fn molt_escape_non_ascii(text: &str) -> String {\n",
                "    let mut out = String::with_capacity(text.len());\n",
                "    for ch in text.chars() {\n",
                "        let code = ch as u32;\n",
                "        if ch.is_ascii() {\n",
                "            out.push(ch);\n",
                "        } else if code <= 0xff {\n",
                "            out.push_str(&format!(\"\\\\x{code:02x}\"));\n",
                "        } else if code <= 0xffff {\n",
                "            out.push_str(&format!(\"\\\\u{code:04x}\"));\n",
                "        } else {\n",
                "            out.push_str(&format!(\"\\\\U{code:08x}\"));\n",
                "        }\n",
                "    }\n",
                "    out\n",
                "}\n\n",
            ));
        }
        if used("molt_bridge_unavailable(") {
            self.output.push_str(concat!(
                "fn molt_bridge_unavailable(message: &MoltValue) -> MoltValue {\n",
                "    panic!(\"{}\", molt_str(message));\n",
                "}\n\n",
            ));
        }

        // print
        if used("molt_print(") {
            self.output.push_str(concat!(
                "fn molt_print(args: &[MoltValue]) {\n",
                "    let parts: Vec<String> = args.iter().map(molt_str).collect();\n",
                "    println!(\"{}\", parts.join(\" \"));\n",
                "}\n\n",
            ));
        }

        // len
        if used("molt_len(") {
            self.output.push_str(concat!(
                "fn molt_len(x: &MoltValue) -> MoltValue {\n",
                "    let n = match x {\n",
                "        MoltValue::Str(s) => s.chars().count() as i64,\n",
                "        MoltValue::List(v) => v.len() as i64,\n",
                "        MoltValue::Dict(d) => d.len() as i64,\n",
                "        _ => panic!(\"TypeError: object has no len()\"),\n",
                "    };\n",
                "    MoltValue::Int(n)\n",
                "}\n\n",
            ));
        }

        // range
        if used("molt_range(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_range(start: i64, stop: i64, step: i64) -> MoltValue {\n",
                "    if step == 0 { panic!(\"ValueError: range() arg 3 must not be zero\") }\n",
                "    let mut result = Vec::new();\n",
                "    let mut i = start;\n",
                "    while (step > 0 && i < stop) || (step < 0 && i > stop) {\n",
                "        result.push(MoltValue::Int(i));\n",
                "        i = i.checked_add(step).unwrap_or_else(|| panic!(\"OverflowError: Rust backend range exceeds i64 representation\"));\n",
                "    }\n",
                "    MoltValue::List(result)\n",
                "}\n\n",
            ));
        }

        // arithmetic helpers
        if used("molt_add(") {
            self.output.push_str(concat!(
                "fn molt_add(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(molt_int_add(*x, *y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x + y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 + y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x + *y as f64),\n",
                "        (MoltValue::Str(x), MoltValue::Str(y)) => MoltValue::Str(format!(\"{x}{y}\")),\n",
                "        (MoltValue::List(x), MoltValue::List(y)) => {\n",
                "            let mut v = x.clone(); v.extend_from_slice(y); MoltValue::List(v)\n",
                "        }\n",
                "        _ => MoltValue::Int(molt_int_add(molt_int(&a), molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_sub(") {
            self.output.push_str(concat!(
                "fn molt_sub(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(molt_int_sub(*x, *y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x - y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 - y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x - *y as f64),\n",
                "        _ => MoltValue::Int(molt_int_sub(molt_int(&a), molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_mul(") {
            self.output.push_str(concat!(
                "fn molt_mul(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => MoltValue::Int(molt_int_mul(*x, *y)),\n",
                "        (MoltValue::Float(x), MoltValue::Float(y)) => MoltValue::Float(x * y),\n",
                "        (MoltValue::Int(x), MoltValue::Float(y)) => MoltValue::Float(*x as f64 * y),\n",
                "        (MoltValue::Float(x), MoltValue::Int(y)) => MoltValue::Float(x * *y as f64),\n",
                "        (MoltValue::Str(s), MoltValue::Int(n)) => { let count = if *n <= 0 { 0 } else { usize::try_from(*n).unwrap_or_else(|_| panic!(\"OverflowError: repeated string is too long\")) }; MoltValue::Str(s.repeat(count)) },\n",
                "        _ => MoltValue::Int(molt_int_mul(molt_int(&a), molt_int(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_div(") {
            self.output.push_str(concat!(
                "fn molt_div(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    let bv = molt_float(&b);\n",
                "    if bv == 0.0 { panic!(\"ZeroDivisionError: division by zero\") }\n",
                "    MoltValue::Float(molt_float(&a) / bv)\n",
                "}\n\n",
            ));
        }
        if used("molt_floor_div(") {
            self.output.push_str(concat!(
                "fn molt_floor_div(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y != 0 => {\n",
                "            MoltValue::Int(x.checked_div_euclid(*y).unwrap_or_else(|| panic!(\"OverflowError: Rust backend floor division exceeds i64 representation\")))\n",
                "        }\n",
                "        _ => {\n",
                "            let bv = molt_float(&b);\n",
                "            if bv == 0.0 { panic!(\"ZeroDivisionError: division by zero\") }\n",
                "            MoltValue::Float((molt_float(&a) / bv).floor())\n",
                "        }\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_mod(") {
            self.output.push_str(concat!(
                "fn molt_mod(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y != 0 => {\n",
                "            MoltValue::Int(x.checked_rem_euclid(*y).unwrap_or_else(|| panic!(\"OverflowError: Rust backend modulo exceeds i64 representation\")))\n",
                "        }\n",
                "        _ => {\n",
                "            let av = molt_float(&a); let bv = molt_float(&b);\n",
                "            if bv == 0.0 { panic!(\"ZeroDivisionError: modulo by zero\") }\n",
                "            MoltValue::Float(av - (av / bv).floor() * bv)\n",
                "        }\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_pow(") {
            self.output.push_str(concat!(
                "fn molt_pow(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    match (&a, &b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) if *y >= 0 => {\n",
                "            MoltValue::Int(molt_int_pow(*x, *y))\n",
                "        }\n",
                "        _ => MoltValue::Float(molt_float(&a).powf(molt_float(&b))),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_neg(") {
            self.output.push_str(concat!(
                "fn molt_neg(a: MoltValue) -> MoltValue {\n",
                "    match a {\n",
                "        MoltValue::Int(n) => MoltValue::Int(molt_int_neg(n)),\n",
                "        MoltValue::Float(f) => MoltValue::Float(-f),\n",
                "        other => MoltValue::Int(molt_int_neg(molt_int(&other))),\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // Comparison helpers — produce MoltValue::Bool.
        // Some collection helpers depend on `molt_eq`/`molt_numeric_cmp` even when
        // user IR never emits direct comparison ops, so include those dependencies.
        let needs_compare_helpers = used("molt_cmp(")
            || used("molt_eq(")
            || used("molt_ne(")
            || used("molt_lt(")
            || used("molt_le(")
            || used("molt_gt(")
            || used("molt_ge(")
            || used("molt_get_item(")
            || used("molt_ord_at(")
            || used("molt_set_item(")
            || used("molt_get_attr(")
            || used("molt_get_attr_name(")
            || used("molt_get_attr_name_default(")
            || used("molt_set_attr_name(")
            || used("molt_in(")
            || used("molt_sorted(")
            || used("molt_min(")
            || used("molt_max(");
        if needs_compare_helpers {
            self.output.push_str(concat!(
                "fn molt_is_numeric(x: &MoltValue) -> bool {\n",
                "    matches!(x, MoltValue::Bool(_) | MoltValue::Int(_) | MoltValue::Float(_))\n",
                "}\n",
                "fn molt_numeric_cmp(a: &MoltValue, b: &MoltValue) -> std::cmp::Ordering {\n",
                "    match (a, b) {\n",
                "        (MoltValue::Int(x), MoltValue::Int(y)) => x.cmp(y),\n",
                "        _ => molt_float(a).partial_cmp(&molt_float(b)).unwrap_or(std::cmp::Ordering::Equal),\n",
                "    }\n",
                "}\n",
                "fn molt_eq(a: &MoltValue, b: &MoltValue) -> bool {\n",
                "    match (a, b) {\n",
                "        (MoltValue::None, MoltValue::None) => true,\n",
                "        (MoltValue::Bool(x), MoltValue::Bool(y)) => x == y,\n",
                "        (MoltValue::Str(x), MoltValue::Str(y)) => x == y,\n",
                "        (MoltValue::List(x), MoltValue::List(y)) => x == y,\n",
                "        (MoltValue::Dict(x), MoltValue::Dict(y)) => x == y,\n",
                "        _ if molt_is_numeric(a) && molt_is_numeric(b) => {\n",
                "            matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Equal)\n",
                "        }\n",
                "        _ => false,\n",
                "    }\n",
                "}\n",
                "fn molt_lt(a: &MoltValue, b: &MoltValue) -> bool { matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less) }\n",
                "fn molt_le(a: &MoltValue, b: &MoltValue) -> bool { !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater) }\n",
                "fn molt_gt(a: &MoltValue, b: &MoltValue) -> bool { matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Greater) }\n",
                "fn molt_ge(a: &MoltValue, b: &MoltValue) -> bool { !matches!(molt_numeric_cmp(a, b), std::cmp::Ordering::Less) }\n\n",
            ));
        }

        // Collection helpers
        if used("molt_get_item(") || used("molt_ord_at(") || used("molt_set_item(") {
            self.output.push_str(concat!(
                "fn molt_checked_index(len: usize, idx: i64, kind: &str) -> usize {\n",
                "    let normalized = if idx < 0 { len as i64 + idx } else { idx };\n",
                "    if normalized < 0 || normalized >= len as i64 {\n",
                "        panic!(\"IndexError: {kind} index out of range\");\n",
                "    }\n",
                "    normalized as usize\n",
                "}\n\n",
            ));
        }
        if used("molt_get_item(") || used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_checked_char(s: &str, idx: i64) -> char {\n",
                "    let value = if idx >= 0 {\n",
                "        usize::try_from(idx).ok().and_then(|i| s.chars().nth(i))\n",
                "    } else {\n",
                "        let len = s.chars().count();\n",
                "        let i = molt_checked_index(len, idx, \"string\");\n",
                "        s.chars().nth(i)\n",
                "    };\n",
                "    value.unwrap_or_else(|| panic!(\"IndexError: string index out of range\"))\n",
                "}\n\n",
            ));
        }
        if used("molt_get_item(") || used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_get_item(obj: &MoltValue, key: &MoltValue) -> MoltValue {\n",
                "    match obj {\n",
                "        MoltValue::List(v) => {\n",
                "            let idx = molt_int(key);\n",
                "            let i = molt_checked_index(v.len(), idx, \"list\");\n",
                "            v[i].clone()\n",
                "        }\n",
                "        MoltValue::Dict(d) => d.iter().find(|(k, _)| molt_eq(k, key))\n",
                "            .map(|(_, v)| v.clone()).unwrap_or_else(|| panic!(\"KeyError: {}\", molt_repr_inner(key))),\n",
                "        MoltValue::Str(s) => {\n",
                "            let idx = molt_int(key);\n",
                "            MoltValue::Str(molt_checked_char(s, idx).to_string())\n",
                "        }\n",
                "        _ => panic!(\"TypeError: object is not subscriptable\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_set_item(") || used("molt_set_attr_name(") {
            self.output.push_str(concat!(
                "fn molt_set_item(obj: &mut MoltValue, key: MoltValue, val: MoltValue) {\n",
                "    match obj {\n",
                "        MoltValue::List(v) => {\n",
                "            let idx = molt_int(&key);\n",
                "            let i = molt_checked_index(v.len(), idx, \"list assignment\");\n",
                "            v[i] = val;\n",
                "        }\n",
                "        MoltValue::Dict(d) => {\n",
                "            if let Some(entry) = d.iter_mut().find(|(k, _)| molt_eq(k, &key)) {\n",
                "                entry.1 = val;\n",
                "            } else {\n",
                "                d.push((key, val));\n",
                "            }\n",
                "        }\n",
                "        _ => panic!(\"TypeError: object does not support item assignment\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_list_append(") {
            self.output.push_str(concat!(
                "fn molt_list_append(list: &mut MoltValue, val: MoltValue) {\n",
                "    if let MoltValue::List(v) = list { v.push(val); } else { panic!(\"TypeError: append target is not a list\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_get_attr(") {
            self.output.push_str(concat!(
                "fn molt_get_attr(obj: &MoltValue, attr: &str) -> MoltValue {\n",
                "    molt_get_attr_name(obj, &MoltValue::Str(attr.to_string()))\n",
                "}\n\n",
            ));
        }
        if used("molt_get_attr(")
            || used("molt_get_attr_name(")
            || used("molt_get_attr_name_default(")
        {
            self.output.push_str(concat!(
                "fn molt_get_attr_name(obj: &MoltValue, name: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(d) = obj {\n",
                "        if let Some((_, v)) = d.iter().find(|(k, _)| molt_eq(k, name)) {\n",
                "            return v.clone();\n",
                "        }\n",
                "        let class_key = MoltValue::Str(\"__class__\".to_string());\n",
                "        if let Some((_, class_obj)) = d.iter().find(|(k, _)| molt_eq(k, &class_key)) {\n",
                "            if let MoltValue::Dict(class_dict) = class_obj {\n",
                "                if let Some((_, v)) = class_dict.iter().find(|(k, _)| molt_eq(k, name)) {\n",
                "                    return v.clone();\n",
                "                }\n",
                "            }\n",
                "        }\n",
                "    }\n",
                "    MoltValue::None\n",
                "}\n\n",
                "fn molt_get_attr_name_default(obj: &MoltValue, name: &MoltValue, default: &MoltValue) -> MoltValue {\n",
                "    let value = molt_get_attr_name(obj, name);\n",
                "    if matches!(value, MoltValue::None) { default.clone() } else { value }\n",
                "}\n\n",
            ));
        }
        if used("molt_set_attr_name(") {
            self.output.push_str(concat!(
                "fn molt_set_attr_name(obj: &mut MoltValue, name: MoltValue, val: MoltValue) {\n",
                "    molt_set_item(obj, name, val);\n",
                "}\n\n",
            ));
        }
        if used("molt_class_merge_layout(") {
            self.output.push_str(
                r#"fn molt_class_merge_layout(class_obj: &mut MoltValue, offsets: MoltValue, size: MoltValue) -> MoltValue {
    let class_dict = match class_obj {
        MoltValue::Dict(d) => d,
        _ => panic!("class layout merge expects type"),
    };
    let hinted_size = match size {
        MoltValue::Int(v) if v >= 0 => v as usize,
        _ => panic!("__molt_layout_size__ must be int"),
    };
    let mut merged_offsets: Option<Vec<(MoltValue, MoltValue)>> = None;
    match offsets {
        MoltValue::None => {
            if let Some((_, MoltValue::Dict(existing))) = class_dict.iter().find(
                |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
            ) {
                merged_offsets = Some(existing.clone());
            }
        }
        MoltValue::Dict(source_offsets) => {
            let target_index = if let Some(index) = class_dict.iter().position(
                |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_field_offsets__"),
            ) {
                index
            } else {
                class_dict.push((
                    MoltValue::Str("__molt_field_offsets__".to_string()),
                    MoltValue::Dict(vec![]),
                ));
                class_dict.len() - 1
            };
            let target_offsets = match &mut class_dict[target_index].1 {
                MoltValue::Dict(d) => d,
                _ => panic!("__molt_field_offsets__ must be dict"),
            };
            for (name, offset) in source_offsets {
                if target_offsets.iter().any(|(existing, _)| existing == &name) {
                    continue;
                }
                target_offsets.push((name, offset));
            }
            merged_offsets = Some(target_offsets.clone());
        }
        _ => panic!("__molt_field_offsets__ must be dict or None"),
    }

    let mut layout_size = class_dict
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (MoltValue::Str(name), MoltValue::Int(existing)) if name == "__molt_layout_size__" && *existing > 0 => {
                Some(*existing as usize)
            }
            _ => None,
        })
        .unwrap_or(0);
    layout_size = layout_size.max(hinted_size);
    if let Some(offsets_dict) = merged_offsets.as_ref() {
        let mut max_end = 0usize;
        for (_, offset) in offsets_dict.iter() {
            if let MoltValue::Int(value) = offset {
                if *value < 0 {
                    continue;
                }
                let end = (*value as usize).saturating_add(std::mem::size_of::<u64>());
                if end > max_end {
                    max_end = end;
                }
            }
        }
        layout_size = layout_size.max(max_end.saturating_add(std::mem::size_of::<u64>()));
    }
    if layout_size == 0 {
        layout_size = std::mem::size_of::<u64>();
    }

    if let Some((_, value)) = class_dict.iter_mut().find(
        |(key, _)| matches!(key, MoltValue::Str(name) if name == "__molt_layout_size__"),
    ) {
        *value = MoltValue::Int(layout_size as i64);
    } else {
        class_dict.push((
            MoltValue::Str("__molt_layout_size__".to_string()),
            MoltValue::Int(layout_size as i64),
        ));
    }
    MoltValue::None
}

"#,
            );
        }
        if used("molt_in(") {
            self.output.push_str(concat!(
                "fn molt_in(elem: &MoltValue, container: &MoltValue) -> bool {\n",
                "    match container {\n",
                "        MoltValue::List(v) => v.iter().any(|x| molt_eq(x, elem)),\n",
                "        MoltValue::Dict(d) => d.iter().any(|(k, _)| molt_eq(k, elem)),\n",
                "        MoltValue::Str(s) => {\n",
                "            if let MoltValue::Str(sub) = elem { s.contains(sub.as_str()) } else { panic!(\"TypeError: string containment requires string operand\") }\n",
                "        }\n",
                "        _ => panic!(\"TypeError: object is not a container\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        self.emit_stateful_runtime_prelude(func_body);
        // Higher-order helpers
        if used("molt_enumerate(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_enumerate(t: &MoltValue, start: i64) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        let result = v.iter().enumerate()\n",
                "            .map(|(i, x)| { let offset = i64::try_from(i).unwrap_or_else(|_| panic!(\"OverflowError: enumerate index exceeds i64 representation\")); MoltValue::List(vec![MoltValue::Int(molt_int_add(start, offset)), x.clone()]) })\n",
                "            .collect();\n",
                "        MoltValue::List(result)\n",
                "    } else { panic!(\"TypeError: enumerate() argument must be iterable\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_zip(") || used("molt_builtin_func(") {
            self.output.push_str(concat!(
                "fn molt_zip(a: &MoltValue, b: &MoltValue) -> MoltValue {\n",
                "    match (a, b) {\n",
                "        (MoltValue::List(av), MoltValue::List(bv)) => {\n",
                "            let result = av.iter().zip(bv.iter())\n",
                "                .map(|(x, y)| MoltValue::List(vec![x.clone(), y.clone()]))\n",
                "                .collect();\n",
                "            MoltValue::List(result)\n",
                "        }\n",
                "        _ => panic!(\"TypeError: zip() arguments must be iterable\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_sorted(") {
            self.output.push_str(concat!(
                "fn molt_sorted(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        let mut copy = v.clone();\n",
                "        copy.sort_by(|a, b| molt_numeric_cmp(a, b));\n",
                "        MoltValue::List(copy)\n",
                "    } else { panic!(\"TypeError: sorted() argument must be iterable\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_reversed(") {
            self.output.push_str(concat!(
                "fn molt_reversed(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        MoltValue::List(v.iter().rev().cloned().collect())\n",
                "    } else { panic!(\"TypeError: object is not reversible\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_sum(") {
            self.output.push_str(concat!(
                "fn molt_sum(t: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(v) = t {\n",
                "        v.iter().fold(MoltValue::Int(0), |acc, x| molt_add(acc, x.clone()))\n",
                "    } else { panic!(\"TypeError: sum() argument must be iterable\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_any(") {
            self.output.push_str(concat!(
                "fn molt_any(t: &MoltValue) -> bool {\n",
                "    if let MoltValue::List(v) = t { v.iter().any(molt_bool) } else { panic!(\"TypeError: any() argument must be iterable\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_all(") {
            self.output.push_str(concat!(
                "fn molt_all(t: &MoltValue) -> bool {\n",
                "    if let MoltValue::List(v) = t { v.iter().all(molt_bool) } else { panic!(\"TypeError: all() argument must be iterable\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_keys(") {
            self.output.push_str(concat!(
                "fn molt_dict_keys(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter().map(|(k, _)| k.clone()).collect())\n",
                "    } else { panic!(\"TypeError: dict.keys target is not a dict\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_values(") {
            self.output.push_str(concat!(
                "fn molt_dict_values(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter().map(|(_, v)| v.clone()).collect())\n",
                "    } else { panic!(\"TypeError: dict.values target is not a dict\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_dict_items(") {
            self.output.push_str(concat!(
                "fn molt_dict_items(d: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Dict(pairs) = d {\n",
                "        MoltValue::List(pairs.iter()\n",
                "            .map(|(k, v)| MoltValue::List(vec![k.clone(), v.clone()]))\n",
                "            .collect())\n",
                "    } else { panic!(\"TypeError: dict.items target is not a dict\") }\n",
                "}\n\n",
            ));
        }

        // iter helper for for_iter ops
        if used("molt_iter_list(") || used("molt_iter(") || used("molt_iter_next(") {
            self.output.push_str(concat!(
                "fn molt_iter_list(x: &MoltValue) -> Vec<MoltValue> {\n",
                "    match x {\n",
                "        MoltValue::List(v) => v.clone(),\n",
                "        MoltValue::Dict(d) => d.iter().map(|(k, _)| k.clone()).collect(),\n",
                "        MoltValue::Str(s) => s.chars().map(|c| MoltValue::Str(c.to_string())).collect(),\n",
                "        _ => panic!(\"TypeError: object is not iterable\"),\n",
                "    }\n",
                "}\n\n",
            ));
        }
        if used("molt_iter(") {
            self.output.push_str(concat!(
                "fn molt_iter(x: &MoltValue) -> MoltValue {\n",
                "    let items = molt_iter_list(x);\n",
                "    MoltValue::List(vec![MoltValue::Int(0), MoltValue::List(items)])\n",
                "}\n\n",
            ));
        }
        if used("molt_iter_next(") {
            self.output.push_str(concat!(
                "fn molt_iter_next(iter: &mut MoltValue) -> MoltValue {\n",
                "    if let MoltValue::List(state) = iter {\n",
                "        if state.len() >= 2 {\n",
                "            let idx = molt_int(&state[0]);\n",
                "            if let MoltValue::List(items) = &state[1] {\n",
                "                let done = idx < 0 || (idx as usize) >= items.len();\n",
                "                if done {\n",
                "                    return MoltValue::List(vec![MoltValue::None, MoltValue::Bool(true)]);\n",
                "                }\n",
                "                let value = items[idx as usize].clone();\n",
                "                state[0] = MoltValue::Int(idx + 1);\n",
                "                return MoltValue::List(vec![value, MoltValue::Bool(false)]);\n",
                "            }\n",
                "        }\n",
                "    }\n",
                "    MoltValue::List(vec![MoltValue::None, MoltValue::Bool(true)])\n",
                "}\n\n",
            ));
        }
        if used("molt_unpack_sequence(") {
            self.output.push_str(
                r#"fn molt_unpack_type_name(seq: &MoltValue) -> &'static str {
    match seq {
        MoltValue::None => "NoneType",
        MoltValue::Bool(_) => "bool",
        MoltValue::Int(_) => "int",
        MoltValue::Float(_) => "float",
        MoltValue::Str(_) => "str",
        MoltValue::List(_) => "list",
        MoltValue::Dict(_) => "dict",
        MoltValue::Func(_) => "function",
    }
}

fn molt_unpack_too_many_message(expected_count: usize, actual: usize) -> String {
    if molt_runtime_target_at_least(3, 14) {
        format!("too many values to unpack (expected {}, got {})", expected_count, actual)
    } else {
        format!("too many values to unpack (expected {})", expected_count)
    }
}

fn molt_unpack_sequence(seq: &MoltValue, expected_count: usize) -> Vec<MoltValue> {
    match seq {
        MoltValue::List(values) => {
            let actual = values.len();
            if actual < expected_count {
                panic!("not enough values to unpack (expected {}, got {})", expected_count, actual);
            }
            if actual > expected_count {
                panic!("{}", molt_unpack_too_many_message(expected_count, actual));
            }
            values.clone()
        }
        MoltValue::Dict(entries) => {
            let actual = entries.len();
            if actual < expected_count {
                panic!("not enough values to unpack (expected {}, got {})", expected_count, actual);
            }
            if actual > expected_count {
                panic!("{}", molt_unpack_too_many_message(expected_count, actual));
            }
            entries.iter().map(|(key, _)| key.clone()).collect()
        }
        MoltValue::Str(value) => {
            // Python unpack only needs to distinguish fewer, exact, and more.
            // Probe at most expected+1 Unicode scalar values so a huge mismatch
            // never allocates or scans the remainder of the string.
            let probe_limit = expected_count.saturating_add(1);
            let mut chars = value.chars();
            let mut items = Vec::with_capacity(expected_count.min(value.len()));
            while items.len() < probe_limit {
                let Some(ch) = chars.next() else { break };
                items.push(MoltValue::Str(ch.to_string()));
            }
            let actual = items.len();
            if actual < expected_count {
                panic!("not enough values to unpack (expected {}, got {})", expected_count, actual);
            }
            if actual > expected_count {
                panic!("{}", molt_unpack_too_many_message(expected_count, actual));
            }
            items
        }
        _ => panic!("cannot unpack non-iterable {} object", molt_unpack_type_name(seq)),
    }
}

"#,
            );
        }

        // abs
        if used("molt_abs(") {
            self.output.push_str(concat!(
                "fn molt_abs(x: MoltValue) -> MoltValue {\n",
                "    match x {\n",
                "        MoltValue::Int(n) => MoltValue::Int(n.abs()),\n",
                "        MoltValue::Float(f) => MoltValue::Float(f.abs()),\n",
                "        other => other,\n",
                "    }\n",
                "}\n\n",
            ));
        }

        // min/max
        if used("molt_min(") {
            self.output.push_str(concat!(
                "fn molt_min(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    if matches!(molt_numeric_cmp(&a, &b), std::cmp::Ordering::Less | std::cmp::Ordering::Equal) { a } else { b }\n",
                "}\n\n",
            ));
        }
        if used("molt_max(") {
            self.output.push_str(concat!(
                "fn molt_max(a: MoltValue, b: MoltValue) -> MoltValue {\n",
                "    if matches!(molt_numeric_cmp(&a, &b), std::cmp::Ordering::Greater | std::cmp::Ordering::Equal) { a } else { b }\n",
                "}\n\n",
            ));
        }

        // chr/ord
        if used("molt_chr(") {
            self.output.push_str(concat!(
                "fn molt_chr(x: &MoltValue) -> MoltValue {\n",
                "    let n = molt_int(x);\n",
                "    if !(0..=0x10ffff).contains(&n) { panic!(\"ValueError: chr() arg not in range(0x110000)\") }\n",
                "    MoltValue::Str(char::from_u32(n as u32).map(|c| c.to_string()).unwrap_or_else(|| panic!(\"ValueError: chr() arg not in range(0x110000)\")))\n",
                "}\n\n",
            ));
        }
        if used("molt_ord(") || used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_ord(x: &MoltValue) -> MoltValue {\n",
                "    if let MoltValue::Str(s) = x {\n",
                "        let mut chars = s.chars();\n",
                "        let ch = chars.next().unwrap_or_else(|| panic!(\"TypeError: ord() expected a character, but string of length 0 found\"));\n",
                "        if chars.next().is_some() { panic!(\"TypeError: ord() expected a character, but string of length greater than 1 found\") }\n",
                "        MoltValue::Int(ch as i64)\n",
                "    } else { panic!(\"TypeError: ord() expected string of length 1\") }\n",
                "}\n\n",
            ));
        }
        if used("molt_ord_at(") {
            self.output.push_str(concat!(
                "fn molt_ord_at(obj: &MoltValue, key: &MoltValue) -> MoltValue {\n",
                "    let item = molt_get_item(obj, key);\n",
                "    molt_ord(&item)\n",
                "}\n\n",
            ));
        }

        self.emit_system_runtime_prelude(func_body);
        self.emit_callable_runtime_prelude(func_body);
    }

    // ── Function emission ─────────────────────────────────────────────────────
}
