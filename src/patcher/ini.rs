//! INI reading with the semantics of Python's `configparser` as PyKotor
//! configures it: `delimiters=("=",)`, `allow_no_value=True`, `strict=False`,
//! `interpolation=None`, full-line `#`/`;` comments, indented continuation
//! lines, a `[DEFAULT]` section merged into every other section, later
//! duplicate keys replacing earlier ones, and duplicate section headers
//! merging into the first.
//!
//! Also carries [`CaseInsensitiveDict`], a port of PyKotor's
//! `pykotor.common.misc.CaseInsensitiveDict`, which every reader loop uses.

use std::collections::HashMap;

use super::error::{PResult, PatchException};

/// The key transform applied to option names, like `ConfigParser.optionxform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionXform {
    /// Keep the spelling (PyKotor's `changes.ini` reader).
    Identity,
    /// Lowercase (PyKotor's `namespaces.ini` reader).
    Lower,
}

/// One section: option names in file order with their values.
///
/// A value of `None` is a key written without `=` (`allow_no_value`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    name: String,
    options: Vec<(String, Option<String>)>,
}

impl Section {
    /// The section's header text exactly as written between the brackets.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Options in file order, as `SectionProxy.items()` yields them
    /// (`DEFAULT` entries are already merged in by the parser).
    pub fn items(&self) -> &[(String, Option<String>)] {
        &self.options
    }

    /// Option names in file order (`SectionProxy.keys()`).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.options.iter().map(|(key, _)| key.as_str())
    }

    /// Values in file order (`SectionProxy.values()`).
    pub fn values(&self) -> impl Iterator<Item = Option<&str>> {
        self.options.iter().map(|(_, value)| value.as_deref())
    }

    /// True when the section holds no options.
    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    /// How many options the section holds.
    pub fn len(&self) -> usize {
        self.options.len()
    }

    /// Exact-spelling option lookup (`SectionProxy[key]`).
    pub fn get(&self, key: &str) -> Option<Option<&str>> {
        self.options
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_deref())
    }

    fn set(&mut self, key: String, value: Option<String>) {
        if let Some(slot) = self.options.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.options.push((key, value));
        }
    }
}

/// A parsed INI document.
#[derive(Debug, Clone, Default)]
pub struct ConfigParser {
    sections: Vec<Section>,
    defaults: Section,
}

impl ConfigParser {
    /// Parse text the way `ConfigParser.read_string` does.
    ///
    /// Fails with `MissingSectionHeaderError` when content precedes the first
    /// header, and with `ParsingError` when a line is neither a header, an
    /// option, a comment, a blank line nor a continuation.
    pub fn read_string(text: &str, xform: OptionXform) -> PResult<Self> {
        let mut parser = Self::default();
        let mut errors: Vec<(usize, String)> = Vec::new();
        // Index into `sections`, or `None` for DEFAULT / no section yet.
        let mut cursect: Option<Option<usize>> = None;
        let mut optname: Option<String> = None;
        let mut indent_level: usize = 0;

        for (lineno, raw_line) in text.split_inclusive('\n').enumerate() {
            let line = raw_line.trim_end_matches(['\n', '\r']);
            let lineno = lineno + 1;

            // Full-line comments only (`inline_comment_prefixes=None`).
            let stripped = line.trim();
            let is_comment = stripped.starts_with('#') || stripped.starts_with(';');
            let value = if is_comment { "" } else { stripped };

            if value.is_empty() {
                // Blank lines are kept inside multi-line values.
                if !is_comment {
                    if let (Some(section), Some(name)) = (cursect, optname.as_ref()) {
                        let target = parser.section_slot(section);
                        if let Some(slot) = target.options.iter_mut().find(|(k, _)| k == name) {
                            if let Some(existing) = slot.1.as_mut() {
                                existing.push('\n');
                            }
                        }
                    }
                }
                continue;
            }

            let cur_indent_level = line.len() - line.trim_start().len();
            if let (Some(section), Some(name)) = (cursect, optname.as_ref()) {
                if cur_indent_level > indent_level {
                    // Continuation of the previous value.
                    let target = parser.section_slot(section);
                    if let Some(slot) = target.options.iter_mut().find(|(k, _)| k == name) {
                        if let Some(existing) = slot.1.as_mut() {
                            existing.push('\n');
                            existing.push_str(value);
                            continue;
                        }
                    }
                }
            }

            indent_level = cur_indent_level;
            if let Some(header) = section_header(value) {
                if header == "DEFAULT" {
                    cursect = Some(None);
                } else if let Some(index) = parser.sections.iter().position(|s| s.name == header) {
                    cursect = Some(Some(index));
                } else {
                    parser.sections.push(Section {
                        name: header.to_string(),
                        options: Vec::new(),
                    });
                    cursect = Some(Some(parser.sections.len() - 1));
                }
                optname = None;
                continue;
            }

            let Some(section) = cursect else {
                return Err(PatchException::new(
                    "MissingSectionHeaderError",
                    format!("File contains no section headers.\nfile: '<string>', line: {lineno}\n{raw_line:?}"),
                ));
            };

            match split_option(value) {
                Some((name, option_value)) => {
                    let name = match xform {
                        OptionXform::Identity => name.to_string(),
                        OptionXform::Lower => name.to_lowercase(),
                    };
                    let option_value = option_value.map(|v| v.trim().to_string());
                    parser.section_slot(section).set(name.clone(), option_value);
                    optname = Some(name);
                }
                None => errors.push((lineno, raw_line.to_string())),
            }
        }

        if let Some((lineno, line)) = errors.first() {
            return Err(PatchException::new(
                "ParsingError",
                format!(
                    "Source contains parsing errors: '<string>'\n\t[line {lineno:2}]: {line:?}"
                ),
            ));
        }

        // `_join_multiline_values`: trailing whitespace of joined values is dropped.
        for section in parser
            .sections
            .iter_mut()
            .chain(std::iter::once(&mut parser.defaults))
        {
            for (_, value) in section.options.iter_mut() {
                if let Some(text) = value.as_mut() {
                    let trimmed = text.trim_end().to_string();
                    *text = trimmed;
                }
            }
        }

        // Merge DEFAULT into every section, as `SectionProxy` does on access.
        let defaults = parser.defaults.options.clone();
        for section in parser.sections.iter_mut() {
            for (key, value) in &defaults {
                if !section.options.iter().any(|(k, _)| k == key) {
                    section.options.push((key.clone(), value.clone()));
                }
            }
        }

        Ok(parser)
    }

