//! Keep-raw policy for user-attached images in durable conversation history.
//!
//! After a turn that attached images, older `ContentPart::Image` entries are
//! replaced with their already-rendered description text (or a short
//! placeholder). Token estimates then count description bytes/4 and charge
//! [`supercharge_token_estimation::IMAGE_TOKEN_ESTIMATE`] only for remaining
//! raw image parts.

use std::collections::HashSet;

use supercharge_sampling_types::{ContentPart, ConversationItem};

/// How many user-attached images stay as raw `ContentPart::Image` parts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepRawImages {
    /// Keep every image on the latest user turn that attached images; if that
    /// turn is no longer the last user item, keep only the single most recently
    /// attached image.
    #[default]
    Latest,
    /// Leave every user image part in place.
    All,
    /// Replace every user image part with description text or a placeholder.
    None,
}

/// Stamped when an older image cannot be described and has no envelope in text.
pub const IMAGE_DESCRIPTION_UNAVAILABLE: &str = "[image: description unavailable]";

/// Result of applying [`apply_keep_raw_user_images`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeepRawRewriteStats {
    /// Image parts replaced with text (or dropped because a description envelope
    /// was already present).
    pub replaced: usize,
    /// Image parts still present after the rewrite.
    pub remaining_raw: usize,
}

/// True when any text part already carries an `<image_description>` envelope.
pub fn user_item_has_image_description(parts: &[ContentPart]) -> bool {
    parts.iter().any(|part| match part {
        ContentPart::Text { text } => text.contains("<image_description>"),
        ContentPart::Image { .. } => false,
    })
}

fn last_user_index(conversation: &[ConversationItem]) -> Option<usize> {
    conversation
        .iter()
        .enumerate()
        .rev()
        .find(|(_, item)| matches!(item, ConversationItem::User(_)))
        .map(|(idx, _)| idx)
}

fn user_image_locations(conversation: &[ConversationItem]) -> Vec<(usize, usize)> {
    let mut locations = Vec::new();
    for (item_idx, item) in conversation.iter().enumerate() {
        let ConversationItem::User(user) = item else {
            continue;
        };
        for (part_idx, part) in user.content.iter().enumerate() {
            if matches!(part, ContentPart::Image { .. }) {
                locations.push((item_idx, part_idx));
            }
        }
    }
    locations
}

fn keep_set(conversation: &[ConversationItem], keep: KeepRawImages) -> HashSet<(usize, usize)> {
    let locations = user_image_locations(conversation);
    match keep {
        KeepRawImages::All => locations.into_iter().collect(),
        KeepRawImages::None => HashSet::new(),
        KeepRawImages::Latest => {
            let Some(last_loc) = locations.last().copied() else {
                return HashSet::new();
            };
            let last_user = last_user_index(conversation);
            if last_user == Some(last_loc.0) {
                locations
                    .into_iter()
                    .filter(|(item_idx, _)| *item_idx == last_loc.0)
                    .collect()
            } else {
                HashSet::from([last_loc])
            }
        }
    }
}

