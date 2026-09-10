# Benchmarks

These measurements ran on September 8 and 9, 2026, using MxDemo on Apple Silicon, Xcode 26.5, iOS 26.5, and AXe 1.8.0. The Mac had 16 GiB of memory, other applications open, and existing swap use.

## What the numbers mean

- The 235 ms relaunch measures the edit and retry loop after the app is built and installed. It excludes Xcode build time.
- The 2.63 second capture flow measures three UI checks, two interaction sequences, and the output files. It excludes build and installation.
- The 832 MiB result measures one simulator using the slim profile. It sums per-process physical footprint and is useful for comparing profiles on the same Mac.
- The two-simulator result measures the combined footprint and one concurrent interaction run. It does not predict memory use for larger fleets.

The older 68.6% reduction and the current 832 MiB result use different profile versions. Do not treat them as one before-and-after comparison.

## Capture flow

[flow-20-runs.json](flow-20-runs.json) contains all 20 measured runs. Each run relaunches an already installed app, captures three semantically verified states, writes PNGs and JSON/HTML artifacts, and stops the app. Relaunch and capture-flow wall times are separate measurements. Median flow time was 2,630 ms, nearest-rank p95 was 3,570 ms, and the maximum was 6,264 ms. The median relaunch took 235 ms.

The JSON file contains every sample. Mx does not yet include a Rust command for repeating this older benchmark.

## Simulator memory

[simulator-density.json.gz](simulator-density.json.gz) retains the recorded phases and samples. The same owned simulator rebooted and settled for 30 seconds before ten one-second samples in each phase. Stock, broad slim, and restored states ran twice, with five build-free app workflows per phase.

The two-cycle median footprint was 3,429 MiB stock, 1,077 MiB slim, and 3,465 MiB restored. That is a 68.6% decrease from stock to slim in this experiment. The median workflow took 2.87 seconds stock and 2.71 seconds slim.

Footprint sums `proc_pid_rusage` physical footprint for descendants of the simulator's `launchd_sim`. It includes compressed accounting and is not unique system RAM. This older result used two cycles and fixed settling waits. The service profile has changed since then.

The published JSON copies omit local identity and path fields. Demo screenshots are in [assets/screenshots](../assets/screenshots/).

## Profile memory

[profile-memory.json](profile-memory.json) contains ten samples for each state from the September 9 profile. The idle median was 832.16 MiB, with an 832.05–842.96 MiB range.

Mx then launched Aroli, moved through onboarding with accessibility controls, returned to the name field, and measured while the software keyboard was visible. The median was 812.23 MiB, with an 812.19–813.63 MiB range. Foregrounding Aroli changed the process mix, which is why the active number is lower than idle.

The fix makes post-boot user-agent removal sequential so `launchctl` does not race itself. The slim profile now waits for late Watch agents, removes optional health and device-management agents, and gives idle audio and Metal compiler helpers a second cleanup pass. The helpers restart on demand.

SpringBoard, MercuryPosterExtension, BackBoard, AccessibilityUIServer, and WidgetRenderer remain. WidgetRenderer and MercuryPosterExtension restart when removed. Persistently disabling these agents made the simulator hang during boot, so Mx cleans them up after each boot instead.

## Two simulators

[fleet-memory.json](fleet-memory.json) records the two-simulator run. Both slim simulators launched the same prebuilt MxDemo app, entered different Unicode text, checked the resulting UI, and saved screenshots.

The September 9 profile used 1,840.22 MiB combined at idle and 2,244.99 MiB with both apps running, using the median of ten one-second samples. The ranges were 1,840.06–1,841.73 MiB idle and 2,244.83–2,253.21 MiB active. The older run used 2,329.23 MiB idle and 2,807.01 MiB active.

Both simulators accepted distinct Unicode text and returned the expected labels. The concurrent workflow took 11.051 seconds.

SpringBoard, MercuryPosterExtension, BackBoard, AccessibilityUIServer, and WidgetRenderer remained in the settled process tree. WidgetRenderer respawned after termination and was not added to the profile. Spotlight and NanoTimeKit were absent. InputUI returned after typing and is included in the active samples.

The run measured two simulators. No higher count has completed on this host.
