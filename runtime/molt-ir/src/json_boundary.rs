#![allow(dead_code)]

use serde_json::Value as JsonValue;

pub type JsonObject = serde_json::Map<String, JsonValue>;

pub fn expect_object<'a>(value: &'a JsonValue, ctx: &str) -> Result<&'a JsonObject, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{ctx} must be a JSON object"))
}

pub fn required_field<'a>(
    obj: &'a JsonObject,
    key: &str,
    ctx: &str,
) -> Result<&'a JsonValue, String> {
    obj.get(key)
        .ok_or_else(|| format!("{ctx}.{key} is required"))
}

pub fn required_string(obj: &JsonObject, key: &str, ctx: &str) -> Result<String, String> {
    required_field(obj, key, ctx)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{ctx}.{key} must be a string"))
}

pub fn optional_string(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| format!("{ctx}.{key} must be a string")),
    }
}

pub fn optional_bool(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{ctx}.{key} must be a bool")),
    }
}

pub fn optional_i64(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{ctx}.{key} must be an integer")),
    }
}

pub fn optional_f64(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<f64>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => {
            if let Some(n) = value.as_f64() {
                return Ok(Some(n));
            }
            // Accept string representations of non-finite floats
            if let Some(s) = value.as_str() {
                return match s {
                    "Infinity" => Ok(Some(f64::INFINITY)),
                    "-Infinity" => Ok(Some(f64::NEG_INFINITY)),
                    "NaN" => Ok(Some(f64::NAN)),
                    _ => Err(format!(
                        "{ctx}.{key} must be a number or special float string"
                    )),
                };
            }
            Err(format!("{ctx}.{key} must be a number"))
        }
    }
}

pub fn optional_u32(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<u32>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => {
            let Some(raw) = value.as_u64() else {
                return Err(format!("{ctx}.{key} must be a non-negative integer"));
            };
            u32::try_from(raw)
                .map(Some)
                .map_err(|_| format!("{ctx}.{key} is out of range for u32"))
        }
    }
}

pub fn required_string_list(obj: &JsonObject, key: &str, ctx: &str) -> Result<Vec<String>, String> {
    parse_string_list(required_field(obj, key, ctx)?, &format!("{ctx}.{key}"))
}

pub fn optional_string_list(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Option<Vec<String>>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => parse_string_list(value, &format!("{ctx}.{key}")).map(Some),
    }
}

pub fn optional_bytes(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(value) = obj.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let JsonValue::Array(items) = value else {
        return Err(format!("{ctx}.{key} must be an array of bytes"));
    };
    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let Some(raw) = item.as_u64() else {
            return Err(format!("{ctx}.{key}[{idx}] must be an unsigned byte"));
        };
        let byte = u8::try_from(raw)
            .map_err(|_| format!("{ctx}.{key}[{idx}] is out of range for a byte"))?;
        out.push(byte);
    }
    Ok(Some(out))
}

fn parse_string_list(value: &JsonValue, ctx: &str) -> Result<Vec<String>, String> {
    let JsonValue::Array(items) = value else {
        return Err(format!("{ctx} must be an array of strings"));
    };
    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(format!("{ctx}[{idx}] must be a string"));
        };
        out.push(text.to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod float_roundtrip_regression {
    use crate::json_boundary::optional_f64;
    use serde_json::Value as JsonValue;

    const HARD_FLOAT_CASES: &[(&str, u64)] = &[
        ("0.9999999999999999", 0x3fef_ffff_ffff_ffff),
        ("123456789012345.67", 0x42dc_1221_8377_de6b),
        ("2.2250738585072011e-308", 0x000f_ffff_ffff_ffff),
        ("0.1", 0x3fb9_9999_9999_999a),
    ];

    #[test]
    fn serde_json_from_str_parses_hard_float_literals_exactly() {
        for &(text, want_bits) in HARD_FLOAT_CASES {
            let parsed: f64 = serde_json::from_str(text).expect("parse f64 literal");
            assert_eq!(
                parsed.to_bits(),
                want_bits,
                "serde_json::from_str({text}) = {parsed:?} bits=0x{:016x}, want 0x{want_bits:016x}",
                parsed.to_bits()
            );
        }
    }

    #[test]
    fn optional_f64_reads_const_float_f_value_exactly() {
        for &(text, want_bits) in HARD_FLOAT_CASES {
            let obj_text = format!("{{\"kind\":\"const_float\",\"f_value\":{text}}}");
            let value: JsonValue =
                serde_json::from_str(&obj_text).expect("parse const_float object");
            let obj = value.as_object().expect("object");
            let f_value = optional_f64(obj, "f_value", "test")
                .expect("optional_f64 ok")
                .expect("f_value present");
            assert_eq!(
                f_value.to_bits(),
                want_bits,
                "optional_f64(f_value:{text}) = {f_value:?} bits=0x{:016x}, want 0x{want_bits:016x}",
                f_value.to_bits()
            );
        }
    }
}
