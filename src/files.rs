use std::{
    fs::{self, File},
    ops::Deref,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_valid::{validation, Validate};
use strum::{EnumIter, IntoEnumIterator};

pub trait BaseDir<'a, T>: Clone + Deserialize<'a> + Validate {
    fn path(&self, item: T) -> PathEntry;
}

// ATTENTION: do not forget to update the `Validate`
// implementation when you add a new variant.
pub enum Asset {
    /// A site to host on `/`.
    Site,
    /// Optional GraphQL IDE to host on `/api/graphql`.
    GraphiQL,
    Sound(Sound),
    /// Optional cover image to embed into the piano recordings.
    PianoRecordingCoverJPEG,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, strum::Display, EnumIter)]
#[strum(serialize_all = "kebab-case")]
pub enum Sound {
    Error,
    PauseResume,
    Play,
    RecordStart,
    RecordStop,
}

/// Read-only resources.
#[derive(Clone, Deserialize)]
pub struct AssetsDir(PathBuf);

impl AssetsDir {
    pub fn unset() -> Self {
        Self(PathBuf::new())
    }
}

impl BaseDir<'_, Asset> for AssetsDir {
    fn path(&self, item: Asset) -> PathEntry {
        const SOUNDS_EXTENSION: &str = ".wav";

        let (relative_path, kind, requirement) = match item {
            Asset::Site => (
                "site".into(),
                EntryKind::Directory,
                Some(EntryRequirement::Exists),
            ),
            Asset::GraphiQL => ("graphiql".into(), EntryKind::Directory, None),
            Asset::Sound(sound) => (
                Path::new("sounds").join(sound.to_string() + SOUNDS_EXTENSION),
                EntryKind::File,
                Some(EntryRequirement::Exists),
            ),
            Asset::PianoRecordingCoverJPEG => {
                ("piano-recording-cover.jpg".into(), EntryKind::File, None)
            }
        };
        PathEntry {
            path: self.0.join(relative_path),
            kind,
            requirement,
        }
    }
}

impl Validate for AssetsDir {
    fn validate(&self) -> Result<(), validation::Errors> {
        PathEntry {
            path: self.0.clone(),
            kind: EntryKind::Directory,
            requirement: Some(EntryRequirement::Exists),
        }
        .validate()?;

        [Asset::Site, Asset::GraphiQL, Asset::PianoRecordingCoverJPEG]
            .into_iter()
            .try_for_each(|asset| self.path(asset).validate())?;
        Sound::iter().try_for_each(|sound| self.path(Asset::Sound(sound)).validate())
    }
}

#[derive(EnumIter)]
pub enum Data {
    Preferences,
    PianoRecordings,
}

/// A directory where the server stores all the data.
#[derive(Clone, Deserialize)]
pub struct DataDir(PathBuf);

impl BaseDir<'_, Data> for DataDir {
    fn path(&self, item: Data) -> PathEntry {
        let (relative_path, kind, requirement) = match item {
            Data::Preferences => ("prefs.yaml", EntryKind::File, None),
            Data::PianoRecordings => (
                "piano-recordings",
                EntryKind::Directory,
                Some(EntryRequirement::WritableOrCreate),
            ),
        };
        PathEntry {
            path: self.0.join(relative_path),
            kind,
            requirement,
        }
    }
}

impl From<&Path> for DataDir {
    fn from(path: &Path) -> Self {
        Self(path.into())
    }
}

impl Validate for DataDir {
    fn validate(&self) -> Result<(), validation::Errors> {
        PathEntry {
            path: self.0.clone(),
            kind: EntryKind::Directory,
            requirement: Some(EntryRequirement::WritableOrCreate),
        }
        .validate()?;
        Data::iter().try_for_each(|data| self.path(data).validate())
    }
}

pub struct PathEntry {
    path: PathBuf,
    kind: EntryKind,
    requirement: Option<EntryRequirement>,
}

impl Deref for PathEntry {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Validate for PathEntry {
    fn validate(&self) -> Result<(), validation::Errors> {
        let path_str = self.path.to_string_lossy();
        let err = |message| {
            Err(validation::Errors::NewType(vec![
                validation::Error::Custom(format!("{} '{path_str}': {message}", self.kind)),
            ]))
        };

        if path_str.is_empty() {
            return err(format_args!("path is not set"));
        }
        if self.requirement.is_none() {
            return Ok(());
        }

        let exists = match self.path.try_exists() {
            Ok(exists) => exists,
            Err(e) => return err(format_args!("unable to check existence ({e})")),
        };
        let matches_kind = match self.kind {
            EntryKind::File => self.path.is_file(),
            EntryKind::Directory => self.path.is_dir(),
        };

        match self.requirement.unwrap() {
            EntryRequirement::Exists => {
                if !exists {
                    return err(format_args!("not exists"));
                } else if !matches_kind {
                    return err(format_args!("not a {}", self.kind));
                }
            }
            EntryRequirement::WritableOrCreate => {
                if exists {
                    if !matches_kind {
                        return err(format_args!("not a {}", self.kind));
                    }
                    match self.path.metadata() {
                        Ok(metadata) => {
                            if metadata.permissions().readonly() {
                                return err(format_args!("not writable"));
                            }
                        }
                        Err(e) => return err(format_args!("unable to query metadata ({e})")),
                    }
                } else {
                    let create_result = match self.kind {
                        EntryKind::File => File::create_new(&self.path).map(|_| ()),
                        EntryKind::Directory => fs::create_dir_all(&self.path),
                    };
                    if let Err(e) = create_result {
                        return err(format_args!("unable to create ({e})"));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(strum::Display)]
#[strum(serialize_all = "lowercase")]
enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Copy)]
enum EntryRequirement {
    Exists,
    /// Entry must be writable. If it doesn't exist, create it.
    WritableOrCreate,
}
