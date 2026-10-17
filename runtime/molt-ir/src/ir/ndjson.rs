use std::io::BufRead;

use serde_json::Value;

#[derive(Clone, Copy)]
enum FrameState {
    Start,
    Body,
    End,
}

/// Admit one complete envelope without conflating function and program validation.
pub(super) fn read_frame<R: BufRead>(
    reader: R,
    mut record: impl FnMut(&Value) -> Result<(), String>,
) -> Result<(), String> {
    let mut state = FrameState::Start;
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|err| format!("NDJSON read error: {err}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let fail = |message| format!("NDJSON frame error at line {}: {message}", index + 1);
        if matches!(state, FrameState::End) {
            return Err(fail("nonblank input after ir_stream_end"));
        }
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|err| format!("NDJSON parse error at line {}: {err}", index + 1))?;
        let kind = value
            .as_object()
            .and_then(|object| object.get("kind"))
            .and_then(Value::as_str)
            .ok_or_else(|| fail("record must be an object with a string kind"))?;
        match state {
            FrameState::Start => {
                if kind != "ir_stream_start" {
                    return Err(fail("expected ir_stream_start"));
                }
                state = FrameState::Body;
                record(&value)?;
            }
            FrameState::Body => match kind {
                "ir_stream_start" => return Err(fail("repeated ir_stream_start")),
                "ir_stream_end" => state = FrameState::End,
                _ => record(&value)?,
            },
            FrameState::End => unreachable!("end state rejected before decoding"),
        }
    }
    match state {
        FrameState::Start => Err("NDJSON frame error: missing ir_stream_start".into()),
        FrameState::Body => Err("NDJSON frame error: missing ir_stream_end".into()),
        FrameState::End => Ok(()),
    }
}
