use std::{
    env,
    fs::File,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::Path,
    process::ExitCode,
};

use ryoku_recovery::{Identifier, generation::GenerationReader};

const USAGE: &str = "Usage:\n  ryoku-recovery list STORE_ROOT\n  ryoku-recovery inspect STORE_ROOT TRANSACTION_ID\n  ryoku-recovery verify STORE_ROOT TRANSACTION_ID\n";

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
        [command, store_path] if command == "list" => {
            let reader = open_reader(Path::new(store_path))?;
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
        [command, store_path, transaction_id] if command == "inspect" || command == "verify" => {
            let reader = open_reader(Path::new(store_path))?;
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

fn open_reader(path: &Path) -> Result<GenerationReader, String> {
    let root = File::open(path).map_err(|error| format!("open store root: {error}"))?;
    let owner = root.metadata().map_err(|error| format!("inspect store root: {error}"))?.uid();
    GenerationReader::open_existing(&root, owner).map_err(|error| error.to_string())
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
    fn list_requires_an_existing_store_root() {
        let path =
            std::env::temp_dir().join(format!("missing-recovery-store-{}", std::process::id()));
        let mut output = Vec::new();
        assert!(execute(["list".into(), path.display().to_string()], &mut output).is_err());
    }
}
