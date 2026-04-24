use molt_obj_model::MoltObject;
use num_traits::ToPrimitive;

use crate::{
    PyToken, alloc_tuple, attr_lookup_ptr_allow_missing, bigint_from_f64_trunc,
    bigint_ptr_from_bits, bigint_ref, call_callable0, class_name_for_error, dec_ref_bits,
    exception_pending, intern_static_name, maybe_ptr_from_bits, obj_from_bits, raise_exception,
    runtime_state, to_i64, type_name, type_of_bits,
};

const ONE_THIRD: f64 = 1.0 / 3.0;
const ONE_SIXTH: f64 = 1.0 / 6.0;
const TWO_THIRD: f64 = 2.0 / 3.0;

fn tuple3_bits(_py: &PyToken<'_>, a_bits: u64, b_bits: u64, c_bits: u64) -> u64 {
    let tuple_ptr = alloc_tuple(_py, &[a_bits, b_bits, c_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn coerce_real_f64(_py: &PyToken<'_>, val_bits: u64) -> Option<f64> {
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
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = res_obj.as_float() {
                    return Some(f);
                }
                let owner = class_name_for_error(type_of_bits(_py, val_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(i as f64);
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    if let Some(val) = big.to_f64() {
                        return Some(val);
                    }
                    return raise_exception::<Option<f64>>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    let type_label = type_name(_py, obj);
    let msg = format!("must be real number, not {type_label}");
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn py_max(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

fn py_min(a: f64, b: f64) -> f64 {
    if a > b { b } else { a }
}

fn hsv_sector(_py: &PyToken<'_>, h: f64) -> Option<(i64, f64)> {
    let h6 = h * 6.0;
    if h6.is_nan() {
        return raise_exception::<Option<(i64, f64)>>(
            _py,
            "ValueError",
            "cannot convert float NaN to integer",
        );
    }
    if h6.is_infinite() {
        return raise_exception::<Option<(i64, f64)>>(
            _py,
            "OverflowError",
            "cannot convert float infinity to integer",
        );
    }
    let i_big = bigint_from_f64_trunc(h6);
    let i_float = i_big.to_f64().unwrap_or_else(|| h6.trunc());
    let f = h6 - i_float;
    let mut i_mod: i64 = (&i_big % 6_i32).to_i64().unwrap_or(0);
    if i_mod < 0 {
        i_mod += 6;
    }
    Some((i_mod, f))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_yiq(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(r) = coerce_real_f64(_py, r_bits) else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_real_f64(_py, g_bits) else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_real_f64(_py, b_bits) else {
            return MoltObject::none().bits();
        };
        let y = 0.30 * r + 0.59 * g + 0.11 * b;
        let i = 0.74 * (r - y) - 0.27 * (b - y);
        let q = 0.48 * (r - y) + 0.41 * (b - y);
        tuple3_bits(
            _py,
            MoltObject::from_float(y).bits(),
            MoltObject::from_float(i).bits(),
            MoltObject::from_float(q).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_yiq_to_rgb(y_bits: u64, i_bits: u64, q_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(y) = coerce_real_f64(_py, y_bits) else {
            return MoltObject::none().bits();
        };
        let Some(i) = coerce_real_f64(_py, i_bits) else {
            return MoltObject::none().bits();
        };
        let Some(q) = coerce_real_f64(_py, q_bits) else {
            return MoltObject::none().bits();
        };
        let mut r = y + 0.9468822170900693 * i + 0.6235565819861433 * q;
        let mut g = y - 0.27478764629897834 * i - 0.6356910791873801 * q;
        let mut b = y - 1.1085450346420322 * i + 1.7090069284064666 * q;
        if r < 0.0 {
            r = 0.0;
        }
        if g < 0.0 {
            g = 0.0;
        }
        if b < 0.0 {
            b = 0.0;
        }
        if r > 1.0 {
            r = 1.0;
        }
        if g > 1.0 {
            g = 1.0;
        }
        if b > 1.0 {
            b = 1.0;
        }
        tuple3_bits(
            _py,
            MoltObject::from_float(r).bits(),
            MoltObject::from_float(g).bits(),
            MoltObject::from_float(b).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hls(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(r) = coerce_real_f64(_py, r_bits) else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_real_f64(_py, g_bits) else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_real_f64(_py, b_bits) else {
            return MoltObject::none().bits();
        };
        let maxc = py_max(py_max(r, g), b);
        let minc = py_min(py_min(r, g), b);
        let sumc = maxc + minc;
        let rangec = maxc - minc;
        let l = sumc / 2.0;
        if minc == maxc {
            return tuple3_bits(
                _py,
                MoltObject::from_float(0.0).bits(),
                MoltObject::from_float(l).bits(),
                MoltObject::from_float(0.0).bits(),
            );
        }
        let s = if l <= 0.5 {
            rangec / sumc
        } else {
            rangec / (2.0 - maxc - minc)
        };
        let rc = (maxc - r) / rangec;
        let gc = (maxc - g) / rangec;
        let bc = (maxc - b) / rangec;
        let h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        let h = (h / 6.0).rem_euclid(1.0);
        tuple3_bits(
            _py,
            MoltObject::from_float(h).bits(),
            MoltObject::from_float(l).bits(),
            MoltObject::from_float(s).bits(),
        )
    })
}

fn hls_value(m1: f64, m2: f64, hue: f64) -> f64 {
    let hue = hue.rem_euclid(1.0);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hls_to_rgb(h_bits: u64, l_bits: u64, s_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(h) = coerce_real_f64(_py, h_bits) else {
            return MoltObject::none().bits();
        };
        let Some(l) = coerce_real_f64(_py, l_bits) else {
            return MoltObject::none().bits();
        };
        let Some(s) = coerce_real_f64(_py, s_bits) else {
            return MoltObject::none().bits();
        };
        if s == 0.0 {
            return tuple3_bits(
                _py,
                MoltObject::from_float(l).bits(),
                MoltObject::from_float(l).bits(),
                MoltObject::from_float(l).bits(),
            );
        }
        let m2 = if l <= 0.5 {
            l * (1.0 + s)
        } else {
            l + s - (l * s)
        };
        let m1 = 2.0 * l - m2;
        let r = hls_value(m1, m2, h + ONE_THIRD);
        let g = hls_value(m1, m2, h);
        let b = hls_value(m1, m2, h - ONE_THIRD);
        tuple3_bits(
            _py,
            MoltObject::from_float(r).bits(),
            MoltObject::from_float(g).bits(),
            MoltObject::from_float(b).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hsv(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(r) = coerce_real_f64(_py, r_bits) else {
            return MoltObject::none().bits();
        };
        let Some(g) = coerce_real_f64(_py, g_bits) else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_real_f64(_py, b_bits) else {
            return MoltObject::none().bits();
        };
        let maxc = py_max(py_max(r, g), b);
        let minc = py_min(py_min(r, g), b);
        let rangec = maxc - minc;
        let v = maxc;
        if minc == maxc {
            return tuple3_bits(
                _py,
                MoltObject::from_float(0.0).bits(),
                MoltObject::from_float(0.0).bits(),
                MoltObject::from_float(v).bits(),
            );
        }
        let s = rangec / maxc;
        let rc = (maxc - r) / rangec;
        let gc = (maxc - g) / rangec;
        let bc = (maxc - b) / rangec;
        let h = if r == maxc {
            bc - gc
        } else if g == maxc {
            2.0 + rc - bc
        } else {
            4.0 + gc - rc
        };
        let h = (h / 6.0).rem_euclid(1.0);
        tuple3_bits(
            _py,
            MoltObject::from_float(h).bits(),
            MoltObject::from_float(s).bits(),
            MoltObject::from_float(v).bits(),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hsv_to_rgb(h_bits: u64, s_bits: u64, v_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(h) = coerce_real_f64(_py, h_bits) else {
            return MoltObject::none().bits();
        };
        let Some(s) = coerce_real_f64(_py, s_bits) else {
            return MoltObject::none().bits();
        };
        let Some(v) = coerce_real_f64(_py, v_bits) else {
            return MoltObject::none().bits();
        };
        if s == 0.0 {
            return tuple3_bits(
                _py,
                MoltObject::from_float(v).bits(),
                MoltObject::from_float(v).bits(),
                MoltObject::from_float(v).bits(),
            );
        }
        let Some((i, f)) = hsv_sector(_py, h) else {
            return MoltObject::none().bits();
        };
        let p = v * (1.0 - s);
        let q = v * (1.0 - s * f);
        let t = v * (1.0 - s * (1.0 - f));
        let (r, g, b) = match i {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        tuple3_bits(
            _py,
            MoltObject::from_float(r).bits(),
            MoltObject::from_float(g).bits(),
            MoltObject::from_float(b).bits(),
        )
    })
}
