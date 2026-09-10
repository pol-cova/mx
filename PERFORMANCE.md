# Performance analysis

## Finding

Mx's main latency cost is semantic inspection, not Rust code or screenshot capture. A verified action currently needs multiple accessibility-tree reads. Each read starts a new AXe process, and the settling loop requires two identical trees with a 150 ms interval before it accepts the state.

The checked-in 20-run benchmark reports a 2.63 s median for the complete three-state flow. Semantic transitions account for 1.397 s of that result; three screenshots account for 512.5 ms. A separate ten-run check on the active simulator on September 10, 2026 measured `mx ui` at 0.21-0.22 s after a 0.80 s first call. At that rate, the two-read settling rule alone has a floor of roughly 0.57 s before action and session overhead.

## Recommended order

1. **Keep a persistent accessibility bridge.** Reuse the AXe connection instead of launching `describe-ui` for every observation. Instrument fetch count and elapsed time first, then compare 30 or more warm runs. AXe's accessibility fetcher performs simulator setup and transient-result retries on each fetch, so this is the highest-leverage boundary to remove.
2. **Batch safe action sequences.** Extend the batching already used by capture flows to ordinary actions. Identifier taps and text entry can share one AXe invocation, followed by one full semantic verification. Keep per-step cache invalidation across navigation.
3. **Remove redundant web inspections.** Browser tap, swipe, and type currently run a full `describe-ui` only to validate the foreground PID, then launch a second process for the input. Move PID validation and input delivery into the same serialized bridge request.
4. **Tune settling after the bridge exists.** The 50 ms and 150 ms polling intervals are not the present bottleneck when one tree fetch costs hundreds of milliseconds. With a persistent bridge, benchmark adaptive polling or accessibility-change notifications while preserving the final full-tree and foreground-app checks.
5. **Benchmark screenshot backends separately.** Screenshots are about one fifth of the measured flow. Compare AXe PNG capture with `simctl io screenshot`; keep the MJPEG stream for live preview unless its JPEG frames meet the required evidence quality.

## Lower-priority work

- Parse AX JSON once instead of deserializing the same response twice.
- Replace JSON-serialized diff keys and repeated screen clones with typed borrowed keys.
- Measure session-file `sync_all` before weakening its durability guarantee.
- Replace the MJPEG parser's repeated byte scan and `Vec::drain` only if CPU profiles show it matters at higher frame rates.
- Cache process metadata in metrics collection, while keeping the current implementation as a compatibility fallback.

These changes should be treated as hypotheses until the same workload is re-run with raw p50, p95, and maximum values. The current results support the bottleneck diagnosis; they do not yet quantify the gain from a persistent bridge.

## Sources

- [Mx benchmark data and methodology](benchmarks/README.md)
- [AXe accessibility fetcher](https://github.com/cameroncooke/AXe/blob/30f4bfa9bc81817906a60fadedbc913d7314b7e1/Sources/AXe/Utilities/AccessibilityFetcher.swift)
- [AXe guidance on batch versus discrete commands](https://github.com/cameroncooke/AXe/blob/30f4bfa9bc81817906a60fadedbc913d7314b7e1/Sources/AXe/Resources/skills/axe/SKILL.md#step-5-batch-vs-discrete-commands)
- [AXe batch context implementation](https://github.com/cameroncooke/AXe/blob/30f4bfa9bc81817906a60fadedbc913d7314b7e1/Sources/AXe/Utilities/Batch/BatchContext.swift)
- [Apple Simulator screenshot command](https://developer.apple.com/library/archive/documentation/IDEs/Conceptual/iOS_Simulator_Guide/InteractingwiththeiOSSimulator/InteractingwiththeiOSSimulator.html)
