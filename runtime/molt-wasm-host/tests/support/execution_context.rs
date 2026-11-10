//! Render canonical symbol/fingerprint spellings only. Fixtures themselves
//! declare their complete physical signatures and supply every context argument.
use molt_ir::runtime_callable_abi_generated::{
    ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT, ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX,
};

pub fn render(source: impl AsRef<str>) -> String {
    source
        .as_ref()
        .replace("@CONTEXT_SUFFIX@", ATTACHED_EXECUTION_CONTEXT_ABI_SUFFIX)
        .replace(
            "@CONTEXT_FINGERPRINT@",
            &ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT.to_string(),
        )
}

pub fn application_bytes(source: impl AsRef<str>) -> Vec<u8> {
    let mut bytes = wat::parse_str(render(source)).expect("valid context fixture WAT");
    let name = molt_ir::runtime_callable_abi_generated::ATTACHED_EXECUTION_CONTEXT_ABI_SECTION;
    let mut section = Vec::new();
    leb(name.len() as u64, &mut section);
    section.extend_from_slice(name.as_bytes());
    section.extend_from_slice(&ATTACHED_EXECUTION_CONTEXT_ABI_FINGERPRINT.to_le_bytes());
    bytes.push(0);
    leb(section.len() as u64, &mut bytes);
    bytes.extend(section);
    bytes
}

fn leb(mut value: u64, bytes: &mut Vec<u8>) {
    loop {
        let byte = value as u8 & 0x7f;
        value >>= 7;
        bytes.push(byte | if value == 0 { 0 } else { 0x80 });
        if value == 0 {
            break;
        }
    }
}
