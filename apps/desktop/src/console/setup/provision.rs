//! The plaza's containers, Postgres and Forgejo, and their settings in apid's keys.env (see
//! `super`). Stub: filled in by its own change.

/// Creates (or starts) `agentplaza-postgres` and records `APZ_DATABASE_URL` in keys.env.
pub fn postgres(_log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("not implemented yet".into())
}

/// Creates (or starts) `agentplaza-forgejo`, its admin and token, and records them in keys.env.
pub fn forgejo(_log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("not implemented yet".into())
}
