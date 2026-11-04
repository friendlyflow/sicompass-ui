//! Playing a video inside the window, like a picture.
//!
//! This crate has no decoder. A video backend (the application links FFmpeg) registers
//! itself with [`register_backend`] and opens files as [`VideoStream`]s, which hand out
//! decoded [`Packet`]s in order: pictures as RGBA, sound as stereo 48 kHz samples. A
//! login screen that never registers one pays for nothing.
//!
//! [`Player`] runs a stream: a thread decodes ahead into a bounded queue, and the main
//! thread, once a frame, takes what is due. The clock is the wall clock since play
//! began, held while paused. Sound is fed to an [`AudioSink`] a little ahead of it, so
//! the sink never holds more than a fraction of a second.
//!
//! Everything but the sink is plain Rust, so all of it is tested with a fake stream.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Sound is always handed over as this: interleaved stereo, signed 16-bit, 48 kHz.
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;

/// How far ahead of the clock sound is queued.
const AUDIO_LOOKAHEAD: Duration = Duration::from_millis(300);
/// How many decoded packets the decoder may run ahead.
const QUEUE_PACKETS: usize = 32;

/// What a video file is.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: Option<Duration>,
    pub has_audio: bool,
}

/// One decoded picture.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Where in the video it belongs.
    pub pts: Duration,
}

/// A stretch of decoded sound.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// Interleaved stereo.
    pub samples: Vec<i16>,
    pub pts: Duration,
}

/// What a stream hands out, in the order to present it.
#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Video(VideoFrame),
    Audio(AudioChunk),
    /// Nothing more will come.
    End,
}

/// An opened video file.
pub trait VideoStream: Send {
    fn info(&self) -> VideoInfo;
    /// The next picture or stretch of sound. Blocks while decoding. [`Packet::End`] at
    /// the end, and after it, again.
    fn next(&mut self) -> Packet;
}

/// Opens video files. The application registers one.
pub trait VideoBackend: Send + Sync {
    /// Open `path`, a file or an `asset:` URI. Pictures are made no bigger than
    /// `max_side` on their longer side, if given.
    fn open(&self, path: &str, max_side: Option<u32>) -> Result<Box<dyn VideoStream>, String>;
}

static BACKEND: OnceLock<Box<dyn VideoBackend>> = OnceLock::new();

/// Make video playable. Once: a second call is ignored.
pub fn register_backend(backend: Box<dyn VideoBackend>) {
    let _ = BACKEND.set(backend);
}

/// The registered backend, if any.
pub fn backend() -> Option<&'static dyn VideoBackend> {
    BACKEND.get().map(|b| b.as_ref())
}

/// Whether video can be played here.
pub fn available() -> bool {
    BACKEND.get().is_some()
}

/// The first picture of a video, no bigger than `max_side`: what a row shows before it
/// is played. `None` if it cannot be opened or has no picture.
pub fn first_frame(path: &str, max_side: u32) -> Option<VideoFrame> {
    let mut stream = backend()?.open(path, Some(max_side)).ok()?;
    // A stream may open on sound, so look a few packets in.
    for _ in 0..200 {
        match stream.next() {
            Packet::Video(frame) => return Some(frame),
            Packet::End => return None,
            Packet::Audio(_) => {}
        }
    }
    None
}

/// Names a video's first picture in the picture loader: `video-frame:` and the path.
pub const THUMBNAIL_PREFIX: &str = "video-frame:";

/// A video's size in pixels, from opening it and not playing it. `None` without a
/// backend or for a file it cannot open. Kept, like [`crate::image::dimensions`].
pub fn dimensions(path: &str) -> Option<(u32, u32)> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Sizes = HashMap<String, Option<(u32, u32)>>;
    static KNOWN: OnceLock<Mutex<Sizes>> = OnceLock::new();
    let backend = backend()?;
    let known = KNOWN.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(found) = known.lock().ok()?.get(path) {
        return *found;
    }
    let size = backend
        .open(path, None)
        .ok()
        .map(|stream| stream.info())
        .map(|info| (info.width, info.height));
    let mut known = known.lock().ok()?;
    if known.len() > 5_000 {
        known.clear();
    }
    known.insert(path.to_owned(), size);
    size
}

