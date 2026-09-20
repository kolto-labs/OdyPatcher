//! GUI: classic layout (mod beside exe) or browse layout (pick folders).

mod browse;
mod classic;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui;

use crate::patcher::logger::{LogType, PatchLog, PatchLogger};
use crate::session::{self, RunReport, Session};

/// Which layout the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The mod sits beside the program: fixed layout, one button.
    Classic,
    /// Pick the mod and game folders in the window.
    Browse,
}

/// What a finished run left behind, as the worker thread reports it.
struct RunFinished {
    result: Result<(), String>,
    report: RunReport,
    log_path: Option<PathBuf>,
}

enum Message {
    Line(Box<PatchLog>),
    Progress(usize, usize),
    Finished(Box<RunFinished>),
}

/// Which panel fills the middle of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Description,
    Summary,
    Log,
    Tool,
}

/// What a message box is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    None,
    StartRun,
    Quit,
    RestoreBackup,
    Lowercase,
}

/// How a message box is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Info,
    Warning,
}

/// A message box waiting to be dismissed.
#[derive(Debug, Clone)]
struct Dialog {
    title: String,
    message: String,
    severity: Severity,
    answer: Answer,
}

impl Dialog {
    fn info(title: &str, message: impl Into<String>) -> Self {
        Self {
            title: title.to_string(),
            message: message.into(),
            severity: Severity::Info,
            answer: Answer::None,
        }
    }

    fn warning(title: &str, message: impl Into<String>) -> Self {
        Self {
            title: title.to_string(),
            message: message.into(),
            severity: Severity::Warning,
            answer: Answer::None,
        }
    }

    fn confirm(title: &str, message: impl Into<String>, answer: Answer) -> Self {
        Self {
            title: title.to_string(),
            message: message.into(),
            severity: Severity::Info,
            answer,
        }
    }

    fn asks_a_question(&self) -> bool {
        self.answer != Answer::None
    }
}

/// The window state, shared by both layouts.
pub struct App {
    mode: Mode,
    base_path: String,
    session: Option<Session>,

    view: View,
    info_text: String,
    summary_text: String,
    lines: Vec<PatchLog>,
    progress: (usize, usize),
    running: bool,
    finished: bool,

    incoming: Option<Receiver<Message>>,
    dialogs: VecDeque<Dialog>,
    startup_checked: bool,

    // Classic layout.
    choosing_variant: bool,
    install_path_label: String,
    author_tag_shown: bool,
    author_tag_armed: bool,
    installed: bool,

    // Browse layout.
    mod_label: String,
    game_paths: Vec<String>,
    game_path: String,
    tool_text: String,

    selected_variant: usize,
}

impl App {
    /// The classic layout for a mod found beside the program.
    pub fn classic(session: Session) -> Self {
        let info_text = session.info_text();
        let summary_text = session.summary_text();
        let choosing_variant = !session.namespaces.is_empty();
        let install_path_label = session
            .looked_up_game_folder()
            .unwrap_or_else(|| "User selected.".to_string());
        let base_path = session.base_path.to_string_lossy().into_owned();

        Self {
            mode: Mode::Classic,
            base_path,
            session: Some(session),
            view: View::Description,
            info_text,
            summary_text,
            lines: Vec::new(),
            progress: (0, 0),
            running: false,
            finished: false,
            incoming: None,
            dialogs: VecDeque::new(),
            startup_checked: false,
            choosing_variant,
            install_path_label,
            author_tag_shown: false,
            author_tag_armed: false,
            installed: false,
            mod_label: String::new(),
            game_paths: Vec::new(),
            game_path: String::new(),
            tool_text: String::new(),
            selected_variant: 0,
        }
    }

