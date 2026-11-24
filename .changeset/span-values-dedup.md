---
"sigiro": minor
---

Span attribute values of 1 KiB or more (such as GenAI prompts and responses) are now stored once per value per day in a new `sigiro_span_values` table instead of inline in `span_attributes`. Spans keep a `span_attribute_refs` column mapping each moved key to its value's hash, and the trace detail API and dashboard put the values back. SQL queries over `span_attributes` no longer see these large values; join `sigiro_span_values` on the hash to read them.
