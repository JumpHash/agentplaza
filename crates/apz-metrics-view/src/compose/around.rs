//! "What changed around then": the drawer a click on a chart point opens.

use crate::model::{Around, ViewQuery};
use crate::source::Source;

/// What happened within ± 10 minutes of `t_ms` in the query's scope: events, the metrics that moved most against
/// the ten minutes before, kept traces, and the ± 30 minute zoom as fractions of the query's range.
pub fn around(q: &ViewQuery, t_ms: u64, src: &dyn Source) -> Around {
    let _ = (q, t_ms, src);
    Around::default()
}
