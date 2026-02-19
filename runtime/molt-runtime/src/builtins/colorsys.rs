use crate::PyToken;
use crate::object::ops::type_name;
use crate::{
    MoltObject, alloc_tuple, bigint_ptr_from_bits, bigint_ref, dec_ref_bits, exception_pending,
    missing_bits, obj_from_bits, raise_exception, to_i64,
};
use num_traits::ToPrimitive;

const ONE_THIRD: f64 = 1.0 / 3.0;
const ONE_SIXTH: f64 = 1.0 / 6.0;
const TWO_THIRD: f64 = 2.0 / 3.0;

fn alloc_tuple_bits(_py: &PyToken<'_>, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(_py, elems);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn coerce_f64_strict(_py: &PyToken<'_>, val_bits: u64, op: &str) -> Option<f64> {
    let obj = obj_from_bits(val_bits);
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        let big = unsafe { bigint_ref(ptr) };
        if let Some(val) = big.to_f64() {
            return Some(val);
        }
        return raise_exception::<Option<f64>>(
            _py,
            "OverflowError",
            "int too large to convert to float",
        );
    }
    let type_label = type_name(_py, obj);
    let msg = format!("unsupported operand type(s) for {op}: 'float' and '{type_label}'");
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn max_min_rgb(_py: &PyToken<'_>, r_bits: u64, g_bits: u64, b_bits: u64) -> Option<(f64, f64)> {
    let args_bits = alloc_tuple_bits(_py, &[r_bits, g_bits, b_bits]);
    let none_bits = MoltObject::none().bits();
    let missing = missing_bits(_py);
    let max_bits = crate::object::ops::molt_max_builtin(args_bits, none_bits, missing);
    if exception_pending(_py) {
        return None;
    }
    let min_bits = crate::object::ops::molt_min_builtin(args_bits, none_bits, missing);
    if exception_pending(_py) {
        if obj_from_bits(max_bits).as_ptr().is_some() {
            dec_ref_bits(_py, max_bits);
        }
        return None;
    }
    let maxc = coerce_f64_strict(_py, max_bits, "*")?;
    let minc = coerce_f64_strict(_py, min_bits, "*")?;
    if obj_from_bits(max_bits).as_ptr().is_some() {
        dec_ref_bits(_py, max_bits);
    }
    if obj_from_bits(min_bits).as_ptr().is_some() {
        dec_ref_bits(_py, min_bits);
    }
    if obj_from_bits(args_bits).as_ptr().is_some() {
        dec_ref_bits(_py, args_bits);
    }
    Some((maxc, minc))
}

