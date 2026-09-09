---
name: mx-capture-flow
description: Build an iOS app with Mx, drive it through named UI states, and capture a screenshot set. Use for agent research, bug evidence, before-and-after images, or simulator walkthrough captures. Do not use for App Store upload.
---

# Capture an app flow with Mx

Turn the user's requested images into a short state list. Use their names for filenames. If no output directory was given, use `screenshots/<timestamp-or-task-name>/` inside the project.

Resolve the Xcode container, scheme, and a unique simulator with `mx_inspect` and `mx_devices`. Use `mx_run` once with `inspect_ui: true`. Reuse that session for the full sequence.

Create one `mx_capture_flow` request. For each state:

- Give it a short filename-safe name.
- Add the semantic tap and type actions needed to reach it.
- Set `expect_label` to a stable label that proves arrival.

Mx executes the request in order, settles each state, saves the whole-screen PNG, and writes `flow.json` with every semantic UI snapshot. Read the manifest first. Load individual PNGs only when visual layout matters.

If semantic inspection cannot identify a requested control, stop and explain the missing accessibility information before considering visual computer control.

Keep each device's actions ordered. Distinct devices may run concurrently after `mx_capacity` confirms headroom. Never reuse a screenshot path, and do not change simulator profiles unless the user asked for that work.

Finish with a compact manifest of requested states, passed assertions, captures, and failures. Display the images when the client supports local image rendering.

Read [the navigation flow guide](../../README.md#capture-a-complete-flow) for the plan schema, output format, performance model, and limits.
