//! Canonical native callable ABI tokens shared by SimpleIR validation and backends.

pub const NATIVE_CALLABLE_ABI_OBJECT_CALL_V1: &str = "molt.object_call_v1";
pub const NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1: &str = "molt.object_callargs_v1";
pub const NATIVE_CALLABLE_ABI_FORWARD_F32_V1: &str = "molt.forward_f32_v1";
pub const NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1: &str = "molt.pyinit_module_v1";

pub const NATIVE_CALLABLE_ABIS: &[&str] = &[
    NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
    NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
    NATIVE_CALLABLE_ABI_FORWARD_F32_V1,
    NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
];
pub const NATIVE_CALLABLE_ABI_CHOICES: &str =
    "molt.object_call_v1, molt.object_callargs_v1, molt.forward_f32_v1, molt.pyinit_module_v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCallableAbi {
    ObjectCallV1,
    ObjectCallargsV1,
    ForwardF32V1,
    PyinitModuleV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCallableBrowserSignature {
    pub params: &'static [&'static str],
    pub result: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeCallableWasmSignature {
    pub params: &'static [&'static str],
    pub results: &'static [&'static str],
}

const OBJECT_CALL_BROWSER_PARAMS: &[&str] = &["molt.value..."];
const OBJECT_CALLARGS_BROWSER_PARAMS: &[&str] = &["molt.callargs"];
const FORWARD_F32_BROWSER_PARAMS: &[&str] = &["bytes.float32"];
const PYINIT_MODULE_BROWSER_PARAMS: &[&str] = &[];
const OBJECT_CALL_WASM_PARAMS: &[&str] = &["i64..."];
const OBJECT_CALLARGS_WASM_PARAMS: &[&str] = &["i64"];
const PYINIT_MODULE_WASM_PARAMS: &[&str] = &[];
const OBJECT_CALL_WASM_RESULTS: &[&str] = &["i64"];
const FORWARD_F32_WASM_PARAMS: &[&str] = &["i32", "i64", "i32"];
const FORWARD_F32_WASM_RESULTS: &[&str] = &["i32"];

impl NativeCallableAbi {
    pub fn token(self) -> &'static str {
        match self {
            Self::ObjectCallV1 => NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
            Self::ObjectCallargsV1 => NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1,
            Self::ForwardF32V1 => NATIVE_CALLABLE_ABI_FORWARD_F32_V1,
            Self::PyinitModuleV1 => NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
        }
    }

    pub fn fixed_arity(self) -> Option<usize> {
        match self {
            Self::ObjectCallV1 => None,
            Self::ObjectCallargsV1 => Some(1),
            Self::ForwardF32V1 => Some(1),
            Self::PyinitModuleV1 => Some(0),
        }
    }

    pub fn browser_signature(self) -> NativeCallableBrowserSignature {
        match self {
            Self::ObjectCallV1 => NativeCallableBrowserSignature {
                params: OBJECT_CALL_BROWSER_PARAMS,
                result: "molt.value",
            },
            Self::ObjectCallargsV1 => NativeCallableBrowserSignature {
                params: OBJECT_CALLARGS_BROWSER_PARAMS,
                result: "molt.value",
            },
            Self::ForwardF32V1 => NativeCallableBrowserSignature {
                params: FORWARD_F32_BROWSER_PARAMS,
                result: "bytes.float32",
            },
            Self::PyinitModuleV1 => NativeCallableBrowserSignature {
                params: PYINIT_MODULE_BROWSER_PARAMS,
                result: "molt.pyobject_ptr",
            },
        }
    }

    pub fn wasm_signature(self) -> NativeCallableWasmSignature {
        match self {
            Self::ObjectCallV1 => NativeCallableWasmSignature {
                params: OBJECT_CALL_WASM_PARAMS,
                results: OBJECT_CALL_WASM_RESULTS,
            },
            Self::ObjectCallargsV1 => NativeCallableWasmSignature {
                params: OBJECT_CALLARGS_WASM_PARAMS,
                results: OBJECT_CALL_WASM_RESULTS,
            },
            Self::ForwardF32V1 => NativeCallableWasmSignature {
                params: FORWARD_F32_WASM_PARAMS,
                results: FORWARD_F32_WASM_RESULTS,
            },
            Self::PyinitModuleV1 => NativeCallableWasmSignature {
                params: PYINIT_MODULE_WASM_PARAMS,
                results: OBJECT_CALL_WASM_RESULTS,
            },
        }
    }
}

