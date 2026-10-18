fn main() {
    if let Err(error) = ryoku_service::runtime::run_native() {
        eprintln!("ryoku-service: {error}");
        std::process::exit(1);
    }
}
