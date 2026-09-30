use std::{path::Path, process::Command};

use relay_recorder::{
    aac,
    clock::Timeline,
    h264::{self, AccessUnits},
    hls::{playlist_name, segment_name, AudioPacket, SegmentWriter, VideoPacket},
};

fn tool(name: &str) -> Option<String> {
    let ok = Command::new(name)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success());
    ok.then(|| name.to_string())
}

fn run(cmd: &mut Command) -> String {
    let out = cmd.output().expect("comando non avviato");
    assert!(
        out.status.success(),
        "{:?} fallito: {}",
        cmd,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn access_units(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut units: Vec<Vec<u8>> = Vec::new();
    for nal in h264::nal_units(stream) {
        if h264::nal_type(nal) == h264::NAL_AUD || units.is_empty() {
            units.push(Vec::new());
        }
        let last = units.last_mut().unwrap();
        last.extend_from_slice(&[0, 0, 0, 1]);
        last.extend_from_slice(nal);
    }
    units.retain(|u| {
        h264::nal_units(u)
            .iter()
            .any(|n| h264::nal_type(n) != h264::NAL_AUD)
    });
    units
}

fn adts_frames(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 7 <= stream.len() {
        assert!(aac::is_adts(&stream[i..]), "ADTS non valido a {i}");
        let len = aac::adts_frame_length(&stream[i..]);
        out.push(stream[i..i + len].to_vec());
        i += len;
    }
    out
}

fn probe(ffprobe: &str, path: &Path, entries: &str, stream: Option<&str>) -> String {
    let mut c = Command::new(ffprobe);
    c.args(["-v", "error"]);
    if let Some(s) = stream {
        c.args(["-select_streams", s, "-count_frames"]);
    }
    c.args(["-show_entries", entries, "-of", "csv=p=0"])
        .arg(path);
    run(&mut c).trim().to_string()
}

fn first(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

#[test]
fn ffmpeg_reads_our_segments_like_its_own() {
    let (Some(ffmpeg), Some(ffprobe)) = (tool("ffmpeg"), tool("ffprobe")) else {
        eprintln!("ffmpeg non trovato: test saltato");
        return;
    };
    let dir = std::env::temp_dir().join(format!("relay-recorder-ffmpeg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let video = dir.join("in.h264");
    let audio = dir.join("in.aac");
    run(Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30",
        ])
        .args([
            "-t",
            "10",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .args([
            "-g",
            "120",
            "-keyint_min",
            "120",
            "-sc_threshold",
            "0",
            "-bf",
            "0",
        ])
        .args(["-bsf:v", "h264_metadata=aud=insert", "-f", "h264"])
        .arg(&video));
    run(Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
        ])
        .args([
            "-t", "10", "-ac", "2", "-c:a", "aac", "-b:a", "160k", "-f", "adts",
        ])
        .arg(&audio));

    let units = access_units(&std::fs::read(&video).unwrap());
    let frames = adts_frames(&std::fs::read(&audio).unwrap());
    assert_eq!(units.len(), 300);

    let timeline = Timeline {
        fps: 30,
        segment_secs: 4,
        origin_unix: 0.0,
        start_segment: 5,
    };
    let out = dir.join("out");
    let mut writer = SegmentWriter::new(&out, playlist_name(0), 5, 30, 4, true);
    let mut prepare = AccessUnits::default();
    let mut next_audio = 0usize;
    for (k, unit) in units.iter().enumerate() {
        let (data, keyframe) = prepare.prepare(unit);
        writer
            .push_video(VideoPacket {
                frame: k as u64,
                pts: timeline.video_pts(k as u64),
                keyframe,
                data,
            })
            .unwrap();
        let until = timeline.video_pts(k as u64 + 1);
        while next_audio < frames.len()
            && timeline.audio_pts(next_audio as u64 * aac::SAMPLES_PER_FRAME) < until
        {
            writer
                .push_audio(AudioPacket {
                    pts: timeline.audio_pts(next_audio as u64 * aac::SAMPLES_PER_FRAME),
                    data: frames[next_audio].clone(),
                })
                .unwrap();
            next_audio += 1;
        }
    }
    let (summary, _) = writer.finish(None).unwrap();
    assert_eq!(summary.segments, vec![(5, 4.0), (6, 4.0), (7, 2.0)]);

    for (index, frames) in [(5u64, 120usize), (6, 120), (7, 60)] {
        let path = out.join(segment_name(index));
        let codecs = probe(&ffprobe, &path, "stream=codec_name", None);
        let mut names: Vec<&str> = codecs.lines().filter(|l| !l.is_empty()).collect();
        names.dedup();
        names.truncate(2);
        assert_eq!(names, vec!["h264", "aac"], "{index}");
        let counted: usize = first(&probe(
            &ffprobe,
            &path,
            "stream=nb_read_frames",
            Some("v:0"),
        ))
        .parse()
        .unwrap();
        assert_eq!(counted, frames, "fotogrammi nel pezzo {index}");
        let start: f64 = first(&probe(&ffprobe, &path, "format=start_time", None))
            .parse()
            .unwrap();
        let expected = 1.4 + (index as f64) * 4.0;
        assert!(
            (start - expected).abs() < 0.05,
            "inizio {start} invece di {expected}"
        );
    }

    let joined = format!(
        "concat:{}|{}|{}",
        out.join(segment_name(5)).display(),
        out.join(segment_name(6)).display(),
        out.join(segment_name(7)).display()
    );
    let decoded = Command::new(&ffmpeg)
        .args(["-v", "error", "-i", &joined, "-f", "null", "-"])
        .output()
        .unwrap();
    assert!(decoded.status.success());
    assert!(
        String::from_utf8_lossy(&decoded.stderr).trim().is_empty(),
        "errori di decodifica: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );

    let all = out.join("all.ts");
    let mut bytes = Vec::new();
    for index in 5..=7 {
        bytes.extend(std::fs::read(out.join(segment_name(index))).unwrap());
    }
    std::fs::write(&all, bytes).unwrap();
    let mp4 = out.join("all.mp4");
    let remux = Command::new(&ffmpeg)
        .args(["-v", "error", "-y", "-i"])
        .arg(&all)
        .args(["-c", "copy", "-movflags", "+faststart", "-f", "mp4"])
        .arg(&mp4)
        .output()
        .unwrap();
    assert!(
        remux.status.success() && remux.stderr.is_empty(),
        "rimux in mp4 come fa il server: {}",
        String::from_utf8_lossy(&remux.stderr)
    );
    let mp4_duration: f64 = first(&probe(&ffprobe, &mp4, "format=duration", None))
        .parse()
        .unwrap();
    assert!(
        (mp4_duration - 10.0).abs() < 0.1,
        "durata dell'mp4 {mp4_duration}"
    );

    let playlist = out.join(playlist_name(0));
    let duration: f64 = first(&probe(&ffprobe, &playlist, "format=duration", None))
        .parse()
        .unwrap();
    assert!(
        (duration - 10.0).abs() < 0.1,
        "durata della playlist {duration}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
