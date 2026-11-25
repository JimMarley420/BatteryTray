//! The sounds of the reminder and of the "charged" notice: what a setting can name (the system's own sound, none, one
//! of the app's own, a file the user added), the folder the user's files are kept in, how long a file plays, and the
//! app's own sounds, which are made here from a few tones rather than shipped as files. Playing is each app's own
//! (sound_player.rs on Windows, System.swift on macOS).

use crate::{loc, sys};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// A sound is cut off after this long, whatever the file holds.
pub const MAX_PLAY: Duration = Duration::from_secs(10);
/// The largest file the Sounds folder takes.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// The app's own sounds, as the settings name them and in the order the lists show them.
pub const BUILT_IN: [&str; 5] = ["drop", "chime", "pulse", "alarm", "done"];
/// Where the lists of sounds have a line between their parts: before these places in `choices`. The system's sound and
/// silence, the app's own sounds, then the user's files with "Custom file…" after them.
pub const BREAKS: [usize; 2] = [2, 2 + BUILT_IN.len()];

/// What a sound belongs to: the reminder at the reminder level, the reminder at the critical level, the notice that a
/// device has finished charging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Low,
    Critical,
    Charged,
}

impl Event {
    pub const ALL: [Event; 3] = [Event::Low, Event::Critical, Event::Charged];

    /// The event's row in the settings: 0, 1, 2.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: i32) -> Option<Event> {
        usize::try_from(index).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    /// As the macOS app names it in its commands, and the log.
    pub fn name(self) -> &'static str {
        ["low", "critical", "charged"][self.index()]
    }

    pub fn from_name(name: &str) -> Option<Event> {
        Self::ALL.into_iter().find(|e| e.name() == name)
    }
}

/// A sound as the settings keep it: "system", "none", one of `BUILT_IN`, or "file:<name>" for a file in the Sounds
/// folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    System,
    None,
    BuiltIn(&'static str),
    File(String),
}

impl Choice {
    /// Anything that isn't a sound (a hand edit, a later version's value) is the system's sound.
    pub fn parse(text: &str) -> Choice {
        if text == "none" {
            return Choice::None;
        }
        if let Some(name) = text.strip_prefix("file:") {
            return if is_sound_name(name) { Choice::File(name.to_string()) } else { Choice::System };
        }
        BUILT_IN.iter().copied().find(|&b| b == text).map_or(Choice::System, Choice::BuiltIn)
    }

    pub fn setting(&self) -> String {
        match self {
            Choice::System => "system".into(),
            Choice::None => "none".into(),
            Choice::BuiltIn(id) => (*id).into(),
            Choice::File(name) => format!("file:{name}"),
        }
    }

    /// What the lists call it: a file by its name, the others in the app's language.
    pub fn label(&self) -> String {
        match self {
            Choice::System => loc::t("sounds.system"),
            Choice::None => loc::t("sounds.none"),
            Choice::BuiltIn(id) => loc::t(&format!("sounds.{id}")),
            Choice::File(name) => name.clone(),
        }
    }
}

/// Everything a list of sounds offers, in its order: the system's sound, silence, the app's own, the user's files.
/// `BREAKS` says where the lines between them go.
pub fn choices(files: &[String]) -> Vec<Choice> {
    let mut list = vec![Choice::System, Choice::None];
    list.extend(BUILT_IN.iter().map(|&id| Choice::BuiltIn(id)));
    list.extend(files.iter().cloned().map(Choice::File));
    list
}

/// A file's name that can be a sound of the folder: a WAV or an MP3, a name and not a path.
fn is_sound_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && Path::new(name).file_name().is_some_and(|n| n == name)
        && kind(Path::new(name)).is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Wav,
    Mp3,
}

/// What a file is by its name.
pub fn kind(path: &Path) -> Option<Kind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "wav" => Some(Kind::Wav),
        "mp3" => Some(Kind::Mp3),
        _ => None,
    }
}

