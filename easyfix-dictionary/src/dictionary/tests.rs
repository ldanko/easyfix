use std::{env, fs};

use assert_matches::assert_matches;
use quick_xml::de::from_str;
use uuid::Uuid;

use super::*;

// Basic valid FIX dictionary for testing
const BASIC_FIX_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='BodyLength' required='Y'/>
    <field name='MsgType' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='admin' msgtype='0' name='Heartbeat'>
      <field name='TestReqID' required='N'/>
    </message>
    <message msgcat='admin' msgtype='1' name='TestRequest'>
      <field name='TestReqID' required='Y'/>
      <component name='TestComponent' required='Y'/>
    </message>
  </messages>
  <components>
    <component name='TestComponent'>
      <field name='ComponentField' required='Y'/>
    </component>
  </components>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='BodyLength' number='9' type='LENGTH'/>
    <field name='MsgType' number='35' type='STRING'>
      <value enum='0' description='HEARTBEAT'/>
      <value enum='1' description='TEST_REQUEST'/>
    </field>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='TestReqID' number='112' type='STRING'/>
    <field name='ComponentField' number='999' type='STRING'/>
  </fields>
</fix>
"#;

// Basic valid FIXT dictionary for testing
const BASIC_FIXT_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIXT' major='1' minor='1' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='BodyLength' required='Y'/>
    <field name='MsgType' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='admin' msgtype='0' name='Heartbeat'>
      <field name='TestReqID' required='N'/>
    </message>
  </messages>
  <components>
    <component name='TestComponent'>
      <field name='TestReqID' required='Y'/>
    </component>
  </components>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='BodyLength' number='9' type='LENGTH'/>
    <field name='MsgType' number='35' type='STRING'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='TestReqID' number='112' type='STRING'/>
  </fields>
</fix>
"#;

// Basic valid FIX 5.0 dictionary for testing
const BASIC_FIX50_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='5' minor='0' servicepack='0'>
  <header/>
  <trailer/>
  <messages>
    <message msgcat='app' msgtype='D' name='NewOrderSingle'>
      <field name='ClOrdID' required='Y'/>
    </message>
  </messages>
  <components>
    <component name='TestComponent'>
      <field name='ClOrdID' required='Y'/>
    </component>
  </components>
  <fields>
    <field name='ClOrdID' number='11' type='STRING'/>
  </fields>
</fix>
"#;

const COMPLEX_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='BodyLength' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='app' msgtype='E' name='NewOrderList'>
      <component name='OrderListComponent' required='Y'/>
      <field name='ListID' required='Y'/>
    </message>
  </messages>
  <components>
    <!-- Complex component hierarchy for testing flattening -->
    <component name='OrderListComponent'>
      <field name='ListSeqNo' required='Y'/>
      <component name='InstrumentComponent' required='Y'/>
      <component name='OrderComponent' required='N'/>
      <group name='OrderListGroup' required='Y'>
        <field name='OrderListGroupField' required='Y'/>
        <component name='NestedComponent' required='Y'/>
      </group>
    </component>

    <component name='InstrumentComponent'>
      <field name='Symbol' required='Y'/>
      <field name='SecurityID' required='N'/>
      <component name='SecurityIDComponent' required='N'/>
    </component>

    <component name='SecurityIDComponent'>
      <field name='SecurityIDSource' required='Y'/>
      <field name='SecurityDesc' required='N'/>
    </component>

    <component name='OrderComponent'>
      <field name='OrderID' required='Y'/>
      <field name='OrderQty' required='Y'/>
      <field name='Price' required='N'/>
      <group name='OrderParties' required='N'>
        <field name='PartyID' required='Y'/>
        <field name='PartyRole' required='Y'/>
      </group>
    </component>

    <component name='NestedComponent'>
      <field name='NestedField1' required='Y'/>
      <field name='NestedField2' required='N'/>
    </component>
  </components>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='BodyLength' number='9' type='LENGTH'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='ListID' number='66' type='STRING'/>
    <field name='ListSeqNo' number='67' type='INT'/>
    <field name='Symbol' number='55' type='STRING'/>
    <field name='SecurityID' number='48' type='STRING'/>
    <field name='SecurityIDSource' number='22' type='INT'/>
    <field name='SecurityDesc' number='107' type='STRING'/>
    <field name='OrderID' number='37' type='STRING'/>
    <field name='OrderQty' number='38' type='QTY'/>
    <field name='Price' number='44' type='PRICE'/>
    <field name='OrderListGroup' number='73' type='NUMINGROUP'/>
    <field name='OrderListGroupField' number='74' type='STRING'/>
    <field name='OrderParties' number='453' type='NUMINGROUP'/>
    <field name='PartyID' number='448' type='STRING'/>
    <field name='PartyRole' number='452' type='INT'/>
    <field name='NestedField1' number='1001' type='STRING'/>
    <field name='NestedField2' number='1002' type='STRING'/>
  </fields>
</fix>
"#;

struct TestFile {
    test_dir: PathBuf,
    path: PathBuf,
}

