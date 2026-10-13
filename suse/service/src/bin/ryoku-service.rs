fn main() {
    if let Err(error) = ryoku_service::runtime::run_from_environment() {
        eprintln!("ryoku-service: {error}");
        std::process::exit(1);
    }
}
