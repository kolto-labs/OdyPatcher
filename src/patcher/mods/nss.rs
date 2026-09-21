//! Port of `pykotor/tslpatcher/mods/nss.py` (the `[CompileList]`).
//!
//! A script's `#2DAMEMORY#` / `#StrRef#` tokens are replaced in memory, the
//! processed text is written into the workspace the installer prepared (see
//! [`workspace`]), and the script is compiled: with the mod's
//! `nwnnsscomp.exe` on Windows when one ships (SHA-256 checked, any failure
//! falls back), otherwise with the built-in compiler (`bionsscomp`).
//!
//! `bionsscomp` needs a `nwscript.nss` header, which PyKotor's compiler
//! embeds. One is looked for in the workspace (a copy shipped in a source
//! folder), in the mod folders the reader recorded, then beside this program
//! (`nwscript.k1.nss` / `nwscript.k2.nss` / `nwscript.nss`). No header is a
//! compile failure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "nss-compiler")]
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(feature = "nss-compiler")]
use super::super::casepath::case_sensitive_path;
use super::super::casepath::safe_isfile;
use super::super::config::Game;
use super::super::encoding::decode_bytes_with_fallbacks;
use super::super::error::{PResult, PatchException};
use super::super::logger::PatchLogger;
use super::super::memory::PatcherMemory;
use super::template::{ModBase, PatchOutput, PatcherModifications};

/// `EntryPointError` message PyKotor's compilers raise for an include file.
pub const ENTRY_POINT_ERROR: &str =
    "This file has no entry point and cannot be compiled (Most likely an include file).";

/// The stdout marker `nwnnsscomp.exe` prints for an include file.
const INCLUDE_FILE_MARKER: &str = "File is an include file, ignored";

/// `ModificationsNSS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModificationsNSS {
    /// Shared attributes (`action = "Compile"`, `skip_if_not_replace = True`).
    pub base: ModBase,
    /// `nwnnsscomp_path`: external compiler, when one ships with the mod.
    pub nwnnsscomp_path: Option<PathBuf>,
    /// `temp_script_folder`: workspace with token-substituted sources.
    pub temp_script_folder: Option<PathBuf>,
    /// `compiler_flags`: `[Settings] ScriptCompilerFlags`.
    pub compiler_flags: String,
    /// Folders searched for a `nwscript.nss` header for the built-in
    /// compiler, after the workspace and before the program's own folder.
    /// The reader fills this with the INI folder and `tslpatchdata`.
    pub nwscript_search_folders: Vec<PathBuf>,
}

impl ModificationsNSS {
    /// `ModificationsNSS(filename, replace)`: `saveas` becomes
    /// `PurePath(filename).with_suffix(".ncs")`.
    pub fn new(filename: &str, replace: Option<bool>) -> Self {
        let mut base = ModBase::new(filename, replace);
        base.saveas = posix_with_suffix(filename, ".ncs").unwrap_or_else(|_| filename.to_string());
        base.action = "Compile".to_string();
        base.skip_if_not_replace = true;
        Self {
            base,
            nwnnsscomp_path: None,
            temp_script_folder: None,
            compiler_flags: String::new(),
            nwscript_search_folders: Vec::new(),
        }
    }

    /// `apply(nss_source, memory, logger, game)`: replace every defined
    /// `#2DAMEMORY#` / `#StrRef#` token in place and warn about the rest.
    pub fn apply(
        &self,
        nss_source: &mut String,
        memory: &PatcherMemory,
        logger: &mut PatchLogger,
        _game: Game,
    ) {
        let memory_2da: HashMap<i64, String> = memory
            .memory_2da
            .iter()
            .map(|(id, value)| (*id as i64, value.as_str().to_string()))
            .collect();
        let memory_str: HashMap<i64, String> = memory
            .memory_str
            .iter()
            .map(|(id, value)| (*id as i64, value.to_string()))
            .collect();
        self.replace_tokens(nss_source, "2DAMEMORY", &memory_2da, logger);
        self.replace_tokens(nss_source, "StrRef", &memory_str, logger);
    }

    fn replace_tokens(
        &self,
        nss_source: &mut String,
        token_name: &str,
        memory_dict: &HashMap<i64, String>,
        logger: &mut PatchLogger,
    ) {
        let highest_token = if token_name == "2DAMEMORY" {
            memory_dict.keys().copied().max().unwrap_or(0)
        } else {
            0
        };
        let mut previous_token: i64 = -1;
        loop {
            let token_id = find_tokens(nss_source, token_name)
                .into_iter()
                .filter(|(digits, id)| *digits == id.to_string())
                .map(|(_, id)| id)
                .filter(|id| *id > previous_token)
                .filter(|id| memory_dict.contains_key(id) || (1..=highest_token).contains(id))
                .min();
            let Some(token_id) = token_id else {
                break;
            };
            previous_token = token_id;
            let token = format!("#{token_name}{token_id}#");
            let replacement_value = memory_dict.get(&token_id).cloned().unwrap_or_default();
            logger.add_verbose(format!(
                "{}: Replacing '{token}' with '{replacement_value}'",
                self.base.sourcefile
            ));
            *nss_source = nss_source.replace(&token, &replacement_value);
        }

        let mut undefined_tokens: Vec<i64> = find_tokens(nss_source, token_name)
            .into_iter()
            .map(|(_, id)| id)
            .filter(|id| !memory_dict.contains_key(id))
            .collect();
        undefined_tokens.sort_unstable();
        undefined_tokens.dedup();
        for token_id in undefined_tokens {
            logger.add_warning(format!(
                "{token_name}{token_id} was not defined before use in '{}'; leaving token unchanged.",
                self.base.sourcefile
            ));
        }
    }

