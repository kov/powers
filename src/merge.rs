//! Merge streamed assistant rows into logical turns.
//!
//! Claude writes one assistant turn as several JSONL rows that share the same
//! `message.id` (one row per streamed block: thinking, text, each tool_use, …).
//! Left as-is they appear as many separate messages, inflating counts ~2-3x and
//! fragmenting a single turn across unrelated indices. The [`Merger`] collapses
//! consecutive rows sharing a message id into one message and assigns the final
//! 0-based logical indices.
//!
//! Same-id rows are always contiguous in the file, so a single-slot lookahead is
//! enough. Non-Claude tools pass `None` message ids and are never merged.

use crate::session::{ContentPart, Message, MessageContent, Role};

struct Pending {
    msg: Message,
    uuid: Option<String>,
    message_id: Option<String>,
}

/// Collapses consecutive assistant rows sharing a `message.id` into one message.
pub struct Merger {
    pending: Option<Pending>,
    next_index: usize,
}

impl Merger {
    pub fn new() -> Self {
        Merger {
            pending: None,
            next_index: 0,
        }
    }

    /// Feed one parsed row (`message_id` is the provider id for assistant rows,
    /// `None` otherwise). Returns the previous logical message if this row starts
    /// a new one; returns `None` while a turn is still accumulating.
    pub fn push(
        &mut self,
        msg: Message,
        uuid: Option<String>,
        message_id: Option<String>,
    ) -> Option<(Message, Option<String>)> {
        if let Some(p) = self.pending.as_mut() {
            let continues = msg.role == Role::Assistant
                && p.msg.role == Role::Assistant
                && message_id.is_some()
                && message_id == p.message_id;
            if continues {
                append_content(&mut p.msg.content, msg.content);
                return None;
            }
        }
        let finished = self.take_pending();
        self.pending = Some(Pending {
            msg,
            uuid,
            message_id,
        });
        finished
    }

    /// Flush the final pending message (call once after the last row).
    pub fn finish(&mut self) -> Option<(Message, Option<String>)> {
        self.take_pending()
    }

    fn take_pending(&mut self) -> Option<(Message, Option<String>)> {
        let p = self.pending.take()?;
        let mut msg = p.msg;
        msg.index = self.next_index;
        self.next_index += 1;
        Some((msg, p.uuid))
    }
}

/// Append `src`'s content blocks onto `dst`, promoting to `Parts` as needed.
fn append_content(dst: &mut MessageContent, src: MessageContent) {
    let src_parts = match src {
        MessageContent::Parts(p) => p,
        MessageContent::Text(t) if !t.trim().is_empty() => vec![ContentPart::Text(t)],
        MessageContent::Text(_) => return,
    };
    if src_parts.is_empty() {
        return;
    }
    match dst {
        MessageContent::Parts(d) => d.extend(src_parts),
        MessageContent::Text(t) => {
            let mut v = Vec::new();
            if !t.trim().is_empty() {
                v.push(ContentPart::Text(std::mem::take(t)));
            }
            v.extend(src_parts);
            *dst = MessageContent::Parts(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asst(id: &str, text: &str) -> (Message, Option<String>, Option<String>) {
        (
            Message {
                index: 0,
                role: Role::Assistant,
                content: MessageContent::Parts(vec![ContentPart::Text(text.to_string())]),
                timestamp: None,
            },
            Some(format!("uuid-{text}")),
            Some(id.to_string()),
        )
    }
    fn user(text: &str) -> (Message, Option<String>, Option<String>) {
        (
            Message {
                index: 0,
                role: Role::User,
                content: MessageContent::Text(text.to_string()),
                timestamp: None,
            },
            Some(format!("uuid-{text}")),
            None,
        )
    }

    fn drain(
        rows: Vec<(Message, Option<String>, Option<String>)>,
    ) -> Vec<(Message, Option<String>)> {
        let mut m = Merger::new();
        let mut out = Vec::new();
        for (msg, uuid, mid) in rows {
            if let Some(f) = m.push(msg, uuid, mid) {
                out.push(f);
            }
        }
        if let Some(f) = m.finish() {
            out.push(f);
        }
        out
    }

    #[test]
    fn merges_consecutive_same_id_assistant_rows() {
        let out = drain(vec![
            user("hi"),
            asst("msg_1", "a"),
            asst("msg_1", "b"),
            asst("msg_1", "c"),
            user("next"),
            asst("msg_2", "d"),
        ]);
        // user, merged(a+b+c), user, d  => 4 logical messages with 0..3 indices
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].0.index, 0);
        assert_eq!(out[1].0.index, 1);
        assert_eq!(out[2].0.index, 2);
        assert_eq!(out[3].0.index, 3);
        // merged message keeps the first row's uuid and carries all three blocks
        assert_eq!(out[1].1.as_deref(), Some("uuid-a"));
        match &out[1].0.content {
            MessageContent::Parts(p) => assert_eq!(p.len(), 3),
            _ => panic!("expected merged parts"),
        }
    }

    #[test]
    fn does_not_merge_different_ids_or_none() {
        let out = drain(vec![asst("msg_1", "a"), asst("msg_2", "b")]);
        assert_eq!(out.len(), 2);
        // Two None-id rows (e.g. non-Claude) never merge.
        let out2 = drain(vec![user("a"), user("b")]);
        assert_eq!(out2.len(), 2);
    }
}
