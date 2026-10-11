//! `jhrl check <files…>` and `jhrl test <files…>`. See `jhrl::cli`.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = jhrl::cli::main(&args, &mut std::io::stdout());
    std::process::exit(code);
}