/// Replace older user image parts according to `keep`. Item count and order are
/// unchanged. Returns how many image parts were replaced and how many remain.
pub fn apply_keep_raw_user_images(
    conversation: &mut [ConversationItem],
    keep: KeepRawImages,
) -> KeepRawRewriteStats {
    if matches!(keep, KeepRawImages::All) {
        let remaining_raw = user_image_locations(conversation).len();
        return KeepRawRewriteStats {
            replaced: 0,
            remaining_raw,
        };
    }
    let keep_locations = keep_set(conversation, keep);
    let mut replaced = 0;
    let mut remaining_raw = 0;
    for (item_idx, item) in conversation.iter_mut().enumerate() {
        let ConversationItem::User(user) = item else {
            continue;
        };
        let has_description = user_item_has_image_description(&user.content);
        let mut next = Vec::with_capacity(user.content.len());
        for (part_idx, part) in user.content.iter().cloned().enumerate() {
            match part {
                ContentPart::Image { .. } if keep_locations.contains(&(item_idx, part_idx)) => {
                    remaining_raw += 1;
                    next.push(part);
                }
                ContentPart::Image { .. } => {
                    replaced += 1;
                    if !has_description {
                        next.push(ContentPart::Text {
                            text: std::sync::Arc::<str>::from(IMAGE_DESCRIPTION_UNAVAILABLE),
                        });
                    }
                }
                other => next.push(other),
            }
        }
        user.content = next;
    }
    KeepRawRewriteStats {
        replaced,
        remaining_raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use supercharge_sampling_types::conversation::UserItem;
    use supercharge_token_estimation::{IMAGE_TOKEN_ESTIMATE, estimate_tokens};

    fn img(url: &str) -> ContentPart {
        ContentPart::Image { url: url.into() }
    }

    fn text(s: &str) -> ContentPart {
        ContentPart::Text { text: s.into() }
    }

    fn user_with(parts: Vec<ContentPart>) -> ConversationItem {
        ConversationItem::User(UserItem {
            content: parts,
            ..Default::default()
        })
    }

    fn raw_urls(item: &ConversationItem) -> Vec<&str> {
        match item {
            ConversationItem::User(u) => u
                .content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Image { url } => Some(url.as_ref()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn at<'a>(conv: &'a [ConversationItem], i: usize) -> &'a ConversationItem {
        conv.get(i).expect("index")
    }

    #[test]
    fn latest_keeps_all_images_on_current_turn() {
        let mut conv = vec![user_with(vec![
            text("look"),
            img("data:image/png;base64,AAA"),
            img("data:image/png;base64,BBB"),
        ])];
        let stats = apply_keep_raw_user_images(&mut conv, KeepRawImages::Latest);
        assert_eq!(stats.replaced, 0);
        assert_eq!(stats.remaining_raw, 2);
        assert_eq!(raw_urls(at(&conv, 0)).len(), 2);
    }

    #[test]
    fn latest_strips_older_turn_when_new_image_arrives() {
        let desc = "<image_description>\nold shot\n</image_description>";
        let mut conv = vec![
            user_with(vec![text(desc), img("data:image/png;base64,OLD")]),
            ConversationItem::assistant("ok"),
            user_with(vec![text("newer"), img("data:image/png;base64,NEW")]),
        ];
        let stats = apply_keep_raw_user_images(&mut conv, KeepRawImages::Latest);
        assert_eq!(stats.replaced, 1);
        assert_eq!(stats.remaining_raw, 1);
        assert!(raw_urls(at(&conv, 0)).is_empty());
        assert_eq!(raw_urls(at(&conv, 2)), vec!["data:image/png;base64,NEW"]);
        assert!(at(&conv, 0).text_content().contains("old shot"));
    }

    #[test]
    fn latest_keeps_only_last_image_after_text_followup() {
        let mut conv = vec![
            user_with(vec![
                text("<image_description>\na\n</image_description>"),
                img("data:image/png;base64,A"),
                img("data:image/png;base64,B"),
            ]),
            ConversationItem::assistant("ok"),
            user_with(vec![text("what about the second one")]),
        ];
        let stats = apply_keep_raw_user_images(&mut conv, KeepRawImages::Latest);
        assert_eq!(stats.replaced, 1);
        assert_eq!(stats.remaining_raw, 1);
        assert_eq!(raw_urls(at(&conv, 0)), vec!["data:image/png;base64,B"]);
    }

    #[test]
    fn keep_all_leaves_both_raw() {
        let mut conv = vec![
            user_with(vec![img("data:image/png;base64,A")]),
            user_with(vec![img("data:image/png;base64,B")]),
        ];
        let stats = apply_keep_raw_user_images(&mut conv, KeepRawImages::All);
        assert_eq!(stats.replaced, 0);
        assert_eq!(stats.remaining_raw, 2);
        assert_eq!(raw_urls(at(&conv, 0)).len(), 1);
        assert_eq!(raw_urls(at(&conv, 1)).len(), 1);
    }

    #[test]
    fn keep_none_strips_all_and_uses_placeholder_without_description() {
        let mut conv = vec![user_with(vec![text("see"), img("data:image/png;base64,A")])];
        let stats = apply_keep_raw_user_images(&mut conv, KeepRawImages::None);
        assert_eq!(stats.replaced, 1);
        assert_eq!(stats.remaining_raw, 0);
        assert!(
            at(&conv, 0)
                .text_content()
                .contains(IMAGE_DESCRIPTION_UNAVAILABLE)
        );
        assert!(raw_urls(at(&conv, 0)).is_empty());
    }

    #[test]
    fn token_estimate_drops_image_charge_for_replaced_parts() {
        let desc = "<image_description>\na described screenshot of a terminal\n</image_description>";
        let mut conv = vec![
            user_with(vec![
                text(desc),
                img("data:image/png;base64,OLD"),
            ]),
            user_with(vec![text("next"), img("data:image/png;base64,NEW")]),
        ];
        let before = crate::estimate_conversation_tokens(&conv);
        apply_keep_raw_user_images(&mut conv, KeepRawImages::Latest);
        let after = crate::estimate_conversation_tokens(&conv);
        assert!(before >= IMAGE_TOKEN_ESTIMATE * 2);
        assert_eq!(
            after,
            estimate_tokens(desc)
                + estimate_tokens("next")
                + IMAGE_TOKEN_ESTIMATE
        );
        assert!(after < before);
    }
}
