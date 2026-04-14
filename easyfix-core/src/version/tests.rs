use std::str::FromStr;

use assert_matches::assert_matches;

use super::{UnknownVersionError, Version};

#[test]
fn test_version_constants() {
    let versions = [
        (Version::FIX27, 2, 7, 0),
        (Version::FIX30, 3, 0, 0),
        (Version::FIX40, 4, 0, 0),
        (Version::FIX41, 4, 1, 0),
        (Version::FIX42, 4, 2, 0),
        (Version::FIX43, 4, 3, 0),
        (Version::FIX44, 4, 4, 0),
        (Version::FIX50, 5, 0, 0),
        (Version::FIX50SP1, 5, 0, 1),
        (Version::FIX50SP2, 5, 0, 2),
        (Version::FIXT11, 1, 1, 0),
    ];

    for (version, major, minor, servicepack) in versions {
        assert_eq!(version.major(), major);
        assert_eq!(version.minor(), minor);
        assert_eq!(version.servicepack(), servicepack);
    }
}

#[test]
fn test_version_comparison() {
    assert!(Version::FIX27 < Version::FIX30);
    assert!(Version::FIX30 < Version::FIX40);
    assert!(Version::FIX40 < Version::FIX41);
    assert!(Version::FIX41 < Version::FIX42);
    assert!(Version::FIX42 < Version::FIX43);
    assert!(Version::FIX43 < Version::FIX44);
    assert!(Version::FIX44 < Version::FIX50);
    assert!(Version::FIX50 < Version::FIX50SP1);
    assert!(Version::FIX50SP1 < Version::FIX50SP2);

    // FIXT should be ordered after FIX
    assert!(Version::FIX50SP2 < Version::FIXT11);

    assert_eq!(Version::FIX40, Version::FIX40);
    assert_eq!(Version::FIX44, Version::FIX44);
    assert_eq!(Version::FIXT11, Version::FIXT11);

    assert_ne!(Version::FIX40, Version::FIX41);
    assert_ne!(Version::FIX50, Version::FIX50SP1);
}

#[test]
fn test_version_known_versions() {
    let known_versions = Version::known_versions();

    assert!(known_versions.contains(&Version::FIX27));
    assert!(known_versions.contains(&Version::FIX30));
    assert!(known_versions.contains(&Version::FIX40));
    assert!(known_versions.contains(&Version::FIX41));
    assert!(known_versions.contains(&Version::FIX42));
    assert!(known_versions.contains(&Version::FIX43));
    assert!(known_versions.contains(&Version::FIX44));
    assert!(known_versions.contains(&Version::FIX50));
    assert!(known_versions.contains(&Version::FIX50SP1));
    assert!(known_versions.contains(&Version::FIX50SP2));
    assert!(known_versions.contains(&Version::FIXT11));

    assert_eq!(known_versions.len(), 11);
}

#[test]
fn test_version_type_checks() {
    for version in [
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
    ]
    .iter()
    {
        assert!(version.is_fix());
        assert!(!version.is_fixt());
    }

    assert!(Version::FIXT11.is_fixt());
    assert!(!Version::FIXT11.is_fix());
}

#[test]
fn test_version_begin_string() {
    assert_eq!(Version::FIX27.begin_string(), "FIX.2.7");
    assert_eq!(Version::FIX30.begin_string(), "FIX.3.0");
    assert_eq!(Version::FIX40.begin_string(), "FIX.4.0");
    assert_eq!(Version::FIX41.begin_string(), "FIX.4.1");
    assert_eq!(Version::FIX42.begin_string(), "FIX.4.2");
    assert_eq!(Version::FIX43.begin_string(), "FIX.4.3");
    assert_eq!(Version::FIX44.begin_string(), "FIX.4.4");
    assert_eq!(Version::FIX50.begin_string(), "FIX.5.0");
    assert_eq!(Version::FIX50SP1.begin_string(), "FIX.5.0SP1");
    assert_eq!(Version::FIX50SP2.begin_string(), "FIX.5.0SP2");
    assert_eq!(Version::FIXT11.begin_string(), "FIXT.1.1");
}

#[test]
fn test_version_begin_str() {
    assert_eq!(Version::FIX27.begin_str(), "FIX.2.7");
    assert_eq!(Version::FIX44.begin_str(), "FIX.4.4");
    assert_eq!(Version::FIX50SP2.begin_str(), "FIX.5.0SP2");
    assert_eq!(Version::FIXT11.begin_str(), "FIXT.1.1");
}

#[test]
fn test_version_display() {
    assert_eq!(Version::FIX44.to_string(), "FIX.4.4");
    assert_eq!(Version::FIX50SP2.to_string(), "FIX.5.0SP2");
    assert_eq!(Version::FIXT11.to_string(), "FIXT.1.1");
}

#[test]
fn test_version_from_str() {
    assert_eq!(Version::from_str("FIX.2.7").unwrap(), Version::FIX27);
    assert_eq!(Version::from_str("FIX.3.0").unwrap(), Version::FIX30);
    assert_eq!(Version::from_str("FIX.4.0").unwrap(), Version::FIX40);
    assert_eq!(Version::from_str("FIX.4.1").unwrap(), Version::FIX41);
    assert_eq!(Version::from_str("FIX.4.2").unwrap(), Version::FIX42);
    assert_eq!(Version::from_str("FIX.4.3").unwrap(), Version::FIX43);
    assert_eq!(Version::from_str("FIX.4.4").unwrap(), Version::FIX44);
    assert_eq!(Version::from_str("FIX.5.0").unwrap(), Version::FIX50);
    assert_eq!(Version::from_str("FIX.5.0SP1").unwrap(), Version::FIX50SP1);
    assert_eq!(Version::from_str("FIX.5.0SP2").unwrap(), Version::FIX50SP2);
    assert_eq!(Version::from_str("FIXT.1.1").unwrap(), Version::FIXT11);

    // Roundtrip: version -> string -> version
    for version in Version::known_versions() {
        let begin_string = version.begin_string();
        let parsed = Version::from_str(begin_string.as_utf8()).unwrap();
        assert_eq!(*version, parsed, "Roundtrip failed for {begin_string}");
    }
}

#[test]
fn test_version_from_str_errors() {
    // Invalid format
    assert_matches!(Version::from_str("FIX"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FIX.4"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FIX.4.4.0"), Err(UnknownVersionError));

    // Invalid protocol type
    assert_matches!(Version::from_str("FIXP.4.4"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FOO.4.4"), Err(UnknownVersionError));

    // Invalid numbers
    assert_matches!(Version::from_str("FIX.X.4"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FIX.4.X"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FIX.5.0SPX"), Err(UnknownVersionError));

    // Valid format but unknown version
    assert_matches!(Version::from_str("FIX.9.9"), Err(UnknownVersionError));
    assert_matches!(Version::from_str("FIX.5.0SP99"), Err(UnknownVersionError));
}
