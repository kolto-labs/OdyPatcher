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
                            !self.game_path.is_empty(),
                            egui::Button::new("Force Lowercase Filenames"),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::Lowercase);
                        ui.close();
                    }

                    ui.separator();
                    // Disabled mid-run: the mode is fixed once a run starts,
                    // so a checkbox that moved would say something untrue.
                    ui.add_enabled(
                        !self.running,
                        egui::Checkbox::new(
                            &mut self.experimental_lowercase_paths,
                            "Lowercase names while installing",
                        ),
                    )
                    .on_hover_text(
                        "Experimental. Faster where the filesystem tells Data and data apart. \
Renames files in the mod folder and in the folders the mod writes to.",
                    );
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

                let findings = crate::tools::validate(session);
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
                    "Put every file in this mod's backup folder back into the game?\n\nFiles return to the folder their type came from: the string table beside the game, archives with the modules, everything else to override.",
                    Answer::RestoreBackup,
                ));
            }
            MenuAction::Lowercase => {
                self.dialogs.push_back(Dialog::confirm(
                    "Force Lowercase Filenames",
                    format!(
                        "Rename every file and folder under\n{}\nto lowercase?",
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

        let report = crate::tools::restore_backup(session, &self.game_path);
        let mut text = String::from("Restoring backed-up files\n\n");

        for (name, destination) in &report.restored {
            text.push_str(&format!("  restored {name} -> {destination}\n"));
        }
        for (name, reason) in &report.failed {
            text.push_str(&format!("  failed   {name}: {reason}\n"));
        }
        if report.restored.is_empty() && report.failed.is_empty() {
            text.push_str("  Nothing to restore; the backup folder is empty.\n");
        }

        self.tool_text = text;
        self.view = View::Tool;
    }

    pub(super) fn force_lowercase(&mut self) {
        let report = crate::tools::force_lowercase(&self.game_path);
        let mut text = format!(
            "Renamed {} files and folders to lowercase ({} case-only duplicates merged).\n",
            report.renamed, report.merged
        );
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
            .map(|s| {
                s.namespaces
                    .entries
                    .iter()
                    .map(|e| e.name.clone())
                    .collect()
            })
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
