# Everyday app work

Use `mx_inspect` to resolve the Xcode workspace/project and schemes. Use `mx_devices` to select a unique UDID. Then use `mx_run` with an explicit scheme and `inspect_ui: true`. Its result includes the built app path, session ID, PID, diagnostics, timings, and initial semantic UI.

Use `mx_observe` for stable references and deltas. Prefer `mx_action` when several actions form one verified transition. Use `mx_tap` or `mx_type` for a single exploratory action. Inspect or observe after a state change.

Use `mx_screenshot` with only the device for a quick whole-screen PNG. Pass a new explicit output path when the name belongs to a test, bug, or product state. Mx never replaces an existing capture. Inline images are available up to 8 MiB.

Use `mx_launch` with the `.app` path from a prior run when source and build settings have not changed. This skips Xcode but still installs the app. Once that app is installed, use `mx_relaunch` for retries and branch captures. It skips build, installation, and device discovery; the reference-host median is about 235 ms.

Use `mx_stop` to terminate the app without parking the simulator. Only use fleet shutdown or deletion when the user requested simulator lifecycle changes.