    /// `_split_compiler_flags(flags)`: split Windows compiler arguments
    /// without POSIX backslash escaping.
    pub fn split_compiler_flags(flags: &str) -> PResult<Vec<String>> {
        let chars: Vec<char> = flags.chars().collect();
        let mut arguments: Vec<String> = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            if chars[index] == ' ' || chars[index] == '\t' {
                index += 1;
                continue;
            }
            let mut argument = String::new();
            let mut quoted = false;
            while index < chars.len() && (quoted || (chars[index] != ' ' && chars[index] != '\t')) {
                let mut backslashes = 0;
                while index < chars.len() && chars[index] == '\\' {
                    backslashes += 1;
                    index += 1;
                }
                if index < chars.len() && chars[index] == '"' {
                    argument.extend(std::iter::repeat('\\').take(backslashes / 2));
                    if backslashes % 2 == 1 {
                        argument.push('"');
                    } else if quoted && index + 1 < chars.len() && chars[index + 1] == '"' {
                        argument.push('"');
                        index += 1;
                    } else {
                        quoted = !quoted;
                    }
                    index += 1;
                } else {
                    argument.extend(std::iter::repeat('\\').take(backslashes));
                    if index < chars.len()
                        && (quoted || (chars[index] != ' ' && chars[index] != '\t'))
                    {
                        argument.push(chars[index]);
                        index += 1;
                    }
                }
            }
            if quoted {
                return Err(PatchException::value(
                    "Unclosed double quote in ScriptCompilerFlags.",
                ));
            }
            arguments.push(argument);
        }
        Ok(arguments)
    }

    /// `_compile_with_external(temp_script_file, nwnnsscompiler, logger, game)`.
    ///
    /// `Ok(Skip)` is the include-file case; an `EntryPointError` is returned
    /// as a `PatchException` of that kind so the caller can tell it from
    /// failures that fall back to the built-in compiler.
    fn compile_with_external(
        &self,
        temp_script_file: &Path,
        nwnnsscompiler: &ExternalNCSCompiler,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput> {
        let tempdir = TempDir::new("tmp")?;
        let tempcompiled_filepath = tempdir.path().join("temp_script.ncs");
        let compiler_flags = Self::split_compiler_flags(&self.compiler_flags)?;
        let (stdout, stderr) = nwnnsscompiler.compile_script(
            temp_script_file,
            &tempcompiled_filepath,
            game,
            &compiler_flags,
        )?;
        let result = if stdout.contains(INCLUDE_FILE_MARKER) {
            PatchOutput::Skip
        } else {
            PatchOutput::Bytes(std::fs::read(&tempcompiled_filepath).map_err(|err| {
                PatchException::from_io(&err, Some(&tempcompiled_filepath.to_string_lossy()))
            })?)
        };
        drop(tempdir);

        if !stdout.trim().is_empty() {
            for line in stdout.split('\n') {
                if !line.trim().is_empty() {
                    logger.add_verbose(line.to_string());
                }
            }
        }
        if !stderr.trim().is_empty() {
            for line in stderr.split('\n') {
                if !line.trim().is_empty() {
                    logger.add_error(format!("nwnnsscomp error: {line}"));
                }
            }
        }
        Ok(result)
    }

    /// Compile the processed text with `bionsscomp`, PyKotor's built-in
    /// compiler stand-in.
    fn compile_with_builtin(
        &self,
        source: &str,
        temp_script_folder: &Path,
        source_name: &str,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput> {
        #[cfg(not(feature = "nss-compiler"))]
        {
            let _ = (source, temp_script_folder, source_name, logger, game);
            return Err(PatchException::new(
                "NotImplementedError",
                "built-in NSS compiler unavailable (compiled without the nss-compiler feature)",
            ));
        }
        #[cfg(feature = "nss-compiler")]
        {
            let nwscript = self.load_nwscript(temp_script_folder, game)?;
            let options = bionsscomp::CompileOptions::new(&nwscript)
                .with_source_name(source_name)
                .with_include_path(temp_script_folder);
            match bionsscomp::compile_detailed(source, &options) {
                Ok(compilation) => Ok(PatchOutput::Bytes(compilation.ncs)),
                Err(error) => {
                    let is_include = error
                        .diagnostics()
                        .iter()
                        .any(|diagnostic| diagnostic.message.contains(INCLUDE_FILE_MARKER));
                    if is_include {
                        logger.add_note(ENTRY_POINT_ERROR);
                        return Ok(PatchOutput::Skip);
                    }
                    let kind = match &error {
                        bionsscomp::CompileError::Io { .. } => "OSError",
                        bionsscomp::CompileError::NotImplemented(_) => "NotImplementedError",
                        bionsscomp::CompileError::Diagnostics(_) => "CompileError",
                    };
                    Err(PatchException::new(kind, error.to_string()))
                }
            }
        }
    }

    /// Candidate `nwscript.nss` locations, in order.
    #[cfg(feature = "nss-compiler")]
    fn nwscript_candidates(&self, temp_script_folder: &Path, game: Game) -> Vec<PathBuf> {
        let mut candidates = vec![temp_script_folder.join("nwscript.nss")];
        for folder in &self.nwscript_search_folders {
            candidates.push(case_sensitive_path(&folder.join("nwscript.nss")));
        }
        if let Some(folder) = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        {
            let preferred = match game {
                Game::K1 => "nwscript.k1.nss",
                Game::K2 => "nwscript.k2.nss",
            };
            candidates.push(folder.join(preferred));
            candidates.push(folder.join("nwscript.nss"));
        }
        candidates
    }

    #[cfg(feature = "nss-compiler")]
    fn load_nwscript(
        &self,
        temp_script_folder: &Path,
        game: Game,
    ) -> PResult<Arc<bionsscomp::NwScript>> {
        let bio_game = match game {
            Game::K1 => bionsscomp::Game::K1,
            Game::K2 => bionsscomp::Game::K2,
        };
        let candidates = self.nwscript_candidates(temp_script_folder, game);
        let mut failures: Vec<String> = Vec::new();
        for path in &candidates {
            if !safe_isfile(path) {
                continue;
            }
            let key = (game.number(), path.clone());
            if let Some(cached) = header_cache()
                .lock()
                .ok()
                .and_then(|cache| cache.get(&key).cloned())
            {
                return Ok(cached);
            }
            match bionsscomp::NwScript::load(bio_game, path) {
                Ok(header) => {
                    let header = Arc::new(header);
                    if let Ok(mut cache) = header_cache().lock() {
                        cache.insert(key, header.clone());
                    }
                    return Ok(header);
                }
                Err(err) => failures.push(format!("'{}': {err}", path.display())),
            }
        }
        let mut message = format!(
            "Could not find a 'nwscript.nss' header to compile '{}' with the built-in compiler; looked in: {}",
            self.base.sourcefile,
            candidates
                .iter()
                .map(|path| format!("'{}'", path.display()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if !failures.is_empty() {
            message.push_str(&format!("; unusable headers: {}", failures.join("; ")));
        }
        Err(PatchException::new("CompileError", message))
    }
}

/// Parsed `nwscript.nss` headers, keyed by game and path.
#[cfg(feature = "nss-compiler")]
type HeaderCache = Mutex<HashMap<(i32, PathBuf), Arc<bionsscomp::NwScript>>>;

#[cfg(feature = "nss-compiler")]
fn header_cache() -> &'static HeaderCache {
    static CACHE: OnceLock<HeaderCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Every `#{token_name}([0-9]+)#` match: the digit text and its integer value.
fn find_tokens(text: &str, token_name: &str) -> Vec<(String, i64)> {
    let prefix = format!("#{token_name}");
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&prefix) {
        let after = &rest[start + prefix.len()..];
        let digits_len = after.bytes().take_while(|b| b.is_ascii_digit()).count();
        if digits_len > 0 && after[digits_len..].starts_with('#') {
            let digits = &after[..digits_len];
            // Absurdly long ids cannot be in memory; saturate like Python's unbounded int would compare.
            let value = digits.parse::<i64>().unwrap_or(i64::MAX);
            found.push((digits.to_string(), value));
            rest = &after[digits_len + 1..];
        } else {
            rest = &rest[start + 1..];
        }
    }
    found
}

impl PatcherModifications for ModificationsNSS {
    fn base(&self) -> &ModBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut ModBase {
        &mut self.base
    }

    fn patch_resource(
        &mut self,
        source: &[u8],
        memory: &mut PatcherMemory,
        logger: &mut PatchLogger,
        game: Game,
    ) -> PResult<PatchOutput> {
        let nss_bytes = source;

        // Replace memory tokens in the script, and save to the file.
        let source_text = decode_script_bytes(nss_bytes)?;
        let mut processed = source_text.clone();
        self.apply(&mut processed, memory, logger, game);
        let Some(temp_script_folder) = self.temp_script_folder.clone() else {
            return Err(PatchException::new(
                "RuntimeError",
                "CompileList working directory was not prepared before compilation.",
            ));
        };
        let script_name = windows_name(&self.base.sourcefile).to_lowercase();
        let temp_script_file = temp_script_folder.join(&script_name);

        let processed_bytes = if processed == source_text {
            nss_bytes.to_vec()
        } else {
            encode_windows_1252(&processed)?
        };
        std::fs::write(&temp_script_file, &processed_bytes).map_err(|err| {
            PatchException::from_io(&err, Some(&temp_script_file.to_string_lossy()))
        })?;

        // Compile with external on windows, fall back to built-in if mac/linux or if external fails.
        let is_windows = cfg!(windows);
        let nwnnsscomp_exists = self
            .nwnnsscomp_path
            .as_deref()
            .map(safe_isfile)
            .unwrap_or(false);
        if is_windows && nwnnsscomp_exists {
            let nwnnsscomp_path = self.nwnnsscomp_path.clone().expect("checked above");
            let nwnnsscompiler = ExternalNCSCompiler::new(&nwnnsscomp_path)?;
            let detected_nwnnsscomp = nwnnsscompiler
                .get_info()
                .map(|compiler| compiler.member_name())
                .unwrap_or("<UNKNOWN>");
            if detected_nwnnsscomp != "TSLPATCHER" {
                logger.add_warning(format!(
                    "The nwnnsscomp.exe in the tslpatchdata folder is not the expected TSLPatcher version.\nPyKotor has detected that the provided nwnnsscomp.exe is the '{detected_nwnnsscomp}' version.\nPyKotor will compile regardless, but this may not yield the expected result."
                ));
            }
            match self.compile_with_external(&temp_script_file, &nwnnsscompiler, logger, game) {
                Ok(output) => return Ok(output),
                Err(exc) if exc.kind == "EntryPointError" => {
                    logger.add_note(exc.message);
                    return Ok(PatchOutput::Skip);
                }
                Err(exc) => {
                    logger.add_error(python_tuple_repr(&exc.simplify()));
                }
            }
        }

        if is_windows {
            if !nwnnsscomp_exists {
                logger.add_note("nwnnsscomp.exe was not found in the 'tslpatchdata' folder, using the built-in compilers...");
            } else {
                logger.add_error(format!(
                    "An error occurred while compiling '{}' with nwnnsscomp.exe, falling back to the built-in compilers...",
                    self.base.sourcefile
                ));
            }
        } else {
            logger.add_note(format!(
                "Patching from a unix operating system, compiling '{}' using the built-in compilers...",
                self.base.sourcefile
            ));
        }

        // Compile using built-in script compiler if external compiler fails.
        self.compile_with_builtin(&processed, &temp_script_folder, &script_name, logger, game)
    }

    fn class_name(&self) -> &'static str {
        "ModificationsNSS"
    }
}

/// `decode_bytes_with_fallbacks(nss_bytes)`: strict UTF-8 first; when that
/// fails, windows-1252 stands in for the charset detector PyKotor would use.
fn decode_script_bytes(bytes: &[u8]) -> PResult<String> {
    match decode_bytes_with_fallbacks(bytes) {
        Ok(text) => Ok(text),
        Err(_) => Ok(decode_windows_1252(bytes)),
    }
}

/// `PureWindowsPath(path).name`: the text after the last `\` or `/`.
pub fn windows_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed)
}

