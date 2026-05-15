# drain3

Fast log template extraction via fixed-depth prefix trees.

Rust port of [Axiom's drain3](https://github.com/axiomhq/drain3) (Go). Splits log lines into tokens, clusters them by a prefix tree keyed on token count, and replaces variable tokens with a param placeholder (`<*>`` by default).

## Usage

```rust
use drain3::Config;

let samples: Vec<String> = vec![
    "connection from 10.0.0.1 timeout after 5000ms".into(),
    "connection from 10.0.0.2 timeout after 3000ms".into(),
    "connection from 10.0.0.3 timeout after 1000ms".into(),
];
let matcher = drain3::train(&samples, Config::default()).unwrap();
let (id, args, ok) = matcher.match_line("connection from 192.168.1.1 timeout after 42ms");
assert!(ok);
assert_eq!(args, vec!["192.168.1.1", "42ms"]);
```

## Performance

Key optimizations ported from the Go implementation:

- **First/last token prefilter** — bypasses tree descent for matching via binary-search lookups (3–5× speedup on typical workloads)
- **Frozen dictionary** — read-only sorted-array token→ID map replaces HashMap on the match path
- **hasParamFirst quick rejection** — instantly rejects lines with unknown first tokens without tokenization
- **Anchor checks** — rejects non-matching candidates in 2 comparisons
- **Scratch buffer reuse** — tokenization buffer reused across calls

## License

Apache-2.0
