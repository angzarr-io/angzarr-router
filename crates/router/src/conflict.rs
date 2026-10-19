//! The conflict-unit overlap rule COMMUTATIVE resolution applies
//! (options.proto, the conflict_units region, "Overlap").

/// True iff two unit-path sets overlap: one holds a wildcard (a path starting
/// with `*`) and the other is non-empty, or a unit of one equals a unit of the
/// other or is a path prefix of it ending at a `.` or `[` boundary
/// (`items[42]` overlaps `items[42].weight`; `items[4]` does not overlap
/// `items[42]`). An empty set overlaps nothing.
pub fn overlap(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a.iter().chain(b).any(|unit| unit.starts_with('*')) {
        return true;
    }
    a.iter().any(|x| {
        b.iter()
            .any(|y| x == y || is_prefix(x, y) || is_prefix(y, x))
    })
}

/// `prefix` names a unit enclosing `unit`: a strict prefix ending at a `.` or
/// `[` boundary.
fn is_prefix(prefix: &str, unit: &str) -> bool {
    unit.len() > prefix.len()
        && unit.starts_with(prefix)
        && matches!(unit.as_bytes()[prefix.len()], b'.' | b'[')
}
