# Mx

Mx lets an AI agent build, run, inspect, and control iOS apps in the Simulator.

It is a Rust CLI and MCP server for macOS. Mx uses Xcode, CoreSimulator, and the accessibility tree. An agent can build an app, tap controls by label or identifier, enter text, read logs, verify UI state, and save screenshots.

Mx is at version 0.1.0. The core workflow works on a real SwiftUI project. Fleet profiling is experimental and has only been measured on the machine described below.

## Measured results

Tests ran on a 16 GiB Apple Silicon Mac with Xcode 26.5, iOS 26.5, and AXe 1.8.0.

| Test | Result | Scope |
| --- | ---: | --- |
| Relaunch an installed app | 235 ms median | 20 MxDemo runs |
| Capture three verified screens | 2.63 s median, 3.57 s p95 | 20 MxDemo runs |
| Slim simulator idle memory | 3,429 MiB to 1,077 MiB | Two stock, slim, and restore cycles |
| Restore simulator profile | 3,465 MiB after restore | Returned to the stock range |
| Two simulators idle | 2,329 MiB combined | Two slim simulators |
| Two simulators with apps running | 2,807 MiB combined | Same prebuilt MxDemo app |
| Concurrent app workflow | 11.05 s | Unicode input and screenshots on both simulators |

Swap stayed at 1,594 MiB during the two-simulator test. Mx stopped both apps and shut down both simulators afterward.

The memory number is the summed physical footprint of each simulator's process tree. It is useful for comparing the same machine and workload. It is not the amount of unique system RAM saved.

Two concurrent simulators are proven on this host. More are not. A six-device attempt stopped during device creation because the disk filled up, so it does not count as a concurrency result. Mx can enforce a memory budget before booting another simulator, but that does not prove the machine can run the requested fleet.

The raw flow and memory samples are in [benchmarks](benchmarks/README.md).

## Real app test

Mx also ran against Aroli, a separate SwiftUI app with package dependencies and iPhone and Watch targets.

Mx discovered the Xcode project, built the `Aroli` scheme, booted an iPhone 17 Pro simulator, installed the app, and launched it. It then read the live accessibility tree, entered a name, tapped `Continue` by label and role, and verified that onboarding moved from step 1 to step 2.

The first run took 46.0 seconds. The build took 22.5 seconds and the simulator boot took 9.3 seconds. This was a functional test on one simulator, not a concurrency benchmark. Mx also returned four Swift concurrency warnings from the Aroli build as structured diagnostics.

<img src="assets/screenshots/aroli-real-validation.png" width="280" alt="Aroli onboarding step 2 after Mx entered a name and tapped Continue">

The full record is in [REAL_WORLD_VALIDATION.md](REAL_WORLD_VALIDATION.md).

## What Mx does

- Discovers Xcode projects, workspaces, schemes, and simulators.
- Builds, installs, launches, relaunches, and stops apps.
- Reads semantic UI state and tracks changes between revisions.
- Taps controls and enters ASCII or Unicode text.
- Checks the foreground app before sending input.
- Returns structured build warnings and errors.
- Reads live logs with bounded cursors.
- Captures screenshots and named multi-screen flows.
- Creates isolated simulators and keeps their sessions separate.
- Applies reversible service profiles to Mx-owned iOS 26.5 simulators.
- Checks simulator count and memory headroom before booting another device.

## Install

You need macOS, Xcode with an iOS Simulator runtime, and Rust 1.89 or later.

```sh
git clone https://github.com/pol-cova/mx.git
cd mx
sh scripts/install.sh
mx doctor
```

The installer downloads AXe 1.8.0, verifies its checksum, and builds Mx. Mx finds AXe and its session directory automatically. You do not need to export their paths.

If the checkout is on a small disk, put Rust build output on another volume:

```sh
CARGO_TARGET_DIR=/path/with/free/space sh scripts/install.sh
```

## Run an app

List simulators:

```sh
mx devices
```

Choose a simulator UDID and run the included app:

```sh
mx run \
  --project examples/MxDemo \
  --scheme MxDemo \
  --device SIMULATOR_UDID \
  --inspect-ui

mx tap --device SIMULATOR_UDID --id increment
mx observe --device SIMULATOR_UDID
mx screenshot --device SIMULATOR_UDID
```

Use your own project path and scheme to run another app.

## Connect an agent

Generate the MCP configuration for the installed executable:

```sh
mx mcp-config
```

Copy the JSON into your MCP client. The generated command uses the absolute path to Mx, so it does not depend on the client's shell path.

The agent can then make a request such as:

> Build this iOS app, launch it in the simulator, complete the first screen using accessibility controls, verify the result, and save a screenshot.

The [`/mx`](skills/mx/SKILL.md) skill gives an agent instructions for app work, flow capture, diagnostics, and simulator performance.

## Test status

The repository has 60 passing Rust tests. Many use controlled Rust test executables in place of Xcode and AXe so error and concurrency cases remain repeatable. Those tests check Mx logic. They are separate from the live MxDemo and Aroli runs described above.

See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting a change.

## License

Mx is available under the [MIT License](LICENSE).

The simulator service catalog contains mappings derived from [simslim](https://github.com/MobAI-App/simslim), also under MIT. Mx downloads [AXe](https://github.com/cameroncooke/AXe), which uses the MIT License. Xcode and simulator runtimes remain subject to Apple's terms.

See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the retained notices.
