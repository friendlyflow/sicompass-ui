//! Checks for the help texts: the rules a beginner's manual is written to
//! (`docs/manual-guidelines.md`), as code an embedder's tests can call on its
//! own locale files. The application's manual is checked in this crate's tests,
//! the greeter's in loginsicompass's.
//!
//! A locale file is read the simple way the manual is written: a message is a
//! line `key = value` at the start of a line, and an indented line under it
//! carries the value on. Comments (`#`) and attributes (`.name =`) are skipped.

/// The messages of a Fluent file, in order, as `(key, value)`. A value that runs
/// over several lines is joined with spaces.
pub fn messages(ftl: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in ftl.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if indented {
            let more = line.trim();
            if more.starts_with('.') {
                continue; // an attribute
            }
            if let Some((_, value)) = out.last_mut() {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(more);
            }
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                out.push((key.to_owned(), value.trim().to_owned()));
            }
        }
    }
    out
}

/// What a line of help text may not hold: an em dash or a semicolon. A screen
/// reader turns punctuation into pauses, and these two read oddly. Commas, or
/// two sentences, say the same thing. `None` when the text is fine.
pub fn prose_fault(text: &str) -> Option<&'static str> {
    if text.contains('\u{2014}') {
        Some("an em dash")
    } else if text.contains(';') {
        Some("a semicolon")
    } else {
        None
    }
}

/// Check a manual in one locale file: the keys `<prefix>-01` to
/// `<prefix>-<lines>` are all there, none is empty, there is no line past the
/// last, and none breaks the prose rule ([`prose_fault`]). The error lists
/// every fault found.
pub fn check_manual(ftl: &str, prefix: &str, lines: usize) -> Result<(), String> {
    let msgs = messages(ftl);
    let mut faults: Vec<String> = Vec::new();
    let head = format!("{prefix}-");
    let numbered: Vec<(usize, &str, &str)> = msgs
        .iter()
        .filter_map(|(k, v)| {
            let n = k.strip_prefix(&head)?;
            (n.len() == 2 && n.chars().all(|c| c.is_ascii_digit()))
                .then(|| (n.parse::<usize>().ok().unwrap_or(0), k.as_str(), v.as_str()))
        })
        .collect();
    for n in 1..=lines {
        match numbered.iter().find(|(m, _, _)| *m == n) {
            None => faults.push(format!("{prefix}-{n:02} is missing")),
            Some((_, key, value)) => {
                if value.is_empty() {
                    faults.push(format!("{key} is empty"));
                }
                if let Some(what) = prose_fault(value) {
                    faults.push(format!("{key} holds {what}"));
                }
            }
        }
    }
    for (n, key, _) in &numbered {
        if *n == 0 || *n > lines {
            faults.push(format!(
                "{key} is past the last line ({lines}): raise the line count or remove it"
            ));
        }
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [(&str, &str); 4] = [
        ("en-US", include_str!("../locales/en-US.ftl")),
        ("nl-BE", include_str!("../locales/nl-BE.ftl")),
        ("fr-BE", include_str!("../locales/fr-BE.ftl")),
        ("de-BE", include_str!("../locales/de-BE.ftl")),
    ];

    #[test]
    fn messages_join_continuation_lines_and_skip_comments_and_attributes() {
        let ftl = "# a comment\nkey-a = one\n    two\nkey-b = three\n    .attr = no\n";
        assert_eq!(
            messages(ftl),
            vec![
                ("key-a".to_owned(), "one two".to_owned()),
                ("key-b".to_owned(), "three".to_owned()),
            ]
        );
    }

    #[test]
    fn a_dash_or_a_semicolon_is_a_prose_fault_and_a_comma_is_not() {
        assert_eq!(prose_fault("Press Tab, then Down."), None);
        assert_eq!(
            prose_fault("Press Tab \u{2014} then Down."),
            Some("an em dash")
        );
        assert_eq!(prose_fault("Press Tab; then Down."), Some("a semicolon"));
    }

    #[test]
    fn check_manual_finds_a_gap_an_extra_line_and_a_fault() {
        let ftl = "m-01 = One.\nm-03 = Three; four.\nm-04 = Past the end.\n";
        let err = check_manual(ftl, "m", 3).unwrap_err();
        assert!(err.contains("m-02 is missing"), "{err}");
        assert!(err.contains("m-03 holds a semicolon"), "{err}");
        assert!(err.contains("m-04 is past the last line"), "{err}");
        assert!(check_manual("m-01 = One.\nm-02 = Two.\n", "m", 2).is_ok());
    }

    /// The application's manual is what its line count says, in every language, and
    /// follows the prose rule. Raise `MANUAL_LINES` with a new line, in all four files.
    #[test]
    fn the_apps_manual_follows_the_rules_in_every_locale_file() {
        for (locale, ftl) in LOCALES {
            if let Err(faults) =
                check_manual(ftl, crate::list::MANUAL_PREFIX, crate::list::MANUAL_LINES)
            {
                panic!("{locale}.ftl:\n{faults}");
            }
        }
    }
}
