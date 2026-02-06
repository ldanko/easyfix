use std::str::FromStr;

use super::error::{BuilderError, Error};
use crate::xml::{self, FixType};

/// Represents a specific version of the FIX protocol.
///
/// FIX versions are identified by a type (FIX or FIXT), major version,
/// minor version, and service pack level. This struct provides constants
/// for all standard FIX protocol versions and methods to work with them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version {
    /// The type of FIX protocol (FIX or FIXT)
    fix_type: FixType,

    /// Major version number
    major: u8,

    /// Minor version number
    minor: u8,

    /// Service pack level
    servicepack: u8,
}

impl Version {
    pub const FIX27: Version = Version {
        fix_type: FixType::Fix,
        major: 2,
        minor: 7,
        servicepack: 0,
    };
    pub const FIX30: Version = Version {
        fix_type: FixType::Fix,
        major: 3,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX40: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX41: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 1,
        servicepack: 0,
    };
    pub const FIX42: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 2,
        servicepack: 0,
    };
    pub const FIX43: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 3,
        servicepack: 0,
    };
    pub const FIX44: Version = Version {
        fix_type: FixType::Fix,
        major: 4,
        minor: 4,
        servicepack: 0,
    };
    pub const FIX50: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX50SP1: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 1,
    };
    pub const FIX50SP2: Version = Version {
        fix_type: FixType::Fix,
        major: 5,
        minor: 0,
        servicepack: 2,
    };
    pub const FIXT11: Version = Version {
        fix_type: FixType::Fixt,
        major: 1,
        minor: 1,
        servicepack: 0,
    };
    pub(crate) const UNKNOWN: Version = Version {
        fix_type: FixType::Fix,
        major: 0,
        minor: 0,
        servicepack: 0,
    };

    /// Returns a slice containing all known standard FIX protocol versions
    pub const fn known_versions() -> &'static [Version] {
        &[
            Version::FIX27,
            Version::FIX30,
            Version::FIX40,
            Version::FIX41,
            Version::FIX42,
            Version::FIX43,
            Version::FIX44,
            Version::FIX50,
            Version::FIX50SP1,
            Version::FIX50SP2,
            Version::FIXT11,
        ]
    }

    pub(super) fn from_raw_dictionary(dictionary: &xml::Dictionary) -> Result<Version, Error> {
        let version = Version {
            fix_type: dictionary.fix_type,
            major: dictionary.major,
            minor: dictionary.minor,
            servicepack: dictionary.servicepack,
        };

        if !Version::known_versions().contains(&version) {
            return Err(Error::Builder(BuilderError::UnknownVersion(version)));
        }

        Ok(version)
    }

    /// Returns the type of this FIX version (FIX or FIXT)
    pub const fn fix_type(&self) -> FixType {
        self.fix_type
    }

    /// Returns true if this is a FIX (not FIXT) protocol version
    pub const fn is_fix(&self) -> bool {
        matches!(self.fix_type, FixType::Fix)
    }

    /// Returns true if this is a FIXT protocol version
    pub const fn is_fixt(&self) -> bool {
        matches!(self.fix_type, FixType::Fixt)
    }

    /// Returns the major version number
    pub const fn major(&self) -> u8 {
        self.major
    }

    /// Returns the minor version number
    pub const fn minor(&self) -> u8 {
        self.minor
    }

    /// Returns the service pack level
    pub const fn servicepack(&self) -> u8 {
        self.servicepack
    }

    /// Returns the BeginString representation of this version
    ///
    /// Formats the version as it appears in FIX messages (e.g., "FIX.4.4", "FIXT.1.1", "FIX.5.0SP2").
    pub fn begin_string(&self) -> String {
        if self.servicepack == 0 {
            // Basic format is TYPE.MAJOR.MINOR
            format!("{}.{}.{}", self.fix_type, self.major, self.minor)
        } else {
            // For non-zero servicepack, add SPx suffix
            format!(
                "{}.{}.{}SP{}",
                self.fix_type, self.major, self.minor, self.servicepack
            )
        }
    }
}

impl FromStr for Version {
    type Err = Error;

    /// Parse a BeginString value into a Version
    ///
    /// Accepts strings in the format:
    /// - "FIX.MAJOR.MINOR" (e.g., "FIX.4.4")
    /// - "FIXT.MAJOR.MINOR" (e.g., "FIXT.1.1")
    /// - "FIX.MAJOR.MINORSPx" (e.g., "FIX.5.0SP2")
    ///
    /// # Examples
    ///
    /// ```
    /// use easyfix_dictionary::Version;
    /// use std::str::FromStr;
    ///
    /// let v1 = Version::from_str("FIX.4.4").unwrap();
    /// assert_eq!(v1, Version::FIX44);
    ///
    /// let v2 = Version::from_str("FIXT.1.1").unwrap();
    /// assert_eq!(v2, Version::FIXT11);
    ///
    /// let v3 = Version::from_str("FIX.5.0SP2").unwrap();
    /// assert_eq!(v3, Version::FIX50SP2);
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Error::Builder(BuilderError::UnknownVersion` if:
    /// - The string format is invalid
    /// - The version numbers cannot be parsed
    /// - The version is not in the list of known FIX versions
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split by '.' to get parts
        let parts: Vec<&str> = s.split('.').collect();

        if parts.len() != 3 {
            return Err(Error::Builder(BuilderError::UnknownVersion(
                Version::UNKNOWN,
            )));
        }

        // Parse FIX type (FIX or FIXT)
        let fix_type = match parts[0] {
            "FIX" => FixType::Fix,
            "FIXT" => FixType::Fixt,
            _ => {
                return Err(Error::Builder(BuilderError::UnknownVersion(
                    Version::UNKNOWN,
                )));
            }
        };

        // Parse major version
        let major = parts[1].parse::<u8>().map_err(|_| {
            Error::Builder(BuilderError::UnknownVersion(Version {
                fix_type,
                ..Version::UNKNOWN
            }))
        })?;

        // Parse minor version and optional service pack
        let minor_and_sp = parts[2];
        let (minor, servicepack) = if let Some(sp_pos) = minor_and_sp.find("SP") {
            // Has service pack: "0SP2"
            let minor_str = &minor_and_sp[..sp_pos];
            let sp_str = &minor_and_sp[sp_pos + 2..];

            let minor = minor_str.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    ..Version::UNKNOWN
                }))
            })?;

            let sp = sp_str.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    minor,
                    ..Version::UNKNOWN
                }))
            })?;

            (minor, sp)
        } else {
            // No service pack
            let minor = minor_and_sp.parse::<u8>().map_err(|_| {
                Error::Builder(BuilderError::UnknownVersion(Version {
                    fix_type,
                    major,
                    ..Version::UNKNOWN
                }))
            })?;

            (minor, 0)
        };

        // Create version
        let version = Version {
            fix_type,
            major,
            minor,
            servicepack,
        };

        // Validate it's a known version
        if !Version::known_versions().contains(&version) {
            return Err(Error::Builder(BuilderError::UnknownVersion(version)));
        }

        Ok(version)
    }
}
