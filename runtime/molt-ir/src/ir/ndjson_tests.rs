use super::{BackendIrDocument, SimpleIR};

const START: &str = r#"{"kind":"ir_stream_start"}"#;
const END: &str = r#"{"kind":"ir_stream_end"}"#;

#[test]
fn both_ndjson_consumers_require_one_complete_envelope() {
    let cases = [
        (String::new(), "missing ir_stream_start"),
        (format!("{END}\n"), "expected ir_stream_start"),
        (format!("{START}\n"), "missing ir_stream_end"),
        (
            format!("{START}\n{START}\n{END}"),
            "repeated ir_stream_start",
        ),
        (format!("{START}\n{END}\n{END}"), "after ir_stream_end"),
        (
            format!("{START}\n{END}\n{START}\n{END}"),
            "after ir_stream_end",
        ),
        (
            format!("{START}\n{END}\n{{\"kind\":\"future\"}}"),
            "after ir_stream_end",
        ),
        (
            format!("{{\"kind\":\"future\"}}\n{START}\n{END}"),
            "expected ir_stream_start",
        ),
        (
            format!("{{\"kind\":\"function\"}}\n{START}\n{END}"),
            "expected ir_stream_start",
        ),
        (
            format!("{START}\n{END}\n{{\"kind\":\"function\"}}"),
            "after ir_stream_end",
        ),
        (format!("{START}\n{{}}\n{END}"), "string kind"),
        (format!("{START}\n{{\"kind\":null}}\n{END}"), "string kind"),
        (format!("{START}\n[]\n{END}"), "string kind"),
        (format!("{START}\n{END}\nnot-json"), "after ir_stream_end"),
    ];
    for (input, expected) in cases {
        for error in [
            SimpleIR::from_ndjson_reader(input.as_bytes()).unwrap_err(),
            BackendIrDocument::from_ndjson_reader(input.as_bytes()).unwrap_err(),
        ] {
            assert!(error.contains(expected), "{input:?}: {error}");
        }
    }
}

#[test]
fn both_ndjson_consumers_allow_blank_lines_unknown_body_and_no_final_newline() {
    for input in [
        format!("{START}\n{END}"),
        format!("\n \t\r\n{START}\n{{\"kind\":\"future\",\"data\":[1,2]}}\n\n{END}\n \t\r\n"),
    ] {
        assert!(
            SimpleIR::from_ndjson_reader(input.as_bytes())
                .unwrap()
                .functions
                .is_empty()
        );
        assert!(
            BackendIrDocument::from_ndjson_reader(input.as_bytes())
                .unwrap()
                .ir
                .functions
                .is_empty()
        );
    }
}

#[test]
fn end_marker_does_not_mask_a_following_read_failure() {
    struct FailsAtEof(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for FailsAtEof {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = std::io::Read::read(&mut self.0, buffer)?;
            if count == 0 {
                Err(std::io::Error::other("independent suffix read failure"))
            } else {
                Ok(count)
            }
        }
    }
    let input = format!("{START}\n{END}\n").into_bytes();
    for error in [
        SimpleIR::from_ndjson_reader(std::io::BufReader::new(FailsAtEof(std::io::Cursor::new(
            input.clone(),
        ))))
        .unwrap_err(),
        BackendIrDocument::from_ndjson_reader(std::io::BufReader::new(FailsAtEof(
            std::io::Cursor::new(input),
        )))
        .unwrap_err(),
    ] {
        assert!(error.contains("independent suffix read failure"), "{error}");
    }
}
