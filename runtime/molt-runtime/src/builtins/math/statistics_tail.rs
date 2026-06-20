use super::*;

enum StatisticsNormalDistSamplesMode {
    Gauss,
    InvCdf,
}

#[derive(Clone)]
struct StatisticsRandomRng {
    mt: [u32; STATISTICS_RANDOM_N],
    index: usize,
    gauss_next: Option<f64>,
}

impl StatisticsRandomRng {
    fn from_seed_key(seed_key: &[u32]) -> Self {
        let mut out = Self {
            mt: [0; STATISTICS_RANDOM_N],
            index: STATISTICS_RANDOM_N,
            gauss_next: None,
        };
        out.init_by_array(seed_key);
        out.index = STATISTICS_RANDOM_N;
        out.gauss_next = None;
        out
    }

    fn init_genrand(&mut self, seed: u32) {
        self.mt[0] = seed;
        for i in 1..STATISTICS_RANDOM_N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
    }

    fn init_by_array(&mut self, init_key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let key_length = init_key.len();
        let k = STATISTICS_RANDOM_N.max(key_length);

        for _ in 0..k {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525));
            self.mt[i] = mixed.wrapping_add(init_key[j]).wrapping_add(j as u32);

            i += 1;
            j += 1;
            if i >= STATISTICS_RANDOM_N {
                self.mt[0] = self.mt[STATISTICS_RANDOM_N - 1];
                i = 1;
            }
            if j >= key_length {
                j = 0;
            }
        }

        for _ in 0..(STATISTICS_RANDOM_N - 1) {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941));
            self.mt[i] = mixed.wrapping_sub(i as u32);
            i += 1;
            if i >= STATISTICS_RANDOM_N {
                self.mt[0] = self.mt[STATISTICS_RANDOM_N - 1];
                i = 1;
            }
        }

        self.mt[0] = STATISTICS_RANDOM_UPPER_MASK;
    }

    fn twist(&mut self) {
        for i in 0..STATISTICS_RANDOM_N {
            let y = (self.mt[i] & STATISTICS_RANDOM_UPPER_MASK)
                | (self.mt[(i + 1) % STATISTICS_RANDOM_N] & STATISTICS_RANDOM_LOWER_MASK);
            let mut value = self.mt[(i + STATISTICS_RANDOM_M) % STATISTICS_RANDOM_N] ^ (y >> 1);
            if y & 1 != 0 {
                value ^= STATISTICS_RANDOM_MATRIX_A;
            }
            self.mt[i] = value;
        }
        self.index = 0;
    }

    fn rand_u32(&mut self) -> u32 {
        if self.index >= STATISTICS_RANDOM_N {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    fn random(&mut self) -> f64 {
        let a = (self.rand_u32() >> 5) as u64;
        let b = (self.rand_u32() >> 6) as u64;
        (a as f64 * 67_108_864.0 + b as f64) * STATISTICS_RANDOM_RECIP_BPF
    }

    fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        let z = if let Some(next) = self.gauss_next.take() {
            next
        } else {
            let x2pi = self.random() * core::f64::consts::TAU;
            let g2rad = math_sqrt(-2.0 * math_log(1.0 - self.random()));
            let z = math_cos(x2pi) * g2rad;
            self.gauss_next = Some(math_sin(x2pi) * g2rad);
            z
        };
        mu + z * sigma
    }
}

fn statistics_seed_type_error<T>(_py: &PyToken<'_>) -> Option<T> {
    raise_exception::<Option<T>>(
        _py,
        "TypeError",
        "The only supported seed types are:\nNone, int, float, str, bytes, and bytearray.",
    )
}

