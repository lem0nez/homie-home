use std::time::Duration;

use cpal::{Device, Sample, SupportedStreamConfig};
use log::{error, info, warn};
use rodio::{source::SeekError, OutputStream, OutputStreamHandle, PlayError, Sink, StreamError};
use tokio::{sync::mpsc, task};

use crate::{
    audio::{AudioSource, AudioSourceProperties},
    core::human_duration,
    graphql::GraphQLError,
};

type PlayerResult<T> = Result<T, PlayerError>;

#[derive(Debug, strum::AsRefStr, thiserror::Error)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum PlayerError {
    #[error("Failed to create an output stream: {0}")]
    CreateOutputStreamError(StreamError),
    #[error("Failed to create a sink: {0}")]
    CreateSinkError(PlayError),
    #[error("Playback stream closed")]
    StreamClosed,

    // Errors related to the seeking.
    #[error("Failed to seek: {0}")]
    SeekFailed(SeekError),
    #[error("Total duration of the audio source is unknown")]
    UnknownTotalDuration,
    #[error("Percents number must be in range [0.00, 1.00]")]
    InvalidPercents,
}

impl GraphQLError for PlayerError {}

pub struct PlaybackProperties {
    /// _Secondary_ sink doesn't affect the primary one and other secondary sinks,
    /// so they are will be played together. And it doesn't have the playback control capability.
    ///
    /// If sink is _primary_, playback will be resumed (if paused) and
    /// currently playing audio (if has) will be replaced.
    pub secondary: bool,
    /// Multiplier for samples.
    pub volume: f32,
    pub source_props: AudioSourceProperties,
}