impl TestFile {
    fn new(file_name: &str, data: &str) -> TestFile {
        let test_dir = env::temp_dir().join(Uuid::new_v4().hyphenated().to_string());
        fs::create_dir(test_dir.as_path()).unwrap_or_else(|err| {
            panic!(
                "Failed to crate temporary directory {}: {err}",
                test_dir.display()
            )
        });
        let path = test_dir.join(file_name);
        fs::write(&path, data).unwrap_or_else(|err| {
            panic!("Failed to write temporary file {}: {err}", path.display())
        });
        TestFile { test_dir, path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestFile {
    fn drop(&mut self) {
        if self.test_dir.exists() {
            if self.path.exists() {
                if let Err(err) = fs::remove_file(&self.path) {
                    eprintln!(
                        "Warning: Failed to clean up temporary file {}: {err}",
                        self.path.display(),
                    );
                }
            }
            if let Err(err) = fs::remove_dir(&self.test_dir) {
                eprintln!(
                    "Warning: Failed to clean up temporary directory {}: {err}",
                    self.test_dir.display(),
                );
            }
        }
    }
}

fn setup_fix44_file() -> TestFile {
    TestFile::new("FIX44.xml", BASIC_FIX_DICT)
}

fn setup_fixt11_file() -> TestFile {
    TestFile::new("FIXT11.xml", BASIC_FIXT_DICT)
}

fn setup_fix50_file() -> TestFile {
    TestFile::new("FIXT50.xml", BASIC_FIX50_DICT)
}

// Version tests

#[test]
fn test_version_constants() {
    // Test all standard version constants
    let versions = [
        (Version::FIX27, FixType::Fix, 2, 7, 0),
        (Version::FIX30, FixType::Fix, 3, 0, 0),
        (Version::FIX40, FixType::Fix, 4, 0, 0),
        (Version::FIX41, FixType::Fix, 4, 1, 0),
        (Version::FIX42, FixType::Fix, 4, 2, 0),
        (Version::FIX43, FixType::Fix, 4, 3, 0),
        (Version::FIX44, FixType::Fix, 4, 4, 0),
        (Version::FIX50, FixType::Fix, 5, 0, 0),
        (Version::FIX50SP1, FixType::Fix, 5, 0, 1),
        (Version::FIX50SP2, FixType::Fix, 5, 0, 2),
        (Version::FIXT11, FixType::Fixt, 1, 1, 0),
    ];

    for (version, fix_type, major, minor, servicepack) in versions {
        assert_eq!(version.fix_type(), fix_type);
        assert_eq!(version.major(), major);
        assert_eq!(version.minor(), minor);
        assert_eq!(version.servicepack(), servicepack);
    }
}

#[test]
fn test_version_comparison() {
    // Test version ordering
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

    // Test equality
    assert_eq!(Version::FIX40, Version::FIX40);
    assert_eq!(Version::FIX44, Version::FIX44);
    assert_eq!(Version::FIXT11, Version::FIXT11);

    // Test not equal
    assert_ne!(Version::FIX40, Version::FIX41);
    assert_ne!(Version::FIX50, Version::FIX50SP1);
}

#[test]
fn test_version_known_versions() {
    // Test the known_versions method
    let known_versions = Version::known_versions();

    // Check that all standard versions are included
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

    // Check the count
    assert_eq!(known_versions.len(), 11);
}

#[test]
fn test_version_type_checks() {
    // Test is_fix and is_fixt methods
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

// Builder tests

#[test]
fn test_simple_builder() {
    let fix44_file = setup_fix44_file();

    // Build a simple FIX 4.4 dictionary
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .build();

    let dictionary = result.expect("Failed to build dictionary");
    assert_eq!(dictionary.version(), Version::FIX44);
}

#[test]
fn test_builder_with_custom_rejection_reason() {
    // Create a file directly instead of using setup_test_files
    let fix44_file = setup_fix44_file();

    // Create custom rejection reasons
    let mut custom_reasons = HashMap::new();
    custom_reasons.insert(
        ParseRejectReason::RequiredTagMissing,
        "Custom message for missing tag".to_string(),
    );

    // Build dictionary with custom rejection reason
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .with_custom_rejection_reason(custom_reasons.clone())
        .build();

    let dictionary = result.expect("Failed to build dictionary with custom rejection reason");

    // Verify custom rejection reason is set
    let overrides = dictionary.reject_reason_overrides();
    assert_eq!(
        overrides.get(&ParseRejectReason::RequiredTagMissing),
        Some(&"Custom message for missing tag".to_string())
    );
}

#[test]
fn test_builder_with_flattening() {
    let fix44_file = setup_fix44_file();

    // Build dictionary with component flattening enabled
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .flatten_components(true)
        .build();

    let dictionary = result.expect("Failed to build dictionary with flattening");
    assert_eq!(dictionary.version(), Version::FIX44);
}

#[test]
fn test_builder_with_fixt_and_fix() {
    let fixt11_file = setup_fixt11_file();
    let fix50_file = setup_fix50_file();

    // Build combined FIXT and FIX dictionary
    let result = DictionaryBuilder::new()
        .with_fixt_xml(fixt11_file.path())
        .with_fix_xml(fix50_file.path())
        .build();

    let dictionary = result.expect("Failed to build combined FIXT/FIX dictionary");

    // Main dictionary should be FIXT 1.1
    assert_eq!(dictionary.version(), Version::FIXT11);

    // Should have FIX 5.0 as a subdictionary
    assert!(dictionary.subdictionary(Version::FIX50).is_some());

    // Main dictionary should have Heartbeat (admin) message
    assert!(dictionary.message_by_name("Heartbeat").is_some());

    // Main dictionary should NOT have NewOrderSingle (app) message
    assert!(dictionary.message_by_name("NewOrderSingle").is_none());

    // Subdictionary should have NewOrderSingle message
    let subdictionary = dictionary.subdictionary(Version::FIX50).unwrap();
    assert!(subdictionary.message_by_name("NewOrderSingle").is_some());
}

#[test]
fn test_builder_with_multiple_fix_apps() {
    let fixt11_file = setup_fixt11_file();
    let fix50_file = setup_fix50_file();

    let fix50sp2_dict = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='5' minor='0' servicepack='2'>
          <header/>
          <trailer/>
          <messages>
            <message msgcat='app' msgtype='8' name='ExecutionReport'>
              <field name='OrderID' required='Y'/>
            </message>
          </messages>
          <components>
            <component name='TestComponent'>
              <field name='OrderID' required='Y'/>
            </component>
          </components>
          <fields>
            <field name='OrderID' number='37' type='STRING'/>
          </fields>
        </fix>
    "#;

    // Create another FIX 5.0 SP2 file
    let fix50sp2_file = TestFile::new("FIX50SP2.xml", fix50sp2_dict);

    // Build with multiple application dictionaries
    let result = DictionaryBuilder::new()
        .with_fixt_xml(fixt11_file.path())
        .with_fix_xmls([
            fix50_file.path().to_path_buf(),
            fix50sp2_file.path().to_path_buf(),
        ])
        .build();

    let dictionary = result.expect("Failed to build dictionary with multiple app layers");

    // Main dictionary should be FIXT 1.1
    assert_eq!(dictionary.version(), Version::FIXT11);

    // Should have both FIX 5.0 and FIX 5.0 SP2
    assert!(dictionary.subdictionary(Version::FIX50).is_some());
    assert!(dictionary.subdictionary(Version::FIX50SP2).is_some());

    // FIX 5.0 subdictionary should have NewOrderSingle
    let fix50_subdict = dictionary.subdictionary(Version::FIX50).unwrap();
    assert!(fix50_subdict.message_by_name("NewOrderSingle").is_some());

    // FIX 5.0 SP2 subdictionary should have ExecutionReport
    let fix50sp2_subdict = dictionary.subdictionary(Version::FIX50SP2).unwrap();
    assert!(fix50sp2_subdict
        .message_by_name("ExecutionReport")
        .is_some());
}

#[test]
fn test_from_file_constructor() {
    let fix44_file = setup_fix44_file();

    // Use the simple constructor instead of the builder
    let result = Dictionary::new(&fix44_file.path().display().to_string());

    let dictionary = result.expect("Failed to create dictionary from file");
    assert_eq!(dictionary.version(), Version::FIX44);

    // Check that we can access fields
    assert!(dictionary.field_by_name("BeginString").is_some());

    // Check that we can access messages
    assert!(dictionary.message_by_name("Heartbeat").is_some());
}

#[test]
fn test_incompatible_versions() {
    let fixt11_file = setup_fixt11_file();
    let fix44_file = setup_fix44_file();

    // Attempting to use FIX 4.4 as application layer for FIXT should fail
    let result = DictionaryBuilder::new()
        .with_fixt_xml(fixt11_file.path())
        .with_fix_xml(fix44_file.path())
        .build();

    assert_matches!(result, Err(Error::IncommpatibleVersion));
}

#[test]
fn test_unknown_version_error() {
    // Test with unknown FIX version
    let invalid_version_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='9' minor='9' servicepack='9'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("invalid_version.xml", invalid_version_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::UnknownVersion(_)));
}

#[test]
fn test_unknown_field_error() {
    // Test with reference to unknown field
    let unknown_field_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='UnknownField' required='Y'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <!-- UnknownField is referenced but not defined -->
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("unknown_field.xml", unknown_field_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::UnknownField(name)) if name == "UnknownField");
}

#[test]
fn test_unknown_component_error() {
    // Test with reference to unknown component
    let unknown_component_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <component name='UnknownComponent' required='Y'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("unknown_component.xml", unknown_component_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::UnknownComponent(name)) if name == "UnknownComponent");
}

#[test]
fn test_duplicated_field_error() {
    // Test with duplicated field definition
    let duplicated_field_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
            <!-- Duplicated field with same name -->
            <field name='TestReqID' number='113' type='INT'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("duplicated_field.xml", duplicated_field_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::DuplicatedField(name)) if name == "TestReqID");
}