/// The user's sounds: `Sounds` next to settings.json. A file put there by hand is in the lists like one added in
/// the settings.
pub fn folder() -> PathBuf {
    sys::app_data_folder().join("Sounds")
}

pub fn path_of(name: &str) -> PathBuf {
    folder().join(name)
}

/// The sounds in the folder, by name.
pub fn files() -> Vec<String> {
    files_in(&folder())
}

pub fn files_in(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| is_sound_name(name))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// Why a file wasn't taken into the folder.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Not a WAV or an MP3, by its name or by what it starts with
    NotSound,
    TooBig,
    Io(String),
}

/// Copies a file the user picked into the Sounds folder and returns its name there. A file of the same name is
/// replaced: picking it again is how a sound is updated.
pub fn import(source: &Path) -> Result<String, ImportError> {
    import_into(source, &folder())
}

pub fn import_into(source: &Path, folder: &Path) -> Result<String, ImportError> {
    let name = source.file_name().and_then(|n| n.to_str()).filter(|n| is_sound_name(n)).ok_or(ImportError::NotSound)?;
    let io = |e: std::io::Error| ImportError::Io(e.to_string());
    let size = fs::metadata(source).map_err(io)?.len();
    if size > MAX_FILE_BYTES {
        return Err(ImportError::TooBig);
    }
    let mut start = [0u8; 12];
    let read = File::open(source).and_then(|mut f| f.read(&mut start)).map_err(io)?;
    if !looks_like(kind(source), &start[..read]) {
        return Err(ImportError::NotSound);
    }
    let target = folder.join(name);
    // Picked from the folder itself: it is there already
    if fs::canonicalize(source).ok().is_some_and(|s| fs::canonicalize(&target).ok() == Some(s)) {
        return Ok(name.to_string());
    }
    fs::create_dir_all(folder).map_err(io)?;
    fs::copy(source, &target).map_err(io)?;
    Ok(name.to_string())
}

/// Whether a file begins as its kind does: a RIFF/WAVE header, or an ID3 tag or an MPEG frame.
fn looks_like(kind: Option<Kind>, start: &[u8]) -> bool {
    match kind {
        Some(Kind::Wav) => start.len() >= 12 && &start[..4] == b"RIFF" && &start[8..12] == b"WAVE",
        Some(Kind::Mp3) => {
            start.starts_with(b"ID3") || (start.len() >= 2 && start[0] == 0xFF && start[1] & 0xE0 == 0xE0)
        }
        None => false,
    }
}

/// How long a sound file is: exactly for a WAV, by its first frame for an MP3 (exact with a Xing header or a
/// constant bit rate, close otherwise). None when the file doesn't say.
pub fn length(path: &Path) -> Option<Duration> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    match kind(path)? {
        Kind::Wav => wav_length(&mut file),
        Kind::Mp3 => mp3_length(&mut file, size),
    }
}

/// True for a file that plays longer than `MAX_PLAY`: the settings say that only its beginning will sound.
pub fn is_cut(path: &Path) -> bool {
    length(path).is_some_and(|l| l > MAX_PLAY)
}

/// How long a file sounds: its length, `MAX_PLAY` at most (and when the length isn't known).
pub fn plays_for(path: &Path) -> Duration {
    length(path).map_or(MAX_PLAY, |l| l.min(MAX_PLAY))
}

fn wav_length(file: &mut File) -> Option<Duration> {
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff).ok()?;
    if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
        return None;
    }
    let mut byte_rate = 0u32;
    // The chunks before the samples: "fmt " and whatever a tool has added (LIST, fact, bext)
    for _ in 0..64 {
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk).ok()?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        match &chunk[..4] {
            b"data" => {
                return (byte_rate > 0).then(|| Duration::from_secs_f64(size as f64 / byte_rate as f64));
            }
            b"fmt " if size >= 16 => {
                let mut format = [0u8; 16];
                file.read_exact(&mut format).ok()?;
                byte_rate = u32::from_le_bytes([format[8], format[9], format[10], format[11]]);
                file.seek(SeekFrom::Current((size - 16 + (size & 1)) as i64)).ok()?;
            }
            _ => {
                file.seek(SeekFrom::Current((size + (size & 1)) as i64)).ok()?;
            }
        }
    }
    None
}

