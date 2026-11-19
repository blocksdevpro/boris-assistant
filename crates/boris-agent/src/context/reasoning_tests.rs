//! Raw provider reasoning blocks: kept across the tool rounds of the current
//! human turn, sent back unmodified and in order, stripped afterwards.

use serde_json::{json, Value};

use super::{Context, Role};

fn tool_call_turn(ctx: &mut Context, call_id: &str, details: Value) {
    ctx.push(
        Role::Assistant,
        json!({
            "role": "assistant",
            "content": "",
            "reasoning_details": details,
            "tool_calls": [{"id": call_id, "type": "function",
                "function": {"name": "file_read", "arguments": "{}"}}]
        }),
    );
    ctx.push_tool_result("file_read", call_id, "contents".into(), true);
}

fn wire_details(ctx: &Context) -> Vec<Value> {
    ctx.as_json()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("reasoning_details").cloned())
        .collect()
}

#[test]
fn current_turn_reasoning_is_sent_back_unmodified_and_in_order() {
    let mut ctx = Context::new(20);
    ctx.push(Role::System, "sys");
    ctx.push(Role::User, "read two files");
    let first = json!([
        {"type": "reasoning.text", "text": "Read a first.", "signature": "s1", "index": 0},
        {"type": "reasoning.encrypted", "data": "BLOB1", "index": 1}
    ]);
    let second = json!([{"type": "reasoning.encrypted", "data": "BLOB2", "index": 0}]);
    tool_call_turn(&mut ctx, "c1", first.clone());
    tool_call_turn(&mut ctx, "c2", second.clone());
    assert_eq!(wire_details(&ctx), vec![first, second]);
}

#[test]
fn next_human_turn_strips_reasoning_from_context_and_history() {
    let mut ctx = Context::new(20);
    ctx.push(Role::System, "sys");
    ctx.push(Role::User, "read a file");
    tool_call_turn(
        &mut ctx,
        "c1",
        json!([{"type": "reasoning.encrypted", "data": "BLOB"}]),
    );
    ctx.push(Role::Assistant, "Done.");
    ctx.push(Role::User, "thanks");
    assert!(wire_details(&ctx).is_empty());
    assert!(ctx
        .history()
        .iter()
        .all(|m| m.content.get("reasoning_details").is_none()));
    // Tool calls themselves survive; only the reasoning blob goes.
    assert!(ctx
        .as_json()
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m.get("tool_calls").is_some()));

    // Replacement turns end the scope the same way.
    tool_call_turn(
        &mut ctx,
        "c2",
        json!([{"type": "reasoning.encrypted", "data": "BLOB"}]),
    );
    ctx.push_human_with_control("<turn_replacement>x</turn_replacement>", "new ask");
    assert!(wire_details(&ctx).is_empty());
}

#[test]
fn reloaded_history_never_carries_reasoning() {
    let mut ctx = Context::new(20);
    ctx.push(Role::System, "sys");
    ctx.push(Role::User, "read a file");
    tool_call_turn(
        &mut ctx,
        "c1",
        json!([{"type": "reasoning.encrypted", "data": "BLOB"}]),
    );
    let history = ctx.history().to_vec();
    let mut restored = Context::new(20);
    restored.load_history("sys", history);
    assert!(wire_details(&restored).is_empty());
}

#[test]
fn encrypted_blobs_do_not_inflate_token_estimates_or_digests() {
    let mut ctx = Context::new(20);
    ctx.push(Role::System, "sys");
    ctx.push(Role::User, "read a file");
    let baseline = ctx.estimate_request_tokens(&Value::Null);
    tool_call_turn(
        &mut ctx,
        "c1",
        json!([{"type": "reasoning.encrypted", "data": "Q".repeat(400_000)}]),
    );
    let with_blob = ctx.estimate_request_tokens(&Value::Null);
    assert!(
        with_blob - baseline < 500,
        "a 400 KB encrypted blob must not count as ~100k tokens (got {})",
        with_blob - baseline
    );
    // The wire payload still carries the blob unmodified.
    assert_eq!(
        wire_details(&ctx)[0][0]["data"].as_str().unwrap().len(),
        400_000
    );

    // A compaction digest never includes reasoning blocks.
    for i in 0..6 {
        ctx.push(Role::Assistant, format!("answer {i}"));
        ctx.push(Role::User, format!("question {i}"));
    }
    let digest = ctx.older_turns_digest(2);
    assert!(digest.contains("file_read"));
    assert!(!digest.contains("reasoning_details"));
}
