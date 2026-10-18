use std::{
    env,
    fs::File,
    io::{self, Write},
    process::ExitCode,
};

use ryoku_recovery::{Identifier, generation::GenerationReader, restore::RestoreJournalStore};

const USAGE: &str = "Usage:\n  ryoku-recovery list\n  ryoku-recovery pending\n  ryoku-recovery inspect TRANSACTION_ID\n  ryoku-recovery verify TRANSACTION_ID\n  ryoku-recovery verify-linked TRANSACTION_ID\n";

fn main() -> ExitCode {
    match execute(env::args().skip(1), &mut io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "ryoku-recovery: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(
    arguments: impl IntoIterator<Item = String>,
    output: &mut impl Write,
) -> Result<(), String> {
    let args: Vec<_> = arguments.into_iter().collect();
    match args.as_slice() {
        [command] if command == "pending" => {
            let store = RestoreJournalStore::open_existing_state_mount()
                .map_err(|error| error.to_string())?;
            let pending = store.incomplete_attempts().map_err(|error| error.to_string())?;
            if pending.is_empty() {
                writeln!(output, "No unfinished recovery attempts")
                    .map_err(|error| error.to_string())?;
            } else {
                for attempt in pending {
                    writeln!(output, "{} {:?}", attempt.restore_id.as_str(), attempt.phase)
                        .map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        }
        [command] if command == "list" => {
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let transactions = reader.list_transactions().map_err(|error| error.to_string())?;
            if transactions.is_empty() {
                writeln!(output, "No committed transactions").map_err(|error| error.to_string())?;
            } else {
                for transaction in transactions {
                    writeln!(output, "{}", transaction.as_str())
                        .map_err(|error| error.to_string())?;
                }
            }
            Ok(())
        }
        [command, transaction_id] if command == "verify-linked" => {
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let root = File::open("/").map_err(|error| error.to_string())?;
            let linked = reader
                .open_cataloged_transaction(&root, &transaction)
                .map_err(|error| error.to_string())?;
            let manifest = linked.generation().manifest();
            writeln!(
                output,
                "partners-verified transaction={} generation={} snapshot={}:{}",
                manifest.transaction_id.as_str(),
                manifest.generation_id.as_str(),
                manifest.snapper_config.as_str(),
                manifest.snapper_snapshot_id,
            )
            .map_err(|error| error.to_string())?;
            writeln!(output, "Boot compatibility and restore readiness are not assessed")
                .map_err(|error| error.to_string())?;
            Ok(())
        }
        [command, transaction_id] if command == "inspect" || command == "verify" => {
            let reader = GenerationReader::open_state_mount().map_err(|error| error.to_string())?;
            let transaction = Identifier::new(transaction_id.clone())
                .map_err(|error| format!("invalid transaction ID: {error:?}"))?;
            let manifest =
                reader.verify_transaction(&transaction).map_err(|error| error.to_string())?;
            if command == "verify" {
                writeln!(
                    output,
                    "verified transaction={} generation={} entries={}",
                    manifest.transaction_id.as_str(),
                    manifest.generation_id.as_str(),
                    manifest.files().len()
                )
                .map_err(|error| error.to_string())?;
            } else {
                writeln!(output, "Integrity: verified").map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Original native receipt: {} (partner availability unchecked)",
                    if manifest.root_snapshot_receipt().is_some() { "recorded" } else { "absent" }
                )
                .map_err(|error| error.to_string())?;
                writeln!(output, "Transaction: {}", manifest.transaction_id.as_str())
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Generation: {}", manifest.generation_id.as_str())
                    .map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Snapper snapshot: {}:{}",
                    manifest.snapper_config.as_str(),
                    manifest.snapper_snapshot_id
                )
                .map_err(|error| error.to_string())?;
                writeln!(output, "Filesystem UUID: {}", manifest.filesystem_uuid)
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Created UTC: {}", manifest.created_utc)
                    .map_err(|error| error.to_string())?;
                writeln!(output, "Declared entries: {}", manifest.files().len())
                    .map_err(|error| error.to_string())?;
                writeln!(
                    output,
                    "Manifest SHA-256: {}",
                    manifest.manifest_sha256.as_ref().map_or("missing", |digest| digest.as_str())
                )
                .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_or_mutating_commands_with_read_only_usage() {
        let mut output = Vec::new();
        assert!(execute(["restore".into()], &mut output).is_err());
        assert!(String::from_utf8(output).unwrap().is_empty());
    }

    #[test]
    fn cli_does_not_accept_an_arbitrary_store_path() {
        let mut output = Vec::new();
        assert!(execute(["list".into(), "/tmp".into()], &mut output).is_err());
        assert!(String::from_utf8(output).unwrap().is_empty());
    }

    #[test]
    fn linked_verification_rejects_malformed_ids_and_path_overrides() {
        let mut output = Vec::new();
        let result = execute(["verify-linked".into(), "../outside".into()], &mut output);
        assert!(result.unwrap_err().starts_with("invalid transaction ID:"));
        assert_eq!(
            execute(["verify-linked".into(), "txn-1".into(), "/tmp".into()], &mut output),
            Err(USAGE.into())
        );
        assert!(output.is_empty());
    }
}
