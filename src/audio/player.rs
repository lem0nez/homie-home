use cpal::{Device, Sample, SupportedStreamConfig};
use log::{error, info};
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
    secondary: bool,
    /// Multiplier for samples.
    volume: f32,
    source_props: AudioSourceProperties,
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
    ResumePrimary,
    PausePrimary,
    StopPrimary,
}

enum Response {
    /// Returned on successful player instantiation.
    Initialized,
    PlayStarted,
    /// For resume, pause and stop commands. `true` means that an action performed.
    PrimaryPlayback(bool),
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
                info!("Command {command_str} handled");
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
        self.do_command(Command::Play(source, props))
            .await
            .map(|_| ())
    }

    pub async fn resume_primary(&mut self) -> PlayerResult<bool> {
        self.do_primary_playback_command(Command::ResumePrimary)
            .await
    }

    pub async fn pause_primary(&mut self) -> PlayerResult<bool> {
        self.do_primary_playback_command(Command::PausePrimary)
            .await
    }

    pub async fn stop_primary(&mut self) -> PlayerResult<bool> {
        self.do_primary_playback_command(Command::StopPrimary).await
    }

    /// Returns `true` if some action was performed.
    async fn do_primary_playback_command(&mut self, command: Command) -> PlayerResult<bool> {
        self.do_command(command)
            .await
            .map(|response| match response {
                Response::PrimaryPlayback(status) => status,
                _ => panic!("playback response expected"),
            })
    }

    async fn do_command(&mut self, command: Command) -> PlayerResult<Response> {
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
        Command::ResumePrimary => Ok(Response::PrimaryPlayback(
            if !primary_sink.is_paused() || primary_sink.empty() {
                false
            } else {
                primary_sink.play();
                true
            },
        )),
        Command::PausePrimary => Ok(Response::PrimaryPlayback(
            if primary_sink.is_paused() || primary_sink.empty() {
                false
            } else {
                primary_sink.pause();
                true
            },
        )),
        Command::StopPrimary => Ok(Response::PrimaryPlayback(if primary_sink.empty() {
            false
        } else {
            primary_sink.stop();
            true
        })),
    }
}
