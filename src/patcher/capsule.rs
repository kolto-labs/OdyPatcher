//! Port of the parts of `pykotor/extract/capsule.py` and
//! `pykotor/extract/file.py` (`ResourceIdentifier`) the installer uses, on
//! top of the `kotor-formats` ERF/RIM reader.
//!
//! kotor-formats specifics: `ErfFile::add_resource` only takes a file on disk,
//! so [`Capsule::add`] stages the bytes in a temporary file beside the archive;
//! `to_bytes` sorts members by name, whereas PyKotor keeps insertion order
//! (the game does not care). Resource types are the crate's extension table;
//! dotted PyKotor extensions such as `mdl.ascii` are unknown to it.

use std::path::{Path, PathBuf};

use super::casepath::safe_isfile;
use super::error::{PResult, PatchException};
use super::installation::{is_capsule_path, mkstemp};
use crate::formats::erf::{extension_to_res_type, ErfFile};

/// `ResourceIdentifier`: a resource name and its type (extension).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIdentifier {
    /// Name without extension.
    pub resname: String,
    /// Extension, lower-cased, without the dot.
    pub restype: String,
}

impl ResourceIdentifier {
    /// `ResourceIdentifier(resname, restype)`.
    pub fn new(resname: &str, restype: &str) -> Self {
        Self {
            resname: resname.to_string(),
            restype: restype.trim_start_matches('.').to_lowercase(),
        }
    }

    /// `ResourceIdentifier.from_path(filename)`.
    ///
    /// Tries the longest dotted extension first (`PurePath.split_filename`),
    /// keeping the first one the resource type table knows; otherwise falls
    /// back to the plain stem and last suffix (which may be an invalid type).
    pub fn from_path(filename: &str) -> Self {
        let name = filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_string();
        let max_dots = name.matches('.').count();
        let mut dots = max_dots + 1;
        while dots > 1 {
            let (resname, ext) = split_filename(&name, dots);
            if extension_to_res_type(&ext) != 0xFFFF {
                return Self::new(&resname, &ext);
            }
            dots -= 1;
        }
        let path = Path::new(&name);
        let restype = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let resname = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        Self { resname, restype }
    }

    /// `restype.validate()`: a `ValueError` for an unknown resource type.
    pub fn validate(&self) -> PResult<()> {
        if self.res_type() == 0xFFFF {
            // repr() of the dynamically built invalid member PyKotor produces.
            let extension = if self.restype.is_empty() {
                String::new()
            } else {
                format!("extension={}, ", self.restype)
            };
            return Err(PatchException::value(format!(
                "Invalid ResourceType: 'ResourceType.from_invalid(type_id=-1, {extension}category=Undefined, contents=binary)'"
            )));
        }
        Ok(())
    }

    /// The numeric type from the crate's table (`0xFFFF` when unknown).
    pub fn res_type(&self) -> u16 {
        extension_to_res_type(&self.restype)
    }

    /// `unpack()`: `(resname, restype)`.
    pub fn unpack(&self) -> (&str, &str) {
        (&self.resname, &self.restype)
    }

    /// `str(identifier)`: `name.ext`, lower-cased.
    pub fn filename(&self) -> String {
        if self.restype.is_empty() {
            self.resname.to_lowercase()
        } else {
            format!("{}.{}", self.resname, self.restype).to_lowercase()
        }
    }
}

/// `PurePath.split_filename(dots)` for a positive `dots`.
fn split_filename(name: &str, dots: usize) -> (String, String) {
    let parts: Vec<&str> = name.rsplitn(dots + 2, '.').collect::<Vec<_>>();
    let parts: Vec<&str> = parts.into_iter().rev().collect();
    if parts.len() <= dots {
        return match name.find('.') {
            Some(first_dot) => (
                name[..first_dot].to_string(),
                name[first_dot + 1..].to_string(),
            ),
            None => (name.to_string(), String::new()),
        };
    }
    let split_at = parts.len() - dots;
    (parts[..split_at].join("."), parts[split_at..].join("."))
}

/// `Capsule`: an ERF/MOD/RIM archive on disk.
#[derive(Debug)]
pub struct Capsule {
    path: PathBuf,
}

