# Real-world validation

Mx separates fixture tests from live simulator tests.

## Fixture coverage

`cargo test` uses controlled fake Xcode, AXe, and simulator executables for many cases. These tests cover parsing, locking, cancellation, session behavior, command construction, error handling, and protocol responses. They do not prove that an arbitrary iOS project builds or works in a live simulator.

## Live evidence

The README screenshots and published performance samples came from real CoreSimulator runs of the included MxDemo UIKit app on iOS 26.5 with AXe 1.8.0. Mx built and launched the app, inspected its accessibility tree, entered text, asserted labels, and captured PNG files.

MxDemo is deliberately small. It proves the basic path, but it is not evidence of compatibility with a production app.

## Aroli

Mx completed a live Aroli run on September 9, 2026. This was a real Xcode and CoreSimulator workflow. It did not use the test fixtures.

| Check | Result |
| --- | --- |
| Project | Aroli Xcode project, `Aroli` scheme |
| Simulator | iPhone 17 Pro, iOS 26.5, Mx Agent A |
| Build, boot, install, launch | Passed |
| Bundle ID | `com.paulcontre.Aroli` |
| Semantic inspection | Passed, including the live onboarding heading, field, progress, and controls |
| Text input | Entered 12 characters into the onboarding name field |
| Semantic action | Tapped the unique `Continue` button by label and role |
| State assertion | UI advanced from "1 of 7" to "2 of 7" and exposed "How do you make coffee?" |
| Screenshot | [Aroli onboarding step 2](assets/screenshots/aroli-real-validation.png) |
| Total first run | 46,042 ms |
| Build phase | 22,496 ms |
| Boot phase | 9,325 ms |

The build produced four Swift concurrency warnings in `RecipeAdjustmentView.swift`. Mx returned those warnings as structured diagnostics. They did not stop the app from building or launching.

AXe reported the application accessibility label as "Infuso" while the visible brand and project are Aroli. That belongs to the app's accessibility metadata and does not affect Mx's ability to inspect or control it.

## Installer

The installer downloads a pinned AXe archive, verifies its checksum, builds Mx from source, and installs both under user-owned locations. A live attempt on September 9, 2026 first exposed insufficient system disk space. The installer now checks the home and build volumes separately and prints download and build progress.

A second live run succeeded with `CARGO_TARGET_DIR` on an external volume. It installed AXe 1.8.0 under `~/Library/Application Support/Mx/tools/`, installed Mx under `~/.cargo/bin/`, and required no `MX_AXE_PATH` or `MX_STATE_DIR` exports. `mx doctor` then found Xcode 26.5, AXe 1.8.0, the native session directory, and 13 available iOS simulators.