/// Sound that goes nowhere, for a computer with no audio device.
pub struct NullSink;

impl AudioSink for NullSink {
    fn queue(&mut self, _: &[i16]) {}
    fn set_paused(&mut self, _: bool) {}
    fn clear(&mut self) {}
}

/// The window's audio device.
pub struct SdlAudio {
    stream: sdl3::audio::AudioStreamOwner,
}

impl SdlAudio {
    /// Open the default playback device for stereo 48 kHz. `Err` where there is none,
    /// which the caller answers with [`NullSink`]: the picture still plays.
    pub fn open(sdl: &sdl3::Sdl) -> Result<SdlAudio, String> {
        let audio = sdl.audio().map_err(|e| e.to_string())?;
        let spec = sdl3::audio::AudioSpec {
            freq: Some(AUDIO_RATE as i32),
            channels: Some(AUDIO_CHANNELS as i32),
            format: Some(sdl3::audio::AudioFormat::s16_sys()),
        };
        let stream = audio
            .default_playback_device()
            .open_device_stream(Some(&spec))
            .map_err(|e| e.to_string())?;
        Ok(SdlAudio { stream })
    }
}

impl AudioSink for SdlAudio {
    fn queue(&mut self, samples: &[i16]) {
        let _ = self.stream.put_data_i16(samples);
    }
    fn set_paused(&mut self, paused: bool) {
        if paused {
            let _ = self.stream.pause();
        } else {
            let _ = self.stream.resume();
        }
    }
    fn clear(&mut self) {
        let _ = self.stream.clear();
    }
}

/// What the key handlers ask of the frame loop, which owns the player and the audio.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Open this video and start playing it.
    Open(String),
    /// Pause it, or resume.
    TogglePause,
    /// Stop and forget it.
    Stop,
}

/// A video that is open, with where its sound goes.
pub struct Active {
    pub player: Player,
    pub sink: Box<dyn AudioSink>,
    pub path: String,
    /// The clock `Player` is given: when this was opened.
    origin: std::time::Instant,
}

impl Active {
    /// Open `path` and start playing it. The sound goes to `sdl`'s audio device if it
    /// has one.
    pub fn start(path: &str, sdl: Option<&sdl3::Sdl>) -> Result<Active, String> {
        let player = Player::open(path)?;
        let mut sink: Box<dyn AudioSink> = match sdl.map(SdlAudio::open) {
            Some(Ok(audio)) => Box::new(audio),
            _ => Box::new(NullSink),
        };
        let origin = std::time::Instant::now();
        let mut active = Active {
            player,
            sink: Box::new(NullSink),
            path: path.to_owned(),
            origin,
        };
        active.player.play(Duration::ZERO, sink.as_mut());
        active.sink = sink;
        Ok(active)
    }

    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    /// Present what is due. Returns the new picture, if there is one.
    pub fn update(&mut self) -> Polled {
        let now = self.now();
        self.player.poll(now, self.sink.as_mut())
    }

    pub fn toggle_pause(&mut self) {
        let now = self.now();
        if self.player.is_paused() {
            self.player.play(now, self.sink.as_mut());
        } else {
            self.player.pause(now, self.sink.as_mut());
        }
    }
}

/// Where sound goes. The window's audio device implements this.
pub trait AudioSink {
    /// Add interleaved stereo samples to what is playing.
    fn queue(&mut self, samples: &[i16]);
    fn set_paused(&mut self, paused: bool);
    /// Drop what is queued.
    fn clear(&mut self);
}

/// What [`Player::poll`] found.
#[derive(Debug, Default, PartialEq)]
pub struct Polled {
    /// The picture to show now, if it changed.
    pub frame: Option<VideoFrame>,
    /// The video has played to its end.
    pub ended: bool,
}

/// Plays one video.
pub struct Player {
    info: VideoInfo,
    packets: Receiver<Packet>,
    /// The next packet, taken off the queue and waiting for its time.
    pending: Option<Packet>,
    /// The clock when play last started or resumed, and how far in it was.
    started_at: Option<Duration>,
    base: Duration,
    paused: bool,
    ended: bool,
}