impl Capsule {
    /// `Capsule(path)`; a `ValueError` when the extension is not `.erf/.mod/.rim`.
    pub fn new(path: &Path) -> PResult<Self> {
        if !is_capsule_path(path) {
            return Err(PatchException::value(format!(
                "Invalid file extension in capsule filepath '{}'.",
                path.display()
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// `filepath()`.
    pub fn filepath(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> PResult<ErfFile> {
        if !safe_isfile(&self.path) {
            return Err(PatchException::file_not_found(
                "No such file or directory",
                self.path.to_string_lossy(),
            ));
        }
        Ok(ErfFile::load(&self.path.to_string_lossy())?)
    }

    /// `contains(resname, restype)`.
    pub fn contains(&self, resname: &str, restype: &str) -> PResult<bool> {
        let archive = self.load()?;
        let query = ResourceIdentifier::new(resname, restype);
        Ok(find_resource(&archive, &query).is_some())
    }

    /// `resource(resname, restype)`: the bytes, or `None`.
    pub fn resource(&self, resname: &str, restype: &str) -> PResult<Option<Vec<u8>>> {
        let archive = self.load()?;
        let query = ResourceIdentifier::new(resname, restype);
        Ok(find_resource(&archive, &query).map(|r| r.data.clone()))
    }

    /// `add(resname, restype, data)`: write the resource into the file on disk,
    /// replacing an existing member of the same name and type.
    pub fn add(&mut self, resname: &str, restype: &str, data: &[u8]) -> PResult<()> {
        let identifier = ResourceIdentifier::new(resname, restype);
        identifier.validate()?;
        let mut archive = self.load()?;
        let parent = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let staged = mkstemp(&parent, ".holopatcher_res_", ".tmp")?;
        let result = (|| -> PResult<()> {
            std::fs::write(&staged, data)?;
            archive.add_resource(&staged.to_string_lossy(), true, &identifier.filename())?;
            archive.save(None)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&staged);
        result
    }
}

/// The stored member matching an identifier, compared case-insensitively.
fn find_resource<'a>(
    archive: &'a ErfFile,
    query: &ResourceIdentifier,
) -> Option<&'a crate::formats::erf::Resource> {
    let wanted = query.filename();
    archive
        .resources()
        .iter()
        .find(|r| r.file_name().to_lowercase() == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::erf::ArchiveKind;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-capsule-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn identifiers_split_like_pykotor() {
        let id = ResourceIdentifier::from_path("Modules/danm13.MOD");
        assert_eq!(id.unpack(), ("danm13", "mod"));
        assert_eq!(id.filename(), "danm13.mod");
        let dotted = ResourceIdentifier::from_path("k_pdan.hist1.utc");
        assert_eq!(dotted.unpack(), ("k_pdan.hist1", "utc"));
        let bad = ResourceIdentifier::from_path("readme.txt.bogus");
        assert_eq!(bad.unpack(), ("readme.txt", "bogus"));
        assert_eq!(bad.validate().unwrap_err().kind, "ValueError");
        assert!(ResourceIdentifier::from_path("spells.2da")
            .validate()
            .is_ok());
        assert_eq!(split_filename("a.b.c", 2), ("a".into(), "b.c".into()));
        assert_eq!(split_filename("a.b.c", 3), ("a".into(), "b.c".into()));
        assert_eq!(split_filename("abc", 1), ("abc".into(), String::new()));
    }

    #[test]
    fn capsule_reads_and_writes_members() {
        let dir = temp_dir("rw");
        let path = dir.join("test.mod");
        let mut archive = ErfFile::create(&path.to_string_lossy(), ArchiveKind::Mod);
        let seed = dir.join("seed.utc");
        std::fs::write(&seed, b"seed").unwrap();
        archive
            .add_resource(&seed.to_string_lossy(), true, "seed.utc")
            .unwrap();
        archive.save(None).unwrap();

        let mut capsule = Capsule::new(&path).unwrap();
        assert!(capsule.contains("SEED", "utc").unwrap());
        assert!(!capsule.contains("seed", "uti").unwrap());
        assert_eq!(
            capsule.resource("seed", "utc").unwrap(),
            Some(b"seed".to_vec())
        );
        assert_eq!(capsule.resource("nope", "utc").unwrap(), None);

        capsule.add("extra", "uti", b"extra-data").unwrap();
        capsule.add("seed", "utc", b"replaced").unwrap();
        let reread = Capsule::new(&path).unwrap();
        assert_eq!(
            reread.resource("extra", "uti").unwrap(),
            Some(b"extra-data".to_vec())
        );
        assert_eq!(
            reread.resource("seed", "utc").unwrap(),
            Some(b"replaced".to_vec())
        );
        assert_eq!(ErfFile::load(&path.to_string_lossy()).unwrap().count(), 2);
        assert!(!dir.join("seed.utc.tmp").exists());

        let err = capsule.add("x", "bogus", b"").unwrap_err();
        assert_eq!(err.kind, "ValueError");
        assert_eq!(
            Capsule::new(&dir.join("x.sav")).unwrap_err().kind,
            "ValueError"
        );
        assert_eq!(
            Capsule::new(&dir.join("missing.erf"))
                .unwrap()
                .contains("a", "utc")
                .unwrap_err()
                .kind,
            "FileNotFoundError"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
