# Performance and fleet work

Measure before tuning. Separate cold build/boot, warm launch, semantic transition, screenshot capture, and agent-transfer time. Use `scripts/benchmark_flow.py` for repeated warm journey samples. Report the median and every raw sample; five repetitions are a development signal, while twenty or more are appropriate for a demo gate.

Use `mx_capacity`, `mx_top`, and `mx_metrics` before adding concurrent simulators. Build once and use `mx_launch` on compatible devices. Distinct device sessions may run concurrently; operations on one device stay serialized.

Use `mx_create`, `mx_clone`, `mx_boot`, `mx_shutdown`, `mx_delete`, and `mx_fleet` only for requested fleet work. Creation consumes substantial disk space. Deletion requires an owned, shut-down simulator and is irreversible.

Capability profiles are experimental and restricted to Mx-owned iOS 26.5 devices. Inspect `mx_profile_catalog` and `mx_profile_status` before planning a change. Retain every app capability the workload needs. Applying or restoring a profile reboots the simulator.

The focused phase reduced the five-run median for a three-screen warm journey from 6.16 seconds to 2.25 seconds. The first 20-run gate, which reinstalled the app before every journey, measured a 4.01-second median. The final install-free `mx_relaunch` gate measured a 2.63-second journey median, 3.57-second p95, and 235 ms relaunch median. Direct AXe reads impose the current floor. Read `benchmarks/README.md` for the comparison and remaining work.
