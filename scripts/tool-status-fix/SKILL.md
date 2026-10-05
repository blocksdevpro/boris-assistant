# Tool status migration

This change preserves tool success and failure from execution to the agent's
events, task state, and model routing. It is the first standalone optimization
fix. Do not implement other optimization work before the user reviews it.

## Contract

- Keep `Tool::execute` returning `Result<String, ToolError>`. A failed operation
  must return a typed error, retaining useful diagnostic output in its message.
- Carry `ToolObservation` in `InvokeResult::Observation`. Read its status before
  rendering provider text; never infer runtime success from the text body.
- Preserve error kind and use recovery guidance appropriate to that kind.
- Store trusted success metadata with internal tool history. Keep it out of
  provider messages and preserve it through session serialization. Routing and
  post-tool guidance should use host status rather than failure-related words
  found in successful file contents or logs.
- Apply the same contract to sequential, parallel, approval-resume, typed-input,
  denied, and unknown-tool paths. Preserve ordering and approval boundaries.
- Preserve the existing completion policy, research quotas, output truncation,
  model choices, deadlines, and speech behavior for subsequent fixes.

## Ownership

- Root owns runtime invocation, observation/error rendering, loop handling,
  reminders, session serialization, eval fixtures, and this file.
- Tool adapter delegate owns shell execution, grep, and MCP result parsing.
- Routing delegate owns internal context status, routing, completion options,
  and the per-round request wiring.
- Do not edit another owner's files without coordinating first.

## Review

Run tests only after the user explicitly authorizes them. Until then, regression
test source may be added or adapted, and compile-only checks and formatting are
permitted. Once authorized, run the affected crates' regression tests. Leave live
tests ignored unless separately requested. Review the actual source and diff.
Root will review the combined change and stop before any commit or next
optimization.
