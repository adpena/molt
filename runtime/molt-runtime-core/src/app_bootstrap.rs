//! Application-owned initialization for a fresh native isolate.
//!
//! Compiled applications provide `molt_isolate_bootstrap` in their generated
//! object. Direct-link hosts and test executables must instead declare their
//! final image's provider explicitly. This crate never exports that symbol as
//! a dependency, including under Cargo feature unification or fuzzing builds.

/// The final image's application initializer, or its explicit absence.
#[derive(Clone, Copy)]
pub enum AppBootstrapProvider {
    /// Initialize application modules in the current isolate and return its
    /// owned Molt object result (including None on failure). Runtime
    /// initialization and GIL custody precede this
    /// callback; it must not unwind through the C ABI.
    Initializer(unsafe extern "C" fn() -> u64),
    /// This image has no compiled application to initialize. The label names
    /// the owning host or harness in the fatal diagnostic.
    Unavailable(&'static str),
}

impl AppBootstrapProvider {
    /// Invoke this image's initializer, failing closed if it has none.
    ///
    /// # Safety
    /// The caller must satisfy the selected initializer's safety contract.
    /// Application initializers require the initialized isolate's GIL and any
    /// additional runtime preconditions imposed by the application.
    pub unsafe fn invoke(self) -> u64 {
        match self {
            Self::Initializer(initialize) => unsafe { initialize() },
            Self::Unavailable(image) => unavailable(image),
        }
    }
}

#[cold]
fn unavailable(image: &str) -> ! {
    use std::io::Write;

    // A failed stderr write must not unwind or turn absence into success.
    let _ = writeln!(
        std::io::stderr().lock(),
        "MOLT_APP_BOOTSTRAP_UNAVAILABLE: {image}: application initializer absent; molt_isolate_bootstrap cannot initialize a native isolate"
    );
    std::process::abort()
}

/// Declare the native isolate-bootstrap symbol in its owning final image.
///
/// Invoke exactly once in a direct-link host or test executable, never in a
/// dependency or alongside a compiler-emitted application initializer. WASM
/// hosts retain their per-Store application-export registration instead.
#[macro_export]
macro_rules! declare_app_bootstrap {
    ($provider:expr) => {
        #[cfg(not(target_arch = "wasm32"))]
        #[unsafe(no_mangle)]
        /// Initialize the final image's application in the current isolate.
        ///
        /// # Safety
        /// The caller must satisfy the declared initializer's safety contract,
        /// including initialized-isolate GIL custody for application code.
        pub unsafe extern "C" fn molt_isolate_bootstrap() -> u64 {
            const PROVIDER: $crate::app_bootstrap::AppBootstrapProvider = $provider;
            unsafe { PROVIDER.invoke() }
        }
    };
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::AppBootstrapProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static INITIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn initialize_application() -> u64 {
        INITIALIZATIONS.fetch_add(1, Ordering::SeqCst);
        0x1234_5678_9abc_def0
    }

    crate::declare_app_bootstrap!(AppBootstrapProvider::Initializer(initialize_application));

    #[test]
    fn final_image_forwards_initializer_once_and_preserves_result() {
        let before = INITIALIZATIONS.load(Ordering::SeqCst);
        // This test initializer only updates an atomic and needs no runtime.
        let result = unsafe { molt_isolate_bootstrap() };
        assert_eq!(result, 0x1234_5678_9abc_def0);
        assert_eq!(INITIALIZATIONS.load(Ordering::SeqCst), before + 1);
    }

    #[test]
    fn unavailable_initializer_aborts_with_owner_diagnostic() {
        const CHILD: &str = "MOLT_TEST_APP_BOOTSTRAP_UNAVAILABLE_CHILD";
        if std::env::var_os(CHILD).is_some() {
            unsafe { AppBootstrapProvider::Unavailable("bootstrap-contract-test").invoke() };
            panic!("an unavailable application initializer returned");
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app_bootstrap::tests::unavailable_initializer_aborts_with_owner_diagnostic",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("run unavailable-bootstrap child");
        assert!(!output.status.success(), "unavailable bootstrap succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(
                "MOLT_APP_BOOTSTRAP_UNAVAILABLE: bootstrap-contract-test: application initializer absent"
            ),
            "missing fail-closed owner diagnostic: {stderr}"
        );
        assert!(!stderr.contains("an unavailable application initializer returned"));
    }
}
