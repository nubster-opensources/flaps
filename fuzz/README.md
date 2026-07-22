# Flaps fuzz targets

The fuzz workspace is intentionally independent from the release workspace.
Run targets on a Unix-like host with `cargo-fuzz` installed. The repository
`rust-toolchain.toml` pins a stable release that libFuzzer cannot use, and it
outranks the default toolchain, so the nightly has to be selected explicitly:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-07-15 cargo fuzz run flagset_from_json
```

Continuous integration pins the same dated nightly and `cargo-fuzz 0.13.2`. A
floating nightly would make a crash reproduce on one day and vanish the next.

## Budgets

| Trigger | Budget per target | Maximum input |
| --- | --- | --- |
| Pull request | 10 000 runs | 64 KiB |
| Push to `main`, weekly schedule | 300 seconds | 1 MiB |

Pull requests stay short enough to keep the check interactive; the scheduled
run is where genuine exploration happens.

Crashing inputs belong in `artifacts/` while being investigated. Minimize them
with `cargo fuzz tmin`, then preserve the minimized input in the target corpus
or convert it into a deterministic regression test before closing the defect.