    fn section_slot(&mut self, section: Option<usize>) -> &mut Section {
        match section {
            Some(index) => &mut self.sections[index],
            None => &mut self.defaults,
        }
    }

    /// Section names in file order, excluding `DEFAULT` (`ConfigParser.sections()`).
    pub fn sections(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|s| s.name.as_str())
    }

    /// The section with exactly this name (`ConfigParser[name]`).
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }
}

/// `\[(?P<header>.+)\]` matched against the stripped line: text between the
/// first `[` and the last `]`, untrimmed; anything after the last `]` is ignored.
fn section_header(value: &str) -> Option<&str> {
    let rest = value.strip_prefix('[')?;
    let end = rest.rfind(']')?;
    let header = &rest[..end];
    if header.is_empty() {
        None
    } else {
        Some(header)
    }
}

/// `(?P<option>.*?)\s*(?:(?P<vi>=)\s*(?P<value>.*))?$` — the option is the
/// text before the first `=`, right-stripped; no `=` means no value.
fn split_option(value: &str) -> Option<(&str, Option<&str>)> {
    match value.find('=') {
        Some(index) => {
            let name = value[..index].trim_end();
            if name.is_empty() {
                return None;
            }
            Some((name, Some(value[index + 1..].trim_start())))
        }
        None => Some((value.trim_end(), None)),
    }
}

/// Port of PyKotor's `CaseInsensitiveDict[str]`: lookups ignore case, the
/// spelling of keys is kept, insertion order is preserved, and setting an
/// existing key removes the old entry first (so the key moves to the end).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaseInsensitiveDict {
    entries: Vec<(String, String)>,
    case_map: HashMap<String, usize>,
}

impl CaseInsensitiveDict {
    /// An empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from a section's items, as `CaseInsensitiveDict(self.ini[section])`.
    ///
    /// Bare keys (`allow_no_value`) become empty strings; every reader treats
    /// a `None` value as `""` or skips it, and the callers that need to tell
    /// the two apart use [`Section`] directly.
    pub fn from_section(section: &Section) -> Self {
        let mut dict = Self::new();
        for (key, value) in section.items() {
            dict.insert(key.clone(), value.clone().unwrap_or_default());
        }
        dict
    }

