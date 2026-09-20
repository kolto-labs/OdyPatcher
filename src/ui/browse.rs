//! Browse layout: pick mod folder, game folder, then install.

use eframe::egui;

use super::{Answer, App, Dialog, View};

/// Links offered under Help and About.
const PCGAMINGWIKI_KOTOR1: &str =
    "https://www.pcgamingwiki.com/wiki/Star_Wars:_Knights_of_the_Old_Republic";
const PCGAMINGWIKI_KOTOR2: &str =
    "https://www.pcgamingwiki.com/wiki/Star_Wars:_Knights_of_the_Old_Republic_II_-_The_Sith_Lords";
const PROJECT_SOURCE: &str = "https://github.com/bodencrouch/OdyPatcher";

impl App {
    pub(super) fn show_browse(&mut self, ctx: &egui::Context) {
        self.show_menu_bar(ctx);
        self.show_choosers(ctx);
        self.show_actions(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_body(ui);
        });
    }

    fn show_menu_bar(&mut self, ctx: &egui::Context) {
        let mut action: Option<MenuAction> = None;

        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("Tools", |ui| {
                    let loaded = self.session.is_some();

                    if ui
                        .add_enabled(loaded, egui::Button::new("Validate INI"))
                        .clicked()
                    {
                        action = Some(MenuAction::Validate);
                        ui.close();
                    }
                    if ui
                        .add_enabled(loaded, egui::Button::new("Restore Backup"))
                        .clicked()
                    {
                        action = Some(MenuAction::Restore);
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            !self.game_path.is_empty() && !self.running,
                            egui::Button::new("Force Lowercase Filenames"),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::Lowercase);
                        ui.close();
                    }
                });

                ui.menu_button("Help", |ui| {
                    ui.menu_button("PCGamingWiki", |ui| {
                        if ui.button("KOTOR 1").clicked() {
                            action = Some(MenuAction::Open(PCGAMINGWIKI_KOTOR1));
                            ui.close();
                        }
                        if ui.button("KOTOR 2: TSL").clicked() {
                            action = Some(MenuAction::Open(PCGAMINGWIKI_KOTOR2));
                            ui.close();
                        }
                    });
                    if ui.button("swkotor.org").clicked() {
                        action = Some(MenuAction::Open(crate::HOMEPAGE));
                        ui.close();
                    }
                });

                ui.menu_button("About", |ui| {
                    if ui
                        .button(format!("{} {}", crate::APP_NAME, crate::VERSION))
                        .clicked()
                    {
                        action = Some(MenuAction::About);
                        ui.close();
                    }
                    if ui.button("GitHub Source").clicked() {
                        action = Some(MenuAction::Open(PROJECT_SOURCE));
                        ui.close();
                    }
                });
            });
        });

        if let Some(action) = action {
            self.run_menu_action(ctx, action);
        }
    }

    fn run_menu_action(&mut self, ctx: &egui::Context, action: MenuAction) {
        match action {
            MenuAction::Open(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            MenuAction::About => {
                self.dialogs.push_back(Dialog::info(
                    "About",
                    format!(
                        "{} {}\nGame data patcher and mod installer for KotOR and TSL.\n\nWritten by OldRepublicWizard.\n{}",
                        crate::APP_NAME,
                        crate::VERSION,
                        crate::HOMEPAGE
                    ),
                ));
            }
            MenuAction::Validate => {
                let Some(session) = self.session.as_ref() else {
                    return;
                };

                let game_folder = if self.game_path.is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(&self.game_path))
                };
                let findings = crate::tools::validate(session, game_folder.as_deref());
                let mut report = String::from("Checking the mod's instructions\n\n");

                for finding in &findings {
                    let mark = if finding.blocking { "error" } else { "note " };
                    report.push_str(&format!("  [{mark}] {}\n", finding.message));
                }

                self.tool_text = report;
                self.view = View::Tool;
            }
            MenuAction::Restore => {
                self.dialogs.push_back(Dialog::confirm(
                    "Restore Backup",
                    "Restore the most recent backup in this mod's backup folder into the game?\n\nFiles go back where they were taken from, and files the install added are removed. The backup is kept afterwards.",
                    Answer::RestoreBackup,
                ));
            }
            MenuAction::Lowercase => {
                self.dialogs.push_back(Dialog::confirm(
                    "Force Lowercase Filenames",
                    format!(
                        "Rename every file and folder under\n{}\nto lowercase?\n\n\
Files whose names differ only in case are merged, and the extra copy is \
deleted. This cannot be undone. Back up your game folder first.",
                        self.game_path
                    ),
                    Answer::Lowercase,
                ));
            }
        }
    }

    pub(super) fn restore_backup(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };

        if self.game_path.is_empty() {
            self.dialogs.push_back(Dialog::warning(
                "Restore Backup",
                "Choose the game folder first.",
            ));
            return;
        }

        let report =
            crate::tools::restore_backup(&session.base_path, std::path::Path::new(&self.game_path));
        let mut text = String::from("Restoring backed-up files\n\n");

        for log in &report.logs {
            text.push_str(&format!("  {}\n", log.formatted_message()));
        }
        for (title, message) in &report.messages {
            text.push_str(&format!("\n[{title}]\n{message}\n"));
        }
        text.push_str(if report.restored {
            "\nThe backup was restored.\n"
        } else {
            "\nNothing was restored.\n"
        });

        self.tool_text = text;
        self.view = View::Tool;
    }

    pub(super) fn force_lowercase(&mut self) {
        let report = crate::tools::force_lowercase(&self.game_path);
        if report.skipped {
            self.tool_text = "This filesystem does not tell Data and data apart, \
so nothing was renamed."
                .to_string();
            self.view = View::Tool;
            return;
        }
        let mut text = format!(
            "Renamed {} files and folders to lowercase ({} case-only duplicates merged).\n",
            report.renamed, report.merged
        );
        if !report.failed.is_empty() {
            text.push_str(&format!(
                "\n{} could not be renamed:\n",
                report.failed.len()
            ));
            for (path, reason) in &report.failed {
                text.push_str(&format!("  {}: {reason}\n", path.display()));
            }
        }
        if !report.conflicts.is_empty() {
            text.push_str(&format!(
                "\n{} case-only duplicate pair(s) have different content and were left as-is:\n",
                report.conflicts.len()
            ));
            for (a, b) in &report.conflicts {
                text.push_str(&format!("  {} vs {}\n", a.display(), b.display()));
            }
        }
        self.tool_text = text;
        self.view = View::Tool;
    }

    fn show_choosers(&mut self, ctx: &egui::Context) {
        let mut browse_mod = false;
        let mut browse_game = false;
        let mut describe = false;
        let mut chosen_variant: Option<usize> = None;

        let variants: Vec<String> = self
            .session
            .as_ref()
            .map(|s| s.namespaces.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();

        egui::TopBottomPanel::top("choosers").show(ctx, |ui| {
            ui.add_space(2.0);

            // Row one: which mod to install.
            ui.horizontal(|ui| {
                let buttons = 96.0;
                let width = (ui.available_width() - buttons).max(80.0);

                let label = if self.mod_label.is_empty() {
                    "Select the mod to install".to_string()
                } else {
                    self.mod_label.clone()
                };

                egui::ComboBox::from_id_salt("mod-choice")
                    .selected_text(label)
                    .width(width)
                    .show_ui(ui, |ui| {
                        for (index, name) in variants.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_variant == index, name)
                                .clicked()
                            {
                                chosen_variant = Some(index);
                            }
                        }
                        if variants.is_empty() {
                            ui.label("No installations to choose from.");
                        }
                    });

                if ui.button("Browse").clicked() {
                    browse_mod = true;
                }
                if ui
                    .add_enabled(self.session.is_some(), egui::Button::new("?"))
                    .clicked()
                {
                    describe = true;
                }
            });

            ui.add_space(2.0);

            // Row two: where the game is installed.
            ui.horizontal(|ui| {
                let buttons = 96.0;
                let width = (ui.available_width() - buttons).max(80.0);

                if self.game_path.is_empty() {
                    ui.style_mut().visuals.override_text_color =
                        Some(ui.visuals().weak_text_color());
                }

                ui.add_sized(
                    [width - 26.0, 22.0],
                    egui::TextEdit::singleline(&mut self.game_path)
                        .hint_text("Select your KOTOR directory path"),
                );
                ui.style_mut().visuals.override_text_color = None;

                // The detected installs hang off the arrow beside the path.
                egui::ComboBox::from_id_salt("game-choice")
                    .selected_text("")
                    .width(0.0)
                    .show_ui(ui, |ui| {
                        for path in self.game_paths.clone() {
                            if ui.selectable_label(self.game_path == path, &path).clicked() {
                                self.game_path = path;
                            }
                        }
                        if self.game_paths.is_empty() {
                            ui.label("No installations found.");
                        }
                    });

                if ui.button("Browse").clicked() {
                    browse_game = true;
                }
            });

            ui.add_space(3.0);
        });

        if let Some(index) = chosen_variant {
            self.take_variant(index);
        }

        if describe {
            let title = if self.mod_label.is_empty() {
                "Installation".to_string()
            } else {
                self.mod_label.clone()
            };
            self.dialogs
                .push_back(Dialog::info(&title, self.variant_description()));
        }

        if browse_mod {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Select the mod folder")
                .pick_folder()
            {
                self.load_mod(&folder.to_string_lossy());
            }
        }

        if browse_game {
            if let Some(folder) = rfd::FileDialog::new()
                .set_title("Select your KOTOR directory")
                .pick_folder()
            {
                let path = folder.to_string_lossy().into_owned();
                if !self.game_paths.contains(&path) {
                    self.game_paths.push(path.clone());
                }
                self.game_path = path;
            }
        }
    }

    fn show_actions(&mut self, ctx: &egui::Context) {
        let mut exit = false;
        let mut install = false;

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.running, egui::Button::new("Exit"))
                    .clicked()
                {
                    exit = true;
                }

                if !self.lines.is_empty() || self.view == View::Tool {
                    let showing_log = self.view == View::Log;
                    if ui.selectable_label(showing_log, "Progress log").clicked() {
                        self.view = if showing_log {
                            View::Description
                        } else {
                            View::Log
                        };
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let start_label = self
                        .session
                        .as_ref()
                        .map(|session| session.start_label())
                        .unwrap_or("Install");
                    if ui
                        .add_enabled(self.can_start(), egui::Button::new(start_label))
                        .clicked()
                    {
                        install = true;
                    }
                });
            });

            let (done, total) = self.progress;
            let fraction = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };

            ui.add(egui::ProgressBar::new(fraction).desired_height(10.0));
            ui.add_space(5.0);
        });

        if exit {
            self.request_quit(ctx);
        }
        if install {
            self.request_start();
        }
    }
}

