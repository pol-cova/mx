# Diagnostics

Use the structured diagnostics and diagnostic delta returned by `mx_run` before reading its full build log. Mx caps parsed diagnostics and preserves the complete log on disk.

Use `mx_logs` for a bounded recent window. For ongoing work, start `mx_logs_start`, pull cursor batches with `mx_logs_read`, and stop the stream with `mx_logs_stop`. Cursor gaps mean retained entries were overwritten; reproduce the event instead of assuming the remaining tail is complete.

Use `mx_status` to confirm simulator state and `mx_sessions` to inspect persistent bindings. After reconnecting an MCP client, call `mx_use_session` with the exact session ID. Mx rejects input if another client replaced the binding or if accessibility reports a different foreground PID.

Pair behavioral claims with a semantic assertion and, when visually relevant, a screenshot. Logs alone do not prove UI state, and screenshots alone do not prove behavior.
