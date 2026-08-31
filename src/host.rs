//! Interaction with whoever is running the patch.
//!
//! The engine never opens dialogs itself. When it needs the game folder, a file
//! it could not find, or wants to report progress, it asks the host. A GUI shows
//! a dialog; a command-line run answers from arguments or declines.

/// A file the engine could not locate and needs the operator to point at.
#[derive(Debug, Clone)]
pub struct FileRequest {
    /// Prompt describing what is wanted.
    pub title: String,
    /// Filename to suggest.
    pub default_name: String,
    /// Extension to filter by, without the leading dot.
    pub extension: String,
    /// Human-readable description of the file type.
    pub description: String,
}

/// Answers the engine's questions and receives its progress reports.
pub trait Host: Send {
    /// Ask for the folder the game is installed in.
    ///
    /// Returning `None` aborts the run.
    fn choose_game_folder(&mut self, prompt: &str) -> Option<String>;

    /// Ask for a file the engine could not find.
    ///
    /// Returning `None` aborts the step that needed it.
    fn choose_file(&mut self, request: &FileRequest) -> Option<String>;

    /// Report that the game folder has been settled.
    fn install_path_changed(&mut self, _path: &str) {}

    /// Report how far along the run is.
    fn progress(&mut self, _current: usize, _max: usize) {}
}

/// A host that answers nothing, for runs that must not prompt.
#[derive(Debug, Default, Clone)]
pub struct SilentHost {
    /// Game folder to hand out, if one is known up front.
    pub game_folder: Option<String>,
}

impl SilentHost {
    /// A host that will supply this game folder and decline everything else.
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
