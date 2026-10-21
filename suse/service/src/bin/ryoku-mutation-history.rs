//! Independent root console for protected mutation history and safe pre-apply
//! interruption reconciliation. It never restores state or repeats a provider.
use ryoku_service::{OperationId, native_mutation::ProtectedMutationHistory};
fn main() {
    if let Err(error) = run() {
        eprintln!("ryoku-mutation-history: {error}");
        std::process::exit(2);
    }
}
fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if !matches!(arguments.as_slice(), [command] if command == "status")
        && !matches!(arguments.as_slice(), [command, _] if command == "reconcile-before-change")
    {
        return Err(
            "usage: ryoku-mutation-history status | reconcile-before-change OPERATION_ID".into()
        );
    }
    let mut history = ProtectedMutationHistory::open().map_err(|e| format!("{e:?}"))?;
    if let [command, id] = arguments.as_slice()
        && command == "reconcile-before-change"
    {
        let id = OperationId::new(id).map_err(|e| format!("{e:?}"))?;
        history.reconcile_before_change(&id).map_err(|e| format!("{e:?}"))?;
        println!("Recorded failure before mutation; no provider action repeated");
        return Ok(());
    }
    for status in history.journal().statuses() {
        let context = history.journal().mutation_context(&status.operation_id);
        println!("{}", serde_json::json!({"status": status, "mutation": context}));
    }
    println!("recovery_required={}", history.journal().recovery_required().len());
    Ok(())
}
