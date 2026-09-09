---
name: mx-demo
description: Prepare, run, or assess the Mx v0.1 technical demo. Use when a user asks whether Mx is demo-ready or wants the checked-in iOS demo built, exercised, and captured.
---

# Run the Mx demo

Read [the demo guide](../../README.md#try-it) before acting. Treat its environment and scope limits as part of the result.

For a readiness assessment, run the offline Rust tests, Clippy with warnings denied, and rustfmt. Then use `scripts/demo.py` with the release binary, a unique booted simulator UDID, and a new screenshot path. Do not claim live readiness if the simulator run was skipped.

For a presentation, use the checked-in `examples/MxDemo` app. Show semantic inspection and identifier-based interaction before taking the final screenshot. Report the JSON evidence path, screenshot path, elapsed time, and any check that did not run.

Do not silently apply the experimental slim profile, create simulators, or delete simulator data. Those actions have separate capacity and ownership considerations.
