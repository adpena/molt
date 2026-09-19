//! Float immediate transport: binary preserves every bit; JSON uses explicit
//! non-finite tokens rather than silently replacing legal constants with null.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

pub(super) fn serialize<S>(value: &Option<f64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if serializer.is_human_readable()
        && let Some(value) = value
        && !value.is_finite()
    {
        let token = if value.is_nan() {
            "NaN"
        } else if value.is_sign_negative() {
            "-Infinity"
        } else {
            "Infinity"
        };
        return serializer.serialize_some(token);
    }
    value.serialize(serializer)
}

pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct Float(f64);

    impl<'de> Deserialize<'de> for Float {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct Visitor;

            impl de::Visitor<'_> for Visitor {
                type Value = Float;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a number or NaN, Infinity, -Infinity float token")
                }

                fn visit_f64<E: de::Error>(self, value: f64) -> Result<Float, E> {
                    Ok(Float(value))
                }

                fn visit_i64<E: de::Error>(self, value: i64) -> Result<Float, E> {
                    Ok(Float(value as f64))
                }

                fn visit_u64<E: de::Error>(self, value: u64) -> Result<Float, E> {
                    Ok(Float(value as f64))
                }

                fn visit_str<E: de::Error>(self, value: &str) -> Result<Float, E> {
                    match value {
                        "NaN" => Ok(Float(f64::NAN)),
                        "Infinity" => Ok(Float(f64::INFINITY)),
                        "-Infinity" => Ok(Float(f64::NEG_INFINITY)),
                        _ => Err(E::invalid_value(de::Unexpected::Str(value), &self)),
                    }
                }
            }

            deserializer.deserialize_any(Visitor)
        }
    }

    Option::<Float>::deserialize(deserializer).map(|value| value.map(|value| value.0))
}

#[cfg(test)]
mod tests {
    use crate::{BackendIrDocument, FunctionIR, OpIR, SimpleIR};

    #[test]
    fn json_roundtrips_legal_floats_through_all_document_boundaries() {
        for value in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            0.0,
            f64::from_bits(1),
            f64::MAX,
            0.9999999999999999,
            123456789012345.67,
            2.2250738585072011e-308,
        ] {
            let ir = SimpleIR {
                functions: vec![FunctionIR {
                    name: "float_wire".into(),
                    ops: vec![OpIR {
                        kind: "const_float".into(),
                        f_value: Some(value),
                        out: Some("result".into()),
                        ..OpIR::default()
                    }],
                    ..FunctionIR::default()
                }],
                profile: None,
            };
            let json = serde_json::to_string(&ir).unwrap();
            let manual = BackendIrDocument::from_json_str(&json).unwrap();
            let typed: BackendIrDocument = serde_json::from_str(&json).unwrap();
            let mut function = serde_json::to_value(&ir.functions[0]).unwrap();
            function["kind"] = serde_json::json!("function");
            let ndjson = format!("{function}\n");
            let streamed = BackendIrDocument::from_ndjson_reader(ndjson.as_bytes()).unwrap();
            for document in [manual, typed, streamed] {
                let actual = document.ir.functions[0].ops[0].f_value.unwrap();
                if value.is_nan() {
                    assert!(actual.is_nan()); // JSON names NaN, not its binary payload.
                } else {
                    assert_eq!(actual.to_bits(), value.to_bits(), "{json}");
                }
            }
        }
    }

    #[test]
    fn float_field_accepts_only_numbers_special_tokens_or_absence() {
        for (token, expected) in [("42", 42.0_f64), ("-42", -42.0_f64)] {
            let op: OpIR =
                serde_json::from_str(&format!(r#"{{"kind":"const_float","f_value":{token}}}"#))
                    .unwrap();
            assert_eq!(op.f_value.unwrap().to_bits(), expected.to_bits());
        }
        for source in [
            r#"{"kind":"const_float"}"#,
            r#"{"kind":"const_float","f_value":null}"#,
        ] {
            assert!(
                serde_json::from_str::<OpIR>(source)
                    .unwrap()
                    .f_value
                    .is_none()
            );
        }
        for token in ["true", "[]", "{}", r#""1.25""#, r#""nan""#, r#""inf""#] {
            let source = format!(r#"{{"kind":"const_float","f_value":{token}}}"#);
            assert!(serde_json::from_str::<OpIR>(&source).is_err(), "{source}");
        }
    }
}
