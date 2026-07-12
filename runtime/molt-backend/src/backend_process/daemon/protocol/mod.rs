#[cfg(any(unix, test))]
mod request;
#[cfg(any(unix, test))]
mod response;

#[cfg(any(unix, test))]
pub(crate) use request::{DaemonJobRequest, DaemonRequest};
#[cfg(any(unix, test))]
pub(crate) use response::{DaemonHealthResponse, DaemonJobResponse, DaemonResponse, is_false};