#[test]
fn test_duplicated_message_name_error() {
    // Test with duplicated message name
    let duplicated_message_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
            <!-- Same message name, different type -->
            <message msgcat='admin' msgtype='1' name='Heartbeat'>
              <field name='TestReqID' required='Y'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("duplicated_message.xml", duplicated_message_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::DuplicatedMessageName(name)) if name == "Heartbeat");
}

#[test]
fn test_duplicated_message_type_error() {
    // Test with duplicated message type
    let duplicated_msgtype_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
            <!-- Same message type, different name -->
            <message msgcat='admin' msgtype='0' name='TestHeartbeat'>
              <field name='TestReqID' required='Y'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("duplicated_msgtype.xml", duplicated_msgtype_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::DuplicatedMessageType(msg_type)) if msg_type.as_str() == "0");
}

#[test]
fn test_empty_container_error() {
    // Test with empty component
    let empty_component_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <component name='EmptyComponent' required='Y'/>
            </message>
          </messages>
          <components>
            <!-- Empty component with no members -->
            <component name='EmptyComponent'>
            </component>
          </components>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("empty_component.xml", empty_component_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::EmptyContainer(name)) if name == "EmptyComponent");
}

#[test]
fn test_empty_message_error() {
    // Test with empty message
    let empty_message_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <!-- Empty message with no members -->
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("empty_message.xml", empty_message_xml);
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::EmptyMessage(name)) if name == "Heartbeat");
}