impl Player {
    /// Open `path` and start decoding. Not yet playing: call [`Player::play`].
    pub fn open(path: &str) -> Result<Player, String> {
        let backend = backend().ok_or_else(|| "video is not available in this build".to_owned())?;
        Ok(Player::from_stream(backend.open(path, None)?))
    }

    /// A player over any stream, for a backend of its own and for tests.
    pub fn from_stream(mut stream: Box<dyn VideoStream>) -> Player {
        let info = stream.info();
        let (send, packets): (SyncSender<Packet>, Receiver<Packet>) = sync_channel(QUEUE_PACKETS);
        std::thread::spawn(move || {
            loop {
                let packet = stream.next();
                let last = packet == Packet::End;
                // The player going away closes the queue, which ends the thread.
                if send.send(packet).is_err() || last {
                    return;
                }
            }
        });
        Player {
            info,
            packets,
            pending: None,
            started_at: None,
            base: Duration::ZERO,
            paused: true,
            ended: false,
        }
    }

    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Begin, or resume. `now` is a clock that only goes forward.
    pub fn play(&mut self, now: Duration, sink: &mut dyn AudioSink) {
        if self.started_at.is_none() && !self.ended {
            self.started_at = Some(now);
            self.paused = false;
            sink.set_paused(false);
        }
    }

