//! YAML frontmatter parse/strip for `SKILL.md` files (no full YAML dependency).

/// Strip YAML frontmatter delimited by `---` lines; return the body.
pub fn strip_frontmatter(content: &str) -> &str {
    if !content.starts_with("---") {
        return content;
    }
    if let Some(end) = content[3..].find("\n---") {
        let after = end + 3 + 4; // past "\n---"
        if after < content.len() {
            content[after..].trim_start_matches(['\r', '\n'])
        } else {
            ""
        }
    } else {
        content
    }
}

/// Parse `name` + `description` from frontmatter (single-line or `>` folded).
pub(super) fn parse_frontmatter(content: &str) -> Option<(String, String)> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("\n---")?;
    let yaml_block = &rest[..end];

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut folding_desc = false;
    let mut desc_buf = String::new();

    for line in yaml_block.lines() {
        if folding_desc {
            // Folded description continues on indented lines.
            if line.starts_with(' ') || line.starts_with('\t') {
                let t = line.trim();
                if !t.is_empty() {
                    if !desc_buf.is_empty() {
                        desc_buf.push(' ');
                    }
                    desc_buf.push_str(t);
                }
                continue;
            }
            folding_desc = false;
            description = Some(desc_buf.clone());
            // fall through to parse this line as a new key
        }

        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(unquote(val.trim()));
        } else if let Some(val) = line.strip_prefix("description:") {
            let val = val.trim();
            if val == ">" || val == "|" || val.is_empty() {
                folding_desc = true;
                desc_buf.clear();
            } else {
                description = Some(unquote(val));
            }
        }
    }
    if folding_desc && !desc_buf.is_empty() {
        description = Some(desc_buf);
    }

    match (name, description) {
        (Some(n), Some(d)) if !n.is_empty() && !d.is_empty() => Some((n, d)),
        _ => None,
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Validate skill name: lowercase alphanumeric + hyphens, 1–64 chars.
pub fn is_valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_and_strip() {
        let content = "---\nname: my-skill\ndescription: Does things\n---\n# Hello\nworld";
        let (n, d) = parse_frontmatter(content).unwrap();
        assert_eq!(n, "my-skill");
        assert_eq!(d, "Does things");
        assert_eq!(strip_frontmatter(content), "# Hello\nworld");
    }

    #[test]
    fn folded_description() {
        let content = "---\nname: x\ndescription: >\n  Line one\n  line two\n---\nbody";
        let (n, d) = parse_frontmatter(content).unwrap();
        assert_eq!(n, "x");
        assert!(d.contains("Line one"));
        assert!(d.contains("line two"));
    }

    #[test]
    fn valid_names() {
        assert!(is_valid_name("get-things-done"));
        assert!(!is_valid_name("Bad"));
        assert!(!is_valid_name("-x"));
        assert!(!is_valid_name("a--b"));
    }
}
