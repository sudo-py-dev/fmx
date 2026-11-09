//! Media tool recipes, kept pure so validation and stream selection can be tested.
use super::*;
use crate::options::MediaToolOptions;

pub(super) fn validate(tool: Tool, o: &MediaToolOptions) -> Result<(), PlanError> {
    let bounded = |v: f32, lo: f32, hi: f32| v.is_finite() && (lo..=hi).contains(&v);
    let valid = match tool {
        Tool::Fade => {
            bounded(o.fade_in_s, 0.0, 60.0)
                && bounded(o.fade_out_s, 0.0, 60.0)
                && (o.fade_in_s > 0.0 || o.fade_out_s > 0.0)
        }
        Tool::Denoise => bounded(o.denoise_strength, 0.1, 10.0),
        Tool::Sharpen => bounded(o.sharpen_strength, 0.1, 2.0),
        Tool::AudioDenoise => bounded(o.noise_reduction_db, 0.01, 30.0),
        Tool::Karaoke => bounded(o.vocal_reduction_percent, 0.0, 100.0),
        Tool::TrimSilence => {
            bounded(o.silence_threshold_db, -80.0, -10.0)
                && bounded(o.silence_duration_s, 0.01, 5.0)
        }
        Tool::ThumbnailSheet => {
            (1..=8).contains(&o.sheet_columns)
                && (1..=8).contains(&o.sheet_rows)
                && (80..=480).contains(&o.thumbnail_width)
        }
        Tool::EditTags => {
            matches!(
                o.tag_format,
                AudioFormat::Mp3 | AudioFormat::M4a | AudioFormat::Flac
            ) && [&o.title, &o.artist, &o.album]
                .iter()
                .all(|v| v.len() <= 4096 && !v.chars().any(char::is_control))
                && crate::tagging::valid_fields(&o.extra_tags, &o.tag_scopes)
                && !(o.remove_cover && o.cover.is_some())
                && o.cover.as_ref().is_none_or(|p| {
                    !p.as_os_str().is_empty()
                        && p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                            ["png", "jpg", "jpeg"]
                                .iter()
                                .any(|x| e.eq_ignore_ascii_case(x))
                        })
                })
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(PlanError::InvalidValue(tool.label().to_owned()))
    }
}

pub(super) fn build(
    tool: Tool,
    inputs: &[Input],
    output: &Path,
    opts: &Options,
) -> Option<Result<Plan, PlanError>> {
    if !matches!(
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
            | Tool::EditTags
    ) {
        return None;
    }
    Some(single(inputs).and_then(|input| recipe(tool, input, output, opts)))
}

