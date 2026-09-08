//! Strip RTF markup from mod readme files; plain text passes through.

use crate::compat::latin1;

pub fn to_plain_text(bytes: &[u8]) -> String {
    let content = latin1::decode(bytes);

    if !content.trim_start().starts_with("{\\rt") {
        return content;
    }

    let chars: Vec<char> = content.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    let mut skip_depth: Option<usize> = None;
    let mut group_depth = 0usize;

    while index < chars.len() {
        match chars[index] {
            '{' => {
                group_depth += 1;
                index += 1;
            }
            '}' => {
                group_depth = group_depth.saturating_sub(1);
                if let Some(depth) = skip_depth {
                    if group_depth < depth {
                        skip_depth = None;
                    }
                }
                index += 1;
            }
            '\\' => {
                let (token, argument, next) = read_control_word(&chars, index);
                index = next;

                if skip_depth.is_some() {
                    continue;
                }

                match token.as_str() {
                    // Paragraph and line breaks become real line breaks.
                    "par" | "line" | "pard" if (token != "pard" || !out.ends_with('\n')) => {
                        out.push('\n');
                    }
                    "tab" => out.push('\t'),
                    // A hex escape names one literal byte.
                    "'" => {
                        if let Some(value) = argument {
                            out.push(value as u8 as char);
                        }
                    }
                    // Literal punctuation that would otherwise start markup.
                    "\\" => out.push('\\'),
                    "{" => out.push('{'),
                    "}" => out.push('}'),
                    // Groups whose contents are metadata, not body text.
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "generator" | "pict"
                    | "object" | "themedata" | "datastore" | "listtable" | "listoverridetable" => {
                        skip_depth = Some(group_depth);
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {
                // Line breaks inside the markup are formatting, not content.
                index += 1;
            }
            character => {
                if skip_depth.is_none() {
                    out.push(character);
                }
                index += 1;
            }
        }
    }

    tidy(&out)
}

/// Read one control word starting at a backslash.
///
/// Returns the word, its numeric argument when it has one, and where to resume.
fn read_control_word(chars: &[char], start: usize) -> (String, Option<i32>, usize) {
    let mut index = start + 1;

    if index >= chars.len() {
        return (String::new(), None, index);
    }

    // A hex escape: \'xx names one byte.
    if chars[index] == '\'' {
        let hex: String = chars.iter().skip(index + 1).take(2).collect();
        let value = i32::from_str_radix(&hex, 16).ok();
        return ("'".to_string(), value, index + 3);
    }

    // An escaped punctuation character rather than a word.
    if !chars[index].is_ascii_alphabetic() {
        let token = chars[index].to_string();
        return (token, None, index + 1);
    }

    let mut word = String::new();
    while index < chars.len() && chars[index].is_ascii_alphabetic() {
        word.push(chars[index]);
        index += 1;
    }

    let mut number = String::new();
    if index < chars.len() && (chars[index] == '-' || chars[index].is_ascii_digit()) {
        while index < chars.len() && (chars[index] == '-' || chars[index].is_ascii_digit()) {
            number.push(chars[index]);
            index += 1;
        }
    }

    // A single trailing space belongs to the control word, not the text.
    if index < chars.len() && chars[index] == ' ' {
        index += 1;
    }

    (word, number.parse().ok(), index)
}

/// Collapse the runs of blank lines that stripping markup leaves behind.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank_run = 0usize;

    for line in text.lines() {
        let trimmed = line.trim_end();

        if trimmed.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }

        out.push_str(trimmed);
        out.push('\n');
    }

    out.trim_start_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through() {
        let text = "Just a readme.\nSecond line.\n";
        assert_eq!(to_plain_text(text.as_bytes()), text);
    }

    #[test]
    fn markup_is_stripped_and_words_kept() {
        let rtf = br"{\rtf1\ansi\deff0 Hello there.\par General Kenobi.\par}";
        let text = to_plain_text(rtf);

        assert!(text.contains("Hello there."));
        assert!(text.contains("General Kenobi."));
        assert!(!text.contains("rtf1"));
        assert!(!text.contains('\\'));
    }

    #[test]
    fn font_and_colour_tables_are_dropped() {
        let rtf = br"{\rtf1{\fonttbl{\f0\froman Times New Roman;}}{\colortbl ;\red0\green0\blue0;}Body text.\par}";
        let text = to_plain_text(rtf);

        assert!(text.contains("Body text."));
        assert!(!text.contains("Times New Roman"));
        assert!(!text.contains("red0"));
    }

    #[test]
    fn paragraph_breaks_become_line_breaks() {
        let rtf = br"{\rtf1 One\par Two\par Three\par}";
        let text = to_plain_text(rtf);
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();

        assert_eq!(lines, vec!["One", "Two", "Three"]);
    }

    #[test]
    fn escaped_characters_are_restored() {
        let rtf = br"{\rtf1 Braces \{ and \} and a backslash \\ stay.\par}";
        let text = to_plain_text(rtf);

        assert!(text.contains('{'));
        assert!(text.contains('}'));
        assert!(text.contains('\\'));
    }

    #[test]
    fn hex_escapes_become_their_characters() {
        // \'e9 is an accented e.
        let rtf = br"{\rtf1 caf\'e9 time\par}";
        let text = to_plain_text(rtf);

        assert!(text.starts_with("caf"));
        assert_eq!(latin1::encode(&text)[3], 0xE9);
    }

    #[test]
    fn tabs_are_kept() {
        let rtf = br"{\rtf1 Name\tab Value\par}";
        assert!(to_plain_text(rtf).contains('\t'));
    }

    #[test]
    fn runs_of_blank_lines_are_collapsed() {
        let rtf = br"{\rtf1 One\par\par\par\par Two\par}";
        let text = to_plain_text(rtf);

        assert!(!text.contains("\n\n\n"));
    }

    #[test]
    fn an_empty_document_yields_nothing() {
        assert_eq!(to_plain_text(br"{\rtf1}").trim(), "");
        assert_eq!(to_plain_text(b""), "");
    }
}
