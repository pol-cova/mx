# Benchmark evidence

These are local exploratory measurements from September 8 and 9, 2026, using MxDemo on Apple Silicon, Xcode 26.5, iOS 26.5, and AXe 1.8.0. The memory experiment used a 16 GiB Mac with other applications running and existing swap use. They do not establish an app compatibility guarantee or a fleet capacity limit.

## Capture flow

[flow-20-runs.json](flow-20-runs.json) contains all 20 measured runs. Each run relaunches an already installed app, captures three semantically verified states, writes PNGs and JSON/HTML artifacts, and stops the app. Relaunch and capture-flow wall times are separate measurements. Median flow time was 2,630 ms, nearest-rank p95 was 3,570 ms, and the maximum was 6,264 ms. The median relaunch took 235 ms.

The raw report is retained so readers can inspect every sample. The original Python measurement harness has been removed from the public project because Python is not part of Mx's runtime or supported toolchain. A replacement benchmark command should be implemented in Rust before these measurements are refreshed.

## Simulator memory

[simulator-density.json.gz](simulator-density.json.gz) retains the recorded phases and samples. The same owned simulator rebooted and settled for 30 seconds before ten one-second samples in each phase. Stock, broad slim, and restored states ran twice, with five build-free app workflows per phase.

The two-cycle median footprint was 3,429 MiB stock, 1,077 MiB slim, and 3,465 MiB restored. That is a 68.6% decrease from stock to slim in this experiment. The median workflow took 2.87 seconds stock and 2.71 seconds slim.

Footprint sums `proc_pid_rusage` physical footprint for descendants of the simulator's `launchd_sim`. It includes compressed accounting and is not unique system RAM. These small samples and fixed settling waits do not satisfy the project's stricter repeated A/B/A release evidence contract. The service profile has evolved since this experiment, so a current checkout is not an exact replay of that binary.

The original Python measurement harness has been removed. A future Rust benchmark command must preserve the same sampling method before this result can be reproduced from the public interface.

The published JSON copies omit local identity/path fields; measurement values are unchanged. Demo screenshots are in [assets/screenshots](../assets/screenshots/). No new simulator measurements were taken for this README.

## Two-simulator run

[fleet-validation-summary.json](fleet-validation-summary.json) records the live two-simulator result. Both slim simulators launched the same prebuilt MxDemo app, entered different Unicode text, verified the resulting semantic UI, and saved screenshots.

The two simulators used 2,329.23 MiB combined while idle and 2,807.01 MiB with both apps running. The concurrent workflows finished in 11.051 seconds. Swap stayed at 1,594 MiB.

This proves two concurrent simulators on the measured host. It does not establish a higher fleet limit.
