# Contributing to Mx

Open an issue for bugs or proposed changes. For larger changes, describe the problem and intended behavior before starting a pull request.

## Development

Follow the README setup, then run:

```sh
cargo test --locked -j 1
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

The Rust tests use fixtures and fake tools for automated coverage. Changes to real simulator behavior also need a live check on macOS with Xcode and AXe. Include the device type, runtime version, commands, and observed result in your pull request. Mx's machine-facing implementation is Rust. Do not add Python as a runtime or development requirement.

Keep changes focused. Add a regression test when fixing behavior. For performance claims, include sample counts, median and tail latency, the measurement method, and enough evidence to reproduce the result. See [benchmarks](benchmarks/README.md).

## Reporting a bug

Include your Mx revision, macOS/Xcode/AXe versions, simulator model and runtime, reproduction steps, expected behavior, and actual behavior. Attach relevant diagnostics or screenshots after removing private data.

## Licenses

Contributions to Mx's original code use the project's MIT license. Only submit work you have permission to contribute. Preserve third-party copyright and license notices, and document the source and license of any imported code or data in THIRD_PARTY_NOTICES.md.
