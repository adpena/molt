#[cfg(molt_has_mpdec)]
#[path = "decimal_with_mpdec.rs"]
mod decimal_with_mpdec;

#[cfg(molt_has_mpdec)]
pub use decimal_with_mpdec::*;

#[cfg(not(molt_has_mpdec))]
#[path = "decimal_without_mpdec.rs"]
mod decimal_without_mpdec;

#[cfg(not(molt_has_mpdec))]
pub use decimal_without_mpdec::*;
