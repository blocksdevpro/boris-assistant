//! Ephemeral tool images (screen captures, image files).
//!
//! Chat-completions tool messages are text-only on many providers, so an
//! image a tool attaches is delivered as a following user-role message with
//! a text label and an `image_url` data-URL part. The message is never
//! persisted, is replaced by `[image omitted]` text at the next human turn
//! (or when the model rejects images), and token estimates count a fixed
//! per-image cost instead of the base64 length.

use serde_json::{json, Value};

use super::{Context, Message, MessageOrigin, Role};
use crate::tool_context::ImageAttachment;

/// Rough provider cost of one image at ≤ 1568 px on the long side (OpenAI
/// high detail ≈ 1.4k tokens, Anthropic ≈ 1.8k).
pub(crate) const IMAGE_TOKEN_ESTIMATE: usize = 1_600;

impl Context {
    /// Append an attached image after the resolved tool batch.
    pub(crate) fn push_tool_image(&mut self, image: &ImageAttachment) {
        let label = format!(
            "Image from {} (call {}): {}",
            image.tool_name, image.call_id, image.caption
        );
        self.messages.push(Message::with_origin(
            Role::User,
            MessageOrigin::ToolImage,
            json!([
                {"type": "text", "text": label},
                {"type": "image_url", "image_url": {
                    "url": format!("data:{};base64,{}", image.mime, image.data_base64)
                }}
            ]),
        ));
        self.prune();
    }

    /// True when an image (not yet omitted) is in the model view.
    pub(crate) fn has_tool_images(&self) -> bool {
        self.messages
            .iter()
            .any(|message| message.origin == MessageOrigin::ToolImage && message.content.is_array())
    }

    /// Replace image parts with a short text note; returns how many changed.
    pub(crate) fn omit_tool_images(&mut self, reason: &str) -> usize {
        let mut omitted = 0;
        for message in &mut self.messages {
            if message.origin != MessageOrigin::ToolImage || !message.content.is_array() {
                continue;
            }
            let label = message
                .content
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|part| part.get("text").and_then(Value::as_str))
                .unwrap_or("image")
                .to_string();
            message.content = Value::String(format!("[image omitted: {reason}] {label}"));
            omitted += 1;
        }
        omitted
    }
}

/// Blank image data URLs in serialized wire messages; returns how many images.
pub(crate) fn redact_image_data(messages: &mut Value) -> usize {
    let mut images = 0;
    for message in messages.as_array_mut().into_iter().flatten() {
        for part in message
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if part.get("type").and_then(Value::as_str) != Some("image_url") {
                continue;
            }
            if let Some(url) = part.get_mut("image_url").and_then(|i| i.get_mut("url")) {
                *url = Value::String(String::new());
            }
            images += 1;
        }
    }
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(data: String) -> ImageAttachment {
        ImageAttachment {
            call_id: "c1".into(),
            tool_name: "screen_capture".into(),
            mime: "image/png".into(),
            data_base64: data,
            caption: "primary monitor, 1568x882".into(),
        }
    }

    fn tool_turn(ctx: &mut Context) {
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "what's on my screen?");
        ctx.push(
            Role::Assistant,
            json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1",
                "type": "function", "function": {"name": "screen_capture", "arguments": "{}"}}]}),
        );
        ctx.push_tool_result(
            "screen_capture",
            "c1",
            "Captured the primary monitor. The image follows.".into(),
            true,
        );
    }

    #[test]
    fn image_follows_the_tool_result_and_is_omitted_next_turn() {
        let mut ctx = Context::new(u32::MAX);
        tool_turn(&mut ctx);
        ctx.push_tool_image(&attachment("QUJD".into()));

        let wire = ctx.as_json();
        let messages = wire.as_array().unwrap();
        let image_index = messages
            .iter()
            .position(|m| m["content"].is_array())
            .expect("image message");
        assert_eq!(messages[image_index - 1]["role"], "tool");
        assert_eq!(messages[image_index]["role"], "user");
        assert_eq!(
            messages[image_index]["content"][1]["image_url"]["url"],
            "data:image/png;base64,QUJD"
        );
        assert!(ctx.has_tool_images());
        // Never persisted.
        assert!(!ctx
            .history()
            .iter()
            .any(|m| m.content.to_string().contains("base64")));

        ctx.push(Role::Assistant, "You have a terminal open.");
        ctx.push(Role::User, "thanks");
        assert!(!ctx.has_tool_images());
        let wire = ctx.as_json().to_string();
        assert!(!wire.contains("base64") && wire.contains("[image omitted"));
    }

    #[test]
    fn estimates_count_images_by_a_fixed_cost_not_base64_length() {
        let mut ctx = Context::new(u32::MAX);
        tool_turn(&mut ctx);
        let before = ctx.estimate_request_tokens(&Value::Null);
        ctx.push_tool_image(&attachment("A".repeat(2_000_000)));
        let added = ctx.estimate_request_tokens(&Value::Null) - before;
        assert!(
            (IMAGE_TOKEN_ESTIMATE..IMAGE_TOKEN_ESTIMATE + 200).contains(&added),
            "{added}"
        );
    }

    #[test]
    fn rejected_images_become_text_notes() {
        let mut ctx = Context::new(u32::MAX);
        tool_turn(&mut ctx);
        ctx.push_tool_image(&attachment("QUJD".into()));
        assert_eq!(ctx.omit_tool_images("this model does not accept images"), 1);
        let wire = ctx.as_json().to_string();
        assert!(wire.contains("this model does not accept images"));
        assert!(wire.contains("primary monitor"));
        assert!(!wire.contains("QUJD"));
    }
}
