//! Typed kind of a task created from the IR constructor metadata.

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TaskKind {
    Future,
    Generator,
    Coroutine,
    AsyncGenerator,
}

impl TaskKind {
    pub const fn parse(kind: &str) -> Option<Self> {
        match kind.as_bytes() {
            b"future" => Some(Self::Future),
            b"generator" => Some(Self::Generator),
            b"coroutine" => Some(Self::Coroutine),
            b"async_generator" => Some(Self::AsyncGenerator),
            _ => None,
        }
    }

    /// A Python function definition can construct a suspended body. A
    /// runtime-native future has no Python callable trampoline.
    pub const fn callable(self) -> bool {
        !matches!(self, Self::Future)
    }
}
