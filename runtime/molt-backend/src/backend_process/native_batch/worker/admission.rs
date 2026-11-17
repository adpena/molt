//! Memory admission for native batch worker processes.
//!
//! The compiler pool's width bounds how many workers run at once by CPU. This
//! gate also bounds them by memory: a worker starts only when its expected
//! peak fits in the worker-tree budget beside the workers already running
//! (`molt_passes::memory_budget::worker_tree_budget_bytes`, the launcher's live
//! availability after its reserve). The expected peak is the largest
//! resident peak a worker of this process has reached; before any worker has
//! finished it is a conservative prior. One worker always runs, so a budget smaller than one
//! worker slows a build to one worker at a time; it never stalls it.

use std::sync::{Condvar, Mutex};

/// Expected peak of a worker before any has finished. Measured 2026-11-15
/// (macOS arm64, release backend): a micro-profile stdlib batch worker peaks
/// near 110 MB; the prior leaves room for larger programs until a measurement
/// replaces it.
const PRIOR_WORKER_PEAK_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
struct AdmissionState {
    running: usize,
    reserved_bytes: u64,
    /// The largest resident peak a finished worker reached.
    measured_peak_bytes: Option<u64>,
}

impl AdmissionState {
    fn expected_peak_bytes(&self) -> u64 {
        self.measured_peak_bytes.unwrap_or(PRIOR_WORKER_PEAK_BYTES)
    }

    /// Admit one worker under `budget_bytes` and return its reservation, or
    /// `None` to wait. With no budget every worker is admitted.
    fn try_admit(&mut self, budget_bytes: Option<u64>) -> Option<u64> {
        let reservation = self.expected_peak_bytes();
        let fits = match budget_bytes {
            None => true,
            Some(budget) => self.reserved_bytes.saturating_add(reservation) <= budget,
        };
        if self.running > 0 && !fits {
            return None;
        }
        self.running += 1;
        self.reserved_bytes = self.reserved_bytes.saturating_add(reservation);
        Some(reservation)
    }

    /// Return a worker's reservation and record the peak it reached.
    fn release(&mut self, reservation: u64, measured_peak_bytes: Option<u64>) {
        self.running -= 1;
        self.reserved_bytes = self.reserved_bytes.saturating_sub(reservation);
        if let Some(peak) = measured_peak_bytes {
            self.measured_peak_bytes =
                Some(self.measured_peak_bytes.map_or(peak, |max| max.max(peak)));
        }
    }
}

struct WorkerAdmission {
    state: Mutex<AdmissionState>,
    released: Condvar,
}

static ADMISSION: WorkerAdmission = WorkerAdmission {
    state: Mutex::new(AdmissionState {
        running: 0,
        reserved_bytes: 0,
        measured_peak_bytes: None,
    }),
    released: Condvar::new(),
};

/// One admitted worker's reservation; [`WorkerPermit::finish`] returns it with
/// the worker's measured peak, and dropping it unfinished returns it alone.
pub(super) struct WorkerPermit {
    reservation: Option<u64>,
}

impl WorkerPermit {
    /// Block until a worker fits the current request's budget.
    pub(super) fn acquire() -> Self {
        let budget = molt_passes::memory_budget::worker_tree_budget_bytes();
        let mut state = ADMISSION
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(reservation) = state.try_admit(budget) {
                return Self {
                    reservation: Some(reservation),
                };
            }
            state = ADMISSION
                .released
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Release the reservation, recording the worker's resident peak.
    pub(super) fn finish(mut self, measured_peak_bytes: Option<u64>) {
        self.release(measured_peak_bytes);
    }

    fn release(&mut self, measured_peak_bytes: Option<u64>) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        ADMISSION
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release(reservation, measured_peak_bytes);
        ADMISSION.released.notify_all();
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.release(None);
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionState, PRIOR_WORKER_PEAK_BYTES};

    const MIB: u64 = 1024 * 1024;

    fn state(measured_peak: Option<u64>) -> AdmissionState {
        AdmissionState {
            running: 0,
            reserved_bytes: 0,
            measured_peak_bytes: measured_peak,
        }
    }

    #[test]
    fn admits_workers_until_their_expected_peaks_fill_the_budget() {
        let mut state = state(Some(512 * MIB));
        let budget = Some(1536 * MIB);
        let admitted: Vec<_> = (0..5).map_while(|_| state.try_admit(budget)).collect();
        assert_eq!(admitted, vec![512 * MIB; 3]);
        assert_eq!(state.try_admit(budget), None);
    }

    #[test]
    fn one_worker_always_runs_even_past_the_budget() {
        let mut state = state(Some(512 * MIB));
        assert_eq!(state.try_admit(Some(MIB)), Some(512 * MIB));
        assert_eq!(state.try_admit(Some(MIB)), None);
    }

    #[test]
    fn the_largest_measured_peak_replaces_the_prior() {
        let mut state = state(None);
        let reservation = state.try_admit(Some(4096 * MIB)).unwrap();
        assert_eq!(reservation, PRIOR_WORKER_PEAK_BYTES);
        state.release(reservation, Some(110 * MIB));
        assert_eq!(state.expected_peak_bytes(), 110 * MIB);
        let reservation = state.try_admit(Some(4096 * MIB)).unwrap();
        state.release(reservation, Some(2048 * MIB));
        let reservation = state.try_admit(Some(4096 * MIB)).unwrap();
        state.release(reservation, Some(300 * MIB));
        assert_eq!(state.expected_peak_bytes(), 2048 * MIB);
        assert_eq!((state.running, state.reserved_bytes), (0, 0));
        let budget = Some(4096 * MIB);
        assert!(state.try_admit(budget).is_some());
        assert!(state.try_admit(budget).is_some());
        assert_eq!(state.try_admit(budget), None);
    }

    #[test]
    fn without_a_budget_every_worker_is_admitted() {
        let mut state = state(None);
        assert!((0..64).all(|_| state.try_admit(None).is_some()));
    }
}
