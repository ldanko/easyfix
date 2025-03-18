use quick_xml::de::from_str;

use super::*;

#[test]
fn parse_msg_type() {
    assert!(MsgType::from_str("").is_err());
    assert!(MsgType::from_str("\0").is_err());
    assert!(MsgType::from_str("\0\0").is_err());
    assert!(MsgType::from_str("\0\0\0").is_err());
    assert!(MsgType::from_str("A").is_ok());
    assert!(MsgType::from_str("AA").is_ok());
    assert!(MsgType::from_str("AAA").is_err());
    assert!(MsgType::from_str("\0A").is_err());
    assert!(MsgType::from_str("Ą").is_err());
    assert!(MsgType::from_str("ĄA").is_err());
}

#[test]
fn test_msg_type_methods() {
    // Test MsgType creation and accessor methods
    let msg_type_single = MsgType::from_str("0").unwrap();
    let msg_type_double = MsgType::from_str("AB").unwrap();

    // Test as_bytes()
    assert_eq!(msg_type_single.as_bytes(), b"0");
    assert_eq!(msg_type_double.as_bytes(), b"AB");

    // Test as_str()
    assert_eq!(msg_type_single.as_str(), "0");
    assert_eq!(msg_type_double.as_str(), "AB");

    // Test Display implementation
    assert_eq!(msg_type_single.to_string(), "0");
    assert_eq!(msg_type_double.to_string(), "AB");
}

#[test]
fn parse_basic_types() {
    // Test parsing all defined basic types
    #[rustfmt::skip]
    let type_pairs = [
        ("<field name='AmtField' number='1' type='AMT'/>", BasicType::Amt),
        ("<field name='BoolField' number='2' type='BOOLEAN'/>", BasicType::Boolean),
        ("<field name='CharField' number='3' type='CHAR'/>", BasicType::Char),
        ("<field name='CountryField' number='4' type='COUNTRY'/>", BasicType::Country),
        ("<field name='CurrencyField' number='5' type='CURRENCY'/>", BasicType::Currency),
        ("<field name='DataField' number='6' type='DATA'/>", BasicType::Data),
        ("<field name='ExchangeField' number='7' type='EXCHANGE'/>", BasicType::Exchange),
        ("<field name='FloatField' number='8' type='FLOAT'/>", BasicType::Float),
        ("<field name='IntField' number='9' type='INT'/>", BasicType::Int),
        ("<field name='LanguageField' number='10' type='LANGUAGE'/>", BasicType::Language),
        ("<field name='LengthField' number='11' type='LENGTH'/>", BasicType::Length),
        ("<field name='LocalMktDateField' number='12' type='LOCALMKTDATE'/>", BasicType::LocalMktDate),
        ("<field name='MonthYearField' number='13' type='MONTHYEAR'/>", BasicType::MonthYear),
        ("<field name='MultipleCharValueField' number='14' type='MULTIPLECHARVALUE'/>", BasicType::MultipleCharValue),
        ("<field name='MultipleStringValueField' number='15' type='MULTIPLESTRINGVALUE'/>", BasicType::MultipleStringValue),
        ("<field name='NumInGroupField' number='16' type='NUMINGROUP'/>", BasicType::NumInGroup),
        ("<field name='PercentageField' number='17' type='PERCENTAGE'/>", BasicType::Percentage),
        ("<field name='PriceField' number='18' type='PRICE'/>", BasicType::Price),
        ("<field name='PriceOffsetField' number='19' type='PRICEOFFSET'/>", BasicType::PriceOffset),
        ("<field name='QtyField' number='20' type='QTY'/>", BasicType::Qty),
        ("<field name='SeqNumField' number='21' type='SEQNUM'/>", BasicType::SeqNum),
        ("<field name='StringField' number='22' type='STRING'/>", BasicType::String),
        ("<field name='TzTimeOnlyField' number='23' type='TZTIMEONLY'/>", BasicType::TzTimeOnly),
        ("<field name='TzTimestampField' number='24' type='TZTIMESTAMP'/>", BasicType::TzTimestamp),
        ("<field name='UtcDateOnlyField' number='25' type='UTCDATEONLY'/>", BasicType::UtcDateOnly),
        ("<field name='UtcTimeOnlyField' number='26' type='UTCTIMEONLY'/>", BasicType::UtcTimeOnly),
        ("<field name='UtcTimestampField' number='27' type='UTCTIMESTAMP'/>", BasicType::UtcTimestamp),
        ("<field name='XmlDataField' number='28' type='XMLDATA'/>", BasicType::XmlData),
    ];

    for (xml, expected_type) in type_pairs {
        let field: Field = from_str(xml).unwrap_or_else(|_| panic!("Failed to parse: {xml}"));
        assert_eq!(field.data_type, expected_type, "Type mismatch for: {xml}");
    }
}

