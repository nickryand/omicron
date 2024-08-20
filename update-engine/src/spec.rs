// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Copyright 2023 Oxide Computer Company

use std::{fmt, fmt::Write, marker::PhantomData};

use anyhow::anyhow;
use indent_write::fmt::IndentWriter;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Serialize};

/// A specification for an [`UpdateEngine`](crate::UpdateEngine).
///
/// This defines the set of types required to use an `UpdateEngine`.
///
/// NOTE: `StepSpec` is only required to implement `JsonSchema` to obtain the
/// name of the schema. This is an upstream limitation in `JsonSchema`.
pub trait StepSpec: JsonSchema + Send + 'static {
    /// A component associated with each step.
    type Component: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// The step identifier.
    type StepId: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// Metadata associated with each step.
    ///
    /// This can be `()` if there's no metadata associated with the step, or
    /// `serde_json::Value` for freeform metadata.
    type StepMetadata: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// Metadata associated with an individual progress event.
    ///
    /// This can be `()` if there's no metadata associated with the step, or
    /// `serde_json::Value` for freeform metadata.
    type ProgressMetadata: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// Metadata associated with each step's completion.
    ///
    /// This can be `()` if there's no metadata associated with the step, or
    /// `serde_json::Value` for freeform metadata.
    type CompletionMetadata: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// Metadata associated with a step being skipped.
    ///
    /// This can be `()` if there's no metadata associated with the step, or
    /// `serde_json::Value` for freeform metadata.
    type SkippedMetadata: Clone
        + fmt::Debug
        + DeserializeOwned
        + Serialize
        + Eq
        + JsonSchema
        + Send
        + Sync;

    /// The error type associated with each step.
    ///
    /// Ideally this would just have a trait bound of `std::error::Error` --
    /// however, `anyhow::Error` doesn't implement `std::error::Error`. Both can
    /// be converted to a dynamic `Error`, though. We use `AsError` to abstract
    /// over both sorts of errors.
    type Error: AsError + fmt::Debug + Send + Sync;
}

/// Represents a fully generic step specification, as can be serialized over
/// JSON.
///
/// Since errors aren't directly serialized, they can be any type that
/// implements [`AsError`].
///
/// Use this if you don't care about assigning types to any of the metadata
/// components.
pub struct GenericSpec<E> {
    _marker: PhantomData<E>,
}

impl<E> JsonSchema for GenericSpec<E> {
    fn schema_name() -> String {
        // All generic specs serialize the exact same way, so it's OK to just
        // call this GenericSpec.
        "GenericSpec".to_owned()
    }

    fn json_schema(
        _: &mut schemars::gen::SchemaGenerator,
    ) -> schemars::schema::Schema {
        // This means "accept any value here" -- this is irrelevant since we
        // only care about the schema name.
        schemars::schema::Schema::Bool(true)
    }
}

impl<E: AsError> StepSpec for GenericSpec<E> {
    type Component = serde_json::Value;
    type StepId = serde_json::Value;
    type StepMetadata = serde_json::Value;
    type ProgressMetadata = serde_json::Value;
    type CompletionMetadata = serde_json::Value;
    type SkippedMetadata = serde_json::Value;
    type Error = E;
}

/// A generic spec used for nested errors.
pub type NestedSpec = GenericSpec<NestedError>;

/// A nested error.
///
/// This is the error type for [`NestedSpec`]. It can be used to represent any
/// set of nested errors.
#[derive(Clone, Debug)]
pub struct NestedError {
    message: String,
    source: Option<Box<NestedError>>,
}

impl NestedError {
    /// Creates a new `NestedError` from an error.
    pub fn new(error: &dyn std::error::Error) -> Self {
        Self {
            message: format!("{}", error),
            source: error.source().map(|s| Box::new(Self::new(s))),
        }
    }

    /// Creates a new `NestedError` from a message and a list of causes.
    pub fn from_message_and_causes(
        message: String,
        causes: Vec<String>,
    ) -> Self {
        // Yes, this is an actual singly-linked list. You rarely ever see them
        // in Rust but they're required to implement Error::source.
        let mut next = None;
        for cause in causes.into_iter().rev() {
            let error = Self { message: cause, source: next.map(Box::new) };
            next = Some(error);
        }
        Self { message, source: next.map(Box::new) }
    }
}

impl fmt::Display for NestedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NestedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|s| s as &(dyn std::error::Error + 'static))
    }
}

mod nested_error_serde {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize)]
    struct SerializedNestedError {
        message: String,
        causes: Vec<String>,
    }

    impl Serialize for NestedError {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            let mut causes = Vec::new();
            let mut cause = self.source.as_ref();
            while let Some(c) = cause {
                causes.push(c.message.clone());
                cause = c.source.as_ref();
            }

            let serialized =
                SerializedNestedError { message: self.message.clone(), causes };
            serialized.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for NestedError {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Self, D::Error> {
            let serialized = SerializedNestedError::deserialize(deserializer)?;
            Ok(NestedError::from_message_and_causes(
                serialized.message,
                serialized.causes,
            ))
        }
    }
}

impl AsError for NestedError {
    fn as_error(&self) -> &(dyn std::error::Error + 'static) {
        self
    }
}

/// Trait that abstracts over concrete errors and `anyhow::Error`.
///
/// This needs to be manually implemented for any custom error types.
pub trait AsError: fmt::Debug + Send + Sync + 'static {
    fn as_error(&self) -> &(dyn std::error::Error + 'static);
}

impl AsError for anyhow::Error {
    fn as_error(&self) -> &(dyn std::error::Error + 'static) {
        self.as_ref()
    }
}

/// A temporary hack to convert a list of errors into a single `anyhow::Error`.
/// If no errors are provided, panic (this should be handled at a higher
/// level).
///
/// Eventually we should gain first-class support for representing errors as
/// trees, but this will do for now.
pub fn error_list_to_anyhow<I, E>(errors: I) -> anyhow::Error
where
    I: IntoIterator<Item = E>,
    E: AsError,
{
    let mut iter = errors.into_iter().peekable();
    // How many errors are there?
    let Some(first_error) = iter.next() else {
        // No errors: panic.
        panic!("error_list_to_anyhow called with no errors");
    };

    if iter.peek().is_none() {
        // One error. (Currently we lose the error type here, because all we
        // have to work with is a borrowed error. it would be nice to preserve
        // it somehow. Again, this is a temporary hack!)
        return anyhow!(NestedError::new(first_error.as_error()));
    }

    // Multiple errors.
    let mut out = String::new();
    let mut nerrors = 0;
    for error in std::iter::once(first_error).chain(iter) {
        nerrors += 1;
        let mut current = error.as_error();

        let mut writer = IndentWriter::new_skip_initial("  ", &mut out);
        writeln!(writer, "+ {current}").unwrap();

        while let Some(cause) = current.source() {
            let mut writer = IndentWriter::new_skip_initial("    ", &mut out);
            writeln!(writer, "   - {cause}").unwrap();
            current = cause;
        }
    }
    anyhow!(out).context(format!("{nerrors} errors encountered"))
}