/// `PurePath(path).with_suffix(suffix)` (POSIX flavour): only `/` separates.
pub fn posix_with_suffix(path: &str, suffix: &str) -> PResult<String> {
    with_suffix_impl(path, suffix, &['/'], "PurePosixPath")
}

/// `str(PureWindowsPath(path).with_suffix(suffix))`: `/` and `\` separate,
/// and the result is printed with backslashes.
pub fn windows_with_suffix(path: &str, suffix: &str) -> PResult<String> {
    with_suffix_impl(path, suffix, &['\\', '/'], "PureWindowsPath")
        .map(|text| text.replace('/', "\\"))
}

fn with_suffix_impl(
    path: &str,
    suffix: &str,
    separators: &[char],
    class_name: &str,
) -> PResult<String> {
    let split_at = path.rfind(separators).map(|index| index + 1).unwrap_or(0);
    let (parent, name) = path.split_at(split_at);
    if name.is_empty() {
        return Err(PatchException::value(format!(
            "{class_name}({}) has an empty name",
            python_str_repr(path)
        )));
    }
    let old_suffix = match name.rfind('.') {
        Some(index) if index > 0 && index < name.len() - 1 => &name[index..],
        _ => "",
    };
    let stem = &name[..name.len() - old_suffix.len()];
    Ok(format!("{parent}{stem}{suffix}"))
}

