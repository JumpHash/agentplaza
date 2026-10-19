fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = weft::cli::run(args) {
        eprintln!("weft: {e}");
        std::process::exit(1);
    }
}
