use cpal::{Device, Sample, SupportedStreamConfig};
use log::{debug, error, info};
use rodio::{OutputStream, OutputStreamHandle, PlayError, Sink, StreamError};
use tokio::{sync::mpsc, task};

use crate::audio::{AudioSource, AudioSourceProperties};

type PlayerResult<T> = Result<T, PlayerError>;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("failed to create an output stream: {0}")]
    CreateOutputStream(StreamError),
    #[error("failed to create a sink: {0}")]
    CreateSink(PlayError),
    #[error("playback stream closed")]
    StreamClosed,
}

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

#[derive(strum::Display)]
enum Command {
    Play(AudioSource, PlaybackProperties),
    // The following four commands applicable for the primary sink only.
    IsPlaying,
    Resume,
    Pause,
    Stop,
}

enum Response {
    /// Returned on successful player instantiation.
    Initialized,
    PlayStarted,
    PrimaryPlaybackResult(bool),
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
                    Err(e) => return send_error(PlayerError::CreateOutputStream(e)),
                };
            let primary_sink = match Sink::try_new(&stream_handle) {
                Ok(sink) => sink,
                Err(e) => return send_error(PlayerError::CreateSink(e)),
            };
            let _ = result_tx.blocking_send(Ok(Response::Initialized));
            info!("Playback started");

            while let Some(command) = command_rx.blocking_recv() {
                let command_str = command.to_string();
                match handle_command(command, &stream_handle, &primary_sink) {
                    Ok(response) => {
                        let _ = result_tx.blocking_send(Ok(response));
                    }
                    Err(e) => send_error(e),
                }
                debug!("Command {command_str} handled");
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

    pub async fn play(
        &mut self,
        source: AudioSource,
        props: PlaybackProperties,
    ) -> PlayerResult<()> {
        self.perform(Command::Play(source, props)).await.map(|_| ())
    }

    /// Is _primary_ playback playing some source.
    pub async fn is_playing(&mut self) -> PlayerResult<bool> {
        self.primary_playback_command(Command::IsPlaying).await
    }

    /// Resume the _primary_ playback.
    pub async fn resume(&mut self) -> PlayerResult<bool> {
        self.primary_playback_command(Command::Resume).await
    }

    /// Pause the _primary_ playback.
    pub async fn pause(&mut self) -> PlayerResult<bool> {
        self.primary_playback_command(Command::Pause).await
    }

    /// Stop the _primary_ playback.
    pub async fn stop(&mut self) -> PlayerResult<bool> {
        self.primary_playback_command(Command::Stop).await
    }

    async fn primary_playback_command(&mut self, command: Command) -> PlayerResult<bool> {
        self.perform(command).await.map(|response| match response {
            Response::PrimaryPlaybackResult(status) => status,
            _ => panic!("primary playback response expected"),
        })
    }

    async fn perform(&mut self, command: Command) -> PlayerResult<Response> {
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

fn handle_command(
    command: Command,
    stream_handle: &OutputStreamHandle,
    primary_sink: &Sink,
) -> PlayerResult<Response> {
    match command {
        Command::Play(source, props) => {
            let play = |sink: &Sink| {
                sink.set_volume(props.volume);
                source.append_to(sink, props.source_props);
                sink.play();
            };
            if props.secondary {
                let secondary_sink = match Sink::try_new(stream_handle) {
                    Ok(sink) => sink,
                    Err(e) => return Err(PlayerError::CreateSink(e)),
                };
                play(&secondary_sink);
                secondary_sink.detach();
            } else {
                // Empty the queue.
                primary_sink.stop();
                play(primary_sink);
            }
            Ok(Response::PlayStarted)
        }
        Command::IsPlaying => Ok(Response::PrimaryPlaybackResult(
            !primary_sink.is_paused() && !primary_sink.empty(),
        )),
        Command::Resume => Ok(Response::PrimaryPlaybackResult(
            if !primary_sink.is_paused() || primary_sink.empty() {
                false
            } else {
                primary_sink.play();
                true
            },
        )),
        Command::Pause => Ok(Response::PrimaryPlaybackResult(
            if primary_sink.is_paused() || primary_sink.empty() {
                false
            } else {
                primary_sink.pause();
                true
            },
        )),
        Command::Stop => Ok(Response::PrimaryPlaybackResult(if primary_sink.empty() {
            false
        } else {
            primary_sink.stop();
            true
        })),
    }
}
