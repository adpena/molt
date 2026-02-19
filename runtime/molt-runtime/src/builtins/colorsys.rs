use crate::PyToken;
use crate::object::ops::type_name;
use crate::{
    MoltObject, alloc_tuple, bigint_ptr_from_bits, bigint_ref, obj_from_bits, raise_exception,
    to_i64,
};
use num_traits::ToPrimitive;

fn coerce_real_to_f64_named(_py: &PyToken<'_>, val_bits: u64, name: &str) -> Option<f64> {
    let obj = obj_from_bits(val_bits);
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        if let Some(val) = unsafe { bigint_ref(ptr) }.to_f64() {
            return Some(val);
        }
        return raise_exception::<Option<f64>>(
            _py,
            "OverflowError",
            "int too large to convert to float",
        );
    }
    let type_label = type_name(_py, obj);
    let msg = format!("{name}() argument must be a real number, not {type_label}");
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn coerce_triplet(
    _py: &PyToken<'_>,
    name: &str,
    a_bits: u64,
    b_bits: u64,
    c_bits: u64,
) -> Option<(f64, f64, f64)> {
    let Some(a) = coerce_real_to_f64_named(_py, a_bits, name) else {
        return None;
    };
    let Some(b) = coerce_real_to_f64_named(_py, b_bits, name) else {
        return None;
    };
    let Some(c) = coerce_real_to_f64_named(_py, c_bits, name) else {
        return None;
    };
    Some((a, b, c))
}

fn float_tuple_bits(_py: &PyToken<'_>, values: (f64, f64, f64)) -> u64 {
    let elems = [
        MoltObject::from_float(values.0).bits(),
        MoltObject::from_float(values.1).bits(),
        MoltObject::from_float(values.2).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn rgb_to_hls_impl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let l = (minc + maxc) / 2.0;
    if minc == maxc {
        return (0.0, l, 0.0);
    }
    let s = if l <= 0.5 {
        (maxc - minc) / (maxc + minc)
    } else {
        (maxc - minc) / (2.0 - maxc - minc)
    };
    let rc = (maxc - r) / (maxc - minc);
    let gc = (maxc - g) / (maxc - minc);
    let bc = (maxc - b) / (maxc - minc);
    let mut h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    h = (h / 6.0).rem_euclid(1.0);
    (h, l, s)
}

fn hls_interp(m1: f64, m2: f64, h: f64) -> f64 {
    let mut h = h;
    if h < 0.0 {
        h += 1.0;
    }
    if h > 1.0 {
        h -= 1.0;
    }
    if h * 6.0 < 1.0 {
        return m1 + (m2 - m1) * h * 6.0;
    }
    if h * 2.0 < 1.0 {
        return m2;
    }
    if h * 3.0 < 2.0 {
        return m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0;
    }
    m1
}

fn hls_to_rgb_impl(h: f64, l: f64, s: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (l, l, l);
    }
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let m1 = 2.0 * l - m2;
    (
        hls_interp(m1, m2, h + 1.0 / 3.0),
        hls_interp(m1, m2, h),
        hls_interp(m1, m2, h - 1.0 / 3.0),
    )
}

fn rgb_to_hsv_impl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let v = maxc;
    if minc == maxc {
        return (0.0, 0.0, v);
    }
    let s = (maxc - minc) / maxc;
    let rc = (maxc - r) / (maxc - minc);
    let gc = (maxc - g) / (maxc - minc);
    let bc = (maxc - b) / (maxc - minc);
    let mut h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    h = (h / 6.0).rem_euclid(1.0);
    (h, s, v)
}

fn hsv_to_rgb_impl(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (v, v, v);
    }
    let scaled = h * 6.0;
    let i = scaled.trunc() as i64;
    let f = scaled - (i as f64);
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn rgb_to_yiq_impl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let y = 0.30 * r + 0.59 * g + 0.11 * b;
    let i = 0.74 * (r - y) - 0.27 * (b - y);
    let q = 0.48 * (r - y) + 0.41 * (b - y);
    (y, i, q)
}

fn yiq_to_rgb_impl(y: f64, i: f64, q: f64) -> (f64, f64, f64) {
    let r = y + 0.946_882_217_090_069_3 * i + 0.623_556_581_986_143_3 * q;
    let g = y - 0.274_787_646_298_978_34 * i - 0.635_691_079_187_380_1 * q;
    let b = y - 1.108_545_034_642_032_2 * i + 1.709_006_928_406_466_6 * q;
    (r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hls(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((r, g, b)) = coerce_triplet(_py, "rgb_to_hls", r_bits, g_bits, b_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, rgb_to_hls_impl(r, g, b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hls_to_rgb(h_bits: u64, l_bits: u64, s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((h, l, s)) = coerce_triplet(_py, "hls_to_rgb", h_bits, l_bits, s_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, hls_to_rgb_impl(h, l, s))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_hsv(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((r, g, b)) = coerce_triplet(_py, "rgb_to_hsv", r_bits, g_bits, b_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, rgb_to_hsv_impl(r, g, b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_hsv_to_rgb(h_bits: u64, s_bits: u64, v_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((h, s, v)) = coerce_triplet(_py, "hsv_to_rgb", h_bits, s_bits, v_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, hsv_to_rgb_impl(h, s, v))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_rgb_to_yiq(r_bits: u64, g_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((r, g, b)) = coerce_triplet(_py, "rgb_to_yiq", r_bits, g_bits, b_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, rgb_to_yiq_impl(r, g, b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_colorsys_yiq_to_rgb(y_bits: u64, i_bits: u64, q_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((y, i, q)) = coerce_triplet(_py, "yiq_to_rgb", y_bits, i_bits, q_bits) else {
            return MoltObject::none().bits();
        };
        float_tuple_bits(_py, yiq_to_rgb_impl(y, i, q))
    })
}
