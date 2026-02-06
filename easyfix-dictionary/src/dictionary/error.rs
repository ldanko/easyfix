use std::io;

use super::Version;
use crate::xml::{BasicType, MsgCat, MsgType};

/// Errors that can occur during dictionary operations.
///
/// This enum organizes errors into categories based on their source:
/// - I/O and parsing errors
/// - Dictionary validation errors
/// - Builder configuration errors
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Input/output error during file operations
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// XML parsing error when reading dictionary files
    #[error("XML parsing error: {0}")]
    XmlParse(#[from] quick_xml::de::DeError),

    /// Dictionary validation failed
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Dictionary builder configuration error
    #[error("Builder error: {0}")]
    Builder(#[from] BuilderError),
}

/// Errors related to dictionary structure and content validation.
///
/// These errors indicate problems with the dictionary's structure, such as
/// missing references, duplicates, or invalid relationships between elements.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// Referenced field was not found in the dictionary
    #[error("Unknown field {0}")]
    UnknownField(String),

    /// Referenced component was not found in the dictionary
    #[error("Unknown component {0}")]
    UnknownComponent(String),

    /// A field with the same name already exists in the dictionary
    #[error("Duplicated field {0}")]
    DuplicatedField(String),

    /// A component with the same name already exists in the dictionary
    #[error("Duplicated component {0}")]
    DuplicatedComponent(String),

    /// A group with the same name already exists in the dictionary
    #[error("Duplicated group {0}")]
    DuplicatedGroup(String),

    /// A message with the same name already exists in the dictionary
    #[error("Duplicated message name {0}")]
    DuplicatedMessageName(String),

    /// A message with the same type already exists in the dictionary
    #[error("Duplicated message type {0}")]
    DuplicatedMessageType(MsgType),

    /// Component or group has no members defined
    #[error("Component/group {0} has no members")]
    EmptyContainer(String),

    /// Message has no fields or components defined
    #[error("Message {0} has no members")]
    EmptyMessage(String),

    /// Message has an unexpected category for this dictionary type
    #[error("Unexpected message category {0:?} ({1})")]
    UnexpectedMessageCategory(MsgCat, String),

    /// Field was defined in the dictionary but not used in any message, component, or group
    ///
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Unused field {0}({1})")]
    UnusedField(String, u16),

    /// Component was defined in the dictionary but not used in any message or other component
    ///
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Unused component {0}")]
    UnusedComponent(String),

    /// A required standard field in the header/trailer has incorrect properties
    ///
    /// This error occurs when a required FIX field (like BeginString or BodyLength)
    /// has the wrong name, tag number, or data type in the dictionary.
    /// This error only occurs when strict validation is enabled with `with_strict_check(true)`.
    #[error("Invalid required field {0}({1}) [{2:?}]")]
    InvalidRequiredField(String, u16, BasicType),

    /// A circular dependency was detected in component or group references
    ///
    /// This error occurs when components or groups reference each other in a way that
    /// creates an infinite loop. For example, if Component A contains Component B, and
    /// Component B contains Component A, this would create a circular reference.
    #[error("Circular reference found: {0}")]
    CircularReference(String),
}

/// Errors related to dictionary builder configuration.
///
/// These errors indicate problems with how the dictionary builder is being used,
/// such as missing required configuration or incompatible version combinations.
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// FIX version not recognized or supported
    #[error("Unknown version {}", .0.begin_string())]
    UnknownVersion(Version),

    /// No dictionary was specified in the builder
    #[error("No dictionary specified")]
    Unspecified,

    /// Incompatible FIX version combinations
    #[error("Incompatible version combination")]
    IncompatibleVersion,
}