fn statistics_seed_bigint(_py: &PyToken<'_>, seed_bits: u64) -> Option<BigInt> {
    let seed_obj = obj_from_bits(seed_bits);
    if let Some(i) = to_i64(seed_obj) {
        return Some(BigInt::from(i).abs());
    }
    if let Some(ptr) = bigint_ptr_from_bits(seed_bits) {
        return Some(unsafe { bigint_ref(ptr).abs() });
    }
    if seed_obj.as_float().is_some() {
        let hash_bits = crate::molt_hash_builtin(seed_bits);
        if exception_pending(_py) {
            return None;
        }
        let hash_obj = obj_from_bits(hash_bits);
        let hash_u64 = if let Some(i) = to_i64(hash_obj) {
            i as u64
        } else if let Some(ptr) = bigint_ptr_from_bits(hash_bits) {
            let hash_big = unsafe { bigint_ref(ptr).clone() };
            let modulus = BigInt::one() << 64;
            hash_big.mod_floor(&modulus).to_u64().unwrap_or(0)
        } else {
            if maybe_ptr_from_bits(hash_bits).is_some() {
                dec_ref_bits(_py, hash_bits);
            }
            return raise_exception::<Option<BigInt>>(
                _py,
                "TypeError",
                "hash() should return an integer",
            );
        };
        if maybe_ptr_from_bits(hash_bits).is_some() {
            dec_ref_bits(_py, hash_bits);
        }
        return Some(BigInt::from(hash_u64));
    }

    let Some(seed_ptr) = seed_obj.as_ptr() else {
        return statistics_seed_type_error(_py);
    };
    let seed_bytes: Vec<u8> = unsafe {
        match object_type_id(seed_ptr) {
            TYPE_ID_STRING => {
                std::slice::from_raw_parts(string_bytes(seed_ptr), string_len(seed_ptr)).to_vec()
            }
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let Some(slice) = bytes_like_slice(seed_ptr) else {
                    return statistics_seed_type_error(_py);
                };
                slice.to_vec()
            }
            _ => return statistics_seed_type_error(_py),
        }
    };
    #[cfg(feature = "stdlib_crypto")]
    {
        let digest = Sha512::digest(&seed_bytes);
        let mut payload = Vec::with_capacity(seed_bytes.len() + digest.len());
        payload.extend_from_slice(&seed_bytes);
        payload.extend_from_slice(&digest);
        Some(BigInt::from(BigUint::from_bytes_be(&payload)))
    }
    #[cfg(not(feature = "stdlib_crypto"))]
    {
        // Without crypto support, fall back to using the raw seed bytes.
        Some(BigInt::from(BigUint::from_bytes_be(&seed_bytes)))
    }
}

fn statistics_seed_key(seed: &BigInt) -> Vec<u32> {
    let mut key = seed
        .abs()
        .to_biguint()
        .map_or_else(Vec::new, |v| v.to_u32_digits());
    if key.is_empty() {
        key.push(0);
    }
    key
}

fn statistics_normal_dist_sample_count(_py: &PyToken<'_>, n_bits: u64) -> Option<usize> {
    let n_type = type_name(_py, obj_from_bits(n_bits));
    let err = format!("'{n_type}' object cannot be interpreted as an integer");
    let n_big = index_bigint_from_obj(_py, n_bits, &err)?;
    if n_big.is_negative() {
        return Some(0);
    }
    if let Some(n) = n_big.to_usize() {
        return Some(n);
    }
    raise_exception::<Option<usize>>(
        _py,
        "OverflowError",
        "Python int too large to convert to C ssize_t",
    )
}

fn statistics_normal_dist_samples_mode(
    _py: &PyToken<'_>,
    seed_bits: u64,
) -> Option<(StatisticsNormalDistSamplesMode, u64)> {
    let seed_obj = obj_from_bits(seed_bits);
    let Some(seed_ptr) = seed_obj.as_ptr() else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let pair = unsafe {
        if object_type_id(seed_ptr) != TYPE_ID_TUPLE {
            None
        } else {
            let elems = seq_vec_ref(seed_ptr);
            if elems.len() == 2 {
                Some((elems[0], elems[1]))
            } else {
                None
            }
        }
    };
    let Some((marker_bits, inner_seed_bits)) = pair else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let marker_obj = obj_from_bits(marker_bits);
    let Some(marker_ptr) = marker_obj.as_ptr() else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let is_inv_cdf_mode = unsafe {
        if object_type_id(marker_ptr) != TYPE_ID_STRING {
            false
        } else {
            let marker_bytes =
                std::slice::from_raw_parts(string_bytes(marker_ptr), string_len(marker_ptr));
            marker_bytes == STATISTICS_NORMAL_DIST_INV_CDF_MODE_MARKER
        }
    };
    if !is_inv_cdf_mode {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    }
    Some((StatisticsNormalDistSamplesMode::InvCdf, inner_seed_bits))
}

