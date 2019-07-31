// Copyright 2018-2019 Parity Technologies (UK) Ltd.
// This file is part of ink!.
//
// ink! is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// ink! is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with ink!.  If not, see <http://www.gnu.org/licenses/>.

use std::{
    io::Error as IoError,
    result::Result as StdResult,
};
use zip::result::ZipError;

/// The kinds of command errors.
#[derive(Debug)]
pub enum CommandErrorKind {
    Io(IoError),
    UnimplementedCommand,
    UnimplementedAbstractionLayer,
    ZipError(ZipError),
    Other(String),
}

/// An error that can be encountered while executing commands.
#[derive(Debug)]
pub struct CommandError {
    kind: CommandErrorKind,
}

impl From<IoError> for CommandError {
    fn from(error: IoError) -> Self {
        Self {
            kind: CommandErrorKind::Io(error),
        }
    }
}

impl From<ZipError> for CommandError {
    fn from(error: ZipError) -> Self {
        Self {
            kind: CommandErrorKind::ZipError(error),
        }
    }
}

impl From<&str> for CommandError {
    fn from(error: &str) -> Self {
        Self {
            kind: CommandErrorKind::Other(error.to_string()),
        }
    }
}

impl CommandError {
    /// Creates a new command error from the given kind.
    pub fn new(kind: CommandErrorKind) -> Self {
        Self { kind }
    }
}

/// Result type that has a `CommandError`.
pub type Result<T> = StdResult<T, CommandError>;
