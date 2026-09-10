# Mx agent guidance

Use `skills/mx/SKILL.md` when the user invokes `/mx` or asks for general Mx work. It routes to focused knowledge without loading the whole manual.

The narrower skills remain available when a request matches only one operation:

- `skills/mx-quick-screenshot/SKILL.md` for one-off simulator screenshots.
- `skills/mx-capture-flow/SKILL.md` for named screenshot sequences, bug evidence, or before-and-after captures.

Read the selected file before acting. Do not load both for a simple capture.

Prefer Mx semantic UI tools over coordinates or generic computer control. Screenshots are the fallback for missing semantics and the proof artifact for visual tasks.
