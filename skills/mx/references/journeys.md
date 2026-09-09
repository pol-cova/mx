# Journey knowledge

Use `mx_capture_flow` for two or more meaningful screens. It replaces repeated agent round trips with one ordered plan, verifies each destination semantically, captures whole-screen PNGs, and writes two complementary artifacts:

- `flow.json` is the agent knowledge base. It contains state names, parent relationships, transition labels, accessibility elements, stable references, values, screenshot paths, and timing data.
- `flow.html` is the audience-facing map. It lays out screen cards and labeled connections on a navigable canvas.

Every state should have a concise filename-safe `name` and a stable `expect_label`. Use `from` and `transition` to describe branching routes, modals, tabs, back navigation, or deep links. The parent must appear earlier in the plan. Actions use accessibility identifiers or exact labels.

Read `flow.json` before opening every PNG. It is enough for navigation reasoning, test planning, implementation review, and control discovery. Open PNGs when spatial layout, styling, clipping, or visual comparison matters. Share `flow.html` when a person needs to understand the application journey.

A declared plan is safer than automatic crawling. Do not activate purchases, account deletion, external messages, or other consequential controls while exploring. Automatic graph discovery needs explicit action policies and a reset strategy.

For the complete schema and example, read `examples/demo-flow.json` and `src/flow.rs` in the Mx repository.
