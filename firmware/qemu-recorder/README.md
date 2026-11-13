# QEMU recorder fixture

MIT OR Apache-2.0. Imported from `akshatagarwl/sigiro-micro` commit `d631614`.

This independent target workspace builds actual Cortex-M3 firmware without a
global allocator. It uses the shared `../../crates/otlp-outbox` path dependency,
not the host's DuckDB/Arrow/Tokio dependency graph. `embassy-futures::block_on`
supplies the simulator's polling executor; no bespoke executor is maintained.
A hardware adapter still needs a board-appropriate executor and drivers.

```sh
rustup target add --toolchain stable thumbv7m-none-eabi
rustup run stable cargo build --locked --release --target thumbv7m-none-eabi
```

Run from this directory so `.cargo/config.toml` supplies the Cortex linker flags.
`../../tests/micro/simulate.py` drives fresh `mps2-an385` QEMU boots and byte-budget
resets. `../../tests/micro/native.py` bridges its mailboxes to an authenticated
TLS broker and the real Sigiro binary. Semihosting files model 8 KiB NOR; no real
flash controller, MCU networking or physical power-cut behavior is tested.

Fixture IDs are deterministic, **not** a production unique-ID allocator.