pub fn parse_native_callable_abi(abi: &str) -> Option<NativeCallableAbi> {
    match abi {
        NATIVE_CALLABLE_ABI_OBJECT_CALL_V1 => Some(NativeCallableAbi::ObjectCallV1),
        NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1 => Some(NativeCallableAbi::ObjectCallargsV1),
        NATIVE_CALLABLE_ABI_FORWARD_F32_V1 => Some(NativeCallableAbi::ForwardF32V1),
        NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1 => Some(NativeCallableAbi::PyinitModuleV1),
        _ => None,
    }
}

pub fn is_known_native_callable_abi(abi: &str) -> bool {
    parse_native_callable_abi(abi).is_some()
}

#[cfg(test)]
mod tests {
    use super::{
        NATIVE_CALLABLE_ABI_FORWARD_F32_V1, NATIVE_CALLABLE_ABI_OBJECT_CALL_V1,
        NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1, NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1,
        NativeCallableAbi, parse_native_callable_abi,
    };

    #[test]
    fn native_callable_abi_contracts_are_canonical() {
        let object_call = parse_native_callable_abi(NATIVE_CALLABLE_ABI_OBJECT_CALL_V1).unwrap();
        assert_eq!(object_call, NativeCallableAbi::ObjectCallV1);
        assert_eq!(object_call.token(), NATIVE_CALLABLE_ABI_OBJECT_CALL_V1);
        assert_eq!(object_call.fixed_arity(), None);
        assert_eq!(object_call.browser_signature().params, ["molt.value..."]);
        assert_eq!(object_call.browser_signature().result, "molt.value");
        assert_eq!(object_call.wasm_signature().params, ["i64..."]);
        assert_eq!(object_call.wasm_signature().results, ["i64"]);

        let object_callargs =
            parse_native_callable_abi(NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1).unwrap();
        assert_eq!(object_callargs, NativeCallableAbi::ObjectCallargsV1);
        assert_eq!(
            object_callargs.token(),
            NATIVE_CALLABLE_ABI_OBJECT_CALLARGS_V1
        );
        assert_eq!(object_callargs.fixed_arity(), Some(1));
        assert_eq!(
            object_callargs.browser_signature().params,
            ["molt.callargs"]
        );
        assert_eq!(object_callargs.browser_signature().result, "molt.value");
        assert_eq!(object_callargs.wasm_signature().params, ["i64"]);
        assert_eq!(object_callargs.wasm_signature().results, ["i64"]);

        let forward_f32 = parse_native_callable_abi(NATIVE_CALLABLE_ABI_FORWARD_F32_V1).unwrap();
        assert_eq!(forward_f32, NativeCallableAbi::ForwardF32V1);
        assert_eq!(forward_f32.token(), NATIVE_CALLABLE_ABI_FORWARD_F32_V1);
        assert_eq!(forward_f32.fixed_arity(), Some(1));
        assert_eq!(forward_f32.browser_signature().params, ["bytes.float32"]);
        assert_eq!(forward_f32.browser_signature().result, "bytes.float32");
        assert_eq!(forward_f32.wasm_signature().params, ["i32", "i64", "i32"]);
        assert_eq!(forward_f32.wasm_signature().results, ["i32"]);

        let pyinit_module =
            parse_native_callable_abi(NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1).unwrap();
        assert_eq!(pyinit_module, NativeCallableAbi::PyinitModuleV1);
        assert_eq!(pyinit_module.token(), NATIVE_CALLABLE_ABI_PYINIT_MODULE_V1);
        assert_eq!(pyinit_module.fixed_arity(), Some(0));
        assert!(pyinit_module.browser_signature().params.is_empty());
        assert_eq!(
            pyinit_module.browser_signature().result,
            "molt.pyobject_ptr"
        );
        assert!(pyinit_module.wasm_signature().params.is_empty());
        assert_eq!(pyinit_module.wasm_signature().results, ["i64"]);
    }
}