#[test]
fn test_unexpected_message_category_error() {
    // Test with unexpected message category in FIXT dictionary
    let unexpected_category_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIXT' major='1' minor='1' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
            <!-- App message in FIXT dictionary (should be admin only) -->
            <message msgcat='app' msgtype='D' name='NewOrder'>
              <field name='TestReqID' required='N'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("unexpected_category.xml", unexpected_category_xml);
    let result = DictionaryBuilder::new()
        .with_fixt_xml(test_file.path())
        .build();

    assert_matches!(result, Err(Error::UnexpectedMessageCategory(MsgCat::App, name)) if name == "NewOrder");
}

#[test]
fn test_missing_required_fields() {
    // Missing required fields
    let missing_required = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header/>
          <trailer/>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("invalid_dict.xml", missing_required);

    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .build();

    // Should fail with EmptyMessage or EmptyContainer
    assert!(result.is_err())
}

#[test]
fn test_builder_with_unspecified_dictionary() {
    let result = DictionaryBuilder::new().build();
    assert!(result.is_err());
}

// Component flattening test

#[test]
fn test_component_flattening() {
    // XML with nested components to test flattening
    let xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
            <field name='BodyLength' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <component name='TestComponent' required='Y'/>
            </message>
          </messages>
          <components>
            <component name='TestComponent'>
              <field name='TestField1' required='Y'/>
              <component name='NestedComponent' required='N'/>
            </component>
            <component name='NestedComponent'>
              <field name='TestField2' required='Y'/>
            </component>
          </components>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='BodyLength' number='9' type='LENGTH'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestField1' number='1000' type='STRING'/>
            <field name='TestField2' number='1001' type='STRING'/>
          </fields>
        </fix>
    "#
    .trim();

    // Parse with flattening disabled
    let normal_dict_raw: xml::Dictionary = from_str(xml).unwrap();
    let normal_dict = Dictionary::from_raw_dictionary(normal_dict_raw, false, false).unwrap();

    // Get the Heartbeat message
    let heartbeat = normal_dict.message_by_name("Heartbeat").unwrap();

    // In the normal case, the message should have one component member
    assert_eq!(heartbeat.members().len(), 1);

    // The member should be a component
    if !matches!(
        heartbeat.members()[0].definition(),
        MemberDefinition::Component(_)
    ) {
        panic!("Expected component member but found something else");
    }

    // Now parse with flattening enabled
    let flattened_dict_raw: xml::Dictionary = from_str(xml).unwrap();
    let flattened_dict = Dictionary::from_raw_dictionary(flattened_dict_raw, true, false)
        .unwrap()
        .flatten()
        .unwrap();

    // Get the Heartbeat message again
    let flattened_heartbeat = flattened_dict.message_by_name("Heartbeat").unwrap();

    // In the flattened case, the message should have TestField1 and TestField2
    assert_eq!(flattened_heartbeat.members().len(), 2);

    // Check the members
    let mut found_field1 = false;
    let mut found_field2 = false;

    for member in flattened_heartbeat.members() {
        match member.definition() {
            MemberDefinition::Field(field) => {
                if field.name == "TestField1" {
                    found_field1 = true;
                    assert!(member.required(), "TestField1 should be required");
                } else if field.name == "TestField2" {
                    found_field2 = true;
                    // TestField2 should not be required because:
                    // - TestField2 is required in NestedComponent
                    // - NestedComponent is not required in TestComponent
                    // - TestComponent is required in Heartbeat
                    // required = Y && N && Y = false
                    assert!(!member.required(), "TestField2 should not be required");
                }
            }
            _ => panic!("Expected field but found something else"),
        }
    }

    assert!(found_field1, "TestField1 not found in flattened message");
    assert!(found_field2, "TestField2 not found in flattened message");
}

fn complex_dictionary(flatten: bool) -> Dictionary {
    let file = TestFile::new("FIX44.xml", COMPLEX_DICT);
    DictionaryBuilder::new()
        .with_fix_xml(file.path())
        .flatten_components(flatten)
        .build()
        .unwrap()
}

