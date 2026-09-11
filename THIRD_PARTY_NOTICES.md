# Third-party notices

The root MIT license covers Mx's original work. It does not replace the licenses of dependencies, derived data, or external tools.

## Runtime catalog mappings

Initial service category and capability mappings in `data/mx-runtime-ios-26.5.json` derive from `profiles.go` and `features.go` in [MobAI-App/simslim](https://github.com/MobAI-App/simslim/tree/f3b979ecd913f56904a9b6100cad1f84fe01d228).

Copyright 2026 Interlap. Licensed under MIT. The full original [license](third-party/mx-runtime-seed/LICENSE) and [provenance notice](third-party/mx-runtime-seed/NOTICE.md) are retained. Mx has no build or runtime dependency on simslim.

## mx-guest / idb

The in-simulator `mx-guest` accessibility server is derived from the MIT-licensed [facebook/idb](https://github.com/facebook/idb) `SimulatorFrameworkBridge` (AXRuntime + XCTAutomationSupport snapshot, framed Unix-socket protocol). Copyright Meta Platforms, Inc. and affiliates. Mx does not vendor idb binaries.

## Rust dependencies

`Cargo.toml` declares dependencies and `Cargo.lock` pins the resolved versions. Each crate retains its own license. Mx's MIT license does not relicense those crates. When distributing a compiled binary, review the resolved dependency licenses and include their required notices.

## Apple tools

Xcode, Apple SDKs, CoreSimulator, and simulator runtimes are external prerequisites governed by Apple's terms. They are not included in this repository or covered by Mx's MIT license.