    pub fn pause(&mut self, now: Duration, sink: &mut dyn AudioSink) {
        if let Some(start) = self.started_at.take() {
            self.base += now.saturating_sub(start);
            self.paused = true;
            sink.set_paused(true);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn has_ended(&self) -> bool {
        self.ended
    }

    /// How far into the video, at `now`.
    pub fn position(&self, now: Duration) -> Duration {
        match self.started_at {
            Some(start) => self.base + now.saturating_sub(start),
            None => self.base,
        }
    }

    /// Present what is due at `now`: sound up to a little ahead, and the newest picture
    /// whose time has come. Older pictures that are also due are dropped, so a slow
    /// frame catches up instead of falling behind.
    pub fn poll(&mut self, now: Duration, sink: &mut dyn AudioSink) -> Polled {
        let mut out = Polled::default();
        if self.paused || self.ended {
            return out;
        }
        let clock = self.position(now);
        loop {
            if self.pending.is_none() {
                match self.packets.try_recv() {
                    Ok(packet) => self.pending = Some(packet),
                    Err(TryRecvError::Empty) => break,
                    // The decoder is gone without saying so.
                    Err(TryRecvError::Disconnected) => {
                        self.finish(sink);
                        out.ended = true;
                        break;
                    }
                }
            }
            match self.pending.as_ref() {
                Some(Packet::Audio(chunk)) if chunk.pts <= clock + AUDIO_LOOKAHEAD => {
                    if let Some(Packet::Audio(chunk)) = self.pending.take() {
                        sink.queue(&chunk.samples);
                    }
                }
                Some(Packet::Video(frame)) if frame.pts <= clock => {
                    if let Some(Packet::Video(frame)) = self.pending.take() {
                        out.frame = Some(frame);
                    }
                }
                Some(Packet::End) => {
                    self.pending = None;
                    // The last picture stays up, and sound already queued plays out.
                    self.finish(sink);
                    out.ended = true;
                    break;
                }
                _ => break,
            }
        }
        out
    }

    fn finish(&mut self, _sink: &mut dyn AudioSink) {
        self.ended = true;
        self.started_at = None;
        self.paused = true;
    }
}

/// Keep a player's stream alive across threads without copying it: the decoder thread
/// owns the stream, this only exists so a backend can share what it opened.
pub type SharedBackend = Arc<dyn VideoBackend>;

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// A stream of `frames` pictures at 25 per second, each 1x1, with sound in chunks of
    /// 20 ms, which ends after the last.
    pub struct FakeStream {
        pub frames: u32,
        next_frame: u32,
        next_audio: u32,
        pub with_audio: bool,
    }

    impl FakeStream {
        pub fn new(frames: u32, with_audio: bool) -> Self {
            FakeStream {
                frames,
                next_frame: 0,
                next_audio: 0,
                with_audio,
            }
        }
    }

    const FRAME: Duration = Duration::from_millis(40);
    const CHUNK: Duration = Duration::from_millis(20);

    impl VideoStream for FakeStream {
        fn info(&self) -> VideoInfo {
            VideoInfo {
                width: 1,
                height: 1,
                duration: Some(FRAME * self.frames),
                has_audio: self.with_audio,
            }
        }

        fn next(&mut self) -> Packet {
            let video_at = FRAME * self.next_frame;
            let audio_at = CHUNK * self.next_audio;
            let video_left = self.next_frame < self.frames;
            let audio_left = self.with_audio && audio_at < FRAME * self.frames;
            match (video_left, audio_left) {
                (false, false) => Packet::End,
                (true, false) => self.video(),
                (false, true) => self.audio(),
                (true, true) if audio_at <= video_at => self.audio(),
                (true, true) => self.video(),
            }
        }
    }

    impl FakeStream {
        fn video(&mut self) -> Packet {
            let pts = FRAME * self.next_frame;
            self.next_frame += 1;
            Packet::Video(VideoFrame {
                width: 1,
                height: 1,
                rgba: vec![self.next_frame as u8, 0, 0, 255],
                pts,
            })
        }

        fn audio(&mut self) -> Packet {
            let pts = CHUNK * self.next_audio;
            self.next_audio += 1;
            // 20 ms of stereo at 48 kHz.
            Packet::Audio(AudioChunk {
                samples: vec![0; (AUDIO_RATE / 50 * AUDIO_CHANNELS) as usize],
                pts,
            })
        }
    }

    /// A sink that remembers what it was given.
    #[derive(Default)]
    pub struct RecordingSink {
        pub queued: usize,
        pub paused: Vec<bool>,
        pub cleared: usize,
    }

    impl AudioSink for RecordingSink {
        fn queue(&mut self, samples: &[i16]) {
            self.queued += samples.len();
        }
        fn set_paused(&mut self, paused: bool) {
            self.paused.push(paused);
        }
        fn clear(&mut self) {
            self.cleared += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Wait for the decoder thread to fill the queue.
    fn settle() {
        std::thread::sleep(ms(50));
    }

    fn player(frames: u32, audio: bool) -> Player {
        Player::from_stream(Box::new(FakeStream::new(frames, audio)))
    }

    #[test]
    fn a_player_does_nothing_until_it_plays() {
        let mut p = player(5, true);
        let mut sink = RecordingSink::default();
        settle();
        assert_eq!(p.poll(ms(1000), &mut sink), Polled::default());
        assert!(p.is_paused());
        assert_eq!(sink.queued, 0);
    }

    #[test]
    fn the_first_picture_is_due_at_the_start_and_the_next_when_its_time_comes() {
        let mut p = player(5, false);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(100), &mut sink);
        let first = p.poll(ms(100), &mut sink);
        assert_eq!(first.frame.as_ref().map(|f| f.pts), Some(ms(0)));
        // 30 ms in: the second picture (40 ms) is not due yet.
        assert_eq!(p.poll(ms(130), &mut sink).frame, None);
        // 40 ms in: it is.
        assert_eq!(
            p.poll(ms(140), &mut sink).frame.map(|f| f.pts),
            Some(ms(40))
        );
    }

    /// A slow frame must catch up and not fall further behind.
    #[test]
    fn a_late_poll_shows_only_the_newest_due_picture() {
        let mut p = player(10, false);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        let got = p.poll(ms(170), &mut sink);
        // 170 ms in: pictures at 0, 40, 80, 120, 160 are due, and only 160 is shown.
        assert_eq!(got.frame.map(|f| f.pts), Some(ms(160)));
    }

    #[test]
    fn sound_is_queued_only_a_little_ahead_of_the_clock() {
        let mut p = player(50, true);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        p.poll(ms(0), &mut sink);
        // Up to 300 ms ahead: chunks at 0, 20, ... 300 ms, sixteen of 20 ms.
        let per_chunk = (AUDIO_RATE / 50 * AUDIO_CHANNELS) as usize;
        assert!(sink.queued > 0);
        assert!(sink.queued <= 16 * per_chunk, "{} samples", sink.queued);
        let before = sink.queued;
        p.poll(ms(100), &mut sink);
        assert!(sink.queued > before, "more as the clock moves");
        assert!(sink.queued <= 21 * per_chunk);
    }

    #[test]
    fn pausing_holds_the_clock_and_resuming_continues_from_there() {
        let mut p = player(50, false);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        p.poll(ms(100), &mut sink);
        p.pause(ms(100), &mut sink);
        assert_eq!(p.position(ms(5000)), ms(100));
        assert_eq!(p.poll(ms(5000), &mut sink), Polled::default());
        p.play(ms(5000), &mut sink);
        assert_eq!(p.position(ms(5040)), ms(140));
        assert_eq!(sink.paused, [false, true, false]);
    }

    #[test]
    fn the_end_is_reported_once_the_last_picture_is_shown() {
        let mut p = player(3, false);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        let mut ended = false;
        let mut last = None;
        for t in (0..1000).step_by(10) {
            let got = p.poll(ms(t), &mut sink);
            if let Some(f) = got.frame {
                last = Some(f.pts);
            }
            if got.ended {
                ended = true;
                break;
            }
        }
        assert!(ended);
        assert_eq!(last, Some(ms(80)));
        assert!(p.has_ended());
        // Nothing more after the end.
        assert_eq!(p.poll(ms(5000), &mut sink), Polled::default());
    }

    #[test]
    fn a_video_with_no_pictures_still_ends() {
        let mut p = player(0, true);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        let mut ended = false;
        for t in (0..2000).step_by(20) {
            ended |= p.poll(ms(t), &mut sink).ended;
        }
        assert!(ended);
    }

    /// The decoder runs ahead only as far as the queue lets it.
    #[test]
    fn the_decoder_is_held_back_by_the_queue() {
        let (decoded, counter) = {
            struct Counting(FakeStream, std::sync::Arc<std::sync::atomic::AtomicUsize>);
            impl VideoStream for Counting {
                fn info(&self) -> VideoInfo {
                    self.0.info()
                }
                fn next(&mut self) -> Packet {
                    self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    self.0.next()
                }
            }
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Counting(FakeStream::new(100_000, false), counter.clone()),
                counter,
            )
        };
        let _player = Player::from_stream(Box::new(decoded));
        settle();
        let n = counter.load(std::sync::atomic::Ordering::SeqCst);
        assert!(n <= QUEUE_PACKETS + 2, "decoded {n} packets ahead");
        assert!(n >= QUEUE_PACKETS, "and did fill the queue: {n}");
    }

    #[test]
    fn dropping_the_player_stops_the_decoder() {
        let mut p = player(100_000, false);
        let mut sink = RecordingSink::default();
        settle();
        p.play(ms(0), &mut sink);
        p.poll(ms(0), &mut sink);
        drop(p);
        // Nothing to assert but that the thread does not hang the process: the queue
        // closing is what ends it.
        settle();
    }

    #[test]
    fn a_stream_that_opens_on_sound_still_has_a_first_frame() {
        struct Backend;
        impl VideoBackend for Backend {
            fn open(&self, _: &str, _: Option<u32>) -> Result<Box<dyn VideoStream>, String> {
                Ok(Box::new(FakeStream::new(3, true)))
            }
        }
        // A local backend, since the registered one is for the whole process.
        let mut stream = Backend.open("x", None).unwrap();
        let mut found = None;
        for _ in 0..50 {
            match stream.next() {
                Packet::Video(f) => {
                    found = Some(f);
                    break;
                }
                Packet::End => break,
                Packet::Audio(_) => {}
            }
        }
        assert_eq!(found.map(|f| f.pts), Some(ms(0)));
    }

    #[test]
    fn without_a_backend_nothing_opens() {
        // The test process registers none.
        assert!(!available());
        assert!(first_frame("clip.mp4", 256).is_none());
        assert!(Player::open("clip.mp4").is_err());
    }
}
