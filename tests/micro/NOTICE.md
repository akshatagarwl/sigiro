# Provenance and license

The Python simulator harness in this directory is **MIT OR Apache-2.0**.
`simulate.py` and its fixture firmware/outbox were imported from
https://github.com/akshatagarwl/sigiro-micro commit `d631614`.
`native.py` extends that test approach to the actual Sigiro host binary.

License texts are included in `../../crates/otlp-outbox/LICENSE-MIT` and
`../../crates/otlp-outbox/LICENSE-APACHE`. This exception does not relicense
existing Sigiro host code or the host integration in `src/micro`.

The MOSQUITTO and COLLECTOR image digests in `simulate.py` are the original lab's
pinned fixtures. `native.py` uses only Mosquitto; it does not substitute a Python
gateway or Collector for Sigiro.