fn statistics_normal_dist_samples_value(
    _py: &PyToken<'_>,
    mu_bits: u64,
    sigma_bits: u64,
    n_bits: u64,
    seed_bits: u64,
    random_fn_bits: u64,
) -> Option<u64> {
    let (mu, sigma) = statistics_normal_dist_params(_py, mu_bits, sigma_bits)?;
    let (mode, effective_seed_bits) = statistics_normal_dist_samples_mode(_py, seed_bits)?;
    let count = statistics_normal_dist_sample_count(_py, n_bits)?;
    let mut out_bits: Vec<u64> = Vec::with_capacity(count);
    let gauss_mu_bits = float_result_bits(_py, mu);
    let gauss_sigma_bits = float_result_bits(_py, sigma);

    let mut seeded_rng = if obj_from_bits(effective_seed_bits).is_none() {
        None
    } else {
        let seed_big = statistics_seed_bigint(_py, effective_seed_bits)?;
        Some(StatisticsRandomRng::from_seed_key(&statistics_seed_key(
            &seed_big,
        )))
    };

    for _ in 0..count {
        let sample = match mode {
            StatisticsNormalDistSamplesMode::Gauss => {
                if let Some(rng) = seeded_rng.as_mut() {
                    rng.gauss(mu, sigma)
                } else {
                    let sample_bits = unsafe {
                        call_callable2(_py, random_fn_bits, gauss_mu_bits, gauss_sigma_bits)
                    };
                    if exception_pending(_py) {
                        return None;
                    }
                    let sample_obj = obj_from_bits(sample_bits);
                    let Some(sample_real) = coerce_real_named(_py, sample_bits, "sample") else {
                        if sample_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, sample_bits);
                        }
                        return None;
                    };
                    let Some(sample_float) = coerce_to_f64(_py, sample_real) else {
                        if sample_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, sample_bits);
                        }
                        return None;
                    };
                    if sample_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, sample_bits);
                    }
                    sample_float
                }
            }
            StatisticsNormalDistSamplesMode::InvCdf => {
                let probability = if let Some(rng) = seeded_rng.as_mut() {
                    rng.random()
                } else {
                    let p_bits = unsafe { call_callable0(_py, random_fn_bits) };
                    if exception_pending(_py) {
                        return None;
                    }
                    let p_obj = obj_from_bits(p_bits);
                    let Some(p_real) = coerce_real_named(_py, p_bits, "p") else {
                        if p_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, p_bits);
                        }
                        return None;
                    };
                    let Some(p_float) = coerce_to_f64(_py, p_real) else {
                        if p_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, p_bits);
                        }
                        return None;
                    };
                    if p_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, p_bits);
                    }
                    p_float
                };
                if probability <= 0.0 || probability >= 1.0 {
                    return raise_exception::<Option<u64>>(
                        _py,
                        "ValueError",
                        "inv_cdf undefined for these parameters",
                    );
                }
                statistics_normal_dist_inv_cdf_raw(probability, mu, sigma)
            }
        };
        out_bits.push(float_result_bits(_py, sample));
    }

    let list_ptr = alloc_list(_py, &out_bits);
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn statistics_normal_dist_params(
    _py: &PyToken<'_>,
    mu_bits: u64,
    sigma_bits: u64,
) -> Option<(f64, f64)> {
    let mu_real = coerce_real_named(_py, mu_bits, "mu")?;
    let mu = coerce_to_f64(_py, mu_real)?;
    let sigma_real = coerce_real_named(_py, sigma_bits, "sigma")?;
    let sigma = coerce_to_f64(_py, sigma_real)?;
    if sigma < 0.0 {
        return raise_exception::<Option<(f64, f64)>>(
            _py,
            "ValueError",
            "sigma must be non-negative",
        );
    }
    Some((mu, sigma))
}

fn horner_eval(x: f64, coeffs: &[f64]) -> f64 {
    let mut acc = 0.0;
    for &coeff in coeffs {
        acc = acc * x + coeff;
    }
    acc
}

