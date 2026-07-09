use std::{env, fs, process};

use easyfix_dictionary::fix_str;

use super::*;

const DOC_DICT: &str = r#"
<?xml version='1.0' encoding='UTF-8'?>
<fix type='FIX' major='4' minor='4' servicepack='0'>
  <header>
    <field name='BeginString' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message msgcat='app' msgtype='D' name='NewOrderSingle' doc='Submits a new order.'>
      <field name='ClOrdID' required='Y'/>
      <field name='Side' required='Y'/>
      <group name='NoAllocs' required='N' doc='Allocations of the order.'>
        <field name='AllocAccount' required='N'/>
      </group>
    </message>
    <message msgcat='app' msgtype='G' name='OrderCancelReplaceRequest'>
      <field name='OrigClOrdID' required='Y'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field name='BeginString' number='8' type='STRING'/>
    <field name='CheckSum' number='10' type='STRING'/>
    <field name='ClOrdID' number='11' type='STRING' doc='Unique order id.'/>
    <field name='OrigClOrdID' number='41' type='STRING'/>
    <field name='Side' number='54' type='CHAR' doc='Side of order.'>
      <value enum='1' description='BUY' doc='Buy order.'/>
      <value enum='2' description='SELL'/>
    </field>
    <field name='NoAllocs' number='78' type='NUMINGROUP'/>
    <field name='AllocAccount' number='79' type='STRING'/>
  </fields>
</fix>
"#;

fn doc_dictionary(file_name: &str) -> Dictionary {
    let path = env::temp_dir().join(format!("easyfix_{}_{}.xml", file_name, process::id()));
    fs::write(&path, DOC_DICT).unwrap();
    let dictionary = Dictionary::new(path.to_str().unwrap())
        .unwrap()
        .flatten()
        .unwrap();
    fs::remove_file(&path).unwrap();
    dictionary
}

#[test]
fn message_doc_comments_emitted() {
    let dictionary = doc_dictionary("message_doc");
    let msg = dictionary
        .message_by_name(fix_str!("NewOrderSingle"))
        .unwrap();
    let code = MessageCodeGen::new(
        msg.name(),
        msg.msg_type(),
        convert_members(msg.members()),
        msg.msg_cat(),
        msg.doc(),
    )
    .generate(false, false)
    .to_string();

    // Message doc on the struct, followed by the technical MsgType line
    assert!(code.contains("Submits a new order."));
    assert!(code.contains(r#"MsgType \"D\"."#));
    // Field doc on the struct member, followed by the technical tag line
    assert!(code.contains("Unique order id."));
    assert!(code.contains("Tag 11."));
    // Group member slot picks up the group doc
    assert!(code.contains("Allocations of the order."));
}

#[test]
fn message_without_doc_has_no_doc_separator() {
    let dictionary = doc_dictionary("message_no_doc");
    let msg = dictionary
        .message_by_name(fix_str!("OrderCancelReplaceRequest"))
        .unwrap();
    let code = MessageCodeGen::new(
        msg.name(),
        msg.msg_type(),
        convert_members(msg.members()),
        msg.msg_cat(),
        msg.doc(),
    )
    .generate(false, false)
    .to_string();

    // Without doc attributes the output stays exactly as before:
    // no empty separator doc lines anywhere
    assert!(!code.contains(r#"doc = """#));
    assert!(code.contains(r#"MsgType \"G\"."#));
}

#[test]
fn group_doc_comments_emitted() {
    let dictionary = doc_dictionary("group_doc");
    let group = dictionary.group(fix_str!("Allocs")).unwrap();
    let code = GroupCodeGen::new(
        group.name(),
        group.num_in_group().number(),
        convert_members(group.members()),
        group.doc(),
    )
    .generate(false, false)
    .to_string();

    assert!(code.contains("Allocations of the order."));
    assert!(code.contains("NumInGroup tag 78."));
}

#[test]
fn enum_doc_comments_emitted() {
    let dictionary = doc_dictionary("enum_doc");
    let side = dictionary.field_by_id(54).unwrap();
    let code = EnumCodeGen::new(
        side.name(),
        side.number(),
        EnumerableType::try_from_basic_type(side.data_type()).unwrap(),
        side.variants().to_vec(),
        side.doc(),
    )
    .generate(false, false)
    .to_string();

    // Field doc on the enum type
    assert!(code.contains("Side of order."));
    // Variant doc, followed by the technical value line
    assert!(code.contains("Buy order."));
    assert!(code.contains(r#"Value \"1\""#));
}

#[test]
fn identical_field_definitions_accepted() {
    validate_field_agreement(
        58,
        "Text",
        dict::BasicType::String,
        "Text",
        dict::BasicType::String,
    );
}

#[test]
#[should_panic(expected = "tag 1130 is DefaultVerIndicator in the transport dictionary")]
fn field_name_clash_rejected() {
    validate_field_agreement(
        1130,
        "DefaultVerIndicator",
        dict::BasicType::Boolean,
        "RefApplVerID",
        dict::BasicType::String,
    );
}

#[test]
#[should_panic(expected = "field SessionStatus(1409) is Int in the transport dictionary")]
fn field_type_clash_rejected() {
    validate_field_agreement(
        1409,
        "SessionStatus",
        dict::BasicType::Int,
        "SessionStatus",
        dict::BasicType::String,
    );
}

#[test]
fn full_appl_ver_id_codeset_accepted() {
    validate_appl_ver_id_codeset(
        "DefaultApplVerID",
        1137,
        &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
    );
}

#[test]
fn empty_appl_ver_id_value_list_accepted() {
    validate_appl_ver_id_codeset("DefaultApplVerID", 1137, &[]);
}

#[test]
#[should_panic(expected = "DefaultApplVerID(1137) customizes the ApplVerIDCodeSet")]
fn trimmed_appl_ver_id_codeset_rejected() {
    validate_appl_ver_id_codeset("DefaultApplVerID", 1137, &["9"]);
}

#[test]
#[should_panic(expected = "DefaultApplVerID(1137) customizes the ApplVerIDCodeSet")]
fn extended_appl_ver_id_codeset_rejected() {
    validate_appl_ver_id_codeset(
        "DefaultApplVerID",
        1137,
        &["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11"],
    );
}
