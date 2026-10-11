//! Prototype of the JHRL engine (decision AP-074, language in docs/jhrl.md).
//!
//! v0 covers the front end (lexer with units, parser for the section 17 grammar), name and
//! dimension checking, the expression evaluator, the explicit-state model checker with mutants
//! and witnesses, machine and pure scenarios, and the constraint resolver. Std only, no
//! dependencies (AP-065). The real crate is `jhrl` in the `agentplaza` monorepo (AP-004).
//!
//! | Module | Section of docs/jhrl.md |
//! |---|---|
//! | [`lexer`], [`parser`], [`ast`] | 4, 8, 17 |
//! | [`num`] | 5 (dimensions and units) |
//! | [`program`] | 3 (packs, imports, name resolution) |
//! | [`check`] | 5, 9 (static names and dimensions) |
//! | [`eval`], [`value`] | 7, 8, 9.2, 9.3 |
//! | [`machine`], [`mc`] | 9.9, 9.10, 11 |
//! | [`scenario`] | 9.12 |
//! | [`resolve`] | 10 |
//! | [`cli`] | `jhrl check`, `jhrl test` |

pub mod ast;
pub mod authz;
pub mod check;
pub mod cli;
pub mod eval;
pub mod lexer;
pub mod machine;
pub mod mc;
pub mod num;
pub mod parser;
pub mod program;
pub mod resolve;
pub mod scenario;
pub mod span;
pub mod value;

pub use program::Program;