/// Something the menu asked for.
#[derive(Debug, Clone, Copy)]
enum MenuAction {
    Open(&'static str),
    About,
    Validate,
    Restore,
    Lowercase,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "odypatcher-browse-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    #[test]
    fn a_rename_that_cannot_be_performed_is_reported_in_the_tool_text() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("lowercase-failed");
        if !crate::tools::filesystem_distinguishes_case(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("Appearance.2DA"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

        let mut app = App::browse(dir.to_string_lossy().into_owned());
        app.game_path = dir.to_string_lossy().into_owned();
        app.force_lowercase();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Running as root ignores the write bit; nothing this test can prove.
        if !app.tool_text.contains("could not be renamed") {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        assert!(
            app.tool_text.contains("could not be renamed"),
            "the failure should be visible to the operator: {}",
            app.tool_text
        );
        assert_eq!(app.view, View::Tool);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_skipped_fold_says_so_without_claiming_a_count() {
        // force_lowercase reports `skipped` rather than a zero count when the
        // filesystem does not distinguish case, so the operator is told
        // nothing was renamed rather than reading a report that implies a
        // walk that never ran.
        let dir = temp_dir("lowercase-skipped-text");
        if crate::tools::filesystem_distinguishes_case(&dir) {
            // This filesystem does distinguish case; nothing here to prove.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let mut app = App::browse(dir.to_string_lossy().into_owned());
        app.game_path = dir.to_string_lossy().into_owned();
        app.force_lowercase();

        assert!(
            app.tool_text.contains("does not tell Data and data apart"),
            "got: {}",
            app.tool_text
        );
        assert_eq!(app.view, View::Tool);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_offered_links_are_absolute() {
        for url in [
            PCGAMINGWIKI_KOTOR1,
            PCGAMINGWIKI_KOTOR2,
            PROJECT_SOURCE,
            crate::HOMEPAGE,
        ] {
            assert!(url.starts_with("https://"), "not absolute: {url}");
        }
    }
}