fn clamp_unit(value: f64) -> f64 {
    if value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

fn hue_mod(value: f64) -> f64 {
    value.rem_euclid(1.0)
}

fn colorsys_v(m1: f64, m2: f64, hue: f64) -> f64 {
    let hue = hue_mod(hue);
    if hue < ONE_SIXTH {
        return m1 + (m2 - m1) * hue * 6.0;
    }
    if hue < 0.5 {
        return m2;
    }
    if hue < TWO_THIRD {
        return m1 + (m2 - m1) * (TWO_THIRD - hue) * 6.0;
    }
    m1
}

fn alloc_tuple3(_py: &PyToken<'_>, a: f64, b: f64, c: f64) -> u64 {
    let elems = [
        MoltObject::from_float(a).bits(),
        MoltObject::from_float(b).bits(),
        MoltObject::from_float(c).bits(),
    ];
    alloc_tuple_bits(_py, &elems)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_yiq(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(r) = coerce_f64_strict(_py, r_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_f64_strict(_py, g_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_f64_strict(_py, b_bits, "*") else {
            return MoltObject::none().bits();
        };
        let y = 0.30 * r + 0.59 * g + 0.11 * b;
        let i = 0.74 * (r - y) - 0.27 * (b - y);
        let q = 0.48 * (r - y) + 0.41 * (b - y);
        alloc_tuple3(_py, y, i, q)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_yiq_to_rgb(y_bits: u64, i_bits: u64, q_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(y) = coerce_f64_strict(_py, y_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(i) = coerce_f64_strict(_py, i_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(q) = coerce_f64_strict(_py, q_bits, "*") else {
            return MoltObject::none().bits();
        };
        let r = clamp_unit(y + 0.9468822170900693 * i + 0.6235565819861433 * q);
        let g = clamp_unit(y - 0.27478764629897834 * i - 0.6356910791873801 * q);
        let b = clamp_unit(y - 1.1085450346420322 * i + 1.7090069284064666 * q);
        alloc_tuple3(_py, r, g, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hls(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((maxc, minc)) = max_min_rgb(_py, r_bits, g_bits, b_bits) else {
            return MoltObject::none().bits();
        };
        let Some(r) = coerce_f64_strict(_py, r_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_f64_strict(_py, g_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_f64_strict(_py, b_bits, "*") else {
            return MoltObject::none().bits();
        };
        let sumc = maxc + minc;
        let rangec = maxc - minc;
        let l = sumc / 2.0;
        if minc == maxc {
            return alloc_tuple3(_py, 0.0, l, 0.0);
        }
        let s = if l <= 0.5 {
            rangec / sumc
        } else {
            rangec / (2.0 - maxc - minc)
        };
        let rc = (maxc - r) / rangec;
        let gc = (maxc - g) / rangec;
        let bc = (maxc - b) / rangec;
        let mut h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        h = hue_mod(h / 6.0);
        alloc_tuple3(_py, h, l, s)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hls_to_rgb(h_bits: u64, l_bits: u64, s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(h) = coerce_f64_strict(_py, h_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(l) = coerce_f64_strict(_py, l_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(s) = coerce_f64_strict(_py, s_bits, "*") else {
            return MoltObject::none().bits();
        };
        if s == 0.0 {
            return alloc_tuple3(_py, l, l, l);
        }
        let m2 = if l <= 0.5 {
            l * (1.0 + s)
        } else {
            l + s - (l * s)
        };
        let m1 = 2.0 * l - m2;
        let r = colorsys_v(m1, m2, h + ONE_THIRD);
        let g = colorsys_v(m1, m2, h);
        let b = colorsys_v(m1, m2, h - ONE_THIRD);
        alloc_tuple3(_py, r, g, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hsv(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((maxc, minc)) = max_min_rgb(_py, r_bits, g_bits, b_bits) else {
            return MoltObject::none().bits();
        };
        let Some(r) = coerce_f64_strict(_py, r_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_f64_strict(_py, g_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_f64_strict(_py, b_bits, "*") else {
            return MoltObject::none().bits();
        };
        let rangec = maxc - minc;
        let v = maxc;
        if minc == maxc {
            return alloc_tuple3(_py, 0.0, 0.0, v);
        }
        let s = rangec / maxc;
        let rc = (maxc - r) / rangec;
        let gc = (maxc - g) / rangec;
        let bc = (maxc - b) / rangec;
        let mut h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        h = hue_mod(h / 6.0);
        alloc_tuple3(_py, h, s, v)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hsv_to_rgb(h_bits: u64, s_bits: u64, v_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(h) = coerce_f64_strict(_py, h_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(s) = coerce_f64_strict(_py, s_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(v) = coerce_f64_strict(_py, v_bits, "*") else {
            return MoltObject::none().bits();
        };
        if s == 0.0 {
            return alloc_tuple3(_py, v, v, v);
        }
        let h6 = h * 6.0;
        if h6.is_nan() {
            return raise_exception::<u64>(_py, "ValueError", "cannot convert float NaN to integer");
        }
        if h6.is_infinite() {
            return raise_exception::<u64>(
                _py,
                "OverflowError",
                "cannot convert float infinity to integer",
            );
        }
        let mut i = h6.trunc() as i64;
        let f = h6 - (i as f64);
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        i = i.rem_euclid(6);
        match i {
            0 => alloc_tuple3(_py, v, t, p),
            1 => alloc_tuple3(_py, q, v, p),
            2 => alloc_tuple3(_py, p, v, t),
            3 => alloc_tuple3(_py, p, q, v),
            4 => alloc_tuple3(_py, t, p, v),
            _ => alloc_tuple3(_py, v, p, q),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_v(m1_bits: u64, m2_bits: u64, hue_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(m1) = coerce_f64_strict(_py, m1_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(m2) = coerce_f64_strict(_py, m2_bits, "*") else {
            return MoltObject::none().bits();
        };
        let Some(hue) = coerce_f64_strict(_py, hue_bits, "*") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(colorsys_v(m1, m2, hue)).bits()
    })
}
