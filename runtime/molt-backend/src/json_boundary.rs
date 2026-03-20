#![allow(dead_code)]

use serde_json::Value as JsonValue;

pub(crate) type JsonObject = serde_json::Map<String, JsonValue>;

pub(crate) fn expect_object<'a>(value: &'a JsonValue, ctx: &str) -> Result<&'a JsonObject, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{ctx} must be a JSON object"))
}

pub(crate) fn required_field<'a>(
    obj: &'a JsonObject,
    key: &str,
    ctx: &str,
) -> Result<&'a JsonValue, String> {
    obj.get(key)
        .ok_or_else(|| format!("{ctx}.{key} is required"))
}

pub(crate) fn required_string(obj: &JsonObject, key: &str, ctx: &str) -> Result<String, String> {
    required_field(obj, key, ctx)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{ctx}.{key} must be a string"))
}

pub(crate) fn optional_string(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| format!("{ctx}.{key} must be a string")),
    }
}

pub(crate) fn optional_bool(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Option<bool>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| format!("{ctx}.{key} must be a bool")),
    }
}

pub(crate) fn optional_i64(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<i64>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{ctx}.{key} must be an integer")),
    }
}

pub(crate) fn optional_f64(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<f64>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{ctx}.{key} must be a number")),
    }
}

pub(crate) fn optional_u32(obj: &JsonObject, key: &str, ctx: &str) -> Result<Option<u32>, String> {
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

pub(crate) fn required_string_list(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Vec<String>, String> {
    parse_string_list(required_field(obj, key, ctx)?, &format!("{ctx}.{key}"))
}

pub(crate) fn optional_string_list(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Option<Vec<String>>, String> {
    match obj.get(key) {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => parse_string_list(value, &format!("{ctx}.{key}")).map(Some),
    }
}

pub(crate) fn optional_bytes(
    obj: &JsonObject,
    key: &str,
    ctx: &str,
) -> Result<Option<Vec<u8>>, String> {
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
