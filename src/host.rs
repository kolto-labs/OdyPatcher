//! Host callbacks: the engine asks for folders/files and reports progress.

#[derive(Debug, Clone)]
/// A file the run needs the operator to point at.
pub struct FileRequest {
    /// Shown as the prompt.
    pub title: String,
    /// The name to offer first.
    pub default_name: String,
    /// The extension to filter on.
    pub extension: String,
    /// What that kind of file is, in words.
    pub description: String,
}

/// How a run asks the operator for something.
///
/// The terminal, the window, and the test harness each answer differently.
pub trait Host: Send {
    /// Returning `None` aborts the run.
    fn choose_game_folder(&mut self, prompt: &str) -> Option<String>;

    /// Returning `None` aborts the step that needed it.
    fn choose_file(&mut self, request: &FileRequest) -> Option<String>;

    /// Told when the game folder is settled.
    fn install_path_changed(&mut self, _path: &str) {}

    /// Told how far along the run is.
    fn progress(&mut self, _current: usize, _max: usize) {}
}

#[derive(Debug, Default, Clone)]
/// A host that answers nothing and asks nothing. Used by tests and by
/// `--validate`, which needs no operator.
pub struct SilentHost {
    /// The game folder to hand back, when one was given.
    pub game_folder: Option<String>,
}

impl SilentHost {
    /// A silent host that answers the game-folder question with `path`.
    pub fn with_game_folder(path: impl Into<String>) -> Self {
        Self {
            game_folder: Some(path.into()),
        }
    }
}

impl Host for SilentHost {
    fn choose_game_folder(&mut self, _prompt: &str) -> Option<String> {
        self.game_folder.clone()
    }

    fn choose_file(&mut self, _request: &FileRequest) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_host_supplies_only_what_it_was_given() {
        let mut host = SilentHost::with_game_folder("/games/kotor2");

        assert_eq!(
            host.choose_game_folder("where?"),
            Some("/games/kotor2".to_string())
        );
        assert_eq!(
            host.choose_file(&FileRequest {
                title: "pick".into(),
                default_name: "x.2da".into(),
                extension: "2da".into(),
                description: "table".into(),
            }),
            None
        );
    }

    #[test]
    fn a_default_silent_host_declines_everything() {
        let mut host = SilentHost::default();
        assert_eq!(host.choose_game_folder("where?"), None);
    }
}
