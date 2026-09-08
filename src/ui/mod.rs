//! GUI: classic layout (mod beside exe) or browse layout (pick folders).

mod browse;
mod classic;

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use eframe::egui;

use crate::host::{FileRequest, Host};
use crate::patcher::log::{Log, LogLine};
use crate::patcher::{PatchSummary, Patcher};
use crate::session::{self, Session};

/// Which layout the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Classic,
    Browse,
}

enum Question {
    GameFolder(String),
    File(FileRequest),
}

struct Ask {
    question: Question,
    reply: Sender<Option<String>>,
}

enum Message {
    Line(Box<LogLine>),
    Progress(usize, usize),
    InstallPath(String),
    Ask(Ask),
    Finished(Box<Result<PatchSummary, crate::PatchError>>),
}

struct WindowHost {
    sender: Sender<Message>,
}

impl WindowHost {
    fn ask(&mut self, question: Question) -> Option<String> {
        let (reply, answers) = channel();

        if self
            .sender
            .send(Message::Ask(Ask { question, reply }))
            .is_err()
        {
            return None;
        }

        // A dropped channel means the window closed and the run should stop.
        answers.recv().ok().flatten()
    }
}

impl Host for WindowHost {
    fn choose_game_folder(&mut self, prompt: &str) -> Option<String> {
        self.ask(Question::GameFolder(prompt.to_string()))
    }

    fn choose_file(&mut self, request: &FileRequest) -> Option<String> {
        self.ask(Question::File(request.clone()))
    }

    fn install_path_changed(&mut self, path: &str) {
        let _ = self.sender.send(Message::InstallPath(path.to_string()));
    }

    fn progress(&mut self, current: usize, max: usize) {
        let _ = self.sender.send(Message::Progress(current, max));
    }
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

pub struct App {
    mode: Mode,
    base_path: String,
    session: Option<Session>,

    view: View,
    info_text: String,
    summary_text: String,
    lines: Vec<LogLine>,
    progress: (usize, usize),
    running: bool,
    finished: bool,

