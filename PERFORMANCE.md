# Performance analysis

Mx now owns the accessibility transport (`mx-guest` inside the simulator plus in-process host HID). The remaining latency is guest snapshot time, settle polling, screenshots, and the app's own animations.

The checked-in 20-run benchmark (2.63 s median) is the previous CLI-transport baseline. Re-run that journey on `native-transport` before publishing a new number.

## Next measurements

1. Warm `mx ui` against a booted MxDemo after `mx-guest` is resident.
2. Three-screen capture-flow with install-free relaunch.
3. Screenshot path versus IOSurface frames for `mx web`.

Keep CONTRIBUTING.md rules: 20 samples, median and nearest-rank p95, raw JSON under `benchmarks/`.
