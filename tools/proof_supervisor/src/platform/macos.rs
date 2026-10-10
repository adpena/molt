//! macOS process-closure admission is refused by this binary.
//!
//! The prior Seatbelt/ptrace leaf design could miss an image replacement when
//! the subject blocked SIGTRAP. It cannot supply the required pre-entry image
//! authority. Tree creation also lacks a retained pre-entry authority. Keep the
//! refusal explicit; no rejected execution implementation remains reachable.

use crate::{Admission, CAPABILITY_SCHEMA, Capability, ClosureMode};

const PLATFORM: &str = "macos";
const BACKEND: &str = "seatbelt+ptrace";
const LEAF_REFUSAL: &str = "macOS leaf closure has no valid pre-entry image authority: \
    blocked SIGTRAP can bypass the former Seatbelt/ptrace reexec observation";
const TREE_REFUSAL: &str = "macOS tree closure has no retained pre-entry descendant \
    creation authority in this binary; the former Seatbelt/ptrace leaf backend \
    also cannot enforce pre-entry image custody";

pub fn capability(mode: ClosureMode) -> Capability {
    Capability {
        schema: CAPABILITY_SCHEMA.to_owned(),
        platform: PLATFORM.to_owned(),
        mode,
        backend: BACKEND.to_owned(),
        admission: Admission::Ineligible {
            reason: match mode {
                ClosureMode::Leaf => LEAF_REFUSAL,
                ClosureMode::DeclaredTree | ClosureMode::InventoryTree => TREE_REFUSAL,
            }
            .to_owned(),
        },
        pre_entry_exec_authority: false,
        pre_entry_process_create_authority: false,
        recursive_descendant_authority: false,
        required_environment: super::required_environment(),
    }
}

pub fn capability_contract_is_valid(recorded: &Capability, mode: ClosureMode) -> bool {
    super::planned_contract_is_valid(recorded, &capability(mode))
}