fn recipe(tool: Tool, input: &Input, output: &Path, opts: &Options) -> Result<Plan, PlanError> {
    let o = &opts.media_tools;
    validate(tool, o)?;
    let duration = input.info.duration_s;
    let mut args = base();
    let buffers_stream = matches!(tool, Tool::Reverse | Tool::TrimSilence);
    if buffers_stream {
        // Avoid reserving a stack per host CPU within the address-space budget.
        args = args
            .switch("-xerror")
            .flag("-threads", "1")
            .flag("-filter_threads", "1");
    }
    if tool == Tool::EditTags {
        args = args.flag("-protocol_whitelist", "file,pipe");
    }
    let mut args = args.input(&input.path);
    let format = if matches!(tool, Tool::Reverse | Tool::Fade) {
        VideoFormat::Mkv
    } else {
        VideoFormat::Mp4
    };
    if input.info.has_video
        && matches!(
            tool,
            Tool::Reverse | Tool::Fade | Tool::Stabilize | Tool::Denoise | Tool::Sharpen
        )
        && uses_vaapi(format, opts)
    {
        args = args.flag("-vaapi_device", VAAPI_DEVICE);
    }
    match tool {
        Tool::Reverse | Tool::Fade => {
            if !input.info.has_video && !input.info.has_audio {
                return Err(PlanError::NoTracks);
            }
            // Explicit maps exclude attached artwork, subtitles and secondary streams.
            // Reversing/fading subtitles would otherwise leave their timing unchanged.
            let filters = |audio: bool| -> Result<String, PlanError> {
                if tool == Tool::Reverse {
                    return Ok(if audio {
                        "areverse,asetpts=PTS-STARTPTS"
                    } else {
                        "reverse,setpts=PTS-STARTPTS"
                    }
                    .into());
                }
                let name = if audio { "afade" } else { "fade" };
                let mut f = Vec::new();
                if o.fade_in_s > 0.0 {
                    f.push(format!("{name}=t=in:st=0:d={:.3}", o.fade_in_s));
                }
                if o.fade_out_s > 0.0 {
                    let d = duration
                        .filter(|d| d.is_finite() && *d > 0.0)
                        .ok_or(PlanError::UnknownDuration)?;
                    if f64::from(o.fade_in_s + o.fade_out_s) > d {
                        return Err(PlanError::InvalidValue(tr("tool.fade").into()));
                    }
                    f.push(format!(
                        "{name}=t=out:st={:.6}:d={:.3}",
                        d - f64::from(o.fade_out_s),
                        o.fade_out_s
                    ));
                } else if duration.is_some_and(|d| f64::from(o.fade_in_s) > d) {
                    return Err(PlanError::InvalidValue(tr("tool.fade").into()));
                }
                Ok(f.join(","))
            };
            if input.info.has_video {
                args = args.flag("-map", "0:V:0").extend(video_encode(
                    VideoFormat::Mkv,
                    opts,
                    &Edit::default(),
                    &[filters(false)?],
                    None,
                ));
            } else {
                args = args.switch("-vn").flag("-c:a", "flac");
            }
            if input.info.has_audio {
                args = args.flag("-map", "0:a:0").flag("-af", filters(true)?);
            }
            args = args.flag("-map_chapters", "-1");
        }
        Tool::Stabilize | Tool::Denoise | Tool::Sharpen => {
            with_video(input)?;
            let filter = match tool {
                Tool::Stabilize => "deshake".to_owned(),
                Tool::Denoise => format!("hqdn3d={:.3}", o.denoise_strength),
                _ => format!("unsharp=5:5:{:.3}:5:5:0", o.sharpen_strength),
            };
            args = args
                .flag("-map", "0:V:0")
                .flag("-map", "0:a:0?")
                .extend(video_encode(
                    VideoFormat::Mp4,
                    opts,
                    &Edit::default(),
                    &[filter],
                    None,
                ));
        }
        Tool::AudioDenoise | Tool::TrimSilence | Tool::Karaoke => {
            with_audio(input)?;
            let filter = match tool {
                Tool::AudioDenoise => format!("afftdn=nr={:.3}:tn=1", o.noise_reduction_db),
                // Only remove silence at the edges. Reverse for the trailing edge,
                // so pauses inside a recording remain intact.
                Tool::TrimSilence => {
                    let trim = format!(
                        "silenceremove=start_periods=1:start_duration={:.3}:start_threshold={:.3}dB:start_silence={:.3}",
                        o.silence_duration_s, o.silence_threshold_db, o.silence_duration_s
                    );
                    format!("{trim},areverse,{trim},areverse,asetpts=PTS-STARTPTS")
                }
                // Blend the original with the canceled center. Absolute coefficients
                // sum to one, so adjusting the amount introduces no peak gain.
                // At zero, keep the source's channel layout and samples untouched.
                _ if o.vocal_reduction_percent == 0.0 => "anull".into(),
                _ => {
                    let amount = f64::from(o.vocal_reduction_percent) / 100.0;
                    let cross = amount * 0.5;
                    let direct = 1.0 - cross;
                    format!(
                        "aformat=channel_layouts=stereo,pan=stereo|c0={direct:.6}*c0-{cross:.6}*c1|c1={direct:.6}*c1-{cross:.6}*c0"
                    )
                }
            };
            args = args
                .flag("-map", "0:a:0")
                .switch("-vn")
                .flag("-af", filter)
                .flag("-c:a", "flac");
        }
        Tool::ThumbnailSheet => {
            with_video(input)?;
            let d = duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .ok_or(PlanError::UnknownDuration)?;
            let count = o.sheet_columns * o.sheet_rows;
            let step = d / f64::from(count);
            let filter = format!(
                "select='gte(t,selected_n*{step:.9})',scale={}:{}:force_original_aspect_ratio=decrease:reset_sar=1,pad={}:{}:(ow-iw)/2:(oh-ih)/2,setsar=1,tile={}x{}:padding=4:margin=4",
                o.thumbnail_width,
                o.thumbnail_width * 9 / 16,
                o.thumbnail_width,
                o.thumbnail_width * 9 / 16,
                o.sheet_columns,
                o.sheet_rows
            );
            args = args
                .flag("-map", "0:V:0")
                .flag("-vf", filter)
                .flag("-frames:v", "1")
                .switch("-an")
                .flag("-update", "1");
        }
        Tool::WaveformVideo => {
            with_audio(input)?;
            let visual = if o.spectrum {
                "showspectrum=s=1280x720:slide=scroll:mode=combined:color=intensity:fps=30"
            } else {
                "showwaves=s=1280x720:mode=cline:rate=30:colors=0x3584e4"
            };
            let pixel_format = if uses_vaapi(VideoFormat::Mp4, opts) {
                args = args.flag("-vaapi_device", VAAPI_DEVICE);
                "format=nv12,hwupload"
            } else {
                "format=yuv420p"
            };
            args = args
                .flag(
                    "-filter_complex",
                    format!("[0:a:0]{visual},{pixel_format}[v]"),
                )
                .flag("-map", "[v]")
                .flag("-map", "0:a:0")
                .extend(video_codec(VideoFormat::Mp4, opts))
                .extend(audio_codec_for_video(VideoFormat::Mp4, opts))
                .flag("-movflags", "+faststart")
                .switch("-shortest");
        }
        Tool::EditTags => {
            with_audio(input)?;
            if let Some(cover) = &o.cover {
                args = args.flag("-protocol_whitelist", "file,pipe").input(cover);
            }
            args = args.flag("-map", "0:a:0").flag("-map_metadata", "0");
            // Exclude real video while retaining attached artwork.
            if o.cover.is_some() {
                args = args.flag("-map", "1:v:0");
            } else if !o.remove_cover {
                args = args.flag("-map", "0:v?").flag("-map", "-0:V?");
            }
            let source = input
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if source.eq_ignore_ascii_case(o.tag_format.ext()) {
                args = args.flag("-c:a", "copy");
            } else {
                args = args.extend(audio_codec(o.tag_format, opts));
            }
            args = args
                .flag("-c:v", "copy")
                .flag("-disposition:v", "attached_pic");
            for (index, (key, value)) in [
                ("title", &o.title),
                ("artist", &o.artist),
                ("album", &o.album),
            ]
            .into_iter()
            .enumerate()
            {
                if if o.tag_explicit {
                    o.tag_write[index]
                } else {
                    !value.trim().is_empty()
                } {
                    args = args.flag(
                        &crate::tagging::metadata_flag(o.tag_scopes[index]),
                        format!("{key}={value}"),
                    );
                }
            }
            let cover_count = if o.remove_cover {
                0
            } else if o.cover.is_some() {
                1
            } else {
                input
                    .info
                    .streams
                    .iter()
                    .filter(|s| s.attached_picture)
                    .count()
            };
            // Generic M4A tags use mdta; FFmpeg cannot combine that mode with
            // attached artwork. Keep its artwork-capable mode when covers exist.
            if o.tag_format == AudioFormat::M4a && cover_count == 0 {
                args = args.flag("-movflags", "+use_metadata_tags");
            }
            for field in o.extra_tags.iter().filter(|f| f.write) {
                if matches!(field.scope, crate::tagging::TagScope::Cover(n) if n as usize >= cover_count)
                {
                    continue;
                }
                args = args.flag(
                    &crate::tagging::metadata_flag(field.scope),
                    format!("{}={}", field.key, field.value),
                );
            }
            if o.tag_format == AudioFormat::Mp3 {
                args = args.flag("-id3v2_version", "3");
            }
        }
        _ => return Err(PlanError::UnsupportedTool),
    }
    if buffers_stream {
        args = args.flag("-threads", "1");
    }
    let expected = if tool == Tool::TrimSilence {
        None
    } else {
        duration
    };
    let mut plan = args.output(output).expect(expected);
    if buffers_stream {
        // Shared by all buffering filters in this child, including both streams
        // and both Trim Silence reversals. Never trust compressed size/duration.
        plan.memory_limit = Some(1024 * 1024 * 1024);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(video: bool, audio: bool, duration: Option<f64>) -> Input {
        Input {
            path: "source.mkv".into(),
            info: MediaInfo {
                has_video: video,
                has_audio: audio,
                duration_s: duration,
                ..MediaInfo::default()
            },
        }
    }

    fn plan(tool: Tool, input: &Input, opts: &Options) -> Result<Plan, PlanError> {
        build(
            tool,
            std::slice::from_ref(input),
            Path::new("output.mkv"),
            opts,
        )
        .unwrap()
    }

    #[test]
    fn effects_require_the_right_streams() {
        let opts = Options::default();
        for tool in [
            Tool::Stabilize,
            Tool::Denoise,
            Tool::Sharpen,
            Tool::ThumbnailSheet,
        ] {
            assert_eq!(
                plan(tool, &input(false, true, Some(10.0)), &opts),
                Err(PlanError::NoVideoStream)
            );
        }
        for tool in [
            Tool::AudioDenoise,
            Tool::TrimSilence,
            Tool::Karaoke,
            Tool::WaveformVideo,
            Tool::EditTags,
        ] {
            assert_eq!(
                plan(tool, &input(true, false, Some(10.0)), &opts),
                Err(PlanError::NoAudioStream)
            );
        }
        assert_eq!(
            plan(Tool::Reverse, &input(false, false, None), &opts),
            Err(PlanError::NoTracks)
        );
        assert!(matches!(
            build(Tool::Reverse, &[], Path::new("out.flac"), &opts),
            Some(Err(PlanError::WrongInputCount { .. }))
        ));
    }

    #[test]
    fn fade_checks_clip_length_and_unknown_duration() {
        let mut opts = Options::default();
        assert!(plan(Tool::Fade, &input(true, true, Some(1.0)), &opts).is_err());
        assert_eq!(
            plan(Tool::Fade, &input(true, true, None), &opts),
            Err(PlanError::UnknownDuration)
        );
        opts.media_tools.fade_out_s = 0.0;
        assert!(plan(Tool::Fade, &input(true, true, None), &opts).is_ok());
        opts.media_tools.fade_in_s = 0.0;
        assert!(plan(Tool::Fade, &input(true, true, Some(10.0)), &opts).is_err());
    }

    #[test]
    fn persisted_bad_values_are_rejected_without_panics() {
        let mut opts = Options::default();
        opts.media_tools.sheet_columns = u32::MAX;
        assert!(plan(Tool::ThumbnailSheet, &input(true, true, Some(10.0)), &opts).is_err());
        opts.media_tools.denoise_strength = f32::NAN;
        assert!(plan(Tool::Denoise, &input(true, true, Some(10.0)), &opts).is_err());
        opts.media_tools.fade_in_s = f32::INFINITY;
        assert!(plan(Tool::Fade, &input(true, true, Some(10.0)), &opts).is_err());
        opts.media_tools.tag_format = AudioFormat::Ape;
        assert!(plan(Tool::EditTags, &input(false, true, Some(10.0)), &opts).is_err());
    }

    #[test]
    fn reverse_filters_both_streams_and_excludes_cover_art() {
        let p = plan(
            Tool::Reverse,
            &input(true, true, Some(10.0)),
            &Options::default(),
        )
        .unwrap();
        let args: Vec<_> = p.args.iter().map(|a| a.to_string_lossy()).collect();
        assert!(args.iter().any(|a| a.contains("reverse,setpts")));
        assert!(args.iter().any(|a| a.contains("areverse,asetpts")));
        assert!(args.iter().any(|a| a == "0:V:0"));
        assert!(!args.iter().any(|a| a == "copy"));
    }

    #[test]
    fn buffering_tools_have_one_shared_budget_even_without_duration() {
        for tool in [
            Tool::Reverse,
            Tool::TrimSilence,
            Tool::Fade,
            Tool::AudioDenoise,
        ] {
            let p = plan(tool, &input(false, true, Some(10.0)), &Options::default()).unwrap();
            assert_eq!(
                p.memory_limit,
                if matches!(tool, Tool::Reverse | Tool::TrimSilence) {
                    Some(1024 * 1024 * 1024)
                } else {
                    None
                }
            );
        }
        assert!(
            plan(
                Tool::Reverse,
                &input(false, true, None),
                &Options::default()
            )
            .unwrap()
            .memory_limit
            .is_some()
        );
        assert!(
            plan(
                Tool::TrimSilence,
                &input(false, true, None),
                &Options::default()
            )
            .unwrap()
            .memory_limit
            .is_some()
        );
    }

    #[test]
    fn tags_are_passed_as_one_argument_and_conflicting_art_is_rejected() {
        let mut opts = Options::default();
        opts.media_tools.title = "name; -i other: [title]".into();
        let p = plan(Tool::EditTags, &input(false, true, None), &opts).unwrap();
        assert!(p.args.iter().any(|a| a == "title=name; -i other: [title]"));
        opts.media_tools.cover = Some("cover.png".into());
        opts.media_tools.remove_cover = true;
        assert!(plan(Tool::EditTags, &input(false, true, None), &opts).is_err());
        opts.media_tools.remove_cover = false;
        opts.media_tools.cover = Some("cover.svg".into());
        assert!(plan(Tool::EditTags, &input(false, true, None), &opts).is_err());
    }
}

#[cfg(test)]
mod vocal_tests {
    use super::*;

    #[test]
    fn vocal_reduction_rejects_invalid_saved_values_and_keeps_legacy_default() {
        let mut options: Options = serde_json::from_str("{}").unwrap();
        assert_eq!(options.media_tools.vocal_reduction_percent, 100.0);
        for value in [-1.0, 101.0, f32::NAN, f32::INFINITY] {
            options.media_tools.vocal_reduction_percent = value;
            assert!(options.validate(Tool::Karaoke).is_err());
        }
        for value in [0.0, 50.0, 100.0] {
            options.media_tools.vocal_reduction_percent = value;
            assert!(options.validate(Tool::Karaoke).is_ok());
            let saved = serde_json::to_string(&options).unwrap();
            let restored: Options = serde_json::from_str(&saved).unwrap();
            assert_eq!(restored.media_tools.vocal_reduction_percent, value);
        }
    }
}
