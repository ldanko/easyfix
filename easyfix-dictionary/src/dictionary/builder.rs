use std::{
    fs,
    path::{Path, PathBuf},
};

use quick_xml::de::from_str;

use super::{
    Dictionary,
    error::{BuilderError, Error, ValidationError},
    version::Version,
};
use crate::xml;

/// Builder for creating and configuring FIX dictionaries.
///
/// This builder provides a fluent interface for configuring and creating
/// a FIX protocol dictionary from XML specification files.
pub struct DictionaryBuilder {
    /// Path to the FIXT (transport) XML specification
    fixt_xml_path: Option<PathBuf>,

    /// Paths to FIX application-level XML specifications
    fix_xml_paths: Vec<PathBuf>,

    /// Allow custom FIX versions not in the standard list
    allow_custom_version: bool,

    /// Apply strict validation during parsing
    strict_check: bool,

    /// Whether to flatten component hierarchies
    flatten_components: bool,
}

pub fn read_raw_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let xml = fs::read_to_string(path)?;
    Ok(from_str::<xml::Dictionary>(&xml)?)
}

pub fn read_raw_fixt_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let raw_dictionary = read_raw_dictionary(path)?;
    let version = Version::from_raw_dictionary(&raw_dictionary)?;

    if !version.is_fixt() || version < Version::FIXT11 {
        return Err(Error::Builder(BuilderError::IncompatibleVersion));
    }
    if raw_dictionary.header.members.is_empty() {
        return Err(Error::Validation(ValidationError::EmptyContainer(
            "Header".into(),
        )));
    }
    if raw_dictionary.trailer.members.is_empty() {
        return Err(Error::Validation(ValidationError::EmptyContainer(
            "Trailer".into(),
        )));
    }
    if let Some(msg) = raw_dictionary
        .messages
        .iter()
        .find(|msg| !matches!(msg.msg_cat, xml::MsgCat::Admin))
    {
        return Err(Error::Validation(
            ValidationError::UnexpectedMessageCategory(msg.msg_cat, msg.name.clone()),
        ));
    }

    Ok(raw_dictionary)
}

pub fn read_raw_fix_dictionary(path: &Path) -> Result<xml::Dictionary, Error> {
    let raw_dictionary = read_raw_dictionary(path)?;
    let version = Version::from_raw_dictionary(&raw_dictionary)?;

    if !version.is_fix() {
        return Err(Error::Builder(BuilderError::IncompatibleVersion));
    } else if version >= Version::FIX50 {
        if !raw_dictionary.header.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Header".into(),
            )));
        }
        if !raw_dictionary.trailer.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Trailer".into(),
            )));
        }
        if let Some(msg) = raw_dictionary
            .messages
            .iter()
            .find(|msg| !matches!(msg.msg_cat, xml::MsgCat::App))
        {
            return Err(Error::Validation(
                ValidationError::UnexpectedMessageCategory(msg.msg_cat, msg.name.clone()),
            ));
        }
    } else {
        if raw_dictionary.header.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Header".into(),
            )));
        }
        if raw_dictionary.trailer.members.is_empty() {
            return Err(Error::Validation(ValidationError::EmptyContainer(
                "Trailer".into(),
            )));
        }
    }

    Ok(raw_dictionary)
}

impl Default for DictionaryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DictionaryBuilder {
    /// Creates a new, empty DictionaryBuilder with default settings
    ///
    /// Use the builder's methods to configure and then call `build()` to create
    /// the dictionary.
    pub fn new() -> DictionaryBuilder {
        DictionaryBuilder {
            fixt_xml_path: None,
            fix_xml_paths: Vec::new(),
            allow_custom_version: false,
            strict_check: false,
            flatten_components: false,
        }
    }

    /// Sets whether to allow custom FIX versions not in the standard list
    ///
    /// By default, only standard FIX versions are accepted.
    pub fn allow_custom_version(mut self, allow_custom_version: bool) -> Self {
        self.allow_custom_version = allow_custom_version;
        self
    }

    /// Sets whether to apply strict validation during dictionary parsing
    ///
    /// When enabled, more rigorous checks are applied to the dictionary structure.
    pub fn with_strict_check(mut self, strict_check: bool) -> Self {
        self.strict_check = strict_check;
        self
    }

    /// Adds a FIX application-level XML specification file to the builder
    ///
    /// For FIX versions prior to 5.0, this is the only XML file needed.
    /// For FIX 5.0+, this should be used along with `with_fixt_xml()`.
    pub fn with_fix_xml(mut self, path: impl Into<PathBuf>) -> Self {
        self.fix_xml_paths.push(path.into());
        self
    }

    /// Adds multiple FIX application-level XML specification files to the builder
    ///
    /// This is useful when working with multiple FIX versions or custom extensions.
    pub fn with_fix_xmls(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.fix_xml_paths.extend(paths);
        self
    }

    /// Sets the FIXT transport layer XML specification file
    ///
    /// This is required for FIX 5.0+ versions, which separate transport (FIXT)
    /// from application (FIX) layer specifications.
    pub fn with_fixt_xml(mut self, path: impl Into<PathBuf>) -> Self {
        self.fixt_xml_path = Some(path.into());
        self
    }

    /// Sets whether to flatten component hierarchies
    ///
    /// When enabled, nested components are flattened into their parent containers.
    /// This simplifies the structure by removing intermediate component layers,
    /// resulting in direct field references in messages and groups.
    ///
    /// For example, if Message A contains Component B which contains Field C,
    /// flattening would make Message A directly contain Field C.
    ///
    /// This is useful when you want to simplify the structure and reduce indirection,
    /// particularly for code generation or processing that works better with
    /// flattened structures.
    pub fn flatten_components(mut self, flatten_components: bool) -> Self {
        self.flatten_components = flatten_components;
        self
    }

    /// Builds the Dictionary from the configured sources
    ///
    /// This method parses the XML files and constructs a complete FIX dictionary.
    /// It will return an error if the dictionary configuration is invalid or if
    /// parsing fails.
    pub fn build(self) -> Result<Dictionary, Error> {
        match (self.fixt_xml_path, self.fix_xml_paths.as_slice()) {
            (None, []) => Err(Error::Builder(BuilderError::Unspecified)),
            (None, [fix_xml_path]) => {
                // Legacy FIX version
                let dict = read_raw_fix_dictionary(fix_xml_path)?;
                let dictionary = Dictionary::from_raw_dictionary(
                    dict,
                    self.flatten_components,
                    self.strict_check,
                )?;
                if self.flatten_components {
                    Ok(dictionary.flatten()?)
                } else {
                    Ok(dictionary)
                }
            }
            (None, [_, ..]) => Err(Error::Builder(BuilderError::IncompatibleVersion)),
            (Some(fixt_xml_path), fix_xml_paths) => {
                let fixt = read_raw_fixt_dictionary(&fixt_xml_path)?;
                let mut fixt_dict = Dictionary::from_raw_dictionary(
                    fixt,
                    self.flatten_components,
                    self.strict_check,
                )?;

                for fix_xml_path in fix_xml_paths {
                    let fix = read_raw_fix_dictionary(fix_xml_path)?;
                    if fix.major < 5 {
                        return Err(Error::Builder(BuilderError::IncompatibleVersion));
                    }
                    let subdict = Dictionary::from_raw_dictionary(
                        fix,
                        self.flatten_components,
                        self.strict_check,
                    )?;
                    fixt_dict.subdictionaries.insert(subdict.version, subdict);
                }

                if self.flatten_components {
                    Ok(fixt_dict.flatten()?)
                } else {
                    Ok(fixt_dict)
                }
            }
        }
    }
}
