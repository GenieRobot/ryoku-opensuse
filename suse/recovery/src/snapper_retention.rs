//! Read-only live partner observations for the Snapper retention planner.
//!
//! This module derives both partner flags from a sealed generation store and
//! a fresh config-scoped Snapper inventory. Callers must hold the common
//! mutation lock from catalog read through any later action and revalidate the
//! catalog before writing. The result is evidence for planning only; it grants
//! no deletion authority.

use std::{collections::BTreeMap, fs::File};

use ryoku_snapper::{
    CatalogIndex, ConfigName, RetentionPartnerObservation, SnapperBackend, SnapshotId,
    SnapshotKind, SnapshotRecord, TransactionId,
};

use crate::{
    Identifier,
    generation::{GenerationReader, VerifiedGeneration},
    native_checkpoint::{RootSnapshotReceipt, verify_historical_root_checkpoint},
};

#[derive(Debug, Eq, PartialEq)]
pub enum RetentionObservationError<E> {
    Snapper(E),
    InvalidSnapperInventory,
}

/// Observe every catalog row against one fresh Snapper inventory per config
/// and the exact verified transaction/generation link. Any backend failure,
/// foreign-config row, or duplicate snapshot ID aborts the whole observation;
/// missing or mismatched partners become `false` and therefore cannot prune.
pub fn observe_retention_partners<B: SnapperBackend>(
    catalog: &CatalogIndex,
    generations: &GenerationReader,
    snapper: &B,
    trusted_root: &File,
) -> Result<Vec<RetentionPartnerObservation>, RetentionObservationError<B::Error>> {
    observe_with_snapshot_verifier(
        catalog,
        generations,
        snapper,
        trusted_root,
        |root, entry, receipt| {
            receipt.link().snapper_config.as_str() == entry.root_config.as_str()
                && receipt.link().snapper_snapshot_id == entry.root_snapshot.as_str()
                && verify_historical_root_checkpoint(root, receipt).is_ok()
        },
    )
}

#[cfg(test)]
pub(crate) fn observe_retention_partners_with_verifier<B: SnapperBackend>(
    catalog: &CatalogIndex,
    generations: &GenerationReader,
    snapper: &B,
    trusted_root: &File,
    verify_snapshot: impl FnMut(&File, &ryoku_snapper::CatalogEntry, &RootSnapshotReceipt) -> bool,
) -> Result<Vec<RetentionPartnerObservation>, RetentionObservationError<B::Error>> {
    observe_with_snapshot_verifier(catalog, generations, snapper, trusted_root, verify_snapshot)
}

fn observe_with_snapshot_verifier<B: SnapperBackend>(
    catalog: &CatalogIndex,
    generations: &GenerationReader,
    snapper: &B,
    trusted_root: &File,
    mut verify_snapshot: impl FnMut(&File, &ryoku_snapper::CatalogEntry, &RootSnapshotReceipt) -> bool,
) -> Result<Vec<RetentionPartnerObservation>, RetentionObservationError<B::Error>> {
    let mut snapshots =
        BTreeMap::<ConfigName, BTreeMap<SnapshotId, (SnapshotKind, Option<TransactionId>)>>::new();
    for entry in catalog.entries() {
        if snapshots.contains_key(&entry.root_config) {
            continue;
        }
        let records =
            snapper.list(&entry.root_config).map_err(RetentionObservationError::Snapper)?;
        snapshots.insert(
            entry.root_config.clone(),
            validate_inventory(&entry.root_config, records)
                .map_err(|_| RetentionObservationError::InvalidSnapperInventory)?,
        );
    }

    Ok(catalog
        .entries()
        .map(|entry| {
            let generation = verified_generation_for_catalog(entry, generations);
            let generation_present_and_verified = generation.is_some();
            let snapper_row_verified = snapshots
                .get(&entry.root_config)
                .and_then(|inventory| inventory.get(&entry.root_snapshot))
                .is_some_and(|(kind, transaction_id)| {
                    *kind == SnapshotKind::Single
                        && transaction_id.as_ref() == Some(&entry.transaction_id)
                });
            let snapper_partner_present_and_verified =
                generation.as_ref().is_some_and(|generation| {
                    generation
                        .manifest()
                        .root_snapshot_receipt()
                        .is_some_and(|receipt| verify_snapshot(trusted_root, entry, receipt))
                }) && snapper_row_verified;
            RetentionPartnerObservation {
                transaction_id: entry.transaction_id.clone(),
                generation_present_and_verified,
                snapper_partner_present_and_verified,
            }
        })
        .collect())
}

fn validate_inventory(
    config: &ConfigName,
    records: Vec<SnapshotRecord>,
) -> Result<BTreeMap<SnapshotId, (SnapshotKind, Option<TransactionId>)>, ()> {
    let mut inventory = BTreeMap::new();
    for record in records {
        if &record.config != config
            || inventory.insert(record.id, (record.kind, record.transaction_id)).is_some()
        {
            return Err(());
        }
    }
    Ok(inventory)
}

fn verified_generation_for_catalog(
    entry: &ryoku_snapper::CatalogEntry,
    generations: &GenerationReader,
) -> Option<VerifiedGeneration> {
    let Ok(transaction_id) = Identifier::new(entry.transaction_id.as_str()) else {
        return None;
    };
    let generation = generations.open_verified_transaction(&transaction_id).ok()?;
    let manifest = generation.manifest();
    (manifest.transaction_id == transaction_id
        && manifest.generation_id.as_str() == entry.generation_id.as_str()
        && manifest.snapper_config.as_str() == entry.root_config.as_str()
        && manifest.snapper_snapshot_id == entry.root_snapshot.as_str())
    .then_some(generation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(config: &str, id: &str, kind: SnapshotKind) -> SnapshotRecord {
        SnapshotRecord {
            config: ConfigName::new(config).unwrap(),
            id: SnapshotId::new(id).unwrap(),
            kind,
            pre_snapshot: None,
            transaction_id: None,
        }
    }

    #[test]
    fn snapshot_inventory_requires_unique_rows_from_the_requested_config() {
        let config = ConfigName::new("root").unwrap();
        let single =
            validate_inventory(&config, vec![record("root", "42", SnapshotKind::Single)]).unwrap();
        assert_eq!(
            single.get(&SnapshotId::new("42").unwrap()),
            Some(&(SnapshotKind::Single, None))
        );

        assert!(
            validate_inventory(
                &config,
                vec![
                    record("root", "42", SnapshotKind::Single),
                    record("root", "42", SnapshotKind::Single)
                ]
            )
            .is_err()
        );
        assert!(
            validate_inventory(&config, vec![record("home", "42", SnapshotKind::Single)]).is_err()
        );
    }

    #[test]
    fn only_single_snapshots_can_be_catalog_root_partners() {
        let config = ConfigName::new("root").unwrap();
        for kind in [SnapshotKind::Pre, SnapshotKind::Post] {
            let inventory = validate_inventory(&config, vec![record("root", "42", kind)]).unwrap();
            assert_ne!(
                inventory.get(&SnapshotId::new("42").unwrap()).map(|(kind, _)| kind),
                Some(&SnapshotKind::Single)
            );
        }
    }
}