    /// The browse layout, with nothing loaded yet.
    pub fn browse(base_path: String) -> Self {
        let game_paths: Vec<String> = crate::gamepaths::detect()
            .into_iter()
            .map(|install| install.path)
            .collect();

        Self {
            mode: Mode::Browse,
            base_path,
            session: None,
            view: View::Description,
            info_text: String::new(),
            summary_text: String::new(),
            lines: Vec::new(),
            progress: (0, 0),
            running: false,
            finished: false,
            incoming: None,
            dialogs: VecDeque::new(),
            startup_checked: true,
            choosing_variant: false,
            install_path_label: String::new(),
            author_tag_shown: false,
            author_tag_armed: false,
            installed: false,
            mod_label: String::new(),
            game_paths,
            game_path: String::new(),
            tool_text: String::new(),
            selected_variant: 0,
        }
    }

    /// Load a mod from a folder the operator pointed at.
    fn load_mod(&mut self, chosen: &str) {
        let Some(root) = session::locate_mod(chosen) else {
            self.dialogs.push_back(Dialog::warning(
                "Error",
                "Could not find a mod located at the given folder.",
            ));
            return;
        };

        let session = Session::discover(&root.to_string_lossy(), "changes.ini", "info.rtf");

        if let Some(error) = &session.namespace_error {
            self.dialogs
                .push_back(Dialog::warning("Error", format!("Cannot read namespaces.ini: {error}")));
        }

        self.mod_label = mod_label(&session, 0);
        self.selected_variant = 0;
        self.info_text = session.info_text();
        self.summary_text = session.summary_text();
        self.view = View::Description;
        self.lines.clear();
        self.progress = (0, 0);
        self.finished = false;
        self.base_path = session.base_path.to_string_lossy().into_owned();
        self.session = Some(session);
    }

    fn take_variant(&mut self, index: usize) {
        let Some(session) = self.session.as_mut() else {
            return;
        };

        session.select_namespace(index);
        self.selected_variant = index;
        self.info_text = session.info_text();
        self.summary_text = session.summary_text();
        self.mod_label = mod_label(session, index);
    }

    fn variant_description(&self) -> String {
        self.session
            .as_ref()
            .and_then(|s| s.namespaces.get(self.selected_variant))
            .map(|entry| entry.description.clone())
            .unwrap_or_else(|| "This mod offers a single installation.".to_string())
    }

    fn request_start(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        if let Some(reason) = session.blocked_reason() {
            self.dialogs.push_back(Dialog::warning("Error", reason));
            return;
        }

        if session.starts_without_asking() {
            self.start();
        } else {
            let prompt = session.confirm_message();
            self.dialogs
                .push_back(Dialog::confirm("Ready to start", prompt, Answer::StartRun));
        }
    }

    /// Which game folder to install into: the one chosen in the window, else
    /// the mod's registry lookup, else a lone detected install, else ask.
    fn settle_game_folder(&self) -> Option<PathBuf> {
        if self.mode == Mode::Browse && !self.game_path.is_empty() {
            return Some(PathBuf::from(&self.game_path));
        }

        if let Some(found) = self.session.as_ref().and_then(|s| s.looked_up_game_folder()) {
            if Path::new(&found).is_dir() {
                return Some(PathBuf::from(found));
            }
        }

        let detected = crate::gamepaths::detect();
        if detected.len() == 1 {
            return Some(PathBuf::from(&detected[0].path));
        }

        rfd::FileDialog::new()
            .set_title("Select the folder the game is installed in")
            .pick_folder()
    }

    fn start(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        let Some(game_folder) = self.settle_game_folder() else {
            self.dialogs
                .push_back(Dialog::warning("Error", "No game folder was chosen."));
            return;
        };
        self.install_path_label = game_folder.to_string_lossy().into_owned();

        let (sender, receiver) = channel();
        self.incoming = Some(receiver);
        self.running = true;
        self.finished = false;
        self.lines.clear();
        self.view = View::Log;

        let session = session.clone();
        let total = session
            .config
            .clone()
            .map(|mut config| config.patch_count())
            .unwrap_or(0);

        thread::spawn(move || {
            let mut logger = PatchLogger::new();
            let line_sender = sender.clone();
            logger.add_observer(Box::new(move |log| {
                let _ = line_sender.send(Message::Line(Box::new(log.clone())));
            }));

            let done = Arc::new(Mutex::new(0usize));
            let progress_sender = sender.clone();
            let counter = Arc::clone(&done);
            let mut progress = move || {
                let mut done = counter.lock().unwrap();
                *done += 1;
                let _ = progress_sender.send(Message::Progress(*done, total.max(*done)));
            };

            let outcome = session.run_install(&game_folder, logger, None, Some(&mut progress));
            let _ = sender.send(Message::Finished(Box::new(RunFinished {
                result: outcome.result.map_err(|err| err.simplified()),
                report: outcome.report,
                log_path: outcome.log_path,
            })));
        });
    }

