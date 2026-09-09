---
name: mx-quick-screenshot
description: Capture one or more screenshots from booted iOS simulators with Mx. Use for quick simulator screenshots, visual checks, or saving the current app screen. Do not use for App Store framing or upload.
---

# Quick Mx screenshot

Use Mx instead of generic computer control when the target is an iOS simulator.

1. Run `mx devices` if the user did not identify a unique booted simulator.
2. For the current screen, call `mx_screenshot` with `device` and omit `output`. Inline content is enabled by default and Mx returns the saved absolute path.
3. When the user names a state or destination, pass a descriptive new `.png` path. Never delete or overwrite an earlier capture to make the call succeed.
4. Show the resulting image and report its path.

Use `mx_ui` before capture only when the requested state needs verification. Use semantic `mx_tap`, `mx_type`, or `mx_action` to reach that state. Do not guess coordinates.

For several booted simulators, capture distinct devices concurrently only when the client supports parallel tool calls. Use a unique filename per device when explicit paths are needed.

Read [the screenshot guide](../../README.md#screenshots) when the request involves several states, concurrency, or capture limits.