#[test]
fn parse_long_as_int() {
    let xml = r#"<field name='TEST_FIELD' number='1000' type='LONG'/>"#;
    let field: Field = from_str(xml).unwrap();
    assert_eq!(field.data_type, BasicType::Int);
}

#[test]
fn parse_field_with_values() {
    let xml = r#"
    <field name='MsgType' number='35' type='STRING'>
        <value enum='0' description='HEARTBEAT'/>
        <value enum='1' description='TEST_REQUEST'/>
        <value enum='2' description='RESEND_REQUEST'/>
    </field>
    "#;

    let field: Field = from_str(xml).unwrap();

    assert_eq!(field.name, "MsgType");
    assert_eq!(field.number, 35);
    assert_eq!(field.data_type, BasicType::String);

    let values = field.values.as_ref().expect("No values found");
    assert_eq!(values.len(), 3);

    assert_eq!(values[0].value_enum, "0");
    assert_eq!(values[0].description, "HEARTBEAT");

    assert_eq!(values[1].value_enum, "1");
    assert_eq!(values[1].description, "TEST_REQUEST");

    assert_eq!(values[2].value_enum, "2");
    assert_eq!(values[2].description, "RESEND_REQUEST");
}

#[test]
fn test_required_flag_parsing() {
    // Test different formats of the required flag
    let xml_variants = [
        ("<field name='Test' required='Y'/>", true),
        ("<field name='Test' required='YES'/>", true),
        ("<field name='Test' required='y'/>", true),
        ("<field name='Test' required='yes'/>", true),
        ("<field name='Test' required='N'/>", false),
        ("<field name='Test' required='NO'/>", false),
        ("<field name='Test' required='n'/>", false),
        ("<field name='Test' required='no'/>", false),
    ];

    for (xml, expected) in xml_variants {
        let member: Member = from_str(xml).unwrap_or_else(|_| panic!("Failed to parse: {xml}"));
        match member {
            Member::Field(ref_field) => {
                assert_eq!(
                    ref_field.required, expected,
                    "Required flag mismatch for: {}",
                    xml
                );
            }
            _ => panic!("Expected field member for: {}", xml),
        }
    }

    // Test invalid required flag
    let invalid_xml = "<field name='Test' required='MAYBE'/>";
    let result: Result<Member, _> = from_str(invalid_xml);
    assert!(result.is_err(), "Should fail with invalid required value");
}

#[test]
fn test_member_methods() {
    // Test name() and required() methods on Member enum variants

    // Field member
    let field_member = Member::Field(MemberRef {
        name: "TestField".to_string(),
        required: true,
    });

    assert_eq!(field_member.name(), "TestField");
    assert!(field_member.required());

    // Component member
    let component_member = Member::Component(MemberRef {
        name: "TestComponent".to_string(),
        required: false,
    });

    assert_eq!(component_member.name(), "TestComponent");
    assert!(!component_member.required());

    // Group member
    let group_member = Member::Group(Group {
        name: "TestGroup".to_string(),
        required: true,
        members: vec![],
    });

    assert_eq!(group_member.name(), "TestGroup");
    assert!(group_member.required());
}

