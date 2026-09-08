//! Classic layout for a mod shipped beside the patcher (fixed panel measurements).

use eframe::egui;

use super::{App, View};

pub(super) const DEFAULT_SIZE: [f32; 2] = [550.0, 461.0];
pub(super) const MIN_SIZE: [f32; 2] = [480.0, 400.0];

/// Height of the status bar along the bottom.
const STATUS_HEIGHT: f32 = 19.0;
/// Height of the buttons along the bottom of the panel.
const BUTTON_HEIGHT: f32 = 25.0;
/// Width of the Exit and start buttons.
const BUTTON_WIDTH: f32 = 103.0;
/// The summary toggle is a small square button.
const SUMMARY_SIZE: f32 = 25.0;
/// Height of the progress bar.
const PROGRESS_HEIGHT: f32 = 6.0;
/// Margin between the panel's edge and what it holds.
const PANEL_MARGIN: f32 = 8.0;

impl App {
    pub(super) fn show_classic(&mut self, ctx: &egui::Context) {
        if self.choosing_variant {
            self.show_variant_chooser(ctx);
            return;
        }

        self.show_status_bar(ctx);
        self.show_controls(ctx);
        self.show_main_panel(ctx);
    }

    /// The chooser shown when a mod offers several installations.
    ///
    /// A heading, the list of installations, the chosen one's description, then
    fn show_variant_chooser(&mut self, ctx: &egui::Context) {
        let entries: Vec<(String, String)> = self
            .session
            .as_ref()
            .map(|s| {
                s.namespaces
                    .entries
                    .iter()
                    .map(|e| (e.name.clone(), e.description.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut chosen: Option<usize> = None;

        egui::CentralPanel::default()
            .frame(panel_frame(ctx))
            .show(ctx, |ui| {
                ui.add_space(PANEL_MARGIN);

                ui.label(
                    egui::RichText::new("Please select what to install:")
                        .size(21.0)
                        .strong(),
                );

                ui.add_space(15.0);

                let selected = entries
                    .get(self.selected_variant)
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default();

                egui::ComboBox::from_id_salt("variant")
                    .selected_text(selected)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (index, (name, _)) in entries.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_variant == index, name)
                                .clicked()
                            {
                                chosen = Some(index);
                            }
                        }
                    });

                ui.add_space(11.0);

                // The description sits in a sunken box, as a read-only field.
                let description = entries
                    .get(self.selected_variant)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default();

                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_height(97.0);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| ui.label(description));
                    });

