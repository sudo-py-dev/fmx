//! Task-specific settings composed from the editor's existing adaptive rows.
use super::*;

pub(super) fn supports(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Reverse
            | Tool::Fade
            | Tool::Stabilize
            | Tool::Denoise
            | Tool::Sharpen
            | Tool::AudioDenoise
            | Tool::TrimSilence
            | Tool::Karaoke
            | Tool::ThumbnailSheet
            | Tool::WaveformVideo
    )
}

fn number(
    ui: &mut egui::Ui,
    key: &'static str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
) {
    row(ui, tr(key), |ui| {
        ui.add(DragValue::new(value).range(range).speed(0.1).suffix(suffix));
    });
}

pub(super) fn show(ui: &mut egui::Ui, tool: Tool, opts: &mut Options) {
    let o = &mut opts.media_tools;
    match tool {
        Tool::Reverse => {
            ui.label(tr("media.reverse_hint"));
        }
        Tool::Fade => {
            number(ui, "media.fade_in", &mut o.fade_in_s, 0.0..=60.0, " s");
            number(ui, "media.fade_out", &mut o.fade_out_s, 0.0..=60.0, " s");
            ui.label(tr("media.fade_hint"));
        }
        Tool::Stabilize => {
            ui.label(tr("media.stabilize_hint"));
        }
        Tool::Denoise => number(
            ui,
            "media.strength",
            &mut o.denoise_strength,
            0.1..=10.0,
            "",
        ),
        Tool::Sharpen => number(ui, "media.strength", &mut o.sharpen_strength, 0.1..=2.0, ""),
        Tool::AudioDenoise => number(
            ui,
            "media.noise_reduction",
            &mut o.noise_reduction_db,
            0.01..=30.0,
            " dB",
        ),
        Tool::TrimSilence => {
            number(
                ui,
                "media.silence_threshold",
                &mut o.silence_threshold_db,
                -80.0..=-10.0,
                " dB",
            );
            number(
                ui,
                "media.silence_duration",
                &mut o.silence_duration_s,
                0.01..=5.0,
                " s",
            );
            ui.label(tr("media.silence_hint"));
        }
        Tool::Karaoke => {
            row_hint(
                ui,
                tr("media.vocal_reduction"),
                tr("media.vocal_reduction_hint"),
                |ui| {
                    theme::slider(
                        ui,
                        Slider::new(&mut o.vocal_reduction_percent, 0.0..=100.0)
                            .integer()
                            .suffix(" %"),
                    );
                },
            );
            ui.label(tr("media.karaoke_hint"));
        }
        Tool::ThumbnailSheet => {
            row(ui, tr("media.columns"), |ui| {
                ui.add(DragValue::new(&mut o.sheet_columns).range(1..=8));
            });
            row(ui, tr("media.rows"), |ui| {
                ui.add(DragValue::new(&mut o.sheet_rows).range(1..=8));
            });
            row(ui, tr("media.thumbnail_width"), |ui| {
                ui.add(
                    DragValue::new(&mut o.thumbnail_width)
                        .range(80..=480)
                        .suffix(" px"),
                );
            });
        }
        Tool::WaveformVideo => {
            row(ui, tr("media.visualization"), |ui| {
                widgets::segmented(
                    ui,
                    &mut o.spectrum,
                    &[(false, tr("media.waveform")), (true, tr("media.spectrum"))],
                );
            });
        }

        _ => {}
    }
    if matches!(
        tool,
        Tool::Reverse
            | Tool::Fade
            | Tool::Stabilize
            | Tool::Denoise
            | Tool::Sharpen
            | Tool::WaveformVideo
    ) {
        quality_row(ui, &mut opts.quality);
    }
    if matches!(tool, Tool::AudioDenoise | Tool::TrimSilence | Tool::Karaoke) {
        ui.label(tr("media.flac_hint"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_tool_controls_fit_editor_widths_in_both_themes() {
        for width in [420.0, 700.0] {
            for theme_choice in [ThemeChoice::Light, ThemeChoice::Dark] {
                for tool in Category::ALL
                    .into_iter()
                    .flat_map(Category::tools)
                    .filter(|t| supports(*t))
                {
                    let ctx = egui::Context::default();
                    icons::install(&ctx);
                    theme::apply(&ctx, theme_choice, true);
                    let mut opts = Options::default();
                    let mut used_width = 0.0;
                    for _ in 0..5 {
                        ctx.run_ui(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 1200.0),
                                )),
                                ..Default::default()
                            },
                            |ui| {
                                pref_group(ui, None, |ui| {
                                    show(ui, tool, &mut opts);
                                    used_width = ui.min_rect().width();
                                });
                            },
                        )
                        .textures_delta
                        .clear();
                    }
                    assert!(used_width <= width, "{tool:?}: {used_width} > {width}");
                }
            }
        }
    }
}