    incoming: Option<Receiver<Message>>,
    pending: Option<Ask>,
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
    pub fn classic(session: Session) -> Self {
        let info_text = session.info_text();
        let summary_text = session.summary_text();
        let choosing_variant = !session.namespaces.is_empty();
        let install_path_label = session
            .looked_up_game_folder()
            .unwrap_or_else(|| "User selected.".to_string());
        let base_path = session.base_path.clone();

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
            pending: None,
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
            pending: None,
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
    ///
    fn load_mod(&mut self, chosen: &str) {
        let Some(root) = session::locate_mod(chosen) else {
            self.dialogs.push_back(Dialog::warning(
                "Error",
                "Could not find a mod located at the given folder.",
            ));
            return;
        };

        let session = Session::discover(&root, "changes.ini", "info.rtf");

        self.mod_label = mod_label(&session, 0);
        self.selected_variant = 0;
        self.info_text = session.info_text();
        self.summary_text = session.summary_text();
        self.view = View::Description;
        self.lines.clear();
        self.progress = (0, 0);
        self.finished = false;
        self.base_path = session.base_path.clone();
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
            .and_then(|s| s.namespaces.entries.get(self.selected_variant))
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

        if session.settings.starts_without_asking() {
            self.start();
        } else {
            let prompt = session.settings.confirm_prompt();
            self.dialogs
                .push_back(Dialog::confirm("Ready to start", prompt, Answer::StartRun));
        }
    }

    fn start(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        let (sender, receiver) = channel();
        self.incoming = Some(receiver);
        self.running = true;
        self.finished = false;
        self.lines.clear();
        self.view = View::Log;

        let base = session.base_path.clone();
        let data = session.data_path.clone();
        let ini_name = session.ini_name.clone();
        let level = session.settings.log_level;

        // A folder chosen in the window is used directly; otherwise the mod's
        // own registry lookup decides, and failing that the engine asks.
        let known_folder = if self.mode == Mode::Browse && !self.game_path.is_empty() {
            Some(self.game_path.clone())
        } else {
            session.looked_up_game_folder()
        };

        let log_sender = sender.clone();
        thread::spawn(move || {
            let mut log = Log::new(
                level,
                Box::new(move |line| {
                    let _ = log_sender.send(Message::Line(Box::new(line.clone())));
                }),
            );

            let mut host = WindowHost {
                sender: sender.clone(),
            };

            let mut patcher = Patcher::new(&base, &data, &ini_name);
            if let Some(folder) = known_folder {
                if crate::compat::fsutil::dir_exists(&folder) {
                    patcher.files.set_install_path(&folder);
                }
            }

            let outcome = patcher.run(&mut log, &mut host);
            let _ = sender.send(Message::Finished(Box::new(outcome)));
        });
    }

    fn drain_messages(&mut self) {
        let Some(receiver) = &self.incoming else {
            return;
        };

        let mut finished = None;
        let mut asked = None;

        for message in receiver.try_iter() {
            match message {
                Message::Line(line) => self.lines.push(*line),
                Message::Progress(done, total) => self.progress = (done, total),
                Message::InstallPath(path) => self.install_path_label = path,
                Message::Ask(ask) => {
                    asked = Some(ask);
                    break;
                }
                Message::Finished(result) => {
                    finished = Some(*result);
                    break;
                }
            }
        }

        if let Some(ask) = asked {
            self.pending = Some(ask);
        }

        if let Some(result) = finished {
            self.complete(result);
        }
    }

    fn complete(&mut self, result: Result<PatchSummary, crate::PatchError>) {
        self.running = false;
        self.finished = true;
        self.installed = true;
        self.incoming = None;

        if let Some(session) = self.session.as_ref() {
            if let Some(path) = session.write_install_log(&self.lines) {
                self.lines.push(LogLine {
                    level: crate::patcher::LogLevel::Information,
                    text: format!("Install log written to {path}"),
                    outcome: crate::patcher::Outcome::Clean,
                });
            }
        }

        let installing = self
            .session
            .as_ref()
            .map(|s| s.settings.installer_mode)
            .unwrap_or(true);

        self.dialogs.push_back(match result {
            Ok(summary) if summary.is_clean() => {
                Dialog::info("Finished", finish_message(&summary, installing))
            }
            Ok(summary) => Dialog::warning("Finished", finish_message(&summary, installing)),
            Err(err) => Dialog::warning("Error", format!("An error occured! {err}")),
        });
    }

    fn answer_pending(&mut self) {
        let Some(ask) = self.pending.take() else {
            return;
        };

        let answer = match &ask.question {
            Question::GameFolder(prompt) => rfd::FileDialog::new()
                .set_title(prompt)
                .pick_folder()
                .map(|path| path.to_string_lossy().into_owned()),
            Question::File(request) => rfd::FileDialog::new()
                .set_title(&request.title)
                .set_file_name(&request.default_name)
                .add_filter(&request.description, &[request.extension.as_str()])
                .pick_file()
                .map(|path| path.to_string_lossy().into_owned()),
        };

        let _ = ask.reply.send(answer);
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
        let plain = self
            .session
            .as_ref()
            .map(|s| s.settings.plaintext_log)
            .unwrap_or(false);

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
                        if plain {
                            // A mod asking for a plain log gets one without colour.
                            ui.monospace(line.as_plain_text());
                        } else {
                            let (r, g, b) = line.color();
                            ui.colored_label(egui::Color32::from_rgb(r, g, b), line.as_rich_text());
                        }
                    }
                }
            });
    }

    fn report_startup_problems(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        if !session.has_instructions() {
            let ini_name = session.ini_name.clone();
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
    match session.namespaces.entries.get(index) {
        Some(entry) => entry.name.clone(),
        None => session.settings.window_caption.clone(),
    }
}

fn finish_message(summary: &PatchSummary, installing: bool) -> String {
    let subject = if installing { "Installer" } else { "Patcher" };

    match (summary.errors, summary.warnings) {
        (0, 0) => format!(
            "The {subject} is finished. Please check the progress log for details about what has been done."
        ),
        (0, warnings) => format!(
            "The {subject} is finished, but {warnings} warnings were encountered! The Mod may or may not be properly installed. Please check the progress log for further details."
        ),
        (errors, 0) => format!(
            "The {subject} is finished, but {errors} errors were encountered! The Mod has likely not been properly installed. Please check the progress log for further details."
        ),
        (errors, warnings) => format!(
            "The {subject} is finished, but {errors} errors and {warnings} warnings were encountered! The Mod most likely has not been properly installed. Please check the progress log for further details."
        ),
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();

        if self.pending.is_some() {
            self.answer_pending();
        }

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

pub fn run(base_path: &str, ini_name: &str, info_name: &str) -> Result<(), eframe::Error> {
    let beside = session::mod_is_beside_program(base_path);

    let (app, title, size) = if beside {
        let session = Session::discover(base_path, ini_name, info_name);
        let title = session.settings.window_caption.clone();
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
        let summary = PatchSummary::default();
        let message = finish_message(&summary, true);

        assert!(message.contains("Installer is finished"));
        assert!(!message.contains("errors"));
    }

    #[test]
    fn warnings_and_errors_are_both_reported() {
        let warned = PatchSummary {
            warnings: 3,
            errors: 0,
        };
        assert!(finish_message(&warned, true).contains("3 warnings"));

        let failed = PatchSummary {
            warnings: 0,
            errors: 2,
        };
        assert!(finish_message(&failed, true).contains("2 errors"));

        let both = PatchSummary {
            warnings: 3,
            errors: 2,
        };
        let message = finish_message(&both, true);
        assert!(message.contains("2 errors") && message.contains("3 warnings"));
    }

    #[test]
    fn patch_runs_are_named_differently_from_installs() {
        let summary = PatchSummary::default();
        assert!(finish_message(&summary, false).contains("Patcher"));
        assert!(finish_message(&summary, true).contains("Installer"));
    }

    #[test]
    fn a_question_box_is_told_apart_from_a_report() {
        assert!(!Dialog::info("t", "m").asks_a_question());
        assert!(!Dialog::warning("t", "m").asks_a_question());
        assert!(Dialog::confirm("t", "m", Answer::Quit).asks_a_question());
    }
}