fn statistics_normal_dist_inv_cdf_raw(p: f64, mu: f64, sigma: f64) -> f64 {
    const A: [f64; 8] = [
        2.5090809287301227e3,
        3.343_057_558_358_813e4,
        6.726_577_092_700_87e4,
        4.592_195_393_154_987e4,
        1.373_169_376_550_946e4,
        1.9715909503065514e3,
        1.3314166789178438e2,
        3.3871328727963666,
    ];
    const B: [f64; 8] = [
        5.226_495_278_852_854e3,
        2.8729085735721943e4,
        3.930_789_580_009_271e4,
        2.1213794301586596e4,
        5.394_196_021_424_751e3,
        6.871_870_074_920_579e2,
        4.231_333_070_160_091e1,
        1.0,
    ];
    const C: [f64; 8] = [
        7.745_450_142_783_414e-4,
        2.2723844989269185e-2,
        2.417_807_251_774_506e-1,
        1.2704582524523684,
        3.6478483247632046,
        5.769_497_221_460_691,
        4.630_337_846_156_546,
        1.4234371107496836,
    ];
    const D: [f64; 8] = [
        1.0507500716444168e-9,
        5.475_938_084_995_344e-4,
        1.5198666563616457e-2,
        1.4810397642748008e-1,
        6.897_673_349_851e-1,
        1.6763848301838038,
        2.053_191_626_637_759,
        1.0,
    ];
    const E: [f64; 8] = [
        2.0103343992922881e-7,
        2.7115555687434876e-5,
        1.2426609473880784e-3,
        2.6532189526576123e-2,
        2.9656057182850489e-1,
        1.7848265399172913,
        5.463_784_911_164_114,
        6.657_904_643_501_103,
    ];
    const F: [f64; 8] = [
        2.0442631033899397e-15,
        1.421_511_758_316_446e-7,
        1.8463183175100547e-5,
        7.868_691_311_456_133e-4,
        1.4875361290850615e-2,
        1.369_298_809_227_358e-1,
        5.998_322_065_558_88e-1,
        1.0,
    ];

    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180625 - q * q;
        let x = (horner_eval(r, &A) * q) / horner_eval(r, &B);
        return mu + (x * sigma);
    }

    let mut r = if q <= 0.0 { p } else { 1.0 - p };
    r = math_sqrt(-math_log(r));
    let x = if r <= 5.0 {
        let rr = r - 1.6;
        horner_eval(rr, &C) / horner_eval(rr, &D)
    } else {
        let rr = r - 5.0;
        horner_eval(rr, &E) / horner_eval(rr, &F)
    };
    let x = if q < 0.0 { -x } else { x };
    mu + (x * sigma)
}

fn statistics_normal_dist_cdf_raw(x: f64, mu: f64, sigma: f64) -> f64 {
    0.5 * (1.0 + math_erf((x - mu) / (sigma * core::f64::consts::SQRT_2)))
}

