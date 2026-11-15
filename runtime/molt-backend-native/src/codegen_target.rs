//! The machine one compilation generates code for, from its target triple.

/// `Host` emits a native object for the compiler's own machine. The wasm32
/// targets emit a relocatable WebAssembly object for wasm-ld, linked against
/// the runtime's relocatable module of the same triple: 32-bit pointers inside
/// the same 64-bit NaN-boxed words. Only LLVM generates WebAssembly.
/// `Wasm32Wasip1` is the WASI cell and `Wasm32Unknown` the freestanding one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenTarget {
    Host,
    Wasm32Wasip1,
    Wasm32Unknown,
}

impl CodegenTarget {
    /// The target a backend target triple selects. A `wasm32` triple is
    /// LLVM's wasm32 target; no triple, or any other, is the host, whose
    /// native target the object code owner admits.
    pub fn from_target_triple(triple: Option<&str>) -> Self {
        match triple.map(str::trim) {
            Some("wasm32-wasip1") => Self::Wasm32Wasip1,
            Some("wasm32-unknown-unknown") => Self::Wasm32Unknown,
            Some(other) if other.starts_with("wasm") => {
                panic!("no code generator targets WebAssembly triple {other:?}")
            }
            _ => Self::Host,
        }
    }

    pub const fn is_wasm(self) -> bool {
        matches!(self, Self::Wasm32Wasip1 | Self::Wasm32Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::CodegenTarget;

    #[test]
    fn the_target_triple_selects_the_code_generator_target() {
        assert_eq!(CodegenTarget::from_target_triple(None), CodegenTarget::Host);
        assert_eq!(
            CodegenTarget::from_target_triple(Some("aarch64-apple-darwin")),
            CodegenTarget::Host
        );
        assert_eq!(
            CodegenTarget::from_target_triple(Some("wasm32-wasip1")),
            CodegenTarget::Wasm32Wasip1
        );
        assert_eq!(
            CodegenTarget::from_target_triple(Some("wasm32-unknown-unknown")),
            CodegenTarget::Wasm32Unknown
        );
        assert!(CodegenTarget::Wasm32Wasip1.is_wasm());
        assert!(!CodegenTarget::Host.is_wasm());
    }

    #[test]
    #[should_panic(expected = "no code generator targets WebAssembly triple")]
    fn an_unknown_webassembly_triple_is_refused() {
        CodegenTarget::from_target_triple(Some("wasm64-unknown-unknown"));
    }
}
