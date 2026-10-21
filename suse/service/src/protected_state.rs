//! Startup gate for the privileged helper's protected recovery store.
//!
//! The expected identity is read from the root-side machine record and then
//! checked against live evidence by `GenerationStore::open_initialized_state_mount`. The
//! helper retains the resulting pinned store for its lifetime.

use ryoku_recovery::{
    generation::{GenerationError, GenerationStore},
    machine_state::{MachineStateError, read_machine_state_record},
    state_mount::StateMountIdentity,
};

/// Failures that prevent the helper from entering its request loop.
#[derive(Debug)]
pub enum ProtectedStateError {
    RootDescriptor(rustix::io::Errno),
    MachineIdentity(MachineStateError),
    Store(GenerationError),
}

impl std::fmt::Display for ProtectedStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootDescriptor(error) => write!(formatter, "open process root: {error}"),
            Self::MachineIdentity(error) => write!(formatter, "read machine identity: {error}"),
            Self::Store(error) => write!(formatter, "validate protected state store: {error}"),
        }
    }
}

impl std::error::Error for ProtectedStateError {}

/// Validated state storage pinned for the lifetime of the privileged helper.
pub struct ProtectedState {
    store: GenerationStore,
}

impl ProtectedState {
    /// Read the independent machine identity and open the fixed state mount.
    /// Any missing/corrupt identity or mount mismatch prevents construction.
    pub fn open() -> Result<Self, ProtectedStateError> {
        let store = open_with(
            || {
                let root = rustix::fs::open(
                    "/",
                    rustix::fs::OFlags::RDONLY
                        | rustix::fs::OFlags::DIRECTORY
                        | rustix::fs::OFlags::CLOEXEC,
                    rustix::fs::Mode::empty(),
                )
                .map_err(ProtectedStateError::RootDescriptor)?;
                read_machine_state_record(&root)
                    .map(|record| record.identity().clone())
                    .map_err(ProtectedStateError::MachineIdentity)
            },
            |identity| {
                GenerationStore::open_initialized_state_mount(identity)
                    .map_err(ProtectedStateError::Store)
            },
        )?;
        // The helper and independent root history console replay exactly the
        // same protected journal under the machine mutation lock. A malformed
        // history prevents helper startup, even though its dispatcher currently
        // exposes no provider mutation.
        drop(
            crate::native_mutation::ProtectedMutationHistory::open()
                .map_err(|_| ProtectedStateError::Store(GenerationError::InvalidStore))?,
        );
        Ok(Self { store })
    }

    /// The store descriptor remains pinned to the validated state mount.
    pub fn store(&self) -> &GenerationStore {
        &self.store
    }
}

fn open_with<T>(
    read_identity: impl FnOnce() -> Result<StateMountIdentity, ProtectedStateError>,
    open_store: impl FnOnce(&StateMountIdentity) -> Result<T, ProtectedStateError>,
) -> Result<T, ProtectedStateError> {
    let identity = read_identity()?;
    open_store(&identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn missing_or_invalid_identity_stops_before_store_open() {
        let store_open_attempted = Cell::new(false);
        let result = open_with::<()>(
            || Err(ProtectedStateError::MachineIdentity(MachineStateError::InvalidRecord)),
            |_| {
                store_open_attempted.set(true);
                Ok(())
            },
        );
        assert!(matches!(result, Err(ProtectedStateError::MachineIdentity(_))));
        assert!(!store_open_attempted.get());
    }

    #[test]
    fn validated_identity_is_forwarded_unchanged_to_store_open() {
        let identity = StateMountIdentity {
            filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            subvolume_id: 256,
            parent_subvolume_id: 5,
        };
        let result = open_with(
            || Ok(identity.clone()),
            |received| {
                assert_eq!(received, &identity);
                Ok(())
            },
        );
        assert!(result.is_ok());
    }
}
