//! Split assistant replies into short spoken units for reliable on-device TTS.
//!
//! Supertonic (and similar small models) drop middle clauses when given long
//! multi-sentence monologues in one forward pass. We force sentence-level
//! (and long-clause) units ourselves.

/// Soft per-unit char budget. Prefer complete sentences under this length.
pub const PREFERRED_UNIT_CHARS: usize = 180;

/// Split reply text into short, complete spoken units.
///
/// 1. Split on sentence-ending punctuation (`.?!`) when followed by space/end.
/// 2. Further split very long units on commas / semicolons.
pub fn speakable_units(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut sentences = split_sentences(text);
    if sentences.is_empty() {
        sentences.push(text.to_string());
    }

    let mut units = Vec::new();
    for sentence in sentences {
        units.extend(split_long_unit(&sentence, PREFERRED_UNIT_CHARS));
    }
    units
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        current.push(c);
        if matches!(c, '.' | '!' | '?') {
            let boundary = i + 1 >= chars.len() || chars[i + 1].is_whitespace();
            if boundary {
                let unit = current.trim().to_string();
                if !unit.is_empty() {
                    sentences.push(unit);
                }
                current.clear();
                while i + 1 < chars.len() && chars[i + 1].is_whitespace() {
                    i += 1;
                }
            }
        }
        i += 1;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }
    sentences
}

fn split_long_unit(text: &str, max_chars: usize) -> Vec<String> {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return vec![text.to_string()];
    }

    let tokens: Vec<&str> = text
        .split_inclusive(|c: char| matches!(c, ',' | ';' | ':'))
        .collect();

    if tokens.len() <= 1 {
        return pack_words(text, max_chars);
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;

    for token in tokens {
        let token_chars = token.chars().count();
        if current_chars > 0 && current_chars + token_chars > max_chars {
            let unit = current.trim().to_string();
            if !unit.is_empty() {
                parts.push(unit);
            }
            current.clear();
            current_chars = 0;
        }
        current.push_str(token);
        current_chars += token_chars;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        return vec![text.to_string()];
    }

    // Any leftover mega-clause → word pack.
    parts
        .into_iter()
        .flat_map(|p| {
            if p.chars().count() > max_chars {
                pack_words(&p, max_chars)
            } else {
                vec![p]
            }
        })
        .collect()
}

fn pack_words(text: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if !current.is_empty() && next_len > max_chars {
            parts.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        vec![text.to_string()]
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakable_units_splits_sentences() {
        let units = speakable_units(
            "The web search is empty right now. Try me again in a bit, or pick a city.",
        );
        assert_eq!(units.len(), 2);
        assert!(units[0].ends_with('.'));
        assert!(units[1].ends_with('.'));
    }

    #[test]
    fn speakable_units_keeps_short_reply() {
        let units = speakable_units("Phone stuff is easy.");
        assert_eq!(units, vec!["Phone stuff is easy.".to_string()]);
    }

    #[test]
    fn speakable_units_splits_long_clause_on_commas() {
        let long = "Okay bro, real talk: every search engine and job site is slamming the door on me with robot checks, so I can't pull real live Jharkhand job listings and I will not invent a fake list of openings for you today.";
        let units = speakable_units(long);
        assert!(
            units.len() >= 2,
            "expected multi-unit split, got {:?}",
            units
        );
        assert!(units
            .iter()
            .all(|u| u.chars().count() <= PREFERRED_UNIT_CHARS + 40));
    }

    #[test]
    fn empty_and_whitespace() {
        assert!(speakable_units("").is_empty());
        assert!(speakable_units("   ").is_empty());
    }
}
