//! A source of generated data: the fixture both consoles show with `fixture=1`, and the data the composers' tests
//! run against. Deterministic for a given `now_ms`.

/// The fixture (implemented in a later slice).
pub struct Fixture {
    pub now_ms: u64,
}
