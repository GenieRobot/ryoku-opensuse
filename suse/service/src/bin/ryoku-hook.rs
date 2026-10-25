//! Explicit account opt-in for the bounded configured-command policy class.
use ryoku_recovery::{
    hook_protocol::{HookApprovalRequest, HookOutput},
    wallpaper_protocol::digest,
};
fn main() {
    if let Err(error) = run() {
        eprintln!("ryoku-hook: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let usage = "inspect INDEX | authorize INDEX COMMAND_SHA256 SECONDS apply|restore [kitty_user] [fish_user]";
    if args.len() < 2 {
        return Err(usage.into());
    }
    let index: u16 = args[1].parse()?;
    if args[0] == "inspect" && args.len() == 2 {
        let home = std::env::var("HOME")?;
        let bytes = std::fs::read(format!("{home}/.config/ryoku/ryogami.json"))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("configuration exceeds bound".into());
        }
        let (command, kind, restore) = ryoku_service::hooks::command(&bytes, index)
            .map_err(|_| "unknown or malformed configured hook")?;
        println!(
            "{}",
            serde_json::json!({"index":index,"command":command,"command_sha256":digest(command.as_bytes()),"type":kind,"restore_enabled":restore,
            "policy":"existing captured files only; no children/network; Bash built-ins; 1–30 second deadline"})
        );
        return Ok(());
    }
    if args[0] != "authorize" || !(5..=7).contains(&args.len()) {
        return Err(usage.into());
    }
    let outputs = args[5..]
        .iter()
        .map(|value| match value.as_str() {
            "kitty_user" => Ok(HookOutput::KittyUser),
            "fish_user" => Ok(HookOutput::FishUser),
            _ => Err("unknown output; supported: kitty_user, fish_user"),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let allow_restore = match args[4].as_str() {
        "apply" => false,
        "restore" => true,
        _ => return Err(usage.into()),
    };
    let policy = HookApprovalRequest {
        index,
        command_sha256: args[2].clone(),
        timeout_seconds: args[3].parse()?,
        allow_restore,
        outputs,
    };
    if !policy.validate() {
        return Err("invalid exact policy; inspect the command and choose unique captured outputs and 1–30 seconds".into());
    }
    let result = ryoku_recovery::hook_client::exchange(
        serde_json::json!({"operation":"authorize_configured_hook","policy":policy}),
        "hook_policy_authorized",
    )?;
    println!("{}", result);
    Ok(())
}