                // Select and Exit sit at the lower right.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    ui.add_space(PANEL_MARGIN);
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_sized([75.0, BUTTON_HEIGHT], egui::Button::new("Exit"))
                                .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            ui.add_space(13.0);
                            if ui
                                .add_sized([75.0, BUTTON_HEIGHT], egui::Button::new("Select"))
                                .clicked()
                            {
                                self.choosing_variant = false;
                            }
                        });
                    });
                });
            });

        if let Some(index) = chosen {
            self.take_variant(index);
        }
    }

    fn show_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(STATUS_HEIGHT)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(2.0);
                    ui.small(format!("Game folder: {}", self.install_path_label));
                });
            });
    }

    fn show_controls(&mut self, ctx: &egui::Context) {
        let start_label = self
            .session
            .as_ref()
            .map(|s| s.start_label())
            .unwrap_or("Install Mod");

        let mut start_clicked = false;
        let mut quit_clicked = false;

        egui::TopBottomPanel::bottom("controls")
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: PANEL_MARGIN as i8,
                right: PANEL_MARGIN as i8,
                top: 0,
                bottom: PANEL_MARGIN as i8,
            }))
            .show(ctx, |ui| {
                let (done, total) = self.progress;
                let fraction = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(egui::ProgressBar::new(fraction).desired_height(PROGRESS_HEIGHT));

                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    if ui
                        .add_sized([BUTTON_WIDTH, BUTTON_HEIGHT], egui::Button::new("Exit"))
                        .clicked()
                    {
                        quit_clicked = true;
                    }

                    // The version sits centred between the two sides.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_start = self.can_start();
                        if ui
                            .add_sized(
                                [BUTTON_WIDTH, BUTTON_HEIGHT],
                                egui::Button::new(start_label),
                            )
                            .on_disabled_hover_text("There is nothing here that can be installed.")
                            .clicked()
                            && can_start
                        {
                            start_clicked = true;
                        }

                        ui.add_space(5.0);

                        let showing_summary = self.view == View::Summary;
                        if ui
                            .add_sized(
                                [SUMMARY_SIZE, SUMMARY_SIZE],
                                egui::Button::selectable(showing_summary, "\u{2261}"),
                            )
                            .on_hover_text("Toggle summary of installation configuration.")
                            .clicked()
                        {
                            self.view = if showing_summary {
                                View::Description
                            } else {
                                View::Summary
                            };
                        }

                        if !self.lines.is_empty() {
                            ui.add_space(5.0);
                            let showing_log = self.view == View::Log;
                            if ui.selectable_label(showing_log, "Progress log").clicked() {
                                self.view = if showing_log {
                                    View::Description
                                } else {
                                    View::Log
                                };
                            }
                        }

                        // Whatever room is left carries the version.
                        ui.with_layout(
                            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                            |ui| {
                                ui.vertical_centered(|ui| {
                                    if self.author_tag_shown {
                                        ui.small(
                                            egui::RichText::new("Written by OldRepublicWizard")
                                                .weak(),
                                        );
                                    }
                                    ui.small(
                                        egui::RichText::new(format!(
                                            "{} {}",
                                            crate::APP_NAME,
                                            crate::VERSION
                                        ))
                                        .weak(),
                                    );
                                });
                            },
                        );
                    });
                });
            });

        if start_clicked {
            self.request_start();
        }
        if quit_clicked {
            self.request_quit(ctx);
        }
    }

    fn show_main_panel(&mut self, ctx: &egui::Context) {
        let mut toggle_tag = false;

        egui::CentralPanel::default()
            .frame(panel_frame(ctx))
            .show(ctx, |ui| {
                let problem = self.session.as_ref().and_then(|session| {
                    if !session.has_instructions() {
                        Some(format!(
                            "Cannot find the instruction file \"{}\" in {}.",
                            session.ini_name, session.data_path
                        ))
                    } else {
                        session.blocked_reason()
                    }
                });

                if let Some(problem) = problem {
                    ui.colored_label(egui::Color32::from_rgb(156, 2, 2), problem);
                    ui.add_space(6.0);
                }

                // The whole panel answers a double-click, as it always has.
                let area = ui.available_rect_before_wrap();
                if ui
                    .interact(area, ui.id().with("panel"), egui::Sense::click())
                    .double_clicked()
                {
                    toggle_tag = true;
                }

                // The description wraps; the report and the log do not.
                if self.view != View::Description {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                }

                self.show_body(ui);
            });

        if toggle_tag {
            if self.author_tag_armed {
                self.author_tag_shown = !self.author_tag_shown;
            }
            self.author_tag_armed = !self.author_tag_armed;
        }
    }
}

fn panel_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::new()
        .fill(ctx.style().visuals.panel_fill)
        .stroke(ctx.style().visuals.widgets.noninteractive.bg_stroke)
        .inner_margin(PANEL_MARGIN)
}

// The window can never open smaller than it is allowed to become, and the
// bottom row always fits across it. Checked while compiling, since these are
// fixed measurements rather than anything a run can change.
const _: () = {
    assert!(DEFAULT_SIZE[0] >= MIN_SIZE[0]);
    assert!(DEFAULT_SIZE[1] >= MIN_SIZE[1]);
    assert!(BUTTON_WIDTH * 2.0 + SUMMARY_SIZE + PANEL_MARGIN * 2.0 + 10.0 < MIN_SIZE[0]);
};
