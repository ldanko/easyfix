use core::fmt;

use easyfix_messages::{
    fields::{FixStr, FixString},
    messages::{FixtMessage, Header},
};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq)]
pub struct SessionId {
    begin_string: FixString,
    sender_comp_id: FixString,
    target_comp_id: FixString,
    session_qualifier: String,
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.session_qualifier.is_empty() {
            write!(
                f,
                "{}: {} -> {}",
                self.begin_string, self.sender_comp_id, self.target_comp_id
            )
        } else {
            write!(
                f,
                "{}: {} -> {} ({})",
                self.begin_string, self.sender_comp_id, self.target_comp_id, self.session_qualifier
            )
        }
    }
}

impl SessionId {
    pub fn new(
        begin_string: FixString,
        sender_comp_id: FixString,
        target_comp_id: FixString,
    ) -> SessionId {
        SessionId {
            begin_string,
            sender_comp_id,
            target_comp_id,
            session_qualifier: String::new(),
        }
    }

    pub fn with_session_qualifier(
        begin_string: FixString,
        sender_comp_id: FixString,
        target_comp_id: FixString,
        session_qualifier: String,
    ) -> SessionId {
        SessionId {
            begin_string,
            sender_comp_id,
            target_comp_id,
            session_qualifier,
        }
    }

    pub fn from_input_msg(msg: &FixtMessage) -> SessionId {
        SessionId::from_input_header(&msg.header)
    }

    pub fn from_input_header(header: &Header) -> SessionId {
        SessionId::new(
            header.begin_string.clone(),
            header.target_comp_id.clone(),
            header.sender_comp_id.clone(),
        )
    }

    pub fn from_output_msg(msg: &FixtMessage) -> SessionId {
        SessionId::from_output_header(&msg.header)
    }

    pub fn from_output_header(header: &Header) -> SessionId {
        SessionId::new(
            header.begin_string.clone(),
            header.sender_comp_id.clone(),
            header.target_comp_id.clone(),
        )
    }

    pub fn reverse_route(mut self) -> SessionId {
        std::mem::swap(&mut self.sender_comp_id, &mut self.target_comp_id);
        self
    }

    pub fn begin_string(&self) -> &FixStr {
        &self.begin_string
    }

    pub fn sender_comp_id(&self) -> &FixStr {
        &self.sender_comp_id
    }

    pub fn target_comp_id(&self) -> &FixStr {
        &self.target_comp_id
    }

    pub fn session_qualifier(&self) -> &str {
        &self.session_qualifier
    }

    pub fn is_fixt(&self) -> bool {
        self.begin_string.as_utf8().starts_with("FIXT")
    }
}