    fn drain_messages(&mut self) {
        let Some(receiver) = &self.incoming else {
            return;
        };

        let mut finished = None;

        for message in receiver.try_iter() {
            match message {
                Message::Line(line) => self.lines.push(*line),
                Message::Progress(done, total) => self.progress = (done, total),
                Message::Finished(result) => {
                    finished = Some(*result);
                    break;
                }
            }
        }

        if let Some(result) = finished {
            self.complete(result);
        }
    }

    fn complete(&mut self, finished: RunFinished) {
        self.running = false;
        self.finished = true;
        self.installed = true;
        self.incoming = None;

        if let Some(path) = &finished.log_path {
            self.lines.push(PatchLog {
                message: format!("Install log written to {}", path.display()),
                log_type: LogType::Note,
                timestamp: String::new(),
            });
        }

        self.dialogs.push_back(match finished.result {
            Ok(()) if finished.report.is_clean() => {
                Dialog::info("Finished", finish_message(&finished.report))
            }
            Ok(()) => Dialog::warning("Finished", finish_message(&finished.report)),
            Err(err) => Dialog::warning("Error", format!("An error occured! {err}")),
        });
    }

    fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialogs.front().cloned() else {
            return;
        };

        let mut dismissed = false;
        let mut accepted = false;

        egui::Modal::new(egui::Id::new("message-box")).show(ctx, |ui| {
            ui.set_max_width(420.0);

            ui.horizontal(|ui| {
                let (glyph, tint) = match dialog.severity {
                    Severity::Info => ("\u{2139}", egui::Color32::from_rgb(60, 110, 190)),
                    Severity::Warning => ("\u{26A0}", egui::Color32::from_rgb(200, 140, 20)),
                };
                ui.colored_label(tint, egui::RichText::new(glyph).size(22.0));
                ui.add_space(4.0);
                ui.heading(&dialog.title);
            });

            ui.add_space(10.0);
            ui.label(&dialog.message);
            ui.add_space(14.0);

            ui.horizontal(|ui| {
                if dialog.asks_a_question() {
                    if ui.button("Yes").clicked() {
                        accepted = true;
                        dismissed = true;
                    }
                    if ui.button("No").clicked() {
                        dismissed = true;
                    }
                } else if ui.button("OK").clicked() {
                    dismissed = true;
                }
            });
        });

        if !dismissed {
            return;
        }

        self.dialogs.pop_front();

        if !accepted {
            return;
        }

        match dialog.answer {
            Answer::StartRun => self.start(),
            Answer::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Answer::RestoreBackup => self.restore_backup(),
            Answer::Lowercase => self.force_lowercase(),
            Answer::None => {}
        }
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.installed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.dialogs.push_back(Dialog::confirm(
            "Quit",
            "Are you sure you wish to quit?",
            Answer::Quit,
        ));
    }

    fn show_body(&self, ui: &mut egui::Ui) {
        let level = self
            .session
            .as_ref()
            .map(|s| s.log_level())
            .unwrap_or(crate::patcher::config::LogLevel::Warnings);

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .stick_to_bottom(self.view == View::Log && self.running)
            .show(ui, |ui| match self.view {
                View::Description => {
                    ui.label(&self.info_text);
                }
                View::Summary => {
                    ui.monospace(&self.summary_text);
                }
                View::Tool => {
                    ui.monospace(&self.tool_text);
                }
                View::Log => {
                    for line in &self.lines {
                        // The level only decides what is shown; every line is
                        // still recorded and written to the install log.
                        if !session::shows(level, line.log_type) {
                            continue;
                        }
                        let (r, g, b) = session::log_color(line.log_type);
                        ui.colored_label(
                            egui::Color32::from_rgb(r, g, b),
                            egui::RichText::new(line.formatted_message()).monospace(),
                        );
                    }
                }
            });
    }

    fn report_startup_problems(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        if let Some(error) = &session.namespace_error {
            self.dialogs
                .push_back(Dialog::warning("Error", format!("Cannot read namespaces.ini: {error}")));
        }

        if !session.has_instructions() {
            let ini_name = session.ini_name().to_string();
            self.dialogs.push_back(Dialog::warning(
                "Warning",
                format!(
                    "WARNING! Cannot locate the INI file \"{ini_name}\" with work instructions!"
                ),
            ));
        }
    }

    fn can_start(&self) -> bool {
        !self.running
            && self
                .session
                .as_ref()
                .map(|s| s.has_instructions() && s.blocked_reason().is_none())
                .unwrap_or(false)
    }
}

