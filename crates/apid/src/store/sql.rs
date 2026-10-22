//! Every SQL statement the hub runs by hand (the components' statements come from their table definitions). Each
//! lives in its own file under `crates/apid/sql/queries/`, embedded at compile time, behind one `pub const` here and a
//! line in [`QUERIES`] naming the file it runs against; the test prepares every one against its migrated file and
//! runs the lint on it (docs/dev/sql.md).

// The budgets' ledger (budgets.rs).
pub const BUDGET_LEDGER: &str = include_str!("../../sql/queries/budget_ledger.sql");

/// Every query: (file, name, text), for the test that prepares each against its file.
#[cfg(test)]
pub const QUERIES: &[(&str, &str, &str)] = &[("plaza", "budget_ledger", BUDGET_LEDGER)];
