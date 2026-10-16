//! Small helpers shared by the stores: a time bound as milliseconds since the epoch (integer arithmetic, so there is no
//! floating-point rounding of a stored time), and the statements the explain tests look at.

/// A `timestamptz` from milliseconds bound as parameter `$n`.
macro_rules! ts {
    ($n:literal) => {
        concat!(
            "(timestamptz 'epoch' + ($",
            $n,
            "::bigint::text || ' milliseconds')::interval)"
        )
    };
}
pub(crate) use ts;

/// Milliseconds since the epoch of a `timestamptz` column, for reading a time back.
macro_rules! ms {
    ($col:literal) => {
        concat!("(extract(epoch from ", $col, ") * 1000)::bigint")
    };
}
pub(crate) use ms;