#[test]
fn parse_header() {
    let xml = r#"
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
    "#
    .trim();

    let header = from_str::<Header>(xml).unwrap();

    // Verify header content
    assert_eq!(header.members.len(), 7);

    // Check BeginString field
    let begin_string = header
        .members
        .iter()
        .find(|m| matches!(m, Member::Field(field_ref) if field_ref.name == "BeginString"))
        .expect("BeginString field not found");

    if let Member::Field(field_ref) = begin_string {
        assert_eq!(field_ref.name, "BeginString");
        assert!(field_ref.required);
    } else {
        panic!("Expected field member for BeginString");
    };

    // Check NoHops group
    let no_hops = header
        .members
        .iter()
        .find(|m| matches!(m, Member::Group(group) if group.name == "NoHops"))
        .expect("NoHops group not found");

    if let Member::Group(group) = no_hops {
        assert_eq!(group.name, "NoHops");
        assert!(!group.required);
        assert_eq!(group.members.len(), 3);
    } else {
        panic!("Expected group member for NoHops")
    }
}

#[test]
fn parse_empty_header() {
    let xml = r#"<header/>"#;

    let header = from_str::<Header>(xml).unwrap();
    assert!(header.members.is_empty());
}

#[test]
fn parse_empty_trailer() {
    let xml = r#"<trailer/>"#;

    let trailer = from_str::<Trailer>(xml).unwrap();
    assert!(trailer.members.is_empty());
}

#[test]
fn parse_component() {
    let xml = r#"
        <component name='Instrument'>
          <field name='Symbol' required='Y'/>
          <field name='SecurityID' required='N'/>
          <field name='SecurityIDSource' required='N'/>
          <group name='NoSecurityAltID' required='N'>
            <field name='SecurityAltID' required='N'/>
            <field name='SecurityAltIDSource' required='N'/>
          </group>
        </component>
    "#;

    let component: Component = from_str(xml).unwrap();

    assert_eq!(component.name, "Instrument");
    assert_eq!(component.members.len(), 4);

    // Check the group inside the component
    let no_security_alt_id = &component.members[3];
    if let Member::Group(group) = no_security_alt_id {
        assert_eq!(group.name, "NoSecurityAltID");
        assert!(!group.required);
        assert_eq!(group.members.len(), 2);
    } else {
        panic!("Expected group member for NoSecurityAltID")
    }
}

#[test]
fn parse_group() {
    let xml = r#"
        <group name='NoPartyIDs' required='Y'>
          <field name='PartyID' required='Y'/>
          <field name='PartyIDSource' required='Y'/>
          <field name='PartyRole' required='Y'/>
          <group name='NoPartySubIDs' required='N'>
            <field name='PartySubID' required='Y'/>
            <field name='PartySubIDType' required='Y'/>
          </group>
        </group>
    "#;

    let group: Group = from_str(xml).unwrap();

    assert_eq!(group.name, "NoPartyIDs");
    assert!(group.required);
    assert_eq!(group.members.len(), 4);

    // Check for nested group
    let no_party_sub_ids = &group.members[3];
    if let Member::Group(sub_group) = no_party_sub_ids {
        assert_eq!(sub_group.name, "NoPartySubIDs");
        assert!(!sub_group.required);
        assert_eq!(sub_group.members.len(), 2);
    } else {
        panic!("Expected group member for NoPartySubIDs")
    }
}

