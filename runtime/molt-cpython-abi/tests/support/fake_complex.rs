use molt_cpython_abi::hooks::OwnedHandleResult;
pub fn allocate(real: f64, imag: f64) -> u64 {
    super::fake_runtime::heap_complex(real, imag)
}
pub fn contains(bits: u64) -> bool {
    super::fake_runtime::complex_value(bits).is_some()
}

pub unsafe extern "C" fn from_doubles(real: f64, imag: f64) -> OwnedHandleResult {
    OwnedHandleResult::ok(allocate(real, imag))
}

pub unsafe extern "C" fn parts(bits: u64, real: *mut f64, imag: *mut f64) -> i32 {
    let Some((real_value, imag_value)) = super::fake_runtime::complex_value(bits) else {
        return -1;
    };
    if real.is_null() || imag.is_null() {
        return -1;
    }
    unsafe {
        *real = real_value;
        *imag = imag_value;
    }
    0
}

pub unsafe extern "C" fn hash(bits: u64) -> i64 {
    let Some((real, imag)) = super::fake_runtime::complex_value(bits) else {
        return -1;
    };
    let real_hash =
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(std::ptr::null_mut(), real) }
            as usize;
    let imag_hash =
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(std::ptr::null_mut(), imag) }
            as usize;
    let combined = real_hash.wrapping_add(1_000_003usize.wrapping_mul(imag_hash)) as isize;
    (if combined == -1 { -2 } else { combined }) as i64
}
