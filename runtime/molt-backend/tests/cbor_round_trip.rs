#[cfg(feature = "cbor")]
mod cbor_tests {
    use molt_backend::{FunctionIR, OpIR, SimpleIR};

    #[test]
    fn cbor_round_trip_minimal_ir() {
        let mut c0 = OpIR {
            kind: "const".to_string(),
            value: Some(42),
            out: Some("v0".to_string()),
            ..OpIR::default()
        };

        let mut ret = OpIR {
            kind: "ret".to_string(),
            args: Some(vec!["v0".to_string()]),
            ..OpIR::default()
        };

        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "test_fn".to_string(),
                params: vec!["x".to_string()],
                ops: vec![c0, ret],
                param_types: Some(vec!["int".to_string()]),
            }],
            profile: None,
        };

        // Serialize to CBOR bytes
        let mut cbor_bytes = Vec::new();
        ciborium::ser::into_writer(&ir, &mut cbor_bytes).expect("CBOR serialize failed");
        assert!(!cbor_bytes.is_empty());

        // Deserialize back
        let ir2: SimpleIR =
            ciborium::de::from_reader(&cbor_bytes[..]).expect("CBOR deserialize failed");

        // Verify structural equality
        assert_eq!(ir2.functions.len(), 1);
        let f = &ir2.functions[0];
        assert_eq!(f.name, "test_fn");
        assert_eq!(f.params, vec!["x"]);
        assert_eq!(f.ops.len(), 2);
        assert_eq!(f.ops[0].kind, "const");
        assert_eq!(f.ops[0].value, Some(42));
        assert_eq!(f.ops[0].out.as_deref(), Some("v0"));
        assert_eq!(f.ops[1].kind, "ret");
        assert_eq!(f.ops[1].args.as_deref(), Some(&["v0".to_string()][..]));
        assert_eq!(f.param_types.as_deref(), Some(&["int".to_string()][..]));
        assert!(ir2.profile.is_none());
    }
}