fn mod_label(session: &Session, index: usize) -> String {
    match session.namespaces.get(index) {
        Some(entry) => entry.name.clone(),
        None => session.window_caption(),
    }
}

fn finish_message(report: &RunReport) -> String {
    match (report.errors, report.warnings) {
        (0, 0) => "The Installer is finished. Please check the progress log for details about what has been done.".to_string(),
        (0, warnings) => format!(
            "The Installer is finished, but {warnings} warnings were encountered! The Mod may or may not be properly installed. Please check the progress log for further details."
        ),
        (errors, 0) => format!(
            "The Installer is finished, but {errors} errors were encountered! The Mod has likely not been properly installed. Please check the progress log for further details."
        ),
        (errors, warnings) => format!(
            "The Installer is finished, but {errors} errors and {warnings} warnings were encountered! The Mod most likely has not been properly installed. Please check the progress log for further details."
        ),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();

        if self.running {
            // Keep repainting so progress keeps moving while the worker runs.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        if !self.startup_checked {
            self.startup_checked = true;
            self.report_startup_problems();
        }

        match self.mode {
            Mode::Classic => self.show_classic(ctx),
            Mode::Browse => self.show_browse(ctx),
        }

        self.show_dialog(ctx);
    }
}

/// Open the window: classic when a mod sits beside the program, else browse.
pub fn run(base_path: &str, ini_name: &str, info_name: &str) -> Result<(), eframe::Error> {
    let beside = session::mod_is_beside_program(base_path);

    let (app, title, size) = if beside {
        let session = Session::discover(base_path, ini_name, info_name);
        let title = session.window_caption();
        (App::classic(session), title, classic::DEFAULT_SIZE)
    } else {
        (
            App::browse(base_path.to_string()),
            crate::APP_NAME.to_string(),
            [400.0, 500.0],
        )
    };

    let minimum = if beside {
        classic::MIN_SIZE
    } else {
        [360.0, 380.0]
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_min_inner_size(minimum)
            .with_title(&title),
        ..Default::default()
    };

    eframe::run_native(&title, options, Box::new(|_cc| Ok(Box::new(app))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_run_reports_success() {
        let report = RunReport::default();
        let message = finish_message(&report);

        assert!(message.contains("Installer is finished"));
        assert!(!message.contains("errors"));
    }

    #[test]
    fn warnings_and_errors_are_both_reported() {
        let warned = RunReport {
            warnings: 3,
            ..RunReport::default()
        };
        assert!(finish_message(&warned).contains("3 warnings"));

        let failed = RunReport {
            errors: 2,
            ..RunReport::default()
        };
        assert!(finish_message(&failed).contains("2 errors"));

        let both = RunReport {
            warnings: 3,
            errors: 2,
            ..RunReport::default()
        };
        let message = finish_message(&both);
        assert!(message.contains("2 errors") && message.contains("3 warnings"));
    }

    #[test]
    fn a_question_box_is_told_apart_from_a_report() {
        assert!(!Dialog::info("t", "m").asks_a_question());
        assert!(!Dialog::warning("t", "m").asks_a_question());
        assert!(Dialog::confirm("t", "m", Answer::Quit).asks_a_question());
    }
}
