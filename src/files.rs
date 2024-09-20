use std::{
    fs::{self, File, Permissions},
    ops::Deref,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_valid::{validation, Validate};
use strum::{EnumIter, IntoEnumIterator};

pub trait BaseDir<'a, T>: Clone + Deserialize<'a> + Validate {
    fn path(&self, item: T) -> PathEntry;
}

#[derive(EnumIter)]
pub enum Asset {
    Site,
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
        let (relative_path, kind) = match item {
            Asset::Site => ("site", EntryKind::Directory),
        };
        PathEntry {
            path: self.0.join(relative_path),
            kind,
            requirement: Some(EntryRequirement::Exists),
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
        Asset::iter().try_for_each(|asset| self.path(asset).validate())
    }
}

#[derive(EnumIter)]
pub enum Data {
    Preferences,
}

/// A directory where the server stores all the data.
#[derive(Clone, Deserialize)]
pub struct DataDir(PathBuf);

impl BaseDir<'_, Data> for DataDir {
    fn path(&self, item: Data) -> PathEntry {
        let (relative_path, kind, requirement) = match item {
            Data::Preferences => ("prefs.yaml", EntryKind::File, None),
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
                validation::Error::Custom(format!("{} \"{path_str}\": {message}", self.kind)),
            ]))
        };

        if path_str.is_empty() {
            return err(format_args!("path is not set"));
        }
        if self.requirement.is_none() {
            return Ok(());
        }

        let exists = self.path.exists();
        let (matches_kind, create_perms) = match self.kind {
            EntryKind::File => (self.path.is_file(), 0o600),
            EntryKind::Directory => (self.path.is_dir(), 0o700),
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
                    } else if let Err(e) =
                        fs::set_permissions(&self.path, Permissions::from_mode(create_perms))
                    {
                        return err(format_args!("unable to change permissions ({e})"));
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
