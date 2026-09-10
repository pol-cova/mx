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

The published JSON copies omit local identity and path fields. Demo screenshots are in [assets/screenshots](../assets/screenshots/).

## Current profile target

[target-profile-samples.json](target-profile-samples.json) contains the ten live samples for each state from the final September 9 profile. The idle median was 832.16 MiB, with an 832.05–842.96 MiB range. This met the 800–850 MiB idle goal.

Mx then launched the real Aroli app, moved through onboarding with semantic controls, returned to the name field, and measured while the software keyboard was visible. The median was 812.23 MiB, with an 812.19–813.63 MiB range. The active value is lower because foregrounding Aroli changed the simulator process mix; it is an independently sampled state rather than a projection from idle memory.

The fix makes post-boot user-agent removal sequential so `launchctl` does not race itself. The slim profile now waits for late Watch agents, removes optional health and device-management agents, and gives idle audio and Metal compiler helpers a second cleanup pass. The helpers restart on demand.

SpringBoard, MercuryPosterExtension, BackBoard, AccessibilityUIServer, and WidgetRenderer remain. WidgetRenderer and MercuryPosterExtension respawned when removed, and the retained services are required for a usable simulator or semantic UI control. Persistent `launchctl disable` overrides were rejected because they prevented a healthy boot.

## Two-simulator run

[fleet-validation-summary.json](fleet-validation-summary.json) records the live two-simulator result. Both slim simulators launched the same prebuilt MxDemo app, entered different Unicode text, verified the resulting semantic UI, and saved screenshots.

The September 9 profile used 1,840.22 MiB combined at idle and 2,244.99 MiB with both apps running, using the median of ten one-second samples. The ranges were 1,840.06–1,841.73 MiB idle and 2,244.83–2,253.21 MiB active. The older run used 2,329.23 MiB idle and 2,807.01 MiB active.

Both simulators completed a concurrent Unicode input and semantic UI verification. A later clean timing rerun could not save session state because the system disk filled during app installation, so the previous 11.051-second timing remains the published workflow timing.

SpringBoard, MercuryPosterExtension, BackBoard, AccessibilityUIServer, and WidgetRenderer remained in the settled process tree. WidgetRenderer respawned after termination and was not added to the profile. Spotlight and NanoTimeKit were absent. InputUI returned after typing and is included in the active samples.

This proves two concurrent simulators on the measured host. It does not establish a higher fleet limit.