#[test]
fn parse_message() {
    let xml = r#"
        <message msgcat='admin' msgtype='A' name='Logon'>
          <field name='EncryptMethod' required='Y'/>
          <field name='HeartBtInt' required='Y'/>
          <field name='RawDataLength' required='N'/>
          <field name='RawData' required='N'/>
          <field name='ResetSeqNumFlag' required='N'/>
          <field name='NextExpectedMsgSeqNum' required='N'/>
          <field name='MaxMessageSize' required='N'/>
          <field name='TestMessageIndicator' required='N'/>
          <field name='Username' required='N'/>
          <field name='Password' required='N'/>
          <field name='DefaultApplVerID' required='Y'/>
          <component name='MsgTypeGrp' required='N'/>
        </message>
    "#
    .trim();

    let message: Message = from_str(xml).unwrap();

    assert_eq!(message.name, "Logon");
    assert_eq!(message.msg_type.as_str(), "A");
    assert_eq!(message.msg_cat, MsgCat::Admin);
    assert_eq!(message.members.len(), 12);

    // Check a required field
    let encrypt_method = &message.members[0];
    if let Member::Field(field_ref) = encrypt_method {
        assert_eq!(field_ref.name, "EncryptMethod");
        assert!(field_ref.required);
    } else {
        panic!("Expected field member for EncryptMethod")
    }

    // Check a component
    let msg_type_grp = &message.members[11];
    if let Member::Component(comp_ref) = msg_type_grp {
        assert_eq!(comp_ref.name, "MsgTypeGrp");
        assert!(!comp_ref.required);
    } else {
        panic!("Expected component member for MsgTypeGrp")
    }
}

#[test]
fn parse_app_message() {
    let xml = r#"
        <message msgcat='app' msgtype='D' name='NewOrderSingle'>
          <field name='ClOrdID' required='Y'/>
          <field name='Side' required='Y'/>
          <field name='TransactTime' required='Y'/>
          <field name='OrdType' required='Y'/>
        </message>
    "#;

    let message: Message = from_str(xml).unwrap();

    assert_eq!(message.name, "NewOrderSingle");
    assert_eq!(message.msg_type, MsgType::from_str("D").unwrap());
    assert_eq!(message.msg_cat, MsgCat::App);
    assert_eq!(message.members.len(), 4);
}

#[test]
fn parse_empty_message() {
    let xml = r#"<message msgcat='admin' msgtype='A' name='Logon'></message>"#.trim();

    // No error, empty message error is reported on higher layer
    assert!(from_str::<Message>(xml).is_ok())
}

#[test]
fn test_msg_cat() {
    let admin_xml = r#"
        <message msgcat='admin' msgtype='0' name='Heartbeat'>
          <field name='TestField' required='Y'/>
        </message>
    "#;
    let app_xml = r#"
        <message msgcat='app' msgtype='D' name='NewOrderSingle'>
          <field name='TestField' required='Y'/>
        </message>
    "#;

    let admin_message: Message = from_str(admin_xml).unwrap();
    let app_message: Message = from_str(app_xml).unwrap();

    assert_eq!(admin_message.msg_cat, MsgCat::Admin);
    assert_eq!(app_message.msg_cat, MsgCat::App);
}

#[test]
fn test_fix_type() {
    // Test Display implementation
    assert_eq!(FixType::Fix.to_string(), "FIX");
    assert_eq!(FixType::Fixt.to_string(), "FIXT");
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
            <field name='SignatureLength' required='N'/>
            <field name='Signature' required='N'/>
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

    let dictionary: Dictionary = from_str(xml).unwrap();

    // Check dictionary attributes
    assert_eq!(dictionary.fix_type, FixType::Fixt);
    assert_eq!(dictionary.major, 1);
    assert_eq!(dictionary.minor, 1);
    assert_eq!(dictionary.servicepack, 0);

    // Check components
    assert_eq!(dictionary.components.len(), 1);
    assert_eq!(dictionary.components[0].name, "MsgTypeGrp");

    // Check messages
    assert_eq!(dictionary.messages.len(), 2);
    assert_eq!(dictionary.messages[0].name, "Heartbeat");
    assert_eq!(dictionary.messages[1].name, "TestRequest");

    // Check fields
    assert!(dictionary.fields.len() > 30);

    // Find a specific field
    let msg_type_field = dictionary
        .fields
        .iter()
        .find(|f| f.name == "MsgType")
        .expect("MsgType field not found");

    assert_eq!(msg_type_field.number, 35);
    assert_eq!(msg_type_field.data_type, BasicType::String);

    // Check field values
    let values = msg_type_field
        .values
        .as_ref()
        .expect("MsgType should have values");
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].value_enum, "0");
    assert_eq!(values[0].description, "HEARTBEAT");
}
