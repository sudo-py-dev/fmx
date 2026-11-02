//! Per-file tag drafts. Probe results fill untouched fields; user edits always win.
use super::*;
use crate::options::MediaToolOptions;

struct Draft {
    values: MediaToolOptions,
    loaded: bool,
    available: bool,
    has_cover: bool,
    cover_error: bool,
    quality: Quality,
    new_key: String,
}

impl Draft {
    fn new(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let tag_format = [AudioFormat::Mp3, AudioFormat::M4a, AudioFormat::Flac]
            .into_iter()
            .find(|f| f.ext().eq_ignore_ascii_case(ext))
            .unwrap_or(AudioFormat::Mp3);
        Self {
            values: MediaToolOptions {
                tag_format,
                tag_explicit: true,
                extra_tags: COMMON_TAGS
                    .iter()
                    .map(|(key, _)| crate::tagging::TagField {
                        key: (*key).into(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
            loaded: false,
            available: false,
            has_cover: false,
            cover_error: false,
            quality: Quality::Medium,
            new_key: String::new(),
        }
    }

    fn load(&mut self, tags: Option<&crate::probe::AudioTags>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(tags) = tags {
            self.available = true;
            self.has_cover = tags.has_cover;
            for (index, (value, source)) in [
                (&mut self.values.title, &tags.title),
                (&mut self.values.artist, &tags.artist),
                (&mut self.values.album, &tags.album),
            ]
            .into_iter()
            .enumerate()
            {
                if !self.values.tag_write[index] {
                    *value = source.clone();
                    self.values.tag_scopes[index] = tags.primary_scopes[index];
                }
            }
            for source in &tags.extra {
                if let Some(field) =
                    self.values.extra_tags.iter_mut().find(|f| {
                        f.scope == source.scope && f.key.eq_ignore_ascii_case(&source.key)
                    })
                {
                    if !field.write {
                        field.value = source.value.clone();
                    }
                } else if self.values.extra_tags.len() < crate::tagging::MAX_EXTRA_TAGS {
                    self.values.extra_tags.push(source.clone());
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct TagEditor {
    selected: Option<PathBuf>,
    drafts: HashMap<PathBuf, Draft>,
    order: Vec<PathBuf>,
}

impl TagEditor {
    pub(super) fn sync(&mut self, files: &[PathBuf], details: &HashMap<PathBuf, FileDetails>) {
        if self.order != files {
            let visible: HashSet<_> = files.iter().collect();
            self.drafts.retain(|path, _| visible.contains(path));
            self.order = files.to_vec();
        }
        if self
            .selected
            .as_ref()
            .is_none_or(|path| !files.contains(path))
        {
            self.selected = files.first().cloned();
        }
        for path in files {
            let draft = self
                .drafts
                .entry(path.clone())
                .or_insert_with(|| Draft::new(path));
            if let Some(info) = details.get(path).filter(|d| d.probed) {
                draft.load(info.info.as_ref().and_then(|i| i.tags.as_ref()));
            }
        }
    }

    pub(super) fn validation_error(&self) -> Option<String> {
        self.drafts.values().find_map(|draft| {
            Options {
                media_tools: draft.values.clone(),
                ..Default::default()
            }
            .validate(Tool::EditTags)
            .err()
            .map(|e| e.to_string())
        })
    }

    pub(super) fn plan(
        &self,
        selection: &Selection<'_>,
        claimed: &mut HashSet<PathBuf>,
    ) -> Result<Vec<JobSpec>, crate::jobs::JobError> {
        if selection.files.is_empty() {
            return plan_jobs(selection, claimed);
        }
        let mut specs = Vec::new();
        let mut allocated = claimed.clone();
        for path in selection.files {
            let values = self
                .drafts
                .get(path)
                .map(|d| d.values.clone())
                .unwrap_or_else(|| Draft::new(path).values);
            if let Some(cover) = &values.cover {
                crate::tagging::validate_cover(cover)?;
            }
            let options = Options {
                media_tools: values,
                quality: self.drafts.get(path).map_or(Quality::Medium, |d| d.quality),
                audio: options::AudioSettings::default(),
                edit: Edit::default(),
                ..selection.options.clone()
            };
            let single = Selection {
                files: std::slice::from_ref(path),
                options: &options,
                ..*selection
            };
            specs.extend(plan_jobs(&single, &mut allocated)?);
        }
        *claimed = allocated;
        Ok(specs)
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        let Some(selected) = self.selected.as_mut() else {
            ui.label(tr("tags.add_file"));
            return;
        };
        if self.order.len() > 1 {
            pref_group(ui, None, |ui| {
                widgets::pref_block(ui, tr("tags.edit_file"), |ui| {
                    ComboBox::from_id_salt("tag_source")
                        .selected_text(safe_name(selected))
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for path in &self.order {
                                ui.selectable_value(selected, path.clone(), safe_name(path));
                            }
                        });
                    ui.label(
                        RichText::new(tr("tags.per_file"))
                            .small()
                            .color(theme::palette(ui).text_weak),
                    );
                });
            });
            ui.add_space(GROUP_GAP);
        }
        let Some(draft) = self.drafts.get_mut(selected) else {
            return;
        };
        let status = if !draft.loaded {
            "tags.loading"
        } else if draft.available {
            "tags.loaded"
        } else {
            "tags.unavailable"
        };
        ui.horizontal_wrapped(|ui| {
            if !draft.loaded {
                ui.spinner();
            }
            ui.label(
                RichText::new(tr(status))
                    .small()
                    .color(theme::palette(ui).text_weak),
            );
        });
        ui.add_space(8.0);
        pref_group(ui, None, |ui| {
            for (index, (key, value)) in [
                ("media.title", &mut draft.values.title),
                ("media.artist", &mut draft.values.artist),
                ("media.album", &mut draft.values.album),
            ]
            .into_iter()
            .enumerate()
            {
                widgets::pref_block(ui, tr(key), |ui| {
                    let response = ui.add_sized(
                        [ui.available_width(), theme::BUTTON_HEIGHT],
                        TextEdit::singleline(value)
                            .id_salt((selected.as_path(), key))
                            .char_limit(crate::probe::MAX_TAG_CHARS)
                            .hint_text(tr("tags.no_value")),
                    );
                    if response.changed() {
                        draft.values.tag_write[index] = true;
                    }
                });
            }
        });
        ui.add_space(GROUP_GAP);
        extra_tags(ui, selected, draft);
        ui.add_space(GROUP_GAP);
        pref_group(ui, None, |ui| {
            widgets::pref_block(ui, tr("media.cover"), |ui| {
                let status = if draft.values.cover.is_some() {
                    "tags.cover_replacement"
                } else if draft.values.remove_cover {
                    "tags.cover_removed"
                } else if !draft.loaded {
                    "tags.loading"
                } else if !draft.available {
                    "tags.unavailable"
                } else if draft.has_cover {
                    "tags.cover_existing"
                } else {
                    "tags.cover_none"
                };
                ui.label(tr(status));
                if let Some(cover) = &draft.values.cover {
                    ui.add(Label::new(safe_name(cover)).truncate());
                }
                ui.horizontal_wrapped(|ui| {
                    if widgets::button(
                        ui,
                        Some(icons::FOLDER_OPEN),
                        tr("action.choose"),
                        Kind::Secondary,
                        Size::Small,
                    )
                    .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter(tr("filter.supported"), &["jpg", "jpeg", "png"])
                            .pick_file()
                    {
                        draft.cover_error = crate::tagging::validate_cover(&path).is_err();
                        if !draft.cover_error {
                            draft.values.cover = Some(path);
                            draft.values.remove_cover = false;
                        }
                    }
                    if draft.values.cover.is_some()
                        && widgets::button(
                            ui,
                            Some(icons::X),
                            tr("action.remove"),
                            Kind::Ghost,
                            Size::Small,
                        )
                        .clicked()
                    {
                        draft.values.cover = None;
                        draft.cover_error = false;
                    }
                });
                if draft.cover_error {
                    ui.colored_label(theme::palette(ui).danger, tr("tags.invalid_cover"));
                }
            });
            row(ui, tr("media.remove_cover"), |ui| {
                ui.add_enabled_ui(
                    draft.values.cover.is_none() && (draft.has_cover || !draft.available),
                    |ui| {
                        widgets::switch(ui, &mut draft.values.remove_cover);
                    },
                );
            });
        });
        ui.add_space(GROUP_GAP);
        pref_group(ui, None, |ui| {
            row(ui, tr("option.output_format"), |ui| {
                combo(
                    ui,
                    "tag_format",
                    &mut draft.values.tag_format,
                    &[AudioFormat::Mp3, AudioFormat::M4a, AudioFormat::Flac],
                    |f| f.label().to_owned(),
                );
            });
        });
        if !draft.values.tag_format.is_lossless()
            && selected
                .extension()
                .and_then(|e| e.to_str())
                .is_none_or(|ext| !ext.eq_ignore_ascii_case(draft.values.tag_format.ext()))
        {
            ui.add_space(GROUP_GAP);
            pref_group(ui, None, |ui| quality_row(ui, &mut draft.quality));
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(tr("tags.save_hint"))
                .small()
                .color(theme::palette(ui).text_weak),
        );
    }
}

fn safe_name(path: &Path) -> String {
    crate::probe::display_tag(&file_name(path))
}

const COMMON_TAGS: &[(&str, &str)] = &[
    ("date", "tags.date"),
    ("purl", "tags.source_url"),
    ("comment", "tags.comment"),
    ("genre", "tags.genre"),
    ("album_artist", "tags.album_artist"),
    ("track", "tags.track"),
    ("disc", "tags.disc"),
    ("composer", "tags.composer"),
    ("copyright", "tags.copyright"),
    ("description", "tags.description"),
    ("lyrics", "tags.lyrics"),
    ("language", "info.language"),
    ("publisher", "tags.publisher"),
];

fn extra_field(ui: &mut egui::Ui, path: &Path, field: &mut crate::tagging::TagField, label: &str) {
    widgets::pref_block(ui, label, |ui| {
        let multiline = field.value.contains(['\n', '\r'])
            || ["comment", "description", "lyrics"]
                .iter()
                .any(|name| field.key.eq_ignore_ascii_case(name));
        let managed = crate::tagging::generated_key(&field.key);
        let edit = if multiline {
            TextEdit::multiline(&mut field.value).desired_rows(3)
        } else {
            TextEdit::singleline(&mut field.value)
        };
        let response = ui
            .add_enabled_ui(!managed, |ui| {
                ui.add_sized(
                    [
                        ui.available_width(),
                        if multiline {
                            theme::BUTTON_HEIGHT * 2.0
                        } else {
                            theme::BUTTON_HEIGHT
                        },
                    ],
                    edit.id_salt((path, field.scope, &field.key))
                        .char_limit(crate::tagging::MAX_EXTRA_CHARS)
                        .hint_text(tr("tags.no_value")),
                )
            })
            .inner;
        if managed {
            ui.label(
                RichText::new(tr("tags.generated"))
                    .small()
                    .color(theme::palette(ui).text_weak),
            );
        }
        if response.changed() {
            field.write = true;
        }
        if field.write && field.value.is_empty() {
            ui.label(
                RichText::new(tr("tags.will_remove"))
                    .small()
                    .color(theme::palette(ui).text_weak),
            );
        }
    });
}

fn extra_tags(ui: &mut egui::Ui, path: &Path, draft: &mut Draft) {
    use crate::tagging::{TagField, TagScope};
    pref_group(ui, None, |ui| {
        for &(key, label) in &COMMON_TAGS[..3] {
            if let Some(field) = draft
                .values
                .extra_tags
                .iter_mut()
                .find(|f| f.scope == TagScope::File && f.key.eq_ignore_ascii_case(key))
            {
                extra_field(ui, path, field, tr(label));
            }
        }
    });
    ui.add_space(GROUP_GAP);
    widgets::collapsible_section(
        ui,
        ("more_music_tags", path),
        None,
        tr("tags.more_music"),
        None,
        |ui| {
            pref_group(ui, None, |ui| {
                for &(key, label) in &COMMON_TAGS[3..] {
                    if let Some(field) = draft
                        .values
                        .extra_tags
                        .iter_mut()
                        .find(|f| f.scope == TagScope::File && f.key.eq_ignore_ascii_case(key))
                    {
                        extra_field(ui, path, field, tr(label));
                    }
                }
            });
        },
    );
    ui.add_space(GROUP_GAP);
    widgets::collapsible_section(
        ui,
        ("other_metadata", path),
        None,
        tr("tags.other_tags"),
        None,
        |ui| {
            ui.label(tr("tags.format_support"));
            for (scope, heading) in [
                (TagScope::File, tr("tags.file_tags")),
                (TagScope::Audio, tr("tags.audio_tags")),
            ]
            .into_iter()
            .chain((0..128).map(|index| (TagScope::Cover(index), tr("tags.cover_tags"))))
            {
                let indices: Vec<_> = draft
                    .values
                    .extra_tags
                    .iter()
                    .enumerate()
                    .filter(|(_, field)| {
                        field.scope == scope
                            && !(scope == TagScope::File
                                && COMMON_TAGS
                                    .iter()
                                    .any(|(key, _)| field.key.eq_ignore_ascii_case(key)))
                    })
                    .map(|(index, _)| index)
                    .collect();
                if indices.is_empty() {
                    continue;
                }
                let heading = match scope {
                    TagScope::Cover(index) => format!("{heading} · {}", index + 1),
                    _ => heading.into(),
                };
                let enabled = match scope {
                    TagScope::Cover(index) => {
                        !draft.values.remove_cover && (draft.values.cover.is_none() || index == 0)
                    }
                    _ => true,
                };
                ui.add_space(8.0);
                ui.add_enabled_ui(enabled, |ui| {
                    pref_group(ui, Some((&heading, "")), |ui| {
                        for index in indices {
                            let field = &mut draft.values.extra_tags[index];
                            let label = field.key.clone();
                            extra_field(ui, path, field, &label);
                        }
                    });
                });
            }
            ui.add_space(GROUP_GAP);
            pref_group(ui, None, |ui| {
                widgets::pref_block(ui, tr("tags.add_tag"), |ui| {
                    ui.add_sized(
                        [ui.available_width(), theme::BUTTON_HEIGHT],
                        TextEdit::singleline(&mut draft.new_key)
                            .id_salt((path, "new_tag_key"))
                            .char_limit(128)
                            .hint_text(tr("tags.key_hint")),
                    );
                    let key = draft.new_key.trim();
                    let duplicate =
                        ["title", "artist", "album"]
                            .iter()
                            .zip(draft.values.tag_scopes)
                            .any(|(name, scope)| {
                                scope == TagScope::File && key.eq_ignore_ascii_case(name)
                            })
                            || draft.values.extra_tags.iter().any(|f| {
                                f.scope == TagScope::File && f.key.eq_ignore_ascii_case(key)
                            });
                    if duplicate {
                        ui.label(tr("tags.duplicate"));
                    }
                    let can_add = !crate::tagging::generated_key(key)
                        && crate::tagging::valid_key(key)
                        && !duplicate
                        && draft.values.extra_tags.len() < crate::tagging::MAX_EXTRA_TAGS;
                    if ui
                        .add_enabled_ui(can_add, |ui| {
                            widgets::button(
                                ui,
                                Some(icons::PLUS),
                                tr("tags.add_tag"),
                                Kind::Secondary,
                                Size::Small,
                            )
                        })
                        .inner
                        .clicked()
                    {
                        draft.values.extra_tags.push(TagField {
                            key: key.into(),
                            ..Default::default()
                        });
                        draft.new_key.clear();
                    }
                });
            });
        },
    );
}

#[cfg(test)]
mod extra_draft_tests {
    use super::*;
    use crate::tagging::{TagField, TagScope};
    #[test]
    fn loading_other_tags_preserves_early_edits_and_cover_labels_stay_separate() {
        let mut draft = Draft::new(Path::new("song.mp3"));
        let comment = draft
            .values
            .extra_tags
            .iter_mut()
            .find(|f| f.key == "comment")
            .unwrap();
        comment.value = "My comment".into();
        comment.write = true;
        draft.load(Some(&crate::probe::AudioTags {
            extra: vec![
                TagField {
                    key: "comment".into(),
                    value: "Original".into(),
                    ..Default::default()
                },
                TagField {
                    key: "date".into(),
                    value: "2026".into(),
                    ..Default::default()
                },
                TagField {
                    scope: TagScope::Cover(0),
                    key: "title".into(),
                    value: "Album cover".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        }));
        assert!(draft.values.title.is_empty());
        assert_eq!(
            draft
                .values
                .extra_tags
                .iter()
                .find(|f| f.key == "comment")
                .unwrap()
                .value,
            "My comment"
        );
        assert_eq!(
            draft
                .values
                .extra_tags
                .iter()
                .find(|f| f.key == "date")
                .unwrap()
                .value,
            "2026"
        );
        assert!(
            draft
                .values
                .extra_tags
                .iter()
                .any(|f| f.scope == TagScope::Cover(0) && f.key == "title")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{AudioTags, MediaInfo};

    fn details(title: &str) -> FileDetails {
        FileDetails {
            bytes: None,
            duration: Some(10.0),
            probed: true,
            generation: 1,
            info: Some(MediaInfo {
                tags: Some(AudioTags {
                    title: title.into(),
                    artist: "Artist".into(),
                    album: "Album".into(),
                    has_cover: true,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn late_metadata_never_replaces_typed_or_cleared_fields() {
        let mut draft = Draft::new(Path::new("song.flac"));
        draft.values.title = "My title".into();
        draft.values.tag_write = [true, false, true];
        draft.load(details("Original").info.unwrap().tags.as_ref());
        assert_eq!(draft.values.title, "My title");
        assert_eq!(draft.values.artist, "Artist");
        assert_eq!(draft.values.album, "");
        assert_eq!(draft.values.tag_format, AudioFormat::Flac);
        assert!(draft.has_cover);
    }

    #[test]
    fn separate_files_keep_separate_values_and_removed_drafts_are_discarded() {
        let files: Vec<PathBuf> = ["first.mp3", "second.flac"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let info = HashMap::from([
            (files[0].clone(), details("First")),
            (files[1].clone(), details("Second")),
        ]);
        let mut editor = TagEditor::default();
        editor.sync(&files, &info);
        editor.drafts.get_mut(&files[0]).unwrap().values.title = "Edited".into();
        editor.drafts.get_mut(&files[0]).unwrap().values.tag_write[0] = true;
        editor.selected = Some(files[1].clone());
        editor.sync(&files, &info);
        assert_eq!(editor.drafts[&files[0]].values.title, "Edited");
        assert_eq!(editor.drafts[&files[1]].values.title, "Second");
        let options = Options::default();
        let dir = tempfile::tempdir().unwrap();
        let selection = Selection {
            tool: Tool::EditTags,
            files: &files,
            url: "",
            output_dir: Some(dir.path()),
            options: &options,
        };
        let specs = editor.plan(&selection, &mut HashSet::new()).unwrap();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].options.media_tools.title, "Edited");
        assert_eq!(specs[1].options.media_tools.title, "Second");
        assert_eq!(specs[0].output.extension().unwrap(), "mp3");
        assert_eq!(specs[1].output.extension().unwrap(), "flac");
        assert!(!specs[1].options.media_tools.tag_write[0]);
        editor.sync(&files[..1], &info);
        assert!(!editor.drafts.contains_key(&files[1]));
        assert_eq!(editor.selected.as_ref(), Some(&files[0]));
    }

    #[test]
    fn tag_fields_fit_narrow_and_wide_editors_in_both_themes() {
        for width in [340.0, 700.0] {
            for choice in [ThemeChoice::Light, ThemeChoice::Dark] {
                let ctx = egui::Context::default();
                icons::install(&ctx);
                theme::apply(&ctx, choice, true);
                let files = vec![PathBuf::from("first.mp3"), PathBuf::from("second.flac")];
                let info = HashMap::from([(files[0].clone(), details(&"שלום café ".repeat(80)))]);
                let mut editor = TagEditor::default();
                editor.sync(&files, &info);
                let mut used = 0.0;
                for _ in 0..8 {
                    ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 1400.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            editor.show(ui);
                            used = ui.min_rect().width();
                        },
                    )
                    .textures_delta
                    .clear();
                }
                assert!(used <= width, "{used} > {width}");
                assert_eq!(
                    editor.drafts[&files[0]].values.title,
                    "שלום café ".repeat(80)
                );
            }
        }
    }
}

#[cfg(test)]
mod expanded_layout_tests {
    use super::*;
    use crate::tagging::{TagField, TagScope};
    #[test]
    fn expanded_music_and_scoped_tags_fit_both_themes_and_editor_widths() {
        let path = Path::new("song.mp3");
        for width in [340.0, 700.0] {
            for choice in [ThemeChoice::Light, ThemeChoice::Dark] {
                let ctx = egui::Context::default();
                icons::install(&ctx);
                theme::apply(&ctx, choice, true);
                let mut draft = Draft::new(path);
                draft.loaded = true;
                draft.has_cover = true;
                draft.values.extra_tags.extend([
                    TagField {
                        key: "custom_tag".into(),
                        value: "שלום café ".repeat(200),
                        ..Default::default()
                    },
                    TagField {
                        scope: TagScope::Audio,
                        key: "encoder".into(),
                        value: "Lavc62.28".into(),
                        ..Default::default()
                    },
                    TagField {
                        scope: TagScope::Cover(0),
                        key: "title".into(),
                        value: "Album cover".into(),
                        ..Default::default()
                    },
                ]);
                let mut used = 0.0;
                for _ in 0..8 {
                    ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 2400.0))), ..Default::default() }, |ui| {
                        for salt in ["more_music_tags", "other_metadata"] {
                            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id((salt, path)), true);
                            state.set_open(true);
                            state.store(ui.ctx());
                        }
                        extra_tags(ui, path, &mut draft);
                        used = ui.min_rect().width();
                    }).textures_delta.clear();
                }
                assert!(used <= width, "{used} > {width}");
                assert!(draft.values.extra_tags.iter().all(|field| !field.write));
            }
        }
    }
}