impl Default for PlaybackProperties {
    fn default() -> Self {
        Self {
            secondary: false,
            volume: f32::IDENTITY,
            source_props: AudioSourceProperties::default(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct PlaybackPosition {
    current: Duration,
    /// [None] if total duration is unknown.
    total: Option<Duration>,
}

#[async_graphql::Object]
impl PlaybackPosition {
    async fn current_ms(&self) -> u64 {
        self.current.as_millis() as u64
    }

    async fn current_human(&self) -> String {
        human_duration(self.current)
    }

    async fn total_ms(&self) -> Option<u64> {
        self.total.map(|total| total.as_millis() as u64)
    }

    async fn total_human(&self) -> Option<String> {
        self.total.map(human_duration)
    }

    /// Returns played part percents (from 0.00 to 1.00).
    async fn percents(&self) -> Option<f64> {
        self.total.map(|total| self.current.div_duration_f64(total))
    }
}

pub enum SeekTo {
    /// Seek to `total_duration * percents`. Number is in range `[0.00, 1.00]`.
    Percents(f64),
    /// Seek to the given position.
    Position(Duration),
}

#[derive(strum::Display)]
enum Command {
    Play(AudioSource, PlaybackProperties),

    // The following commands applicable for the primary sink only.
    IsPlaying,
    Resume,
    Pause,
    GetPosition,
    Seek(SeekTo),
}

enum Response {
    /// Returned on successful player instantiation.
    Initialized,
    PlayStarted,

    // For the primary sink only.
    BoolResult(bool),
    /// [None] means there is no playing (or paused) source.
    Position(Option<PlaybackPosition>),
}

pub struct Player {
    // When the command sender drops, playback thread finishes as well.
    command_tx: mpsc::Sender<Command>,
    result_rx: mpsc::Receiver<PlayerResult<Response>>,
}

impl Player {
    pub async fn new(
        device: Device,
        output_stream_config: SupportedStreamConfig,
    ) -> PlayerResult<Self> {
        let (command_tx, mut command_rx) = mpsc::channel::<Command>(1);
        let (result_tx, mut result_rx) = mpsc::channel(1);

        task::spawn_blocking(move || {
            let send_error = |err| {
                error!("Player error: {err}");
                let _ = result_tx.blocking_send(Err(err));
            };

            let (_stream, stream_handle) =
                match OutputStream::try_from_device_config(&device, output_stream_config) {
                    Ok(result) => result,
                    Err(e) => return send_error(PlayerError::CreateOutputStreamError(e)),
                };
            let primary_sink = match Sink::try_new(&stream_handle) {
                Ok(sink) => sink,
                Err(e) => return send_error(PlayerError::CreateSinkError(e)),
            };
            let _ = result_tx.blocking_send(Ok(Response::Initialized));
            info!("Playback started");

            let mut current_source_duration = None;
            while let Some(command) = command_rx.blocking_recv() {
                match handle_command(HandleInput {
                    command,
                    stream_handle: &stream_handle,
                    primary_sink: &primary_sink,
                    current_source_duration: &mut current_source_duration,
                }) {
                    Ok(response) => {
                        let _ = result_tx.blocking_send(Ok(response));
                    }
                    Err(e) => send_error(e),
                }
            }
            info!("Playback thread finished");
        });

        result_rx
            .recv()
            .await
            .map_or(Err(PlayerError::StreamClosed), |result| {
                result.map(|_| Self {
                    command_tx,
                    result_rx,
                })
            })
    }

    /// If the primary sink chosen and it's already playing a source, then it will be replaced.
    pub async fn play(
        &mut self,
        source: AudioSource,
        props: PlaybackProperties,
    ) -> PlayerResult<()> {
        self.perform(Command::Play(source, props)).await.map(|_| ())
    }

    /// Returns `false` if the primary sink is not playing.
    pub async fn is_playing(&mut self) -> PlayerResult<bool> {
        self.perform_and_get_bool(Command::IsPlaying).await
    }

    /// Returns `false` if there is no paused source in the primary sink.
    pub async fn resume(&mut self) -> PlayerResult<bool> {
        self.perform_and_get_bool(Command::Resume).await
    }

    /// Returns `false` if there is no playing source in the primary sink.
    pub async fn pause(&mut self) -> PlayerResult<bool> {
        self.perform_and_get_bool(Command::Pause).await
    }

    /// Returns [None] if the primary sink is empty.
    pub async fn position(&mut self) -> PlayerResult<Option<PlaybackPosition>> {
        self.perform(Command::GetPosition)
            .await
            .map(|response| match response {
                Response::Position(pos) => pos,
                _ => panic!("position response expected"),
            })
    }

    /// Returns `false` if the primary sink is empty.
    pub async fn seek(&mut self, to: SeekTo) -> PlayerResult<bool> {
        if let SeekTo::Percents(percents) = to {
            if !(0.00..1.00).contains(&percents) {
                return Err(PlayerError::InvalidPercents);
            }
        }
        self.perform_and_get_bool(Command::Seek(to)).await
    }

    async fn perform_and_get_bool(&mut self, command: Command) -> PlayerResult<bool> {
        self.perform(command).await.map(|response| match response {
            Response::BoolResult(result) => result,
            _ => panic!("boolean response expected"),
        })
    }

    async fn perform(&mut self, command: Command) -> PlayerResult<Response> {
        if self.result_rx.try_recv().is_ok() {
            // May happen when this method canceled while it was
            // waiting for result just after it sent a command.
            warn!("Unconsumed command result dropped");
        }
        self.command_tx
            .send(command)
            .await
            .map_err(|_| PlayerError::StreamClosed)?;
        self.result_rx
            .recv()
            .await
            .unwrap_or(Err(PlayerError::StreamClosed))
    }
}

struct HandleInput<'a> {
    command: Command,
    stream_handle: &'a OutputStreamHandle,
    primary_sink: &'a Sink,
    current_source_duration: &'a mut Option<Duration>,
}

fn handle_command(input: HandleInput) -> PlayerResult<Response> {
    let response = match input.command {
        Command::Play(source, props) => {
            let duration = source.duration();
            let play = |sink: &Sink, seek_to_zero: bool| {
                sink.set_volume(props.volume);
                source.append_to(sink, props.source_props);
                if seek_to_zero {
                    // Avoid a bug when the sink reports position of the previous source.
                    let _ = sink.try_seek(Duration::ZERO);
                }
                sink.play();
            };
            if props.secondary {
                let secondary_sink =
                    Sink::try_new(input.stream_handle).map_err(PlayerError::CreateSinkError)?;
                play(&secondary_sink, false);
                secondary_sink.detach();
            } else {
                // Empty the queue.
                input.primary_sink.stop();
                play(input.primary_sink, true);
                *input.current_source_duration = duration;
            }
            Response::PlayStarted
        }
        Command::IsPlaying => Response::BoolResult(is_playing(input.primary_sink)),
        Command::Resume => Response::BoolResult(
            if !input.primary_sink.is_paused() || input.primary_sink.empty() {
                false
            } else {
                input.primary_sink.play();
                true
            },
        ),
        Command::Pause => Response::BoolResult(
            is_playing(input.primary_sink)
                .then(|| input.primary_sink.pause())
                .is_some(),
        ),
        Command::GetPosition => {
            Response::Position((!input.primary_sink.empty()).then(|| PlaybackPosition {
                current: input.primary_sink.get_pos(),
                total: *input.current_source_duration,
            }))
        }
        Command::Seek(to) => Response::BoolResult(if input.primary_sink.empty() {
            false
        } else {
            let pos = match to {
                SeekTo::Percents(percents) => input
                    .current_source_duration
                    .ok_or(PlayerError::UnknownTotalDuration)?
                    .mul_f64(percents),
                SeekTo::Position(duration) => duration,
            };
            input
                .primary_sink
                .try_seek(pos)
                .map_err(PlayerError::SeekFailed)?;
            true
        }),
    };
    Ok(response)
}

fn is_playing(sink: &Sink) -> bool {
    !(sink.is_paused() || sink.empty())
}
