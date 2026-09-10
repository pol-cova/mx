# Mx

Build and test iOS apps through an AI agent, with semantic UI inspection and lower simulator overhead.

Mx is a Rust CLI and MCP server for macOS. It builds your app, runs it in the Simulator, reads controls from the accessibility tree, and lets an agent interact by identifier and verify the result. Screenshots are available when you need to check appearance.

<img src="assets/screenshots/simulator-greeting.png" width="280" alt="An iOS app running in the Simulator after Mx entered a name and verified the greeting">

An iOS app after Mx entered a name, selected Greet, and verified the result. Captured directly from the Simulator.

## Performance

Reuse an installed app without rebuilding. Read semantic state changes without requesting another image. Apply optional service profiles to reduce simulator background processes.

| Measurement | Result |
| --- | ---: |
| Warm app relaunch | **235 ms** median |
| Three-state verified capture flow | **2.63 s** median, **3.57 s** p95 |
| Simulator footprint, stock → slim | **3,429 → 1,077 MiB** |
| Later optimized profile, idle | **832 MiB** median |

Measured on Apple Silicon with Xcode 26.5, iOS 26.5, and AXe 1.8.0. The linked benchmark report includes the commands, raw results, and measurement boundaries.

[Raw samples, methodology, and fleet results →](benchmarks/README.md)

## Install

Requires macOS, Xcode with an iOS Simulator runtime, and Rust 1.89+.

```sh
git clone https://github.com/pol-cova/mx.git
cd mx
sh scripts/install.sh
mx doctor
```

The installer builds Mx and downloads checksum-verified AXe 1.8.0. Keep your Cargo bin directory on `PATH` and allow 2 GiB free for the build.

## Connect your agent

```sh
mx mcp-config
```

Merge the generated `mcpServers` JSON into your MCP client. For clients using another format, register the printed executable path as a **STDIO** server with argument `mcp`. The client starts the server.

The [Mx skill](skills/mx/SKILL.md) guides the agent through builds, semantic interaction, captures, and diagnostics. Try:

> Run this iOS project, inspect its accessibility tree, complete the first screen, and verify the result.

## Use the CLI

```sh
mx inspect --project /path/to/YourApp
mx devices

mx run --project /path/to/YourApp --scheme YourApp \
  --device SIMULATOR_UDID --inspect-ui

mx ui --device SIMULATOR_UDID
mx tap --device SIMULATOR_UDID --id ELEMENT_ID
mx observe --device SIMULATOR_UDID
mx relaunch --device SIMULATOR_UDID --inspect-ui
```

Choose a device from `mx devices` and an element identifier from `mx ui`. Use `mx action` for action sequences with an expected result. A runnable example is included under `examples/`. See `mx --help` for all commands.

## Watch in your editor

```sh
mx web --device SIMULATOR_UDID
```

Open the printed URL in your editor or browser and leave the command running. Mx streams directly from AXe, with pointer and text input. No separate streaming service is required. The server is localhost-only; this URL is not an MCP endpoint.

## How it works

```text
Agent → MCP / Rust CLI → Xcode         build and diagnostics
                      → CoreSimulator install, launch, and logs
                      → AXe           semantic tree, input, and video
```

Mx tracks the device, app process, and UI revisions in a session. Semantic actions check the foreground app and reject stale references. Action requests can wait for an expected label; a standalone tap does not verify the whole task.

Mx runs against local iOS simulators. Semantic inspection uses the accessibility information exposed by the app. Simulator profiles currently target Mx-owned iOS 26.5 devices and keep the services selected for each workload.

## Documentation

- [App workflows](skills/mx/references/app-work.md)
- [Capture flows](skills/mx-capture-flow/SKILL.md)
- [Performance and simulator profiles](skills/mx/references/performance.md)
- [Benchmarks](benchmarks/README.md)
- [Performance analysis and next optimizations](PERFORMANCE.md)
- [Development and contributing](CONTRIBUTING.md)

## License

[MIT](LICENSE). Uses [AXe](https://github.com/cameroncooke/AXe) and service mappings derived in part from [simslim](https://github.com/MobAI-App/simslim). See [third-party notices](THIRD_PARTY_NOTICES.md).