fn materialize_statistics_slice(
    _py: &PyToken<'_>,
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> Option<u64> {
    let none_bits = MoltObject::none().bits();
    let start_obj = if is_truthy(_py, obj_from_bits(has_start_bits)) {
        start_bits
    } else {
        none_bits
    };
    let end_obj = if is_truthy(_py, obj_from_bits(has_end_bits)) {
        end_bits
    } else {
        none_bits
    };
    let slice_bits = crate::molt_slice_new(start_obj, end_obj, none_bits);
    if exception_pending(_py) {
        return None;
    }
    let sliced_bits = crate::molt_index(data_bits, slice_bits);
    if maybe_ptr_from_bits(slice_bits).is_some() {
        dec_ref_bits(_py, slice_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    Some(sliced_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(mean) = statistics_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, mean)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_stdev(data_bits: u64, xbar_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(stdev) = statistics_stdev_value(_py, data_bits, xbar_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, stdev)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_variance(data_bits: u64, xbar_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(variance) =
            statistics_variance_value(_py, data_bits, xbar_bits, false, "variance")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, variance)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_pvariance(data_bits: u64, mu_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(variance) = statistics_variance_value(_py, data_bits, mu_bits, true, "pvariance")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, variance)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_pstdev(data_bits: u64, mu_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(variance) = statistics_variance_value(_py, data_bits, mu_bits, true, "pstdev")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_sqrt(variance))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_fmean(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(values) = collect_real_vec(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        if values.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "fmean requires at least one data point",
            );
        }
        float_result_bits(_py, sum_f64_simd(&values) / values.len() as f64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(values) = statistics_collect_sorted_real(_py, data_bits, "median") else {
            return MoltObject::none().bits();
        };
        let n = values.len();
        let mid = n / 2;
        let out = if n % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        };
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_low(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(sorted_bits) = statistics_sorted_values(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        let sorted = obj_from_bits(sorted_bits);
        let Some(sorted_ptr) = sorted.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(sorted_ptr) != TYPE_ID_LIST {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "median_low expected sorted list payload",
                );
            }
            let elems = seq_vec_ref(sorted_ptr);
            if elems.is_empty() {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "median_low requires at least one data point",
                );
            }
            let idx = (elems.len() - 1) / 2;
            let out_bits = elems[idx];
            inc_ref_bits(_py, out_bits);
            if maybe_ptr_from_bits(sorted_bits).is_some() {
                dec_ref_bits(_py, sorted_bits);
            }
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_high(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(sorted_bits) = statistics_sorted_values(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        let sorted = obj_from_bits(sorted_bits);
        let Some(sorted_ptr) = sorted.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(sorted_ptr) != TYPE_ID_LIST {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "median_high expected sorted list payload",
                );
            }
            let elems = seq_vec_ref(sorted_ptr);
            if elems.is_empty() {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "median_high requires at least one data point",
                );
            }
            let idx = elems.len() / 2;
            let out_bits = elems[idx];
            inc_ref_bits(_py, out_bits);
            if maybe_ptr_from_bits(sorted_bits).is_some() {
                dec_ref_bits(_py, sorted_bits);
            }
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_grouped(data_bits: u64, interval_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(values) = statistics_collect_sorted_real(_py, data_bits, "median_grouped") else {
            return MoltObject::none().bits();
        };
        let Some(interval_real) = coerce_real_named(_py, interval_bits, "median_grouped") else {
            return MoltObject::none().bits();
        };
        let Some(interval) = coerce_to_f64(_py, interval_real) else {
            return MoltObject::none().bits();
        };
        let n = values.len();
        let mid = n / 2;
        let x = if n % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        };
        let lower = x - (interval / 2.0);
        let cf = values.iter().filter(|v| **v < x).count() as f64;
        let f = values
            .iter()
            .filter(|v| (**v - x).abs() <= f64::EPSILON)
            .count() as f64;
        if f == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "no grouped median for empty class");
        }
        float_result_bits(_py, lower + interval * ((n as f64 / 2.0 - cf) / f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mode(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = statistics_mode_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_multimode(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = statistics_multimode_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_quantiles(
    data_bits: u64,
    n_bits: u64,
    inclusive_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(bits) = statistics_quantiles_value(_py, data_bits, n_bits, inclusive_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_harmonic_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = statistics_harmonic_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_geometric_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = statistics_geometric_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_covariance(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = statistics_covariance_value(_py, x_bits, y_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_correlation(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = statistics_correlation_value(_py, x_bits, y_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_linear_regression(
    x_bits: u64,
    y_bits: u64,
    proportional_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((slope, intercept)) =
            statistics_linear_regression_value(_py, x_bits, y_bits, proportional_bits)
        else {
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                float_result_bits(_py, slope),
                float_result_bits(_py, intercept),
            ],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_new(mu_bits: u64, sigma_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[float_result_bits(_py, mu), float_result_bits(_py, sigma)],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_samples(
    mu_bits: u64,
    sigma_bits: u64,
    n_bits: u64,
    seed_bits: u64,
    random_fn_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = statistics_normal_dist_samples_value(
            _py,
            mu_bits,
            sigma_bits,
            n_bits,
            seed_bits,
            random_fn_bits,
        ) else {
            return MoltObject::none().bits();
        };
        value
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_inv_cdf(
    p_bits: u64,
    mu_bits: u64,
    sigma_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let Some(p_real) = coerce_real_named(_py, p_bits, "p") else {
            return MoltObject::none().bits();
        };
        let Some(p) = coerce_to_f64(_py, p_real) else {
            return MoltObject::none().bits();
        };
        if p <= 0.0 || p >= 1.0 {
            return raise_exception::<_>(_py, "ValueError", "p must be in the range 0.0 < p < 1.0");
        }
        float_result_bits(_py, statistics_normal_dist_inv_cdf_raw(p, mu, sigma))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_pdf(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let variance = sigma * sigma;
        if variance == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "pdf() not defined when sigma is zero");
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        let diff = x - mu;
        let out = math_exp(diff * diff / (-2.0 * variance))
            / math_sqrt(core::f64::consts::TAU * variance);
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_cdf(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        if sigma == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "cdf() not defined when sigma is zero");
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, statistics_normal_dist_cdf_raw(x, mu, sigma))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_zscore(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        if sigma == 0.0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "zscore() not defined when sigma is zero",
            );
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, (x - mu) / sigma)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_overlap(
    mu_a_bits: u64,
    sigma_a_bits: u64,
    mu_b_bits: u64,
    sigma_b_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some((mut mu_x, mut sigma_x)) =
            statistics_normal_dist_params(_py, mu_a_bits, sigma_a_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some((mut mu_y, mut sigma_y)) =
            statistics_normal_dist_params(_py, mu_b_bits, sigma_b_bits)
        else {
            return MoltObject::none().bits();
        };
        if (sigma_y, mu_y) < (sigma_x, mu_x) {
            core::mem::swap(&mut mu_x, &mut mu_y);
            core::mem::swap(&mut sigma_x, &mut sigma_y);
        }
        let x_var = sigma_x * sigma_x;
        let y_var = sigma_y * sigma_y;
        if x_var == 0.0 || y_var == 0.0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "overlap() not defined when sigma is zero",
            );
        }
        let dv = y_var - x_var;
        let dm = (mu_y - mu_x).abs();
        if dv == 0.0 {
            let out = 1.0 - math_erf(dm / (2.0 * sigma_x * core::f64::consts::SQRT_2));
            return float_result_bits(_py, out);
        }
        let a = mu_x * y_var - mu_y * x_var;
        let inner = dm * dm + dv * math_log(y_var / x_var);
        if inner < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "overlap() domain error");
        }
        let b = sigma_x * sigma_y * math_sqrt(inner);
        let x1 = (a + b) / dv;
        let x2 = (a - b) / dv;
        let delta1 = (statistics_normal_dist_cdf_raw(x1, mu_y, sigma_y)
            - statistics_normal_dist_cdf_raw(x1, mu_x, sigma_x))
        .abs();
        let delta2 = (statistics_normal_dist_cdf_raw(x2, mu_y, sigma_y)
            - statistics_normal_dist_cdf_raw(x2, mu_x, sigma_x))
        .abs();
        float_result_bits(_py, 1.0 - (delta1 + delta2))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mean_slice(
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = obj_from_bits(data_bits);
        if let Some(data_ptr) = data.as_ptr() {
            unsafe {
                let ty = object_type_id(data_ptr);
                if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(data_ptr);
                    let Some((start, end)) = normalize_slice_step1_bounds(
                        _py,
                        elems.len(),
                        start_bits,
                        end_bits,
                        has_start_bits,
                        has_end_bits,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    if start >= end {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "mean requires at least one data point",
                        );
                    }
                    let mut sum = 0.0_f64;
                    let mut compensation = 0.0_f64;
                    let mut count: usize = 0;
                    for &val_bits in &elems[start..end] {
                        let Some(f) = statistics_coerce_elem_fast_f64(_py, val_bits, "mean") else {
                            return MoltObject::none().bits();
                        };
                        let y = f - compensation;
                        let t = sum + y;
                        compensation = (t - sum) - y;
                        sum = t;
                        count += 1;
                    }
                    return float_result_bits(_py, sum / count as f64);
                }
            }
        }
        let Some(sliced_bits) = materialize_statistics_slice(
            _py,
            data_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let out = match statistics_mean_value(_py, sliced_bits) {
            Some(mean) => float_result_bits(_py, mean),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_stdev_slice(
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let data = obj_from_bits(data_bits);
        if let Some(data_ptr) = data.as_ptr() {
            unsafe {
                let ty = object_type_id(data_ptr);
                if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(data_ptr);
                    let Some((start, end)) = normalize_slice_step1_bounds(
                        _py,
                        elems.len(),
                        start_bits,
                        end_bits,
                        has_start_bits,
                        has_end_bits,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let n = end.saturating_sub(start);
                    if n < 2 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "stdev requires at least two data points",
                        );
                    }
                    let mut count = 0.0_f64;
                    let mut mean = 0.0_f64;
                    let mut m2 = 0.0_f64;
                    for &val_bits in &elems[start..end] {
                        let Some(x) = statistics_coerce_elem_fast_f64(_py, val_bits, "stdev")
                        else {
                            return MoltObject::none().bits();
                        };
                        count += 1.0;
                        let delta = x - mean;
                        mean += delta / count;
                        let delta2 = x - mean;
                        m2 += delta * delta2;
                    }
                    let variance = if m2 < 0.0 && m2 > -f64::EPSILON {
                        0.0
                    } else {
                        m2 / (count - 1.0)
                    };
                    return float_result_bits(_py, math_sqrt(variance));
                }
            }
        }
        let Some(sliced_bits) = materialize_statistics_slice(
            _py,
            data_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let none_bits = MoltObject::none().bits();
        let out = match statistics_stdev_value(_py, sliced_bits, none_bits) {
            Some(stdev) => float_result_bits(_py, stdev),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}
