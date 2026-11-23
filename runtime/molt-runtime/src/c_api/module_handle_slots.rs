//! Managed persistent references for first-party Rust extension modules.

use molt_runtime_core::{OwnedRuntimeValue, PyToken as CoreToken};

use super::{c_api_module_state, module_handle_ptr};
use crate::{PyToken, dec_ref_bits};

/// A slot layout or access cannot be admitted by the module's live state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModuleHandleSlotError {
    InvalidModule,
    NotRegistered,
    AlreadyDeclared,
    Unavailable,
    OutOfBounds,
    AllocationFailed,
}

/// One index in a Rust extension's declared module state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModuleHandleSlot(pub usize);

/// Declare the module's fixed persistent handle layout after C-API registration.
/// This layout belongs to the module instance, including across repeated execution.
pub fn declare_module_handle_slots(
    py: &CoreToken,
    module: &OwnedRuntimeValue<'_>,
    count: usize,
) -> Result<(), ModuleHandleSlotError> {
    let py = local_token(py);
    let key = module_key(module)?;
    let mut state = c_api_module_state(py);
    let metadata = state
        .metadata
        .get_mut(&key)
        .ok_or(ModuleHandleSlotError::NotRegistered)?;
    if metadata.free_started || metadata.clear_active {
        return Err(ModuleHandleSlotError::Unavailable);
    }
    if metadata.handle_slots.is_some() {
        return Err(ModuleHandleSlotError::AlreadyDeclared);
    }
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(count)
        .map_err(|_| ModuleHandleSlotError::AllocationFailed)?;
    slots.resize(count, None);
    metadata.handle_slots = Some(slots.into_boxed_slice());
    Ok(())
}

impl ModuleHandleSlot {
    /// Retain the current value so it can outlive a later write or cycle clear.
    pub fn get_owned<'py>(
        self,
        py: &'py CoreToken,
        module: &OwnedRuntimeValue<'_>,
    ) -> Result<Option<OwnedRuntimeValue<'py>>, ModuleHandleSlotError> {
        let mut state = c_api_module_state(local_token(py));
        let slot = state.handle_slot(module_key(module)?, self)?;
        Ok(slot.map(|bits| OwnedRuntimeValue::retain(py, bits)))
    }

    /// Transfer a reference to the slot before releasing its previous owner.
    /// An empty slot is distinct from None and positive zero.
    ///
    /// # Safety
    /// If supplied, the value must belong to the same runtime instance as the
    /// module. Thread attachment does not prove value provenance across isolates.
    pub unsafe fn set(
        self,
        py: &CoreToken,
        module: &OwnedRuntimeValue<'_>,
        value: Option<OwnedRuntimeValue<'_>>,
    ) -> Result<(), ModuleHandleSlotError> {
        let py = local_token(py);
        let key = module_key(module)?;
        let old = {
            let mut state = c_api_module_state(py);
            let slot = state.handle_slot(key, self)?;
            std::mem::replace(slot, value.map(OwnedRuntimeValue::into_bits))
        };
        if let Some(bits) = old {
            dec_ref_bits(py, bits);
        }
        Ok(())
    }
}

impl super::CApiModuleRuntimeState {
    fn handle_slot(
        &mut self,
        key: usize,
        slot: ModuleHandleSlot,
    ) -> Result<&mut Option<u64>, ModuleHandleSlotError> {
        self.metadata
            .get_mut(&key)
            .filter(|metadata| !metadata.free_started && !metadata.clear_active)
            .and_then(|metadata| metadata.handle_slots.as_mut())
            .ok_or(ModuleHandleSlotError::Unavailable)?
            .get_mut(slot.0)
            .ok_or(ModuleHandleSlotError::OutOfBounds)
    }
}

fn module_key(module: &OwnedRuntimeValue<'_>) -> Result<usize, ModuleHandleSlotError> {
    module_handle_ptr(module.bits())
        .map(|ptr| ptr as usize)
        .ok_or(ModuleHandleSlotError::InvalidModule)
}

fn local_token(py: &CoreToken) -> &PyToken<'_> {
    // SAFETY: the canonical core token holds thread and runtime custody for
    // the complete borrow. The local token adds only the lifetime marker.
    unsafe { PyToken::from_attached_context(py.attached_context()) }
}
