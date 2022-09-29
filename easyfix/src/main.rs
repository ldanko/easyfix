//#![feature(option_unwrap_none)]
#![feature(try_blocks)]
#![feature(type_alias_impl_trait)]

use std::time::Instant;

use anyhow::Result;
use easyfix::messages::{FixtMessage, Message};

fn main() -> Result<()> {
    let input = "8=FIXT.1.1|9=116|35=A|49=BuySide|56=SellSide|34=1|52=20190605-11:51:27.848|1128=9|98=0|108=30|141=Y|553=Username|554=Password|1137=9|10=079|";
    println!("Trying to parse: {}", input);
    let input = input.replace('|', "\x01");
    let logon = match FixtMessage::from_bytes(input.as_bytes()) {
        Ok(
            msg @ FixtMessage {
                body: Message::Logon(_),
                ..
            },
        ) => msg,
        Err(err) => panic!("Logon deserialization error: {:?}", err),
        _ => panic!(),
    };
    println!("{:#?}", logon);
    let output = String::from_utf8_lossy(&logon.serialize()).replace('\x01', "|");
    let input = input.replace('\x01', "|");
    println!("{}", input);
    println!("{}", output);
    println!("{}", logon.dbg_fix_str());
    println!("input {} output", if input == output { "==" } else { "!=" });
    assert_eq!(input, output);
    println!("{:?}", logon);

    println!("--- FIX Gen");

    if false {
        let msg = b"8=FIXT.1.19=11635=A49=BuySide56=SellSide34=152=20190605-11:51:27.8481128=998=0108=30141=Y553=Username554=Password1137=910=079";
        let mut sum = 0_i32;
        let start = Instant::now();
        for i in 0..1_000_000 {
            if let Err(e) = FixtMessage::from_bytes(msg) {
                panic!("DUPA: {}", e);
            };
            sum = sum.wrapping_add(i);
        }
        let stop = Instant::now();
        println!("-> {:?} // {}", stop - start, sum);
    }

    Ok(())
}
