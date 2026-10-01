//! W3C trace-context identifiers in their canonical lowercase-hex form.

#[nutype::nutype(
    validate(predicate = |id: &str| {
        id.len() == 32
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && id.bytes().any(|b| b != b'0')
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Display, Serialize, Deserialize)
)]
pub struct TraceId(String);

#[nutype::nutype(
    validate(predicate = |id: &str| {
        id.len() == 16
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && id.bytes().any(|b| b != b'0')
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Display, Serialize, Deserialize)
)]
pub struct SpanId(String);