/// `repr(text)` for a Python `str`.
pub fn python_str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str((name, message))`: how `str(universal_simplify_exception(e))` prints.
pub fn python_tuple_repr(pair: &(String, String)) -> String {
    format!(
        "({}, {})",
        python_str_repr(&pair.0),
        python_str_repr(&pair.1)
    )
}

/// The windows-1252 code points for bytes `0x80..=0x9F`; `0` marks an
/// undefined byte.
const CP1252_HIGH: [u32; 32] = [
    0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
    0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
];

/// `text.encode("windows-1252")`; an unmappable character is a
/// `UnicodeEncodeError`.
pub fn encode_windows_1252(text: &str) -> PResult<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    for (position, ch) in text.chars().enumerate() {
        let code = ch as u32;
        let byte = if code < 0x80 || (0xA0..=0xFF).contains(&code) {
            Some(code as u8)
        } else {
            CP1252_HIGH
                .iter()
                .position(|&mapped| mapped != 0 && mapped == code)
                .map(|index| 0x80 + index as u8)
        };
        match byte {
            Some(byte) => out.push(byte),
            None => {
                return Err(PatchException::new(
                    "UnicodeEncodeError",
                    format!(
                        "'charmap' codec can't encode character '\\u{code:04x}' in position {position}: character maps to <undefined>"
                    ),
                ))
            }
        }
    }
    Ok(out)
}

/// `bytes.decode("windows-1252")` where undefined bytes decode to their
/// Latin-1 control character (the lenient reading a charset detector gives).
pub fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0x9F => {
                let mapped = CP1252_HIGH[(byte - 0x80) as usize];
                if mapped == 0 {
                    byte as char
                } else {
                    char::from_u32(mapped).unwrap_or(byte as char)
                }
            }
            other => other as char,
        })
        .collect()
}

/// SHA-256 of a byte string, as uppercase hex (`generate_hash(..., "sha256").upper()`).
pub fn sha256_hex_upper(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{word:08X}")).collect()
}

/// `KnownExternalCompilers`: the `nwnnsscomp.exe` builds PyKotor knows by hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownExternalCompilers {
    /// The build TSLPatcher ships.
    TslPatcher,
    /// Fred Tetra's KOTOR Tool build.
    KotorTool,
    /// v1.3, the first public release.
    V1,
    /// James Goad's KOTOR Scripting Tool build.
    KotorScriptingTool,
    /// DeNCS (same hash as TSLPatcher's, never reached).
    DeNcs,
}

impl KnownExternalCompilers {
    /// Every member in declaration order, as `for member in cls` iterates.
    pub const ALL: [KnownExternalCompilers; 5] = [
        KnownExternalCompilers::TslPatcher,
        KnownExternalCompilers::KotorTool,
        KnownExternalCompilers::V1,
        KnownExternalCompilers::KotorScriptingTool,
        KnownExternalCompilers::DeNcs,
    ];

    /// The `sha256` of the executable.
    pub fn sha256(self) -> &'static str {
        match self {
            KnownExternalCompilers::TslPatcher | KnownExternalCompilers::DeNcs => {
                "539EB689D2E0D3751AEED273385865278BEF6696C46BC0CAB116B40C3B2FE820"
            }
            KnownExternalCompilers::KotorTool => {
                "E36AA3172173B654AE20379888EDDC9CF45C62FBEB7AB05061C57B52961C824D"
            }
            KnownExternalCompilers::V1 => {
                "EC3E657C18A32AD13D28DA0AA3A77911B32D9661EA83CF0D9BCE02E1C4D8499D"
            }
            KnownExternalCompilers::KotorScriptingTool => {
                "B7344408A47BE8780816CF68F5A171A09640AB47AD1A905B7F87DE30A50A0A92"
            }
        }
    }

    /// The Python enum member name (`KnownExternalCompilers.X.name`).
    pub fn member_name(self) -> &'static str {
        match self {
            KnownExternalCompilers::TslPatcher => "TSLPATCHER",
            KnownExternalCompilers::KotorTool => "KOTOR_TOOL",
            KnownExternalCompilers::V1 => "V1",
            KnownExternalCompilers::KotorScriptingTool => "KOTOR_SCRIPTING_TOOL",
            KnownExternalCompilers::DeNcs => "DENCS",
        }
    }

    /// `from_sha256(sha256)`; unknown hashes are a `ValueError`.
    pub fn from_sha256(sha256: &str) -> PResult<Self> {
        let uppercase = sha256.to_uppercase();
        Self::ALL
            .iter()
            .copied()
            .find(|member| member.sha256() == uppercase)
            .ok_or_else(|| {
                PatchException::value(format!("No compilers found with sha256 hash '{uppercase}'"))
            })
    }

    /// The `compile` command-line template, with `{source}` etc. placeholders.
    fn compile_commandline(self) -> &'static [&'static str] {
        match self {
            KnownExternalCompilers::TslPatcher => &["-c", "{source}", "-o", "{output}"],
            KnownExternalCompilers::V1 => &["-c", "{source}", "{output}"],
            KnownExternalCompilers::KotorTool | KnownExternalCompilers::KotorScriptingTool => &[
                "-c",
                "--outputdir",
                "{output_dir}",
                "-o",
                "{output_name}",
                "-g",
                "{game_value}",
                "{source}",
            ],
            KnownExternalCompilers::DeNcs => NwnnsscompConfig::DEFAULT_COMMANDLINE,
        }
    }
}

/// `NwnnsscompConfig`: the argument list for one compiler run.
#[derive(Debug, Clone)]
pub struct NwnnsscompConfig {
    /// The executable's hash.
    pub sha256_hash: String,
    /// Absolute source path.
    pub source_file: PathBuf,
    /// Absolute output path.
    pub output_file: PathBuf,
    /// Target game.
    pub game: Game,
    /// The recognised build, if any.
    pub chosen_compiler: Option<KnownExternalCompilers>,
}

impl NwnnsscompConfig {
    /// `DEFAULT_COMMANDLINE["compile"]`.
    pub const DEFAULT_COMMANDLINE: &'static [&'static str] = &["-c", "{source}", "-o", "{output}"];

    /// `NwnnsscompConfig(sha256_hash, sourcefile, outputfile, game)`.
    pub fn new(sha256_hash: &str, source_file: PathBuf, output_file: PathBuf, game: Game) -> Self {
        Self {
            sha256_hash: sha256_hash.to_string(),
            chosen_compiler: KnownExternalCompilers::from_sha256(sha256_hash).ok(),
            source_file,
            output_file,
            game,
        }
    }

    /// `get_compile_args(executable, extra_args)`: `[exe, *extra, *template]`.
    pub fn get_compile_args(&self, executable: &str, extra_args: &[String]) -> Vec<String> {
        let template = self
            .chosen_compiler
            .map(KnownExternalCompilers::compile_commandline)
            .unwrap_or(Self::DEFAULT_COMMANDLINE);
        let output_dir = self
            .output_file
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let output_name = self
            .output_file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut args = vec![executable.to_string()];
        args.extend(extra_args.iter().cloned());
        for arg in template {
            args.push(
                arg.replace("{source}", &self.source_file.to_string_lossy())
                    .replace("{output}", &self.output_file.to_string_lossy())
                    .replace("{output_dir}", &output_dir)
                    .replace("{output_name}", &output_name)
                    .replace("{game_value}", if self.game.is_k1() { "1" } else { "2" }),
            );
        }
        args
    }
}

/// `ExternalNCSCompiler`: runs a `nwnnsscomp.exe`.
#[derive(Debug, Clone)]
pub struct ExternalNCSCompiler {
    /// The executable.
    pub nwnnsscomp_path: PathBuf,
    /// Its uppercase SHA-256.
    pub filehash: String,
}

impl ExternalNCSCompiler {
    /// Compile timeout, seconds (`compile_script(timeout=60)`).
    pub const TIMEOUT_SECS: u64 = 60;

    /// `ExternalNCSCompiler(nwnnsscomp_path)`: hashes the file.
    pub fn new(nwnnsscomp_path: &Path) -> PResult<Self> {
        let bytes = std::fs::read(nwnnsscomp_path).map_err(|err| {
            PatchException::from_io(&err, Some(&nwnnsscomp_path.to_string_lossy()))
        })?;
        Ok(Self {
            nwnnsscomp_path: nwnnsscomp_path.to_path_buf(),
            filehash: sha256_hex_upper(&bytes),
        })
    }

    /// `get_info()`.
    pub fn get_info(&self) -> PResult<KnownExternalCompilers> {
        KnownExternalCompilers::from_sha256(&self.filehash)
    }

    /// `config(source_file, output_file, game)`.
    pub fn config(&self, source_file: &Path, output_file: &Path, game: Game) -> NwnnsscompConfig {
        NwnnsscompConfig::new(
            &self.filehash,
            super::super::casepath::abspath(source_file),
            super::super::casepath::abspath(output_file),
            game,
        )
    }

    /// `compile_script(source_file, output_file, game, extra_args=...)`:
    /// `(stdout, stderr)` on success; `EntryPointError`, `RuntimeError`,
    /// `FileNotFoundError` or `ValueError` otherwise.
    pub fn compile_script(
        &self,
        source_file: &Path,
        output_file: &Path,
        game: Game,
        extra_args: &[String],
    ) -> PResult<(String, String)> {
        let config = self.config(source_file, output_file, game);
        let args = config.get_compile_args(&self.nwnnsscomp_path.to_string_lossy(), extra_args);
        let cwd = self
            .nwnnsscomp_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let result = run_with_timeout(&args, &cwd, Self::TIMEOUT_SECS)?;

        let (stdout, stderr) = Self::get_output(&result);
        if stdout.contains(INCLUDE_FILE_MARKER) {
            return Err(PatchException::new("EntryPointError", ENTRY_POINT_ERROR));
        }

        if result.returncode != 0 {
            let diagnostics = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                format!("return code {}", result.returncode)
            };
            return Err(PatchException::new(
                "RuntimeError",
                format!("External script compiler failed: {diagnostics}"),
            ));
        }

        let output_filepath = &config.output_file;
        if !safe_isfile(output_filepath) {
            return Err(PatchException::new(
                "FileNotFoundError",
                format!(
                    "External script compiler did not create '{}'.",
                    output_filepath.display()
                ),
            ));
        }
        let size = std::fs::metadata(output_filepath)
            .map(|m| m.len())
            .unwrap_or(0);
        if size == 0 {
            return Err(PatchException::value(format!(
                "External script compiler created an empty output file at '{}'.",
                output_filepath.display()
            )));
        }
        Ok((stdout, stderr))
    }

    /// `_get_output(result)`: move `Error:` lines from stdout into stderr.
    fn get_output(result: &CompletedProcess) -> (String, String) {
        let mut stdout = result.stdout.clone();
        let mut stderr = if result.returncode != 0 && result.stderr.trim().is_empty() {
            format!(
                "No error provided, but return code is nonzero: ({})",
                result.returncode
            )
        } else {
            result.stderr.clone()
        };

        if stdout.contains("Error:") {
            let mut error_line = String::new();
            let mut filtered: Vec<&str> = Vec::new();
            for line in stdout.split('\n') {
                if line.contains("Error:") {
                    error_line.push('\n');
                    error_line.push_str(line);
                } else {
                    filtered.push(line);
                }
            }
            stdout = filtered.join("\n");
            if !error_line.is_empty() {
                if !stderr.is_empty() {
                    stderr.push('\n');
                    stderr.push_str(&error_line);
                } else {
                    stderr = error_line;
                }
            }
        }
        (stdout, stderr)
    }
}

/// What `subprocess.run` hands back.
#[derive(Debug, Clone)]
struct CompletedProcess {
    returncode: i32,
    stdout: String,
    stderr: String,
}

/// `subprocess.run(args, capture_output=True, text=True, timeout=..., cwd=...)`.
fn run_with_timeout(args: &[String], cwd: &Path, timeout_secs: u64) -> PResult<CompletedProcess> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let (program, rest) = args.split_first().expect("an executable is always present");
    let mut child = Command::new(program)
        .args(rest)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| PatchException::from_io(&err, Some(program)))?;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stdout_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(pipe) = stderr_pipe.as_mut() {
            let _ = pipe.read_to_end(&mut buffer);
        }
        buffer
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PatchException::new(
                        "TimeoutExpired",
                        format!(
                            "Command '[{}]' timed out after {timeout_secs} seconds",
                            args.iter()
                                .map(|a| python_str_repr(a))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(err) => return Err(PatchException::from_io(&err, Some(program))),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(CompletedProcess {
        returncode: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).replace("\r\n", "\n"),
        stderr: String::from_utf8_lossy(&stderr).replace("\r\n", "\n"),
    })
}

/// `tempfile.TemporaryDirectory(prefix=...)`: a fresh folder under the
/// system temp dir, removed on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a unique folder whose name starts with `prefix`.
    pub fn new(prefix: &str) -> PResult<Self> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::temp_dir();
        for _ in 0..100 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = base.join(format!("{prefix}{}_{nanos:x}_{count}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => {
                    return Err(PatchException::from_io(&err, Some(&path.to_string_lossy())))
                }
            }
        }
        Err(PatchException::new(
            "FileExistsError",
            format!(
                "No usable temporary directory name found in {}",
                base.display()
            ),
        ))
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Port of `ModInstaller._prepare_compilelist` and
/// `ModInstaller._save_processed_scripts` (`pykotor/tslpatcher/patcher.py`),
/// as free functions the installer calls.
///
/// The installer owns the containment helpers (`_resolve_source_folder`,
/// `_resolve_source_file_path`, `_resolve_relative_folder_within`,
/// `_ensure_within_root`); it hands them in as closures so this module never
/// needs a `ModInstaller`.
pub mod workspace {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::super::super::casepath::{safe_isdir, safe_isfile, safe_iterdir};
    use super::super::super::config::Game;
    use super::super::super::error::{PResult, PatchException};
    use super::super::super::logger::PatchLogger;
    use super::super::super::memory::PatcherMemory;
    use super::super::ncs::posix_suffix;
    use super::ModificationsNSS;

    /// `tempfile.TemporaryDirectory(prefix="holopatcher_nss_")`: the prefix
    /// the installer uses for the compile workspace.
    pub const WORKSPACE_PREFIX: &str = "holopatcher_nss_";

    /// `_prepare_compilelist(config, log, memory, game, temp_script_folder)`.
    ///
    /// Copies every `.nss` from `.` and each patch's `!SourceFolder` into
    /// `temp_script_folder/source_{i}` (one folder per distinct source
    /// folder, other folders first so the script's own folder wins, names
    /// lower-cased), then points every patch's `temp_script_folder` at its
    /// working folder and re-resolves `nwnnsscomp_path` through
    /// `resolve_source_file_path`.
    ///
    /// * `patches_nss` — `config.patches_nss`.
    /// * `memory`, `game` — unused, kept for the Python signature.
    /// * `temp_script_folder` — the workspace root, e.g.
    ///   `TempDir::new(WORKSPACE_PREFIX)?.path()`.
    /// * `resolve_source_folder(relative_path, description)` —
    ///   `ModInstaller._resolve_source_folder`.
    /// * `resolve_source_file_path(filepath, description)` —
    ///   `ModInstaller._resolve_source_file_path`.
    ///
    /// Returns `Ok(None)` when there are no `[CompileList]` patches, else
    /// `Ok(Some(temp_script_folder))`. Errors from the closures and I/O
    /// failures propagate; the installer logs them as the failing patch's
    /// error, as Python does.
    pub fn prepare_compilelist<F, G>(
        patches_nss: &mut [ModificationsNSS],
        log: &mut PatchLogger,
        _memory: &PatcherMemory,
        _game: Game,
        temp_script_folder: &Path,
        resolve_source_folder: F,
        resolve_source_file_path: G,
    ) -> PResult<Option<PathBuf>>
    where
        F: Fn(&str, &str) -> PResult<PathBuf>,
        G: Fn(&Path, &str) -> PResult<PathBuf>,
    {
        if patches_nss.is_empty() {
            return Ok(None);
        }

        // Copy NSS sources and includes to a system temporary directory where
        // tokens can be replaced without modifying the mod package.
        if safe_isdir(temp_script_folder) {
            let _ = std::fs::remove_dir_all(temp_script_folder);
        }
        std::fs::create_dir_all(temp_script_folder).map_err(|err| {
            PatchException::from_io(&err, Some(&temp_script_folder.to_string_lossy()))
        })?;

        let mut include_folders: Vec<PathBuf> = Vec::new();
        let mut seen_include_folders: Vec<String> = Vec::new();
        let source_folders: Vec<String> = std::iter::once(".".to_string())
            .chain(
                patches_nss
                    .iter()
                    .map(|patch| patch.base.sourcefolder.clone()),
            )
            .collect();
        for source_folder in &source_folders {
            let source_path = resolve_source_folder(source_folder, "CompileList source folder")?;
            let normalized = casefold(&source_path);
            if seen_include_folders.contains(&normalized) {
                continue;
            }
            seen_include_folders.push(normalized);
            include_folders.push(source_path);
        }

        let mut patch_source_folders: Vec<PathBuf> = Vec::new();
        let mut seen_patch_source_folders: Vec<String> = Vec::new();
        for patch in patches_nss.iter() {
            let source_path =
                resolve_source_folder(&patch.base.sourcefolder, "CompileList source folder")?;
            let normalized = casefold(&source_path);
            if seen_patch_source_folders.contains(&normalized) {
                continue;
            }
            seen_patch_source_folders.push(normalized);
            patch_source_folders.push(source_path);
        }

        let mut working_folders: HashMap<String, PathBuf> = HashMap::new();
        let mut script_count = 0usize;
        for (index, source_path) in patch_source_folders.iter().enumerate() {
            let working_folder = temp_script_folder.join(format!("source_{index}"));
            std::fs::create_dir_all(&working_folder).map_err(|err| {
                PatchException::from_io(&err, Some(&working_folder.to_string_lossy()))
            })?;

            // Make includes from every CompileList source directory available,
            // while allowing the current source directory to take precedence.
            let normalized_source_path = casefold(source_path);
            let mut ordered_sources: Vec<&PathBuf> = include_folders
                .iter()
                .filter(|path| casefold(path) != normalized_source_path)
                .collect();
            ordered_sources.push(source_path);
            for include_source in ordered_sources {
                if !safe_isdir(include_source) {
                    continue;
                }
                for source_file in sorted_by_name(safe_iterdir(include_source)) {
                    let name = file_name(&source_file);
                    if posix_suffix(&name).to_lowercase() != ".nss" || !safe_isfile(&source_file) {
                        continue;
                    }
                    let safe_source_file =
                        resolve_source_file_path(&source_file, "CompileList source file")?;
                    let target = working_folder.join(name.to_lowercase());
                    std::fs::copy(&safe_source_file, &target).map_err(|err| {
                        PatchException::from_io(&err, Some(&safe_source_file.to_string_lossy()))
                    })?;
                }
            }

            let scripts = sorted_by_name(safe_iterdir(&working_folder))
                .into_iter()
                .filter(|script| {
                    posix_suffix(&file_name(script)).to_lowercase() == ".nss" && safe_isfile(script)
                })
                .count();
            script_count += scripts;
            working_folders.insert(normalized_source_path, working_folder);
        }

        log.add_verbose(format!(
            "Copied {script_count} CompileList source and include files without rewriting their contents."
        ));
        for nss_patch in patches_nss.iter_mut() {
            let source_path =
                resolve_source_folder(&nss_patch.base.sourcefolder, "CompileList source folder")?;
            nss_patch.temp_script_folder = working_folders.get(&casefold(&source_path)).cloned();
            if let Some(nwnnsscomp_path) = nss_patch.nwnnsscomp_path.clone() {
                nss_patch.nwnnsscomp_path = Some(resolve_source_file_path(
                    &nwnnsscomp_path,
                    "script compiler",
                )?);
            }
        }
        Ok(Some(temp_script_folder.to_path_buf()))
    }

    /// `_save_processed_scripts(workspace)`: copy the whole workspace to
    /// `<package root>/processed_nss/<YYYY-MM-DD_HH.MM.SS.ffffff>/`.
    ///
    /// * `workspace` — the folder `prepare_compilelist` filled.
    /// * `mod_path` — `ModInstaller.mod_path`; the package root is its parent
    ///   when it is itself named `tslpatchdata`.
    /// * `resolve_relative_folder_within(root, relative_path, description)` —
    ///   `ModInstaller._resolve_relative_folder_within`.
    /// * `ensure_within_root(path, root, description)` —
    ///   `ModInstaller._ensure_within_root`.
    ///
    /// Returns the output folder. The installer calls this when
    /// `config.save_processed_scripts != 0` and logs any error as
    /// `Could not save processed CompileList scripts: ...`.
    pub fn save_processed_scripts<F, G>(
        workspace: &Path,
        mod_path: &Path,
        log: &mut PatchLogger,
        resolve_relative_folder_within: F,
        ensure_within_root: G,
    ) -> PResult<PathBuf>
    where
        F: Fn(&Path, &str, &str) -> PResult<PathBuf>,
        G: Fn(&Path, &Path, &str) -> PResult<PathBuf>,
    {
        let is_tslpatchdata = mod_path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase() == "tslpatchdata")
            .unwrap_or(false);
        let package_root: PathBuf = if is_tslpatchdata {
            mod_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| mod_path.to_path_buf())
        } else {
            mod_path.to_path_buf()
        };
        let output_parent = resolve_relative_folder_within(
            &package_root,
            "processed_nss",
            "processed CompileList output folder",
        )?;
        std::fs::create_dir_all(&output_parent)
            .map_err(|err| PatchException::from_io(&err, Some(&output_parent.to_string_lossy())))?;
        let timestamp = super::super::super::timeutil::backup_timestamp_micros();
        let output_folder = ensure_within_root(
            &output_parent.join(timestamp),
            &package_root,
            "processed CompileList output folder",
        )?;
        copytree(workspace, &output_folder)?;
        log.add_note(format!(
            "Saved processed CompileList scripts to '{}'.",
            output_folder.display()
        ));
        Ok(output_folder)
    }

    /// `shutil.copytree(src, dst)`: `dst` must not exist yet.
    pub fn copytree(src: &Path, dst: &Path) -> PResult<()> {
        if dst.exists() {
            return Err(PatchException {
                kind: "FileExistsError",
                message: format!("[Errno 17] File exists: '{}'", dst.display()),
                filename: Some(dst.to_string_lossy().to_string()),
            });
        }
        std::fs::create_dir_all(dst)
            .map_err(|err| PatchException::from_io(&err, Some(&dst.to_string_lossy())))?;
        for entry in safe_iterdir(src) {
            let target = dst.join(file_name(&entry));
            if entry.is_dir() {
                copytree(&entry, &target)?;
            } else {
                std::fs::copy(&entry, &target)
                    .map_err(|err| PatchException::from_io(&err, Some(&entry.to_string_lossy())))?;
            }
        }
        Ok(())
    }

    /// `str(path).casefold()`.
    fn casefold(path: &Path) -> String {
        path.to_string_lossy().to_lowercase()
    }

    fn file_name(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// `sorted(paths, key=lambda path: path.name.casefold())`.
    fn sorted_by_name(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort_by_key(|path| file_name(path).to_lowercase());
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patcher::memory::MemoryValue;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odypatcher-nss-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_forces_an_ncs_saveas_and_a_compile_action() {
        let m = ModificationsNSS::new("dir/a_script.NSS", Some(true));
        assert_eq!(m.base.saveas, "dir/a_script.ncs");
        assert_eq!(m.base.action, "Compile");
        assert!(m.base.replace_file);
        assert!(m.base.skip_if_not_replace);
        assert_eq!(
            ModificationsNSS::new("noext", None).base.saveas,
            "noext.ncs"
        );
        assert_eq!(ModificationsNSS::new("", None).base.saveas, "");
    }

    #[test]
    fn tokens_are_replaced_with_verbose_lines_and_undefined_ones_warn() {
        let mut memory = PatcherMemory::new();
        memory.memory_2da.insert(1, MemoryValue::Str("10".into()));
        memory
            .memory_2da
            .insert(3, MemoryValue::FieldPath("GFF\\Path".into()));
        memory.memory_str.insert(0, 500);
        let mut logger = PatchLogger::new();
        let m = ModificationsNSS::new("s.nss", None);
        let mut text = String::from(
            "a=#2DAMEMORY1#; b=#2DAMEMORY2#; c=#2DAMEMORY3#; d=#2DAMEMORY4#; e=#2DAMEMORY01#; f=#StrRef0#; g=#StrRef7#;",
        );
        m.apply(&mut text, &memory, &mut logger, Game::K1);
        assert_eq!(
            text,
            "a=10; b=; c=GFF\\Path; d=#2DAMEMORY4#; e=#2DAMEMORY01#; f=500; g=#StrRef7#;"
        );
        let verbose: Vec<_> = logger
            .verbose_logs()
            .iter()
            .map(|l| l.message.clone())
            .collect();
        assert_eq!(
            verbose,
            vec![
                "s.nss: Replacing '#2DAMEMORY1#' with '10'",
                "s.nss: Replacing '#2DAMEMORY2#' with ''",
                "s.nss: Replacing '#2DAMEMORY3#' with 'GFF\\Path'",
                "s.nss: Replacing '#StrRef0#' with '500'",
            ]
        );
        let warnings: Vec<_> = logger
            .warnings()
            .iter()
            .map(|l| l.message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec![
                "2DAMEMORY4 was not defined before use in 's.nss'; leaving token unchanged.",
                "StrRef7 was not defined before use in 's.nss'; leaving token unchanged.",
            ],
            "a leading-zero id is never replaced, but int('01') is defined so it does not warn"
        );
    }

    #[test]
    fn compiler_flags_split_like_windows() {
        assert_eq!(
            ModificationsNSS::split_compiler_flags("  -a \"b c\" d\\\\e \\\"q\\\" \"in\"\"side\"")
                .unwrap(),
            vec!["-a", "b c", "d\\\\e", "\"q\"", "in\"side"]
        );
        assert_eq!(
            ModificationsNSS::split_compiler_flags("").unwrap(),
            Vec::<String>::new()
        );
        let err = ModificationsNSS::split_compiler_flags("\"open").unwrap_err();
        assert_eq!(err.kind, "ValueError");
        assert_eq!(err.message, "Unclosed double quote in ScriptCompilerFlags.");
    }

    #[test]
    fn path_helpers_follow_pathlib() {
        assert_eq!(
            windows_with_suffix("sub/x.nss", ".ncs").unwrap(),
            "sub\\x.ncs"
        );
        assert_eq!(windows_with_suffix("x", ".ncs").unwrap(), "x.ncs");
        assert_eq!(windows_with_suffix("x.", ".ncs").unwrap(), "x..ncs");
        assert_eq!(windows_with_suffix(".nss", ".ncs").unwrap(), ".nss.ncs");
        let err = windows_with_suffix("", ".ncs").unwrap_err();
        assert_eq!(err.message, "PureWindowsPath('') has an empty name");
        assert_eq!(windows_name("a\\b/C.nss"), "C.nss");
        assert_eq!(python_str_repr("it's"), "\"it's\"");
        assert_eq!(python_str_repr("a\\b\n"), "'a\\\\b\\n'");
        assert_eq!(
            python_tuple_repr(&("RuntimeError".into(), "boom".into())),
            "('RuntimeError', 'boom')"
        );
    }

    #[test]
    fn windows_1252_round_trips_and_rejects_unmappable() {
        assert_eq!(
            encode_windows_1252("caf\u{e9} \u{20ac}").unwrap(),
            b"caf\xe9 \x80"
        );
        assert_eq!(decode_windows_1252(b"\x80\x81\xe9"), "\u{20ac}\u{81}\u{e9}");
        assert_eq!(
            encode_windows_1252("\u{4e2d}").unwrap_err().kind,
            "UnicodeEncodeError"
        );
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex_upper(b""),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
        assert_eq!(
            sha256_hex_upper(b"abc"),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
        assert_eq!(
            KnownExternalCompilers::from_sha256(
                "539eb689d2e0d3751aeed273385865278bef6696c46bc0cab116b40c3b2fe820"
            )
            .unwrap()
            .member_name(),
            "TSLPATCHER"
        );
        assert_eq!(
            KnownExternalCompilers::from_sha256("nope")
                .unwrap_err()
                .kind,
            "ValueError"
        );
    }

    #[test]
    fn compile_args_follow_the_detected_build() {
        let tsl = NwnnsscompConfig::new(
            KnownExternalCompilers::TslPatcher.sha256(),
            PathBuf::from("/s/a.nss"),
            PathBuf::from("/o/a.ncs"),
            Game::K2,
        );
        assert_eq!(
            tsl.get_compile_args("nwnnsscomp.exe", &["-x".to_string()]),
            vec!["nwnnsscomp.exe", "-x", "-c", "/s/a.nss", "-o", "/o/a.ncs"]
        );
        let kt = NwnnsscompConfig::new(
            KnownExternalCompilers::KotorTool.sha256(),
            PathBuf::from("/s/a.nss"),
            PathBuf::from("/o/a.ncs"),
            Game::K2,
        );
        assert_eq!(
            kt.get_compile_args("x", &[]),
            vec![
                "x",
                "-c",
                "--outputdir",
                "/o",
                "-o",
                "a.ncs",
                "-g",
                "2",
                "/s/a.nss"
            ]
        );
        let unknown = NwnnsscompConfig::new("00", PathBuf::from("a"), PathBuf::from("b"), Game::K1);
        assert_eq!(
            unknown.get_compile_args("x", &[]),
            vec!["x", "-c", "a", "-o", "b"]
        );
    }

    #[test]
    fn get_output_moves_error_lines_to_stderr() {
        let result = CompletedProcess {
            returncode: 1,
            stdout: "ok\nError: bad\nmore".into(),
            stderr: String::new(),
        };
        let (stdout, stderr) = ExternalNCSCompiler::get_output(&result);
        assert_eq!(stdout, "ok\nmore");
        assert_eq!(
            stderr,
            "No error provided, but return code is nonzero: (1)\n\nError: bad"
        );
    }

    #[test]
    #[cfg(feature = "nss-compiler")]
    fn patch_resource_requires_a_workspace_and_compiles_with_the_builtin_compiler() {
        let mut memory = PatcherMemory::new();
        memory.memory_2da.insert(1, MemoryValue::Str("42".into()));
        let mut logger = PatchLogger::new();
        let mut m = ModificationsNSS::new("Script.nss", None);
        let err = m
            .patch_resource(b"void main() {}", &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.kind, "RuntimeError");
        assert_eq!(
            err.message,
            "CompileList working directory was not prepared before compilation."
        );

        let dir = temp_dir("builtin");
        std::fs::write(dir.join("nwscript.nss"), b"// empty header\n").unwrap();
        m.temp_script_folder = Some(dir.clone());
        let out = m
            .patch_resource(
                b"void main() { int a = #2DAMEMORY1#; }",
                &mut memory,
                &mut logger,
                Game::K1,
            )
            .unwrap();
        let PatchOutput::Bytes(ncs) = out else {
            panic!("expected compiled bytes");
        };
        assert_eq!(&ncs[0..8], b"NCS V1.0");
        let written = std::fs::read_to_string(dir.join("script.nss")).unwrap();
        assert_eq!(written, "void main() { int a = 42; }");
        if cfg!(windows) {
            assert!(logger.notes().iter().any(|l| l
                .message
                .contains("was not found in the 'tslpatchdata' folder")));
        } else {
            assert!(logger.notes().iter().any(|l| l.message
                == "Patching from a unix operating system, compiling 'Script.nss' using the built-in compilers..."));
        }

        // An include file is a skip with PyKotor's note.
        let out = m
            .patch_resource(
                b"int helper() { return 1; }",
                &mut memory,
                &mut logger,
                Game::K1,
            )
            .unwrap();
        assert_eq!(out, PatchOutput::Skip);
        assert!(logger
            .notes()
            .iter()
            .any(|l| l.message == ENTRY_POINT_ERROR));

        // A broken script is a CompileError the installer reports per patch.
        let err = m
            .patch_resource(
                b"void main() { int a = ; }",
                &mut memory,
                &mut logger,
                Game::K1,
            )
            .unwrap_err();
        assert_eq!(err.kind, "CompileError");

        // Without any header the compile fails clearly.
        let bare = temp_dir("noheader");
        m.temp_script_folder = Some(bare.clone());
        let err = m
            .patch_resource(b"void main() {}", &mut memory, &mut logger, Game::K1)
            .unwrap_err();
        assert_eq!(err.kind, "CompileError");
        assert!(err
            .message
            .starts_with("Could not find a 'nwscript.nss' header to compile 'Script.nss'"));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn unchanged_sources_keep_their_original_bytes() {
        let dir = temp_dir("bytes");
        std::fs::write(dir.join("nwscript.nss"), b"").unwrap();
        let mut memory = PatcherMemory::new();
        let mut logger = PatchLogger::new();
        let mut m = ModificationsNSS::new("k.nss", None);
        m.temp_script_folder = Some(dir.clone());
        let source = b"\xEF\xBB\xBFvoid main() {}\r\n// caf\xe9\r\n";
        let _ = m.patch_resource(source, &mut memory, &mut logger, Game::K2);
        assert_eq!(std::fs::read(dir.join("k.nss")).unwrap(), source.to_vec());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_copies_every_source_folder_lowercased_with_the_own_folder_winning() {
        let dir = temp_dir("workspace");
        let mod_path = dir.join("tslpatchdata");
        std::fs::create_dir_all(mod_path.join("Sub")).unwrap();
        std::fs::write(mod_path.join("Main.NSS"), b"root main").unwrap();
        std::fs::write(mod_path.join("inc.nss"), b"root inc").unwrap();
        std::fs::write(mod_path.join("notes.txt"), b"x").unwrap();
        std::fs::write(mod_path.join("Sub").join("inc.nss"), b"sub inc").unwrap();
        std::fs::write(mod_path.join("Sub").join("Other.nss"), b"sub other").unwrap();

        let mut patches = vec![
            ModificationsNSS::new("Main.nss", None),
            ModificationsNSS::new("Other.nss", None),
        ];
        patches[1].base.sourcefolder = "Sub".into();
        patches[1].nwnnsscomp_path = Some(mod_path.join("nwnnsscomp.exe"));

        let workspace_root = dir.join("work");
        let mut logger = PatchLogger::new();
        let memory = PatcherMemory::new();
        let mod_for_folder = mod_path.clone();
        let result = workspace::prepare_compilelist(
            &mut patches,
            &mut logger,
            &memory,
            Game::K1,
            &workspace_root,
            |relative, _| {
                Ok(crate::patcher::casepath::abspath(
                    &mod_for_folder.join(relative),
                ))
            },
            |path, description| {
                Ok(if description == "script compiler" {
                    path.with_file_name("resolved.exe")
                } else {
                    path.to_path_buf()
                })
            },
        )
        .unwrap();
        assert_eq!(result, Some(workspace_root.clone()));

        let source_0 = workspace_root.join("source_0");
        let source_1 = workspace_root.join("source_1");
        assert_eq!(patches[0].temp_script_folder, Some(source_0.clone()));
        assert_eq!(patches[1].temp_script_folder, Some(source_1.clone()));
        assert_eq!(
            patches[1].nwnnsscomp_path,
            Some(mod_path.join("resolved.exe"))
        );

        assert_eq!(
            std::fs::read(source_0.join("main.nss")).unwrap(),
            b"root main"
        );
        assert_eq!(
            std::fs::read(source_0.join("inc.nss")).unwrap(),
            b"root inc",
            "own folder wins"
        );
        assert_eq!(
            std::fs::read(source_0.join("other.nss")).unwrap(),
            b"sub other"
        );
        assert!(!source_0.join("notes.txt").exists());
        assert_eq!(
            std::fs::read(source_1.join("inc.nss")).unwrap(),
            b"sub inc",
            "own folder wins"
        );
        assert_eq!(
            std::fs::read(source_1.join("main.nss")).unwrap(),
            b"root main"
        );
        assert_eq!(
            logger.verbose_logs().last().unwrap().message,
            "Copied 6 CompileList source and include files without rewriting their contents."
        );

        // No patches: nothing happens.
        let mut none: Vec<ModificationsNSS> = Vec::new();
        let result = workspace::prepare_compilelist(
            &mut none,
            &mut logger,
            &memory,
            Game::K1,
            &workspace_root,
            |_, _| unreachable!(),
            |_, _| unreachable!(),
        )
        .unwrap();
        assert_eq!(result, None);

        // Saving copies the whole workspace under the package root.
        let saved = workspace::save_processed_scripts(
            &workspace_root,
            &mod_path,
            &mut logger,
            |root, relative, _| Ok(root.join(relative)),
            |path, _, _| Ok(path.to_path_buf()),
        )
        .unwrap();
        assert!(saved.starts_with(dir.join("processed_nss")));
        assert_eq!(
            std::fs::read(saved.join("source_1").join("inc.nss")).unwrap(),
            b"sub inc"
        );
        assert!(logger.notes().iter().any(|l| l.message
            == format!(
                "Saved processed CompileList scripts to '{}'.",
                saved.display()
            )));
        assert_eq!(
            workspace::copytree(&workspace_root, &saved)
                .unwrap_err()
                .kind,
            "FileExistsError"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
