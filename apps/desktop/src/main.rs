//! The dashboard binary: parse the arguments, then hand the register to iced.

use agentplaza_desktop::cli::{self, Invocation};

fn main() -> iced::Result {
    let invocation = match cli::parse(std::env::args().skip(1), &|name| std::env::var(name).ok()) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("agentplaza-desktop: {message}\n\n{}", cli::HELP);
            std::process::exit(2);
        }
    };
    match invocation {
        Invocation::Help => {
            println!("{}", cli::HELP);
            Ok(())
        }
        Invocation::Run(config) => agentplaza_desktop::app::run(config),
    }
}
