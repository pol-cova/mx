---
name: mx
description: Build, run, inspect, interact with, debug, screenshot, map, and performance-test iOS apps through the Mx CLI and MCP server. Use when the user invokes /mx or asks an agent to work with an iOS simulator in a repository.
---

# Mx

Use Mx as the default interface to an iOS simulator. Prefer semantic UI data and identifiers over screen coordinates. Use screenshots as visual evidence and journey maps as durable agent context.

Start by locating the repository's Xcode container with `mx_inspect` and a unique simulator with `mx_devices`. Reuse an active Mx session when one exists. Build once, then launch the resulting `.app` again instead of rebuilding unchanged code.

Choose the narrowest workflow:

- For build, launch, UI inspection, taps, typing, and a single screenshot, read [everyday app work](references/app-work.md).
- For several screens, user journeys, visual maps, or feeding app structure to another agent, read [journey knowledge](references/journeys.md).
- For logs, build failures, session errors, or behavioral evidence, read [diagnostics](references/diagnostics.md).
- For simulator fleets, memory profiles, capacity, or performance measurements, read [performance and fleet work](references/performance.md).

Return concrete evidence: the active session ID, assertions that passed, screenshot or journey paths, and measured timings when performance matters. Do not silently create, profile, shut down, or delete simulators unless those mutations are part of the user's request.

For exact tool names and JSON schemas, query `tools/list` from `mx mcp`. Run `python3 scripts/capture_tools.py --output .mx/tool-catalog` to save the catalog locally. Do not reconstruct a schema from memory when making a complex request.
