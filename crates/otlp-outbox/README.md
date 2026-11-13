# otlp-outbox (experimental)

Allocator-free `no_std` OTLP outbox policy using upstream `sequential-storage`
8.x and `embedded-storage-async`. Original protobuf bytes are stored unchanged;
this crate is not a semantic decoder, flash driver, SDK or network client.

Only an authenticated, matching full-acceptance receipt may pop a batch. Broker
PUBACK, partial acceptance and permanent rejection never delete data. The caller
must define the receiver's acceptance boundary (the native Sigiro integration
uses durable **raw recording**, not query visibility).

The caller supplies persistent unique 128-bit IDs, suitable flash geometry,
network/TLS adapters, bounded retry policy and durable rejection quarantine. No
physical-flash or MCU networking claim follows from the QEMU tests.

MSRV: Rust 1.89. License: **MIT OR Apache-2.0**, explicitly independent of the
host workspace's FSL license. Imported from
https://github.com/akshatagarwl/sigiro-micro at `d631614`.
`publish=false` remains intentional until API/protocol review.

```sh
cargo test --locked -p otlp-outbox
cargo check --locked --lib -p otlp-outbox --target thumbv7m-none-eabi
```

See [`../../docs/micro.md`](../../docs/micro.md) for the native receiver and
[`../../firmware/qemu-recorder`](../../firmware/qemu-recorder) for actual
allocator-free Cortex-M instructions running against a modeled NOR device.
