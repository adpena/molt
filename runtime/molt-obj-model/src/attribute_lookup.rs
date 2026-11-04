//! The shared attribute protocol, independent of value representation.

/// A selected value, ordinary absence, or a pending Python error. Found carries
/// the caller's owned representation: runtime bits or an owned native object.
/// Missing never requires an exception object; only a required Python boundary
/// materializes AttributeError. A callback's real failure remains Error.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub enum AttributeLookup<T = u64> {
    Found(T),
    Missing,
    Error,
}

impl<T> AttributeLookup<T> {
    /// Join a fallible namespace/descriptor computation at its lookup boundary.
    #[inline]
    pub fn from_result(result: Result<Option<T>, ()>) -> Self {
        match result {
            Ok(Some(value)) => Self::Found(value),
            Ok(None) => Self::Missing,
            Err(()) => Self::Error,
        }
    }

    /// Transfer into a fallible computation while retaining absence explicitly.
    #[inline]
    pub fn into_result(self) -> Result<Option<T>, ()> {
        match self {
            Self::Found(value) => Ok(Some(value)),
            Self::Missing => Ok(None),
            Self::Error => Err(()),
        }
    }
}
