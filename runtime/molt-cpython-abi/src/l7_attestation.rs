//! Shared policy for the native L7 performance attestations.
//!
//! This module is absent from default builds. Explicit attestation features
//! compile it into both the ABI-boundary and runtime-backed harnesses so sample
//! calibration and execution control cannot drift independently.

use std::ffi::c_void;

pub const SAMPLE_COUNT: usize = 9;
pub const MINIMUM_SAMPLE_NS: u128 = 20_000_000;
pub const CALIBRATION_TARGET_NS: u128 = 100_000_000;
pub const MAX_TIMED_ITERATIONS: usize = 1 << 28;

/// Calibrate a sample to a target with 5x headroom over the fail-closed floor.
///
/// `measure` must execute and semantically validate exactly the requested
/// number of operations and return their loop-inclusive elapsed nanoseconds.
pub fn calibrate_timed_iterations(
    seed_iterations: usize,
    mut measure: impl FnMut(usize) -> u128,
) -> usize {
    let mut iterations = seed_iterations.max(1).min(MAX_TIMED_ITERATIONS);
    loop {
        let elapsed_ns = measure(iterations).max(1);
        if elapsed_ns >= CALIBRATION_TARGET_NS || iterations == MAX_TIMED_ITERATIONS {
            return iterations;
        }
        let growth = CALIBRATION_TARGET_NS.div_ceil(elapsed_ns).clamp(2, 64) as usize;
        iterations = iterations.saturating_mul(growth).min(MAX_TIMED_ITERATIONS);
    }
}

/// Parse and enforce a single-logical-CPU affinity mask for the current thread.
///
/// Cross-process medians are meaningless on heterogeneous CPUs if the OS may
/// alternate benchmark threads between performance and efficiency cores. The
/// runner supplies the mask and every child applies it before warmup.
pub fn enforce_current_thread_affinity(mask_text: &str) -> usize {
    let digits = mask_text
        .strip_prefix("0x")
        .or_else(|| mask_text.strip_prefix("0X"))
        .unwrap_or(mask_text);
    let mask = usize::from_str_radix(digits, 16)
        .unwrap_or_else(|_| panic!("invalid hexadecimal L7 affinity mask {mask_text:?}"));
    assert!(
        mask.is_power_of_two(),
        "L7 affinity mask must select exactly one logical CPU"
    );
    enforce_platform_affinity(mask);
    mask
}

#[cfg(windows)]
fn enforce_platform_affinity(mask: usize) {
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn SetThreadAffinityMask(thread: *mut c_void, affinity_mask: usize) -> usize;
    }

    let previous = unsafe { SetThreadAffinityMask(GetCurrentThread(), mask) };
    assert_ne!(
        previous,
        0,
        "SetThreadAffinityMask rejected L7 affinity mask {mask:#x}: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(target_os = "linux")]
fn enforce_platform_affinity(mask: usize) {
    let cpu = mask.trailing_zeros() as usize;
    assert!(
        cpu < libc::CPU_SETSIZE as usize,
        "L7 affinity CPU {cpu} exceeds cpu_set_t capacity"
    );
    let mut set = unsafe { std::mem::zeroed::<libc::cpu_set_t>() };
    unsafe {
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
    }
    let status = unsafe {
        libc::sched_setaffinity(
            0,
            std::mem::size_of::<libc::cpu_set_t>(),
            &set as *const libc::cpu_set_t,
        )
    };
    assert_eq!(
        status,
        0,
        "sched_setaffinity rejected L7 affinity mask {mask:#x}: {}",
        std::io::Error::last_os_error()
    );
}

#[cfg(not(any(windows, target_os = "linux")))]
fn enforce_platform_affinity(_mask: usize) {
    panic!("L7 attestation requires current-thread affinity support on this platform");
}

pub fn normalized_affinity_mask(mask: usize) -> String {
    format!("{mask:#x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_uses_shared_target_and_growth_policy() {
        let iterations = calibrate_timed_iterations(4, |count| count as u128 * 1_000_000);
        assert_eq!(iterations, 100);
        assert!(CALIBRATION_TARGET_NS >= MINIMUM_SAMPLE_NS * 5);
    }

    #[test]
    #[should_panic(expected = "exactly one logical CPU")]
    fn affinity_rejects_multi_cpu_masks_before_platform_call() {
        enforce_current_thread_affinity("0x3");
    }
}
