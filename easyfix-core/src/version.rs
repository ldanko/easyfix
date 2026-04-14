//! FIX protocol version identification.
//!
//! [`Version`] represents one of the standard FIX protocol versions
//! (FIX 2.7 - FIX 5.0 SP2, FIXT 1.1). Construction is restricted to the
//! provided constants and the [`FromStr`] parser, which validates against
//! the list of [`Version::known_versions`].

use core::{fmt, str::FromStr};

use crate::{
    basic_types::{FixStr, FixString},
    fix_str,
};

#[cfg(test)]
mod tests;

/// Which FIX protocol family a [`Version`] belongs to.
///
/// FIX (FIX 4.x, 5.x) bundles session and application messages under a
/// single `BeginString`. FIXT (1.1) is the dedicated session-layer
/// protocol; the application version is carried separately via
/// `ApplVerID`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
#[cfg_attr(
    any(feature = "serde-serialize", feature = "serde-deserialize"),
    serde(rename_all = "UPPERCASE")
)]
pub enum SessionProtocol {
    Fix,
    Fixt,
}

impl fmt::Display for SessionProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionProtocol::Fix => f.write_str("FIX"),
            SessionProtocol::Fixt => f.write_str("FIXT"),
        }
    }
}

/// A specific version of the FIX protocol.
///
/// Construct via the provided `Version::FIX*` / `Version::FIXT11`
/// constants or via [`FromStr`] which parses standard `BeginString`
/// representations (`"FIX.4.4"`, `"FIXT.1.1"`, `"FIX.5.0SP2"`).
///
/// Only versions listed in [`Version::known_versions`] are valid.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version {
    session_protocol: SessionProtocol,
    major: u8,
    minor: u8,
    servicepack: u8,
}

/// Error returned when constructing a [`Version`] from components or
/// parsing one from a `BeginString` fails because the value is not
/// among [`Version::known_versions`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Unknown FIX version")]
pub struct UnknownVersionError;

