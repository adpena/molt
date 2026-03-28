//! Verify that thread-local resource tracker references cannot escape the
//! `with_tracker` closure. This prevents use-after-free when a tracker is
//! replaced via `set_tracker`.

fn main() {
    let mut escaped: Option<&mut dyn molt_runtime::resource::ResourceTracker> = None;

    molt_runtime::resource::with_tracker(|t| {
        // ERROR: `t` does not live long enough — the borrow is scoped to the
        // closure body and cannot be stored in `escaped` which outlives it.
        escaped = Some(t);
    });

    // If this compiled, `escaped` could dangle after set_tracker() replaces
    // the thread-local tracker.
    let _ = escaped;
}
