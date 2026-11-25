//! Windows: plays the sounds of the reminder and of the "charged" notice (sounds.rs says what a setting can name). The
//! system's sound for an event is the one the app always played, whatever Windows' sound scheme has there. A sound of
//! Windows by name (a file of its Media folder) and the user's WAV files go through PlaySound, which needs no codec; an
//! MP3 goes through Windows' media player, there wherever Windows has its media features (an N edition without them
//! plays the system's sound instead). One sound at a time: a new one ends the one before.

use crate::log;
use crate::sounds::{self, Choice, Event, Kind};
use crate::win::wide;
use std::path::Path;
use std::ptr::null;
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::{w, HSTRING, PCSTR};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const SND_ASYNC: u32 = 0x0001;
const SND_NODEFAULT: u32 = 0x0002;
const SND_ALIAS: u32 = 0x0001_0000;
const SND_FILENAME: u32 = 0x0002_0000;
const MB_ICONHAND: u32 = 0x10;
const MB_ICONEXCLAMATION: u32 = 0x30;

/// What the settings' play button takes a system sound to last: Windows doesn't say, and the user may have put any
/// file there. Nothing is cut at this.
const SYSTEM_LENGTH: Duration = Duration::from_millis(1500);

#[derive(Default)]
pub struct Player {
    /// Playing an MP3; kept until the next sound or `stop`
    media: Option<MediaPlayer>,
}

impl Player {
    /// Starts the sound chosen for an event and returns how long it is expected to last (`sounds::MAX_PLAY` and more:
    /// the caller ends it there), or None when nothing sounds. A sound that can't be played is the system's instead.
    pub fn play(&mut self, choice: &Choice, event: Event) -> Option<Duration> {
        self.stop();
        match choice {
            Choice::None => None,
            Choice::System => Some(system_sound(event)),
            Choice::Named(id) => match sounds::named_path(id).filter(|path| path.is_file() && play_wave(path)) {
                Some(path) => Some(sounds::plays_for(&path)),
                None => {
                    log::write(&format!("Windows' sound {id} can't be played: the system's sound instead"));
                    Some(system_sound(event))
                }
            },
            Choice::File(name) => {
                let path = sounds::path_of(name);
                let started = path.is_file()
                    && match sounds::kind(&path) {
                        Some(Kind::Wav) => play_wave(&path),
                        Some(Kind::Mp3) => match self.play_media(&path) {
                            Ok(()) => true,
                            Err(e) => {
                                log::write(&format!("sound {name}: {e}"));
                                false
                            }
                        },
                        None => false,
                    };
                if started {
                    Some(sounds::plays_for(&path))
                } else {
                    log::write(&format!("sound {name} can't be played: the system's sound instead"));
                    Some(system_sound(event))
                }
            }
        }
    }

    /// Ends what is playing.
    pub fn stop(&mut self) {
        if let Some(play) = play_sound() {
            unsafe {
                play(null(), 0, 0);
            }
        }
        if let Some(player) = self.media.take() {
            let _ = player.Pause();
            let _ = player.Close();
        }
    }

    fn play_media(&mut self, path: &Path) -> windows::core::Result<()> {
        let player = MediaPlayer::new()?;
        // Not a media app: the keyboard's media keys and the volume flyout leave the sound alone
        player.CommandManager()?.SetIsEnabled(false)?;
        // A file Windows has no decoder for, or a damaged one: said later, on another thread
        player.MediaFailed(&TypedEventHandler::new(|_, _| {
            log::write("the sound file can't be played: the system's sound instead");
            unsafe {
                MessageBeep(MB_ICONEXCLAMATION);
            }
            Ok(())
        }))?;
        let uri = Uri::CreateUri(&HSTRING::from(file_uri(path)))?;
        player.SetSource(&MediaSource::CreateFromUri(&uri)?)?;
        player.Play()?;
        self.media = Some(player);
        Ok(())
    }
}

/// The system's own sound for an event, as the app played it before the sounds could be chosen: Exclamation for the
/// reminder, Critical Stop at the critical level (as the C# app's SystemSounds). The "charged" notice has the sound of
/// a Windows notification, which Windows plays with the notice itself: here it is only for hearing it in the settings.
fn system_sound(event: Event) -> Duration {
    unsafe {
        match event {
            Event::Low => MessageBeep(MB_ICONEXCLAMATION),
            Event::Critical => MessageBeep(MB_ICONHAND),
            Event::Charged => match play_sound() {
                Some(play) => play(wide("Notification.Default").as_ptr(), 0, SND_ALIAS | SND_ASYNC | SND_NODEFAULT),
                None => 0,
            },
        };
    }
    SYSTEM_LENGTH
}

/// A WAV file by PlaySound. Without Windows' default sound when it can't be played.
fn play_wave(path: &Path) -> bool {
    let name = wide(&path.to_string_lossy());
    play_sound().is_some_and(|play| unsafe { play(name.as_ptr(), 0, SND_FILENAME | SND_ASYNC | SND_NODEFAULT) != 0 })
}

type PlaySound = unsafe extern "system" fn(sound: *const u16, module: isize, flags: u32) -> i32;

/// winmm's PlaySoundW, looked up the first time a sound needs it and not imported: on a Windows without its
/// multimedia parts the app still starts, and plays the system's sounds.
fn play_sound() -> Option<PlaySound> {
    static FUNCTION: OnceLock<Option<usize>> = OnceLock::new();
    let address = *FUNCTION.get_or_init(|| unsafe {
        let winmm = LoadLibraryW(w!("winmm.dll")).ok()?;
        GetProcAddress(winmm, PCSTR(c"PlaySoundW".as_ptr().cast())).map(|function| function as usize)
    });
    address.map(|address| unsafe { std::mem::transmute::<usize, PlaySound>(address) })
}

/// A file's path as the URI the media player takes: whatever a name may hold (spaces, "#", "%", other alphabets)
/// percent-encoded.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file:///");
    for c in path.to_string_lossy().chars() {
        match c {
            '\\' => uri.push('/'),
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => uri.push(c),
            _ => {
                let mut bytes = [0u8; 4];
                for byte in c.encode_utf8(&mut bytes).bytes() {
                    uri.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    uri
}

/// Whether Windows shows notifications right now: not while a full-screen game or a presentation holds them back, or
/// the session is locked. The app's own sound goes only with a notice that is shown.
pub fn notices_are_shown() -> bool {
    const ACCEPTS_NOTIFICATIONS: i32 = 5;
    let mut state = 0i32;
    unsafe { SHQueryUserNotificationState(&mut state) >= 0 && state == ACCEPTS_NOTIFICATIONS }
}

#[link(name = "user32", kind = "raw-dylib")]
extern "system" {
    fn MessageBeep(kind: u32) -> i32;
}

#[link(name = "shell32", kind = "raw-dylib")]
extern "system" {
    fn SHQueryUserNotificationState(state: *mut i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_becomes_a_file_uri() {
        assert_eq!(
            file_uri(Path::new(r"C:\Users\Иван\AppData\Local\BatteryTray\Sounds\low battery #1 100%.mp3")),
            "file:///C:/Users/%D0%98%D0%B2%D0%B0%D0%BD/AppData/Local/BatteryTray/Sounds/low%20battery%20%231%20100%25.mp3"
        );
    }
}