impl Version {
    pub const FIX27: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 2,
        minor: 7,
        servicepack: 0,
    };
    pub const FIX30: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 3,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX40: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 4,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX41: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 4,
        minor: 1,
        servicepack: 0,
    };
    pub const FIX42: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 4,
        minor: 2,
        servicepack: 0,
    };
    pub const FIX43: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 4,
        minor: 3,
        servicepack: 0,
    };
    pub const FIX44: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 4,
        minor: 4,
        servicepack: 0,
    };
    pub const FIX50: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 5,
        minor: 0,
        servicepack: 0,
    };
    pub const FIX50SP1: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 5,
        minor: 0,
        servicepack: 1,
    };
    pub const FIX50SP2: Version = Version {
        session_protocol: SessionProtocol::Fix,
        major: 5,
        minor: 0,
        servicepack: 2,
    };
    pub const FIXT11: Version = Version {
        session_protocol: SessionProtocol::Fixt,
        major: 1,
        minor: 1,
        servicepack: 0,
    };

    /// All FIX versions recognized by this crate.
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

    /// Build a `Version` from its components.
    ///
    /// Returns [`UnknownVersionError`] if the combination does not
    /// correspond to a version listed in [`Version::known_versions`].
    pub fn new(
        session_protocol: SessionProtocol,
        major: u8,
        minor: u8,
        servicepack: u8,
    ) -> Result<Version, UnknownVersionError> {
        let candidate = Version {
            session_protocol,
            major,
            minor,
            servicepack,
        };
        if Version::known_versions().contains(&candidate) {
            Ok(candidate)
        } else {
            Err(UnknownVersionError)
        }
    }

    /// Returns true if this is a traditional FIX (FIX 4.x / FIX 5.x)
    /// version.
    pub const fn is_fix(&self) -> bool {
        matches!(self.session_protocol, SessionProtocol::Fix)
    }

    /// Returns true if this is a FIXT version.
    pub const fn is_fixt(&self) -> bool {
        matches!(self.session_protocol, SessionProtocol::Fixt)
    }

    /// Returns the major version number.
    pub const fn major(&self) -> u8 {
        self.major
    }

    /// Returns the minor version number.
    pub const fn minor(&self) -> u8 {
        self.minor
    }

    /// Returns the service pack level.
    pub const fn servicepack(&self) -> u8 {
        self.servicepack
    }

    /// Returns the canonical `BeginString` representation as a
    /// `&'static FixStr`. This is the zero-allocation form intended for
    /// the serialization path.
    pub fn begin_str(&self) -> &'static FixStr {
        use SessionProtocol::{Fix, Fixt};
        match (
            self.session_protocol,
            self.major,
            self.minor,
            self.servicepack,
        ) {
            (Fix, 2, 7, 0) => fix_str!("FIX.2.7"),
            (Fix, 3, 0, 0) => fix_str!("FIX.3.0"),
            (Fix, 4, 0, 0) => fix_str!("FIX.4.0"),
            (Fix, 4, 1, 0) => fix_str!("FIX.4.1"),
            (Fix, 4, 2, 0) => fix_str!("FIX.4.2"),
            (Fix, 4, 3, 0) => fix_str!("FIX.4.3"),
            (Fix, 4, 4, 0) => fix_str!("FIX.4.4"),
            (Fix, 5, 0, 0) => fix_str!("FIX.5.0"),
            (Fix, 5, 0, 1) => fix_str!("FIX.5.0SP1"),
            (Fix, 5, 0, 2) => fix_str!("FIX.5.0SP2"),
            (Fixt, 1, 1, 0) => fix_str!("FIXT.1.1"),
            _ => unreachable!("Version constructors only allow known versions"),
        }
    }

    /// Returns an owned `FixString` copy of the canonical `BeginString`.
    /// Convenience for callers that need to build outgoing FIX messages;
    /// allocates. Prefer [`Version::begin_str`] on the hot path.
    pub fn begin_string(&self) -> FixString {
        self.begin_str().to_owned()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.begin_str().as_utf8())
    }
}

impl FromStr for Version {
    type Err = UnknownVersionError;

    /// Parse a `BeginString` value into a [`Version`].
    ///
    /// Accepts strings in the format:
    /// - `"FIX.MAJOR.MINOR"` (e.g., `"FIX.4.4"`)
    /// - `"FIXT.MAJOR.MINOR"` (e.g., `"FIXT.1.1"`)
    /// - `"FIX.MAJOR.MINORSPx"` (e.g., `"FIX.5.0SP2"`)
    ///
    /// Returns [`UnknownVersionError`] if the format is invalid or the
    /// parsed version is not in [`Version::known_versions`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();

        if parts.len() != 3 {
            return Err(UnknownVersionError);
        }

        let session_protocol = match parts[0] {
            "FIX" => SessionProtocol::Fix,
            "FIXT" => SessionProtocol::Fixt,
            _ => return Err(UnknownVersionError),
        };

        let major = parts[1].parse::<u8>().map_err(|_| UnknownVersionError)?;

        let minor_and_sp = parts[2];
        let (minor, servicepack) = if let Some(sp_pos) = minor_and_sp.find("SP") {
            let minor_str = &minor_and_sp[..sp_pos];
            let sp_str = &minor_and_sp[sp_pos + 2..];

            let minor = minor_str.parse::<u8>().map_err(|_| UnknownVersionError)?;
            let sp = sp_str.parse::<u8>().map_err(|_| UnknownVersionError)?;

            (minor, sp)
        } else {
            let minor = minor_and_sp
                .parse::<u8>()
                .map_err(|_| UnknownVersionError)?;
            (minor, 0)
        };

        Version::new(session_protocol, major, minor, servicepack)
    }
}

#[cfg(feature = "serde-serialize")]
impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.begin_str().as_utf8())
    }
}

#[cfg(feature = "serde-deserialize")]
impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Version::from_str(&s).map_err(D::Error::custom)
    }
}
