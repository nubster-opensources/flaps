# Flaps fuzz targets

The fuzz workspace is intentionally independent from the release workspace.
Run targets on a Unix-like host with Rust nightly and `cargo-fuzz` installed:

```sh
cargo fuzz run flagset_from_json
```

Crashing inputs belong in `artifacts/` while being investigated. Minimize them
with `cargo fuzz tmin`, then preserve the minimized input in the target corpus
or convert it into a deterministic regression test before closing the defect.