    /// Build from key/value pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut dict = Self::new();
        for (key, value) in pairs {
            dict.insert(key.into(), value.into());
        }
        dict
    }

    fn rebuild_map(&mut self) {
        self.case_map.clear();
        for (index, (key, _)) in self.entries.iter().enumerate() {
            self.case_map.insert(key.to_lowercase(), index);
        }
    }

    /// `dict[key] = value`: replaces any same-cased key, moving it to the end.
    pub fn insert(&mut self, key: String, value: String) {
        if let Some(&index) = self.case_map.get(&key.to_lowercase()) {
            self.entries.remove(index);
            self.rebuild_map();
        }
        self.case_map.insert(key.to_lowercase(), self.entries.len());
        self.entries.push((key, value));
    }

    /// `key in dict`.
    pub fn contains(&self, key: &str) -> bool {
        self.case_map.contains_key(&key.to_lowercase())
    }

    /// `dict.get(key)`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.case_map
            .get(&key.to_lowercase())
            .map(|&index| self.entries[index].1.as_str())
    }

    /// `dict[key]`, raising `KeyError` when absent.
    pub fn index(&self, key: &str) -> PResult<&str> {
        self.get(key)
            .ok_or_else(|| PatchException::new("KeyError", format!("{key:?}")))
    }

    /// `dict.pop(key, default)`: removes and returns the value when present.
    pub fn pop(&mut self, key: &str) -> Option<String> {
        let index = *self.case_map.get(&key.to_lowercase())?;
        let (_, value) = self.entries.remove(index);
        self.rebuild_map();
        Some(value)
    }

    /// `dict.pop(key, default)` with a string default.
    pub fn pop_or(&mut self, key: &str, default: &str) -> String {
        self.pop(key).unwrap_or_else(|| default.to_string())
    }

    /// `dict.items()` in insertion order.
    pub fn items(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// `dict.keys()` in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    /// `len(dict)`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `not dict`.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The first key in insertion order, as `next(iter(dict), "")`.
    pub fn first_key(&self) -> Option<&str> {
        self.entries.first().map(|(k, _)| k.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ConfigParser {
        ConfigParser::read_string(text, OptionXform::Identity).unwrap()
    }

    #[test]
    fn comments_blank_lines_and_case_are_handled_like_configparser() {
        let ini = parse("# top\n[Settings]\n; c\nWindowCaption = Hi There\nBare\n# x=y\n");
        let section = ini.section("Settings").unwrap();
        assert_eq!(
            section.items(),
            &[
                ("WindowCaption".to_string(), Some("Hi There".to_string())),
                ("Bare".to_string(), None)
            ]
        );
    }

    #[test]
    fn later_duplicate_keys_replace_earlier_ones_in_place() {
        let ini = parse("[S]\na=1\nb=2\na=3\n");
        let section = ini.section("S").unwrap();
        assert_eq!(section.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(section.get("a"), Some(Some("3")));
    }

    #[test]
    fn duplicate_section_headers_merge_and_case_variants_stay_distinct() {
        let ini = parse("[S]\na=1\n[T]\nx=1\n[S]\nb=2\n[s]\nc=3\n");
        assert_eq!(ini.sections().collect::<Vec<_>>(), vec!["S", "T", "s"]);
        assert_eq!(
            ini.section("S").unwrap().keys().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn continuation_lines_join_with_newlines() {
        let ini = parse("[S]\ntext=first\n  second\n\n  third\nnext=1\n");
        let section = ini.section("S").unwrap();
        assert_eq!(section.get("text"), Some(Some("first\nsecond\n\nthird")));
        assert_eq!(section.get("next"), Some(Some("1")));
    }

    #[test]
    fn quotes_are_kept_and_headers_are_untrimmed() {
        let ini = parse("[ Foo ]\nName=\"Quoted\"\n[Bar]trailing\nk=v\n");
        assert!(ini.section(" Foo ").is_some());
        assert_eq!(
            ini.section(" Foo ").unwrap().get("Name"),
            Some(Some("\"Quoted\""))
        );
        assert!(ini.section("Bar").is_some());
    }

    #[test]
    fn default_section_is_merged_into_every_section() {
        let ini = parse("[DEFAULT]\nz=9\n[S]\na=1\n");
        assert_eq!(ini.sections().collect::<Vec<_>>(), vec!["S"]);
        assert_eq!(ini.section("S").unwrap().get("z"), Some(Some("9")));
    }

    #[test]
    fn content_before_a_header_is_an_error() {
        let err = ConfigParser::read_string("a=1\n[S]\n", OptionXform::Identity).unwrap_err();
        assert_eq!(err.kind, "MissingSectionHeaderError");
    }

    #[test]
    fn lower_xform_folds_option_names() {
        let ini =
            ConfigParser::read_string("[Namespaces]\nNameSpace1=x\n", OptionXform::Lower).unwrap();
        assert_eq!(
            ini.section("Namespaces").unwrap().get("namespace1"),
            Some(Some("x"))
        );
    }

    #[test]
    fn case_insensitive_dict_matches_pykotor_semantics() {
        let mut dict = CaseInsensitiveDict::from_pairs([("Alpha", "1"), ("beta", "2")]);
        assert!(dict.contains("ALPHA"));
        assert_eq!(dict.get("BETA"), Some("2"));
        dict.insert("ALPHA".into(), "3".into());
        assert_eq!(dict.keys().collect::<Vec<_>>(), vec!["beta", "ALPHA"]);
        assert_eq!(dict.pop("alpha"), Some("3".into()));
        assert_eq!(dict.pop("alpha"), None);
        assert_eq!(dict.pop_or("missing", "d"), "d");
        assert_eq!(dict.len(), 1);
    }
}
