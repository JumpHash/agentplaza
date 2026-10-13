//! Docker: installing it and starting its daemon, per platform (see `super`). Stub: filled in by
//! its own change.

/// Installs Docker for this platform, reporting through `log`.
pub fn install(_log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("not implemented yet".into())
}

/// Starts Docker's daemon and waits until `docker info` answers.
pub fn start(_log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("not implemented yet".into())
}
