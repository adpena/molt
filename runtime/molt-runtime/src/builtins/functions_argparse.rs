// Argparse stdlib implementation.
// Extracted from functions.rs for tree shaking.

use crate::*;
use molt_obj_model::MoltObject;
use super::functions::*;


pub(crate) fn argparse_choice_list(parsers: &HashMap<String, ArgparseSpec>) -> String {
    let mut keys: Vec<&str> = parsers.keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys.join(", ")
}


pub(crate) fn argparse_decode_spec(value: &JsonValue) -> Result<ArgparseSpec, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "argparse spec must be a JSON object".to_string())?;

    let mut optionals: Vec<ArgparseOptionalSpec> = Vec::new();
    if let Some(raw_optionals) = obj.get("optionals") {
        let items = raw_optionals
            .as_array()
            .ok_or_else(|| "argparse optionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse optional spec must be object".to_string())?;
            let flag = item_obj
                .get("flag")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string flag".to_string())?
                .to_string();
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string dest".to_string())?
                .to_string();
            let kind = item_obj
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse optional spec missing string kind".to_string())?;
            let parsed_kind = match kind {
                "value" => ArgparseOptionalKind::Value,
                "store_true" => ArgparseOptionalKind::StoreTrue,
                _ => return Err(format!("unsupported argparse optional kind: {kind}")),
            };
            let required = item_obj
                .get("required")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            let default = item_obj.get("default").cloned().unwrap_or_else(|| {
                if parsed_kind == ArgparseOptionalKind::StoreTrue {
                    JsonValue::Bool(false)
                } else {
                    JsonValue::Null
                }
            });
            optionals.push(ArgparseOptionalSpec {
                flag,
                dest,
                kind: parsed_kind,
                required,
                default,
            });
        }
    }

    let mut positionals: Vec<String> = Vec::new();
    if let Some(raw_positionals) = obj.get("positionals") {
        let items = raw_positionals
            .as_array()
            .ok_or_else(|| "argparse positionals must be a JSON array".to_string())?;
        for item in items {
            let item_obj = item
                .as_object()
                .ok_or_else(|| "argparse positional spec must be object".to_string())?;
            let dest = item_obj
                .get("dest")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| "argparse positional spec missing string dest".to_string())?
                .to_string();
            positionals.push(dest);
        }
    }

    let subparsers = if let Some(raw_subparsers) = obj.get("subparsers") {
        let sp_obj = raw_subparsers
            .as_object()
            .ok_or_else(|| "argparse subparsers spec must be object".to_string())?;
        let dest = sp_obj
            .get("dest")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| "argparse subparsers spec missing string dest".to_string())?
            .to_string();
        let required = sp_obj
            .get("required")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let parsers_obj = sp_obj
            .get("parsers")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| "argparse subparsers spec missing parsers object".to_string())?;
        let mut parsers: HashMap<String, ArgparseSpec> = HashMap::new();
        for (name, parser_spec) in parsers_obj {
            let parsed = argparse_decode_spec(parser_spec)?;
            parsers.insert(name.clone(), parsed);
        }
        Some(ArgparseSubparsersSpec {
            dest,
            required,
            parsers,
        })
    } else {
        None
    };

    Ok(ArgparseSpec {
        optionals,
        positionals,
        subparsers,
    })
}


pub(crate) fn argparse_parse_with_spec(
    spec: &ArgparseSpec,


#[unsafe(no_mangle)]
pub extern "C" fn molt_argparse_parse(spec_json_bits: u64, argv_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(spec_json) = string_obj_to_owned(obj_from_bits(spec_json_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "argparse spec_json must be str");
        };
        let argv = match iterable_to_string_vec(_py, argv_bits) {
            Ok(values) => values,
            Err(bits) => return bits,
        };

        let spec_value: JsonValue = match serde_json::from_str(spec_json.as_str()) {
            Ok(value) => value,
            Err(err) => {
                let msg = format!("invalid argparse spec json: {err}");
                return raise_exception::<_>(_py, "ValueError", msg.as_str());
            }
        };
        let spec = match argparse_decode_spec(&spec_value) {
            Ok(spec) => spec,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let parsed = match argparse_parse_with_spec(&spec, argv.as_slice()) {
            Ok(parsed) => parsed,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", msg.as_str()),
        };
        let payload = match serde_json::to_string(&JsonValue::Object(parsed)) {
            Ok(payload) => payload,
            Err(err) => {
                let msg = format!("argparse payload encode failed: {err}");
                return raise_exception::<_>(_py, "RuntimeError", msg.as_str());
            }
        };
        let out_ptr = alloc_string(_py, payload.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