#[test]
fn test_deep_component_flattening() {
    // Get dictionary without flattening
    let normal_dict = complex_dictionary(false);

    // Get dictionary with flattening
    let flattened_dict = complex_dictionary(true);

    // Test the NewOrderList message in both dictionaries
    let normal_msg = normal_dict
        .message_by_name("NewOrderList")
        .expect("Message not found");
    let flattened_msg = flattened_dict
        .message_by_name("NewOrderList")
        .expect("Message not found");

    // Without flattening: should have 2 members (OrderListComponent and ListID)
    assert_eq!(normal_msg.members().len(), 2);

    // With flattening: should have more members due to flattened components
    // Specifically: ListID, ListSeqNo, Symbol, SecurityID, SecurityIDSource, SecurityDesc (if flattened)
    // Plus the OrderListGroup
    assert!(flattened_msg.members().len() > 2);

    // Check that component fields are directly in message when flattened
    let flattened_fields = flattened_msg
        .members()
        .iter()
        .filter_map(|m| match m.definition() {
            MemberDefinition::Field(field) => Some(field.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(flattened_fields.contains(&"ListID"), "ListID missing");
    assert!(
        flattened_fields.contains(&"ListSeqNo"),
        "ListSeqNo missing (from OrderListComponent)"
    );
    assert!(
        flattened_fields.contains(&"Symbol"),
        "Symbol missing (from InstrumentComponent)"
    );

    // Check that groups are preserved in flattening
    let has_order_list_group = flattened_msg
        .members()
        .iter()
        .any(|m| match m.definition() {
            MemberDefinition::Group(group) => group.name() == "OrderListGroup",
            _ => false,
        });

    assert!(
        has_order_list_group,
        "OrderListGroup missing after flattening"
    );

    // Get the group from the flattened message to check its contents
    let flattened_group = flattened_msg
        .members()
        .iter()
        .find_map(|m| match m.definition() {
            MemberDefinition::Group(group) if group.name() == "OrderListGroup" => Some(group),
            _ => None,
        })
        .expect("OrderListGroup not found");

    // In the flattened case, the group should have OrderListGroupField and the fields from NestedComponent
    let group_field_names = flattened_group
        .members()
        .iter()
        .filter_map(|m| match m.definition() {
            MemberDefinition::Field(field) => Some(field.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        group_field_names.contains(&"OrderListGroupField"),
        "OrderListGroupField missing"
    );
    assert!(
        group_field_names.contains(&"NestedField1"),
        "NestedField1 missing from flattened group"
    );
    assert!(
        group_field_names.contains(&"NestedField2"),
        "NestedField2 missing from flattened group"
    );

    // Test the OrderComponent's optional group
    let has_order_component_fields = flattened_msg
        .members()
        .iter()
        .any(|m| match m.definition() {
            MemberDefinition::Field(field) => field.name == "OrderID",
            _ => false,
        });

    assert!(
        has_order_component_fields,
        "OrderID missing from flattened message"
    );

    // The OrderParties group should also be included in the flattened message
    let has_order_parties_group = flattened_msg
        .members()
        .iter()
        .any(|m| match m.definition() {
            MemberDefinition::Group(group) => group.name() == "OrderParties",
            _ => false,
        });

    assert!(
        has_order_parties_group,
        "OrderParties group missing from flattened message"
    );
}

#[test]
fn test_nested_required_flag_propagation() {
    // Get dictionary with flattening
    let flattened_dict = complex_dictionary(true);
    let flattened_msg = flattened_dict
        .message_by_name("NewOrderList")
        .expect("Message not found");

    // Check required flag propagation for fields from components
    for member in flattened_msg.members() {
        match member.definition() {
            MemberDefinition::Field(field) => {
                match field.name.as_str() {
                    // These fields should be required because they're required in a required component
                    "ListSeqNo" => assert!(member.required(), "ListSeqNo should be required"),
                    "Symbol" => assert!(member.required(), "Symbol should be required"),

                    // These fields should not be required because:
                    // - Either they're optional in a required component, or
                    // - They're from optional components
                    "SecurityID" => {
                        assert!(!member.required(), "SecurityID should not be required")
                    }
                    "OrderID" => assert!(!member.required(), "OrderID should not be required"),
                    "Price" => assert!(!member.required(), "Price should not be required"),

                    // SecurityIDSource is required in SecurityIDComponent, but SecurityIDComponent is optional
                    // in InstrumentComponent, so the propagated required flag should be false
                    "SecurityIDSource" => assert!(
                        !member.required(),
                        "SecurityIDSource should not be required"
                    ),

                    // Original required fields should maintain their required status
                    "ListID" => assert!(member.required(), "ListID should be required"),

                    _ => {}
                }
            }
            MemberDefinition::Group(group) => {
                match group.name() {
                    // OrderListGroup is required in its parent component, which is required
                    "OrderListGroup" => {
                        assert!(member.required(), "OrderListGroup should be required")
                    }

                    // OrderParties is optional in OrderComponent, which is optional
                    "OrderParties" => {
                        assert!(!member.required(), "OrderParties should not be required")
                    }

                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Also check nested fields in groups to ensure required flags are propagated correctly
    for member in flattened_msg.members() {
        if let MemberDefinition::Group(group) = member.definition() {
            if group.name() == "OrderListGroup" {
                for group_member in group.members() {
                    if let MemberDefinition::Field(field) = group_member.definition() {
                        match field.name.as_str() {
                            "OrderListGroupField" => assert!(
                                group_member.required(),
                                "OrderListGroupField should be required"
                            ),
                            "NestedField1" => {
                                assert!(group_member.required(), "NestedField1 should be required")
                            }
                            "NestedField2" => assert!(
                                !group_member.required(),
                                "NestedField2 should not be required"
                            ),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
}

const CIRCULAR_COMPONENTS_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='BodyLength' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='app' msgtype='X' name='TestMessage'>
      <component name='ComponentA' required='Y'/>
    </message>
  </messages>
  <components>
    <!-- Circular reference: A -> B -> A -->
    <component name='ComponentA'>
      <field name='FieldA' required='Y'/>
      <component name='ComponentB' required='Y'/>
    </component>

    <component name='ComponentB'>
      <field name='FieldB' required='Y'/>
      <component name='ComponentA' required='N'/>
    </component>
  </components>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='BodyLength' number='9' type='LENGTH'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='FieldA' number='101' type='STRING'/>
    <field name='FieldB' number='102' type='STRING'/>
  </fields>
</fix>
"#;

#[test]
fn test_circular_references() {
    // Attempt to build dictionary with circular references
    // This should fail with NestingLevelExceeded error

    let test_file = TestFile::new("circular_dict.xml", CIRCULAR_COMPONENTS_DICT);

    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .flatten_components(true)
        .build();

    // Should fail with nesting level exceeded or similar error
    assert_matches!(
        result,
        Err(Error::CircularReferenceFound(_)),
        "Circular component references should cause an error"
    );
}

#[test]
fn parse_full_xml() {
    let xml = r#"
            <?xml version='1.0' encoding='UTF-8'?>
            <fix type='FIXT' major='1' minor='1' servicepack='0'>
              <header>
                <field name='BeginString' required='Y'/>
                <field name='BodyLength' required='Y'/>
                <field name='MsgType' required='Y'/>
                <field name='SenderCompID' required='Y'/>
                <field name='TargetCompID' required='Y'/>
                <group name='NoHops' required='N'>
                 <field name='HopCompID' required='N' />
                 <field name='HopSendingTime' required='N' />
                 <field name='HopRefID' required='N' />
                </group>
                <field name='ApplVerID' required='N'/>
              </header>
              <trailer>
                <field name='CheckSum' required='Y'/>
              </trailer>
              <messages>
                <message msgcat='admin' msgtype='0' name='Heartbeat'>
                  <field name='TestReqID' required='N'/>
                </message>
                <message msgcat='admin' msgtype='1' name='TestRequest'>
                  <field name='TestReqID' required='Y'/>
                </message>
              </messages>
              <components>
                <component name='MsgTypeGrp'>
                  <group name='NoMsgTypes' required='N'>
                    <field name='RefMsgType' required='N'/>
                    <field name='MsgDirection' required='N'/>
                  </group>
                </component>
              </components>
              <fields>
                <field name='BeginString' number='8' type='STRING'/>
                <field name='BodyLength' number='9' type='LENGTH'/>
                <field name='CheckSum' number='10' type='STRING'/>
                <field number='35' name='MsgType' type='STRING'>
                  <value enum='0' description='HEARTBEAT'/>
                  <value enum='1' description='TEST_REQUEST'/>
                </field>
                <field name='RefMsgType' number='372' type='STRING'/>
                <field name='PossDupFlag' number='43' type='BOOLEAN'/>
                <field name='RefSeqNum' number='45' type='SEQNUM'/>
                <field name='SenderCompID' number='49' type='STRING'/>
                <field name='SenderSubID' number='50' type='STRING'/>
                <field name='SendingTime' number='52' type='UTCTIMESTAMP'/>
                <field name='TargetCompID' number='56' type='STRING'/>
                <field name='TargetSubID' number='57' type='STRING'/>
                <field name='Text' number='58' type='STRING'/>
                <field name='RawDataLength' number='95' type='LENGTH'/>
                <field name='RawData' number='96' type='DATA'/>
                <field name='HeartBtInt' number='108' type='INT'/>
                <field name='TestReqID' number='112' type='STRING'/>
                <field name='OnBehalfOfCompID' number='115' type='STRING'/>
                <field name='OnBehalfOfSubID' number='116' type='STRING'/>
                <field name='OrigSendingTime' number='122' type='UTCTIMESTAMP'/>
                <field name='GapFillFlag' number='123' type='BOOLEAN'/>
                <field name='DeliverToCompID' number='128' type='STRING'/>
                <field name='DeliverToSubID' number='129' type='STRING'/>
                <field name='ResetSeqNumFlag' number='141' type='BOOLEAN'/>
                <field name='SenderLocationID' number='142' type='STRING'/>
                <field name='TargetLocationID' number='143' type='STRING'/>
                <field name='OnBehalfOfLocationID' number='144' type='STRING'/>
                <field name='DeliverToLocationID' number='145' type='STRING'/>
                <field name='SessionRejectReason' number='373' type='INT'>
                  <value description='INVALID_TAG_NUMBER' enum='0'/>
                  <value description='REQUIRED_TAG_MISSING' enum='1'/>
                </field>
                <field name='MaxMessageSize' number='383' type='LENGTH'/>
                <field name='NoMsgTypes' number='384' type='NUMINGROUP'/>
                <field name='MsgDirection' number='385' type='CHAR'>
                  <value description='RECEIVE' enum='R'/>
                  <value description='SEND' enum='S'/>
                </field>
                <field name='TestMessageIndicator' number='464' type='BOOLEAN'/>
                <field name='Username' number='553' type='STRING'/>
                <field name='Password' number='554' type='STRING'/>
                <field name='NoHops' number='627' type='NUMINGROUP'/>
                <field name='HopCompID' number='628' type='STRING'/>
                <field name='HopSendingTime' number='629' type='UTCTIMESTAMP'/>
                <field name='HopRefID' number='630' type='SEQNUM'/>
                <field name='NextExpectedMsgSeqNum' number='789' type='SEQNUM'/>
                <field name='ApplVerID' number='1128' type='STRING'>
                  <value description='FIX44' enum='6'/>
                  <value description='FIX50SP2' enum='9'/>
                </field>
              </fields>
            </fix>
        "#
    .trim();

    // Parse without flattening
    {
        let raw_dictionary: xml::Dictionary = from_str(xml).unwrap();
        let _dictionary = Dictionary::from_raw_dictionary(raw_dictionary, false, false).unwrap();
    }

    // Parse with flattening enabled
    {
        let raw_dictionary: xml::Dictionary = from_str(xml).unwrap();
        let _dictionary = Dictionary::from_raw_dictionary(raw_dictionary, true, false).unwrap();
    }
}

#[test]
fn test_field_lookups() {
    let fix44_file = setup_fix44_file();
    // Build a simple FIX 4.4 dictionary
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .build();

    let dictionary = result.expect("Failed to build dictionary");

    // Look up field by name
    let begin_string = dictionary
        .field_by_name("BeginString")
        .expect("Field not found");
    assert_eq!(begin_string.number, 8);
    assert_eq!(begin_string.name, "BeginString");
    assert_eq!(begin_string.data_type, BasicType::String);

    // Look up field by number
    let found = dictionary.field_by_id(9).expect("Field not found");
    assert_eq!(found.name, "BodyLength");
    assert_eq!(found.data_type, BasicType::Length);

    // Check field with enumerated values
    let msg_type = dictionary
        .field_by_name("MsgType")
        .expect("Field not found");
    let values = msg_type.values.as_ref().expect("No values found");
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].value_enum, "0");
    assert_eq!(values[0].description, "HEARTBEAT");
    assert_eq!(values[1].value_enum, "1");
    assert_eq!(values[1].description, "TEST_REQUEST");

    // Check non-existent field
    assert!(dictionary.field_by_name("NonExistentField").is_none());
    assert!(dictionary.field_by_id(65535).is_none());

    // Enumerate fields
    let all_fields = dictionary.fields().count();
    assert_eq!(all_fields, 6);
}

#[test]
fn test_message_lookups() {
    let fix44_file = setup_fix44_file();
    // Build a simple FIX 4.4 dictionary
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .build();

    let dictionary = result.expect("Failed to build dictionary");

    // Look up message by name - check if exists first
    let Some(heartbeat) = dictionary.message_by_name("Heartbeat") else {
        panic!("Heartbeat message not found");
    };
    assert_eq!(heartbeat.name(), "Heartbeat");
    assert_eq!(heartbeat.msg_type(), "0".parse().unwrap());
    assert_matches!(heartbeat.msg_cat(), MsgCat::Admin);

    // Look up message by type - check if it exists first
    let Some(test_req) = dictionary.message_by_id(b"1") else {
        panic!("MsgType '1' message not found");
    };
    assert_eq!(test_req.name(), "TestRequest");
    assert_matches!(test_req.msg_cat(), MsgCat::Admin);

    // Check members of message
    let members = heartbeat.members();
    assert_eq!(members.len(), 1);

    let member = &members[0];
    assert!(!member.required());
    assert_matches!(
        member.definition(),
        MemberDefinition::Field(field) if field.name == "TestReqID" && field.number == 112
    );

    // Check non-existent message
    assert!(dictionary.message_by_name("NonExistentMessage").is_none());
    assert!(dictionary.message_by_id(b"X").is_none());

    // Enumerate messages
    let all_messages = dictionary.messages().count();
    assert_eq!(all_messages, 2);
}

#[test]
fn test_component_lookups() {
    let fix44_file = setup_fix44_file();
    // Build a simple FIX 4.4 dictionary
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .build();

    let dictionary = result.expect("Failed to build dictionary");

    let Some(component) = dictionary.component("TestComponent") else {
        panic!("TestComponent component not found");
    };
    assert_eq!(component.name(), "TestComponent");

    // Check members of component
    let members = component.members();
    assert_eq!(members.len(), 1);

    let member = &members[0];
    assert!(member.required());
    assert_matches!(
        member.definition(),
        MemberDefinition::Field(field) if field.name == "ComponentField" && field.number == 999
    );

    // Check non-existent component
    assert!(dictionary.component("NonExistentComponent").is_none());

    // Enumerate components
    let all_components = dictionary.components().count();
    assert_eq!(all_components, 1);
}

#[test]
fn test_builder_with_strict_check_unused_field() {
    // Test case 1: Dictionary with unused field
    let unused_field_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
            <field name='BodyLength' required='Y'/>
            <field name='MsgType' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='BodyLength' number='9' type='LENGTH'/>
            <field name='MsgType' number='35' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
            <!-- This field is defined but not used anywhere -->
            <field name='UnusedField' number='999' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("unused_field.xml", unused_field_xml);

    // Without strict check - should succeed
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(false)
        .build();
    assert!(
        result.is_ok(),
        "Dictionary without strict check should succeed with unused field"
    );

    // With strict check - should fail with UnusedFiled error
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(true)
        .build();
    assert_matches!(result, Err(Error::UnusedField(name, tag)) if name == "UnusedField" && tag == 999);
}

#[test]
fn test_builder_with_strict_check_unused_component() {
    // Test case 2: Dictionary with unused component
    let unused_component_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <field name='BeginString' required='Y'/>
            <field name='BodyLength' required='Y'/>
            <field name='MsgType' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
              <!-- Only using UsedComponent -->
              <component name='UsedComponent' required='Y'/>
            </message>
          </messages>
          <components>
            <component name='UsedComponent'>
              <field name='UsedField' required='Y'/>
            </component>
            <!-- This component is defined but not referenced anywhere -->
            <component name='UnusedComponent'>
              <field name='UnusedCompField' required='Y'/>
            </component>
          </components>
          <fields>
            <field name='BeginString' number='8' type='STRING'/>
            <field name='BodyLength' number='9' type='LENGTH'/>
            <field name='MsgType' number='35' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
            <field name='UsedField' number='998' type='STRING'/>
            <field name='UnusedCompField' number='999' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("unused_component.xml", unused_component_xml);

    // Without strict check - should succeed
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(false)
        .build();
    assert!(
        result.is_ok(),
        "Dictionary without strict check should succeed with unused component"
    );

    // With strict check - should fail with UnusedFiled error (for the field inside unused component)
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(true)
        .build();
    assert_matches!(result, Err(Error::UnusedField(name, _)) if name == "UnusedCompField");
}

#[test]
fn test_builder_with_strict_invalid_header() {
    // Test case 3: Test for required field validation
    let invalid_header_xml = r#"
        <?xml version='1.0' encoding='UTF-8'?>
        <fix type='FIX' major='4' minor='4' servicepack='0'>
          <header>
            <!-- First field is wrong (should be BeginString) -->
            <field name='WrongField' required='Y'/>
            <field name='BodyLength' required='Y'/>
            <field name='MsgType' required='Y'/>
          </header>
          <trailer>
            <field name='CheckSum' required='Y'/>
          </trailer>
          <messages>
            <message msgcat='admin' msgtype='0' name='Heartbeat'>
              <field name='TestReqID' required='N'/>
            </message>
          </messages>
          <components/>
          <fields>
            <field name='WrongField' number='8' type='STRING'/>
            <field name='BodyLength' number='9' type='LENGTH'/>
            <field name='MsgType' number='35' type='STRING'/>
            <field name='CheckSum' number='10' type='STRING'/>
            <field name='TestReqID' number='112' type='STRING'/>
          </fields>
        </fix>
    "#;

    let test_file = TestFile::new("invalid_header.xml", invalid_header_xml);

    // Without strict check - should succeed
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(false)
        .build();
    assert!(
        result.is_ok(),
        "Dictionary without strict check should succeed with invalid header"
    );

    // With strict check - should fail with InvalidRequiredField error
    let result = DictionaryBuilder::new()
        .with_fix_xml(test_file.path())
        .with_strict_check(true)
        .build();
    assert_matches!(
        result,
        Err(Error::InvalidRequiredField(name, _, _)) if name == "BeginString",
        "Expected InvalidRequiredField error for BeginString"
    );
}

#[test]
fn test_header_trailer() {
    let fix44_file = setup_fix44_file();
    // Build a simple FIX 4.4 dictionary
    let result = DictionaryBuilder::new()
        .with_fix_xml(fix44_file.path())
        .build();

    let dictionary = result.expect("Failed to build dictionary");

    // Check header
    let header = dictionary.header();
    assert_eq!(header.name(), "Header");
    assert_eq!(header.members().len(), 3);

    // First field in header should be BeginString
    let first_member = &header.members()[0];
    assert!(first_member.required());
    match first_member.definition() {
        MemberDefinition::Field(field) => {
            assert_eq!(field.name, "BeginString");
        }
        _ => panic!("Expected field member"),
    }

    // Check trailer
    let trailer = dictionary.trailer();
    assert_eq!(trailer.name(), "Trailer");
    assert_eq!(trailer.members().len(), 1);

    // First field in trailer should be CheckSum
    let first_member = &trailer.members()[0];
    assert!(first_member.required());
    match first_member.definition() {
        MemberDefinition::Field(field) => {
            assert_eq!(field.name, "CheckSum");
        }
        _ => panic!("Expected field member"),
    }
}