fn mp3_length(file: &mut File, size: u64) -> Option<Duration> {
    // An ID3v2 tag (the title, often a cover picture) comes before the sound
    let mut tag = [0u8; 10];
    let mut start = 0u64;
    if file.read(&mut tag).ok()? == 10 && &tag[..3] == b"ID3" {
        let length = tag[6..10].iter().fold(0u64, |n, &b| (n << 7) | (b & 0x7F) as u64);
        start = 10 + length + if tag[5] & 0x10 != 0 { 10 } else { 0 };
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut head = [0u8; 512];
    let mut got = 0;
    while got < head.len() {
        match file.read(&mut head[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    let head = &head[..got];
    let at = (0..head.len().saturating_sub(3)).find(|&i| head[i] == 0xFF && head[i + 1] & 0xE0 == 0xE0)?;
    mp3_frame_length(&head[at..], size.saturating_sub(start + at as u64))
}

/// The length of an MP3 from its first frame (`frame` starts at the frame's header) and the bytes of sound from there
/// to the end of the file.
fn mp3_frame_length(frame: &[u8], sound_bytes: u64) -> Option<Duration> {
    const KBPS_1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
    const KBPS_2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    if frame.len() < 4 {
        return None;
    }
    // MPEG 1, 2 or 2.5 (1 is reserved); only Layer III is an MP3
    let version = (frame[1] >> 3) & 3;
    let layer = (frame[1] >> 1) & 3;
    let bit_rate = (frame[2] >> 4) as usize;
    let sample_rate = ((frame[2] >> 2) & 3) as usize;
    if version == 1 || layer != 1 || bit_rate == 0 || bit_rate == 15 || sample_rate == 3 {
        return None;
    }
    let first = version == 3;
    // MPEG 2 has half of MPEG 1's sample rates, MPEG 2.5 a quarter
    let rate = [44_100u32, 48_000, 32_000][sample_rate]
        >> match version {
            3 => 0,
            2 => 1,
            _ => 2,
        };
    // Written by the encoder into the first frame, after its side information: how many frames the file has
    let mono = frame[3] >> 6 == 3;
    let info = 4 + match (first, mono) {
        (true, false) => 32,
        (true, true) | (false, false) => 17,
        (false, true) => 9,
    };
    if frame.len() >= info + 12 && matches!(&frame[info..info + 4], b"Xing" | b"Info") && frame[info + 7] & 1 != 0 {
        let frames = u32::from_be_bytes([frame[info + 8], frame[info + 9], frame[info + 10], frame[info + 11]]);
        let samples = frames as u64 * if first { 1152 } else { 576 };
        return Some(Duration::from_secs_f64(samples as f64 / rate as f64));
    }
    let kbps = if first { KBPS_1[bit_rate] } else { KBPS_2[bit_rate] };
    Some(Duration::from_secs_f64(sound_bytes as f64 * 8.0 / (kbps as f64 * 1000.0)))
}

// The app's own sounds

const RATE: u32 = 44_100;
/// The tones' gains are those of a quiet sketch; the sounds are this much louder
const LOUDNESS: f32 = 2.0;

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
    Square,
}

/// One tone of a sound: `freq` Hz (sliding to `slide` by its end), from `at` seconds for `len` seconds. It comes up
/// in 12 ms and dies away from there to its end, as a struck thing does.
#[derive(Clone, Copy)]
struct Tone {
    freq: f32,
    slide: f32,
    at: f32,
    len: f32,
    wave: Wave,
    gain: f32,
}

const fn tone(freq: f32, at: f32, len: f32) -> Tone {
    Tone { freq, slide: freq, at, len, wave: Wave::Sine, gain: 0.22 }
}

const fn shaped(freq: f32, at: f32, len: f32, wave: Wave, gain: f32) -> Tone {
    Tone { freq, slide: freq, at, len, wave, gain }
}

fn tones(id: &str) -> &'static [Tone] {
    const DROP: [Tone; 1] = [Tone { slide: 659.0, ..tone(1318.0, 0.0, 0.32) }];
    const CHIME: [Tone; 2] = [tone(784.0, 0.0, 0.55), tone(1175.0, 0.15, 0.8)];
    const PULSE: [Tone; 2] =
        [shaped(880.0, 0.0, 0.12, Wave::Triangle, 0.3), shaped(880.0, 0.18, 0.12, Wave::Triangle, 0.3)];
    const ALARM: [Tone; 4] = [
        shaped(988.0, 0.0, 0.14, Wave::Square, 0.1),
        shaped(740.0, 0.17, 0.14, Wave::Square, 0.1),
        shaped(988.0, 0.34, 0.14, Wave::Square, 0.1),
        shaped(740.0, 0.51, 0.22, Wave::Square, 0.1),
    ];
    const DONE: [Tone; 3] = [tone(523.0, 0.0, 0.3), tone(659.0, 0.12, 0.3), tone(784.0, 0.24, 0.6)];
    match id {
        "drop" => &DROP,
        "chime" => &CHIME,
        "pulse" => &PULSE,
        "alarm" => &ALARM,
        "done" => &DONE,
        _ => &[],
    }
}

/// The sound as samples: 16 bits, one channel, `RATE` a second.
fn render(tones: &[Tone]) -> Vec<i16> {
    const ATTACK: f32 = 0.012;
    const QUIET: f32 = 0.0001;
    let end = tones.iter().map(|t| t.at + t.len).fold(0.0, f32::max);
    // A moment of silence after the last tone, so the output never ends on a click
    let count = ((end + 0.05) * RATE as f32) as usize;
    (0..count)
        .map(|n| {
            let time = n as f32 / RATE as f32;
            let sum: f32 = tones
                .iter()
                .filter(|t| time >= t.at && time < t.at + t.len)
                .map(|t| {
                    let local = time - t.at;
                    let level = if local < ATTACK {
                        QUIET * (t.gain / QUIET).powf(local / ATTACK)
                    } else {
                        t.gain * (QUIET / t.gain).powf((local - ATTACK) / (t.len - ATTACK))
                    };
                    let turns = if t.slide == t.freq {
                        t.freq * local
                    } else {
                        let ratio = t.slide / t.freq;
                        t.freq * t.len / ratio.ln() * (ratio.powf(local / t.len) - 1.0)
                    };
                    let wave = (turns * std::f32::consts::TAU).sin();
                    level
                        * match t.wave {
                            Wave::Sine => wave,
                            Wave::Triangle => wave.asin() * std::f32::consts::FRAC_2_PI,
                            Wave::Square => wave.signum(),
                        }
                })
                .sum();
            ((sum * LOUDNESS).clamp(-1.0, 1.0) * i16::MAX as f32) as i16
        })
        .collect()
}

/// The samples as a WAV file's bytes.
fn wav(samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    // PCM, one channel
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// One of the app's own sounds as a WAV file's bytes, made the first time any is asked for and kept from then on.
pub fn built_in_wav(id: &str) -> Option<&'static [u8]> {
    static SOUNDS: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let index = BUILT_IN.iter().position(|&b| b == id)?;
    let sounds = SOUNDS.get_or_init(|| BUILT_IN.iter().map(|id| wav(&render(tones(id)))).collect());
    Some(&sounds[index])
}

/// How long one of the app's own sounds is.
pub fn built_in_length(id: &str) -> Duration {
    let samples = built_in_wav(id).map_or(0, |w| (w.len() - 44) / 2);
    Duration::from_secs_f64(samples as f64 / RATE as f64)
}

/// One of the app's own sounds as a file, for a player that takes files (the macOS app): written into the app's
/// folder the first time, and again when it is gone or isn't what the app makes now.
pub fn built_in_file(id: &str) -> Option<PathBuf> {
    let bytes = built_in_wav(id)?;
    let folder = sys::app_data_folder().join("OwnSounds");
    let path = folder.join(format!("{id}.wav"));
    if fs::metadata(&path).map(|m| m.len()).ok() != Some(bytes.len() as u64) {
        fs::create_dir_all(&folder).ok()?;
        fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of the test's own, emptied before and after.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("BatteryTray-sounds-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_setting_names_a_sound_or_falls_back_to_the_system_s() {
        for text in ["system", "none", "drop", "done", "file:low battery.MP3", "file:сигнал.wav"] {
            assert_eq!(Choice::parse(text).setting(), text);
        }
        assert_eq!(Choice::parse("drop"), Choice::BuiltIn("drop"));
        // Not a sound: a later version's value, a hand edit, a path out of the folder, another kind of file
        for text in
            ["", "System", "gong", "file:", "file:..\\settings.json", "file:a/b.wav", "file:C:x.wav", "file:x.txt"]
        {
            assert_eq!(Choice::parse(text), Choice::System, "{text}");
        }
    }

    #[test]
    fn the_lists_go_system_silence_own_sounds_files() {
        let list = choices(&["b.wav".into()]);
        assert_eq!(list.len(), 8);
        assert_eq!((&list[0], &list[1]), (&Choice::System, &Choice::None));
        assert_eq!(list[BREAKS[0]], Choice::BuiltIn("drop"));
        assert_eq!(list[BREAKS[1]], Choice::File("b.wav".into()));
        assert_eq!(Event::from_index(2), Some(Event::Charged));
        assert_eq!(Event::from_index(3), None);
        assert_eq!(Event::from_name(Event::Critical.name()), Some(Event::Critical));
    }

    #[test]
    fn the_app_s_own_sounds_are_short_wavs() {
        for id in BUILT_IN {
            let bytes = built_in_wav(id).unwrap();
            assert!(looks_like(Some(Kind::Wav), bytes), "{id}");
            let length = built_in_length(id);
            assert!(length > Duration::from_millis(200) && length < Duration::from_secs(2), "{id}: {length:?}");
            // Something sounds, and nothing is cut off at the top
            let samples: Vec<i16> = bytes[44..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
            let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 3000 && peak < i16::MAX as u16, "{id}: {peak}");
            assert_eq!(*samples.last().unwrap(), 0, "{id} ends in silence");
        }
        assert!(built_in_wav("gong").is_none());
    }

    #[test]
    fn a_picked_file_is_copied_into_the_folder() {
        let from = Scratch::new("from");
        let into = Scratch::new("into");
        let sounds = into.0.join("Sounds");
        let wav_bytes = built_in_wav("drop").unwrap();
        let source = from.file("Low Battery.WAV", wav_bytes);
        assert_eq!(import_into(&source, &sounds).as_deref(), Ok("Low Battery.WAV"));
        from.file("b.mp3", b"ID3\x03\x00\x00\x00\x00\x00\x00rest");
        assert_eq!(import_into(&from.0.join("b.mp3"), &sounds).as_deref(), Ok("b.mp3"));
        // Again, from the folder itself: there already
        assert_eq!(import_into(&sounds.join("b.mp3"), &sounds).as_deref(), Ok("b.mp3"));
        // What isn't a sound stays out: by its name, by what is in it
        assert_eq!(import_into(&from.file("notes.txt", b"RIFF"), &sounds), Err(ImportError::NotSound));
        assert_eq!(import_into(&from.file("fake.wav", b"not a wave file at all"), &sounds), Err(ImportError::NotSound));
        assert_eq!(import_into(&from.file("empty.mp3", b""), &sounds), Err(ImportError::NotSound));
        assert!(matches!(import_into(&from.0.join("gone.wav"), &sounds), Err(ImportError::Io(_))));
        // The folder's sounds by name, whatever the case; other files and folders in it aren't sounds
        fs::write(sounds.join("readme.txt"), b"").unwrap();
        fs::create_dir(sounds.join("old.wav")).unwrap();
        assert_eq!(files_in(&sounds), ["b.mp3", "Low Battery.WAV"]);
        assert!(files_in(&into.0.join("none")).is_empty());
    }

    #[test]
    fn a_file_too_big_stays_out() {
        let from = Scratch::new("big");
        let path = from.0.join("long.wav");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        drop(file);
        assert_eq!(import_into(&path, &from.0.join("Sounds")), Err(ImportError::TooBig));
    }

    #[test]
    fn a_wav_says_how_long_it_is() {
        let folder = Scratch::new("wav");
        let path = folder.file("drop.wav", built_in_wav("drop").unwrap());
        assert_eq!(length(&path), Some(built_in_length("drop")));
        assert!(!is_cut(&path));
        assert_eq!(plays_for(&path), built_in_length("drop"));
        // Twelve seconds, with a chunk of an odd size before the samples
        let mut long = wav(&vec![0i16; RATE as usize * 12]);
        long.splice(36..36, *b"LIST\x03\x00\x00\x00abc\x00");
        let path = folder.file("long.wav", &long);
        assert_eq!(length(&path), Some(Duration::from_secs(12)));
        assert!(is_cut(&path));
        assert_eq!(plays_for(&path), MAX_PLAY);
        // No samples chunk: the length isn't known, and the sound is cut where every sound is
        let path = folder.file("broken.wav", &long[..36]);
        assert_eq!(length(&path), None);
        assert_eq!(plays_for(&path), MAX_PLAY);
    }

    #[test]
    fn an_mp3_s_length_comes_from_its_first_frame() {
        // MPEG 1 Layer III, 128 kbit/s, 44.1 kHz, stereo: 16 000 bytes a second
        let header = [0xFF, 0xFB, 0x90, 0x00];
        assert_eq!(mp3_frame_length(&header, 160_000), Some(Duration::from_secs(10)));
        // With the encoder's count of frames, the bit rate doesn't matter: 1000 frames of 1152 samples
        let mut frame = vec![0u8; 4 + 32 + 12];
        frame[..4].copy_from_slice(&header);
        frame[36..40].copy_from_slice(b"Xing");
        frame[43] = 1;
        frame[44..48].copy_from_slice(&1000u32.to_be_bytes());
        assert_eq!(mp3_frame_length(&frame, 1), Some(Duration::from_secs_f64(1_152_000.0 / 44_100.0)));
        // MPEG 2 Layer III, 64 kbit/s, 22.05 kHz, mono
        assert_eq!(mp3_frame_length(&[0xFF, 0xF3, 0x80, 0xC0], 80_000), Some(Duration::from_secs(10)));
        // Not Layer III, or a bit rate the header doesn't give
        assert_eq!(mp3_frame_length(&[0xFF, 0xFD, 0x90, 0x00], 1000), None);
        assert_eq!(mp3_frame_length(&[0xFF, 0xFB, 0x00, 0x00], 1000), None);

        // In a file: after an ID3 tag of 300 bytes
        let folder = Scratch::new("mp3");
        let mut bytes = b"ID3\x04\x00\x00\x00\x00\x02\x2C".to_vec();
        bytes.extend(std::iter::repeat_n(0u8, 300));
        bytes.extend_from_slice(&header);
        bytes.extend(std::iter::repeat_n(0u8, 32_000 - 4));
        let path = folder.file("two.mp3", &bytes);
        assert_eq!(length(&path), Some(Duration::from_secs(2)));
        assert_eq!(length(&folder.file("noise.mp3", &[0u8; 64])), None);
    }
}
