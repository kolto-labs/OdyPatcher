//! Port of the parts of `pykotor/tools/encoding.py` the patcher uses.
//!
//! Without `charset_normalizer` installed, `decode_bytes_with_fallbacks`
//! decodes as UTF-8 (strict) and the caller falls back to
//! `bytes.decode(errors="ignore")`. The optional detector is not modelled.

use super::error::{PResult, PatchException};

/// `decode_bytes_with_fallbacks(bytes)` with strict UTF-8.
pub fn decode_bytes_with_fallbacks(bytes: &[u8]) -> PResult<String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).map_err(|err| {
        PatchException::new(
            "UnicodeDecodeError",
            format!("'utf-8' codec can't decode bytes: {}", err.utf8_error()),
        )
    })
}

/// `bytes.decode(errors="ignore")` for UTF-8: invalid sequences are dropped.
pub fn decode_utf8_ignoring_errors(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(err) => {
                let (valid, after) = rest.split_at(err.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                let skip = err.error_len().unwrap_or(after.len()).max(1);
                rest = &after[skip.min(after.len())..];
                if rest.is_empty() {
                    break;
                }
            }
        }
    }
    out
}

/// Decode an instruction file: strict UTF-8 first. A file that is not valid
/// UTF-8 was most likely saved by a Windows editor, so it is read as
/// Windows-1252 and keeps its accented letters (TSLPatcher reads them the
/// same way) instead of losing them.
///
/// Returns the text and whether the fallback was needed (the caller warns).
pub fn decode_ini_text(bytes: &[u8]) -> (String, bool) {
    match decode_bytes_with_fallbacks(bytes) {
        Ok(text) => (text, false),
        Err(_) => (crate::patcher::mods::nss::decode_windows_1252(bytes), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_is_strict_and_the_ini_fallback_reads_windows_1252() {
        assert_eq!(
            decode_bytes_with_fallbacks("caf\u{e9}".as_bytes()).unwrap(),
            "caf\u{e9}"
        );
        assert!(decode_bytes_with_fallbacks(b"caf\xe9").is_err());
        assert_eq!(decode_utf8_ignoring_errors(b"caf\xe9!"), "caf!");
        assert_eq!(
            decode_ini_text(b"caf\xe9!"),
            ("caf\u{e9}!".to_string(), true)
        );
        assert_eq!(
            decode_ini_text(b"\xEF\xBB\xBF[S]"),
            ("[S]".to_string(), false)
        );
    }
}
