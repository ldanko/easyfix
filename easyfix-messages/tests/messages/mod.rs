#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum MsgType {
    #[default]
    ///Value "0"
    Heartbeat,
    ///Value "1"
    TestRequest,
    ///Value "2"
    ResendRequest,
    ///Value "3"
    Reject,
    ///Value "4"
    SequenceReset,
    ///Value "5"
    Logout,
    ///Value "A"
    Logon,
    ///Value "8"
    ExecutionReport,
    ///Value "D"
    NewOrderSingle,
}
impl MsgType {
    pub const fn from_bytes(input: &[u8]) -> Option<MsgType> {
        match input {
            b"0" => Some(MsgType::Heartbeat),
            b"1" => Some(MsgType::TestRequest),
            b"2" => Some(MsgType::ResendRequest),
            b"3" => Some(MsgType::Reject),
            b"4" => Some(MsgType::SequenceReset),
            b"5" => Some(MsgType::Logout),
            b"A" => Some(MsgType::Logon),
            b"8" => Some(MsgType::ExecutionReport),
            b"D" => Some(MsgType::NewOrderSingle),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<MsgType> {
        MsgType::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            MsgType::Heartbeat => b"0",
            MsgType::TestRequest => b"1",
            MsgType::ResendRequest => b"2",
            MsgType::Reject => b"3",
            MsgType::SequenceReset => b"4",
            MsgType::Logout => b"5",
            MsgType::Logon => b"A",
            MsgType::ExecutionReport => b"8",
            MsgType::NewOrderSingle => b"D",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for MsgType {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<&FixStr> for MsgType {
    type Error = SessionRejectReasonBase;

    fn try_from(input: &FixStr) -> Result<MsgType, SessionRejectReasonBase> {
        match input.as_bytes() {
            b"0" => Ok(MsgType::Heartbeat),
            b"1" => Ok(MsgType::TestRequest),
            b"2" => Ok(MsgType::ResendRequest),
            b"3" => Ok(MsgType::Reject),
            b"4" => Ok(MsgType::SequenceReset),
            b"5" => Ok(MsgType::Logout),
            b"A" => Ok(MsgType::Logon),
            b"8" => Ok(MsgType::ExecutionReport),
            b"D" => Ok(MsgType::NewOrderSingle),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<MsgType> for &'static [u8] {
    fn from(input: MsgType) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OrdStatus {
    #[default]
    ///Value "0"
    New,
    ///Value "1"
    PartiallyFilled,
    ///Value "2"
    Filled,
    ///Value "4"
    Canceled,
    ///Value "8"
    Rejected,
}
impl OrdStatus {
    pub const fn from_bytes(input: &[u8]) -> Option<OrdStatus> {
        match input {
            b"0" => Some(OrdStatus::New),
            b"1" => Some(OrdStatus::PartiallyFilled),
            b"2" => Some(OrdStatus::Filled),
            b"4" => Some(OrdStatus::Canceled),
            b"8" => Some(OrdStatus::Rejected),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<OrdStatus> {
        OrdStatus::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            OrdStatus::New => b"0",
            OrdStatus::PartiallyFilled => b"1",
            OrdStatus::Filled => b"2",
            OrdStatus::Canceled => b"4",
            OrdStatus::Rejected => b"8",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for OrdStatus {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Char> for OrdStatus {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Char) -> Result<OrdStatus, SessionRejectReasonBase> {
        match input {
            48u8 => Ok(OrdStatus::New),
            49u8 => Ok(OrdStatus::PartiallyFilled),
            50u8 => Ok(OrdStatus::Filled),
            52u8 => Ok(OrdStatus::Canceled),
            56u8 => Ok(OrdStatus::Rejected),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<OrdStatus> for &'static [u8] {
    fn from(input: OrdStatus) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OrdType {
    #[default]
    ///Value "1"
    Market,
    ///Value "2"
    Limit,
}
impl OrdType {
    pub const fn from_bytes(input: &[u8]) -> Option<OrdType> {
        match input {
            b"1" => Some(OrdType::Market),
            b"2" => Some(OrdType::Limit),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<OrdType> {
        OrdType::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            OrdType::Market => b"1",
            OrdType::Limit => b"2",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for OrdType {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Char> for OrdType {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Char) -> Result<OrdType, SessionRejectReasonBase> {
        match input {
            49u8 => Ok(OrdType::Market),
            50u8 => Ok(OrdType::Limit),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<OrdType> for &'static [u8] {
    fn from(input: OrdType) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Side {
    #[default]
    ///Value "1"
    Buy,
    ///Value "2"
    Sell,
}
impl Side {
    pub const fn from_bytes(input: &[u8]) -> Option<Side> {
        match input {
            b"1" => Some(Side::Buy),
            b"2" => Some(Side::Sell),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<Side> {
        Side::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            Side::Buy => b"1",
            Side::Sell => b"2",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for Side {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Char> for Side {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Char) -> Result<Side, SessionRejectReasonBase> {
        match input {
            49u8 => Ok(Side::Buy),
            50u8 => Ok(Side::Sell),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<Side> for &'static [u8] {
    fn from(input: Side) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EncryptMethod {
    #[default]
    ///Value "0"
    None,
    ///Value "1"
    Pkcs,
    ///Value "2"
    Des,
    ///Value "3"
    PkcsDes,
    ///Value "4"
    PgpDes,
    ///Value "5"
    PgpDesMd5,
    ///Value "6"
    Pem,
}
impl EncryptMethod {
    pub const fn from_bytes(input: &[u8]) -> Option<EncryptMethod> {
        match input {
            b"0" => Some(EncryptMethod::None),
            b"1" => Some(EncryptMethod::Pkcs),
            b"2" => Some(EncryptMethod::Des),
            b"3" => Some(EncryptMethod::PkcsDes),
            b"4" => Some(EncryptMethod::PgpDes),
            b"5" => Some(EncryptMethod::PgpDesMd5),
            b"6" => Some(EncryptMethod::Pem),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<EncryptMethod> {
        EncryptMethod::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            EncryptMethod::None => b"0",
            EncryptMethod::Pkcs => b"1",
            EncryptMethod::Des => b"2",
            EncryptMethod::PkcsDes => b"3",
            EncryptMethod::PgpDes => b"4",
            EncryptMethod::PgpDesMd5 => b"5",
            EncryptMethod::Pem => b"6",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }

    pub const fn as_int(&self) -> Int {
        match self {
            EncryptMethod::None => 0i64,
            EncryptMethod::Pkcs => 1i64,
            EncryptMethod::Des => 2i64,
            EncryptMethod::PkcsDes => 3i64,
            EncryptMethod::PgpDes => 4i64,
            EncryptMethod::PgpDesMd5 => 5i64,
            EncryptMethod::Pem => 6i64,
        }
    }
}
impl ToFixString for EncryptMethod {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Int> for EncryptMethod {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Int) -> Result<EncryptMethod, SessionRejectReasonBase> {
        match input {
            0i64 => Ok(EncryptMethod::None),
            1i64 => Ok(EncryptMethod::Pkcs),
            2i64 => Ok(EncryptMethod::Des),
            3i64 => Ok(EncryptMethod::PkcsDes),
            4i64 => Ok(EncryptMethod::PgpDes),
            5i64 => Ok(EncryptMethod::PgpDesMd5),
            6i64 => Ok(EncryptMethod::Pem),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<EncryptMethod> for &'static [u8] {
    fn from(input: EncryptMethod) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExecType {
    #[default]
    ///Value "0"
    New,
    ///Value "F"
    Trade,
    ///Value "4"
    Canceled,
    ///Value "8"
    Rejected,
}
impl ExecType {
    pub const fn from_bytes(input: &[u8]) -> Option<ExecType> {
        match input {
            b"0" => Some(ExecType::New),
            b"F" => Some(ExecType::Trade),
            b"4" => Some(ExecType::Canceled),
            b"8" => Some(ExecType::Rejected),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<ExecType> {
        ExecType::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            ExecType::New => b"0",
            ExecType::Trade => b"F",
            ExecType::Canceled => b"4",
            ExecType::Rejected => b"8",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for ExecType {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Char> for ExecType {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Char) -> Result<ExecType, SessionRejectReasonBase> {
        match input {
            48u8 => Ok(ExecType::New),
            70u8 => Ok(ExecType::Trade),
            52u8 => Ok(ExecType::Canceled),
            56u8 => Ok(ExecType::Rejected),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<ExecType> for &'static [u8] {
    fn from(input: ExecType) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionRejectReason {
    #[default]
    ///Value "0"
    InvalidTagNumber,
    ///Value "1"
    RequiredTagMissing,
    ///Value "2"
    TagNotDefinedForThisMessageType,
    ///Value "3"
    UndefinedTag,
    ///Value "4"
    TagSpecifiedWithoutAValue,
    ///Value "5"
    ValueIsIncorrect,
    ///Value "6"
    IncorrectDataFormatForValue,
    ///Value "7"
    DecryptionProblem,
    ///Value "8"
    SignatureProblem,
    ///Value "9"
    CompIdProblem,
    ///Value "10"
    SendingTimeAccuracyProblem,
    ///Value "11"
    InvalidMsgType,
    ///Value "12"
    XmlValidationError,
    ///Value "13"
    TagAppearsMoreThanOnce,
    ///Value "14"
    TagSpecifiedOutOfRequiredOrder,
    ///Value "15"
    RepeatingGroupFieldsOutOfOrder,
    ///Value "16"
    IncorrectNumInGroupCountForRepeatingGroup,
    ///Value "17"
    FieldDelimiterInFieldValue,
    ///Value "18"
    InvalidUnsupportedAppVersion,
}
impl SessionRejectReason {
    pub const fn from_bytes(input: &[u8]) -> Option<SessionRejectReason> {
        match input {
            b"0" => Some(SessionRejectReason::InvalidTagNumber),
            b"1" => Some(SessionRejectReason::RequiredTagMissing),
            b"2" => Some(SessionRejectReason::TagNotDefinedForThisMessageType),
            b"3" => Some(SessionRejectReason::UndefinedTag),
            b"4" => Some(SessionRejectReason::TagSpecifiedWithoutAValue),
            b"5" => Some(SessionRejectReason::ValueIsIncorrect),
            b"6" => Some(SessionRejectReason::IncorrectDataFormatForValue),
            b"7" => Some(SessionRejectReason::DecryptionProblem),
            b"8" => Some(SessionRejectReason::SignatureProblem),
            b"9" => Some(SessionRejectReason::CompIdProblem),
            b"10" => Some(SessionRejectReason::SendingTimeAccuracyProblem),
            b"11" => Some(SessionRejectReason::InvalidMsgType),
            b"12" => Some(SessionRejectReason::XmlValidationError),
            b"13" => Some(SessionRejectReason::TagAppearsMoreThanOnce),
            b"14" => Some(SessionRejectReason::TagSpecifiedOutOfRequiredOrder),
            b"15" => Some(SessionRejectReason::RepeatingGroupFieldsOutOfOrder),
            b"16" => Some(SessionRejectReason::IncorrectNumInGroupCountForRepeatingGroup),
            b"17" => Some(SessionRejectReason::FieldDelimiterInFieldValue),
            b"18" => Some(SessionRejectReason::InvalidUnsupportedAppVersion),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<SessionRejectReason> {
        SessionRejectReason::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            SessionRejectReason::InvalidTagNumber => b"0",
            SessionRejectReason::RequiredTagMissing => b"1",
            SessionRejectReason::TagNotDefinedForThisMessageType => b"2",
            SessionRejectReason::UndefinedTag => b"3",
            SessionRejectReason::TagSpecifiedWithoutAValue => b"4",
            SessionRejectReason::ValueIsIncorrect => b"5",
            SessionRejectReason::IncorrectDataFormatForValue => b"6",
            SessionRejectReason::DecryptionProblem => b"7",
            SessionRejectReason::SignatureProblem => b"8",
            SessionRejectReason::CompIdProblem => b"9",
            SessionRejectReason::SendingTimeAccuracyProblem => b"10",
            SessionRejectReason::InvalidMsgType => b"11",
            SessionRejectReason::XmlValidationError => b"12",
            SessionRejectReason::TagAppearsMoreThanOnce => b"13",
            SessionRejectReason::TagSpecifiedOutOfRequiredOrder => b"14",
            SessionRejectReason::RepeatingGroupFieldsOutOfOrder => b"15",
            SessionRejectReason::IncorrectNumInGroupCountForRepeatingGroup => b"16",
            SessionRejectReason::FieldDelimiterInFieldValue => b"17",
            SessionRejectReason::InvalidUnsupportedAppVersion => b"18",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }

    pub const fn as_int(&self) -> Int {
        match self {
            SessionRejectReason::InvalidTagNumber => 0i64,
            SessionRejectReason::RequiredTagMissing => 1i64,
            SessionRejectReason::TagNotDefinedForThisMessageType => 2i64,
            SessionRejectReason::UndefinedTag => 3i64,
            SessionRejectReason::TagSpecifiedWithoutAValue => 4i64,
            SessionRejectReason::ValueIsIncorrect => 5i64,
            SessionRejectReason::IncorrectDataFormatForValue => 6i64,
            SessionRejectReason::DecryptionProblem => 7i64,
            SessionRejectReason::SignatureProblem => 8i64,
            SessionRejectReason::CompIdProblem => 9i64,
            SessionRejectReason::SendingTimeAccuracyProblem => 10i64,
            SessionRejectReason::InvalidMsgType => 11i64,
            SessionRejectReason::XmlValidationError => 12i64,
            SessionRejectReason::TagAppearsMoreThanOnce => 13i64,
            SessionRejectReason::TagSpecifiedOutOfRequiredOrder => 14i64,
            SessionRejectReason::RepeatingGroupFieldsOutOfOrder => 15i64,
            SessionRejectReason::IncorrectNumInGroupCountForRepeatingGroup => 16i64,
            SessionRejectReason::FieldDelimiterInFieldValue => 17i64,
            SessionRejectReason::InvalidUnsupportedAppVersion => 18i64,
        }
    }
}
impl ToFixString for SessionRejectReason {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Int> for SessionRejectReason {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Int) -> Result<SessionRejectReason, SessionRejectReasonBase> {
        match input {
            0i64 => Ok(SessionRejectReason::InvalidTagNumber),
            1i64 => Ok(SessionRejectReason::RequiredTagMissing),
            2i64 => Ok(SessionRejectReason::TagNotDefinedForThisMessageType),
            3i64 => Ok(SessionRejectReason::UndefinedTag),
            4i64 => Ok(SessionRejectReason::TagSpecifiedWithoutAValue),
            5i64 => Ok(SessionRejectReason::ValueIsIncorrect),
            6i64 => Ok(SessionRejectReason::IncorrectDataFormatForValue),
            7i64 => Ok(SessionRejectReason::DecryptionProblem),
            8i64 => Ok(SessionRejectReason::SignatureProblem),
            9i64 => Ok(SessionRejectReason::CompIdProblem),
            10i64 => Ok(SessionRejectReason::SendingTimeAccuracyProblem),
            11i64 => Ok(SessionRejectReason::InvalidMsgType),
            12i64 => Ok(SessionRejectReason::XmlValidationError),
            13i64 => Ok(SessionRejectReason::TagAppearsMoreThanOnce),
            14i64 => Ok(SessionRejectReason::TagSpecifiedOutOfRequiredOrder),
            15i64 => Ok(SessionRejectReason::RepeatingGroupFieldsOutOfOrder),
            16i64 => Ok(SessionRejectReason::IncorrectNumInGroupCountForRepeatingGroup),
            17i64 => Ok(SessionRejectReason::FieldDelimiterInFieldValue),
            18i64 => Ok(SessionRejectReason::InvalidUnsupportedAppVersion),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<SessionRejectReason> for &'static [u8] {
    fn from(input: SessionRejectReason) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MsgDirection {
    #[default]
    ///Value "S"
    Send,
    ///Value "R"
    Receive,
}
impl MsgDirection {
    pub const fn from_bytes(input: &[u8]) -> Option<MsgDirection> {
        match input {
            b"S" => Some(MsgDirection::Send),
            b"R" => Some(MsgDirection::Receive),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<MsgDirection> {
        MsgDirection::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            MsgDirection::Send => b"S",
            MsgDirection::Receive => b"R",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for MsgDirection {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Char> for MsgDirection {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Char) -> Result<MsgDirection, SessionRejectReasonBase> {
        match input {
            83u8 => Ok(MsgDirection::Send),
            82u8 => Ok(MsgDirection::Receive),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<MsgDirection> for &'static [u8] {
    fn from(input: MsgDirection) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApplVerId {
    #[default]
    ///Value "0"
    Fix27,
    ///Value "1"
    Fix30,
    ///Value "2"
    Fix40,
    ///Value "3"
    Fix41,
    ///Value "4"
    Fix42,
    ///Value "5"
    Fix43,
    ///Value "6"
    Fix44,
    ///Value "7"
    Fix50,
    ///Value "8"
    Fix50Sp1,
    ///Value "9"
    Fix50Sp2,
    ///Value "10"
    FixLatest,
}
impl ApplVerId {
    pub const fn from_bytes(input: &[u8]) -> Option<ApplVerId> {
        match input {
            b"0" => Some(ApplVerId::Fix27),
            b"1" => Some(ApplVerId::Fix30),
            b"2" => Some(ApplVerId::Fix40),
            b"3" => Some(ApplVerId::Fix41),
            b"4" => Some(ApplVerId::Fix42),
            b"5" => Some(ApplVerId::Fix43),
            b"6" => Some(ApplVerId::Fix44),
            b"7" => Some(ApplVerId::Fix50),
            b"8" => Some(ApplVerId::Fix50Sp1),
            b"9" => Some(ApplVerId::Fix50Sp2),
            b"10" => Some(ApplVerId::FixLatest),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<ApplVerId> {
        ApplVerId::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            ApplVerId::Fix27 => b"0",
            ApplVerId::Fix30 => b"1",
            ApplVerId::Fix40 => b"2",
            ApplVerId::Fix41 => b"3",
            ApplVerId::Fix42 => b"4",
            ApplVerId::Fix43 => b"5",
            ApplVerId::Fix44 => b"6",
            ApplVerId::Fix50 => b"7",
            ApplVerId::Fix50Sp1 => b"8",
            ApplVerId::Fix50Sp2 => b"9",
            ApplVerId::FixLatest => b"10",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for ApplVerId {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<&FixStr> for ApplVerId {
    type Error = SessionRejectReasonBase;

    fn try_from(input: &FixStr) -> Result<ApplVerId, SessionRejectReasonBase> {
        match input.as_bytes() {
            b"0" => Ok(ApplVerId::Fix27),
            b"1" => Ok(ApplVerId::Fix30),
            b"2" => Ok(ApplVerId::Fix40),
            b"3" => Ok(ApplVerId::Fix41),
            b"4" => Ok(ApplVerId::Fix42),
            b"5" => Ok(ApplVerId::Fix43),
            b"6" => Ok(ApplVerId::Fix44),
            b"7" => Ok(ApplVerId::Fix50),
            b"8" => Ok(ApplVerId::Fix50Sp1),
            b"9" => Ok(ApplVerId::Fix50Sp2),
            b"10" => Ok(ApplVerId::FixLatest),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<ApplVerId> for &'static [u8] {
    fn from(input: ApplVerId) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DefaultApplVerId {
    #[default]
    ///Value "0"
    Fix27,
    ///Value "1"
    Fix30,
    ///Value "2"
    Fix40,
    ///Value "3"
    Fix41,
    ///Value "4"
    Fix42,
    ///Value "5"
    Fix43,
    ///Value "6"
    Fix44,
    ///Value "7"
    Fix50,
    ///Value "8"
    Fix50Sp1,
    ///Value "9"
    Fix50Sp2,
    ///Value "10"
    FixLatest,
}
impl DefaultApplVerId {
    pub const fn from_bytes(input: &[u8]) -> Option<DefaultApplVerId> {
        match input {
            b"0" => Some(DefaultApplVerId::Fix27),
            b"1" => Some(DefaultApplVerId::Fix30),
            b"2" => Some(DefaultApplVerId::Fix40),
            b"3" => Some(DefaultApplVerId::Fix41),
            b"4" => Some(DefaultApplVerId::Fix42),
            b"5" => Some(DefaultApplVerId::Fix43),
            b"6" => Some(DefaultApplVerId::Fix44),
            b"7" => Some(DefaultApplVerId::Fix50),
            b"8" => Some(DefaultApplVerId::Fix50Sp1),
            b"9" => Some(DefaultApplVerId::Fix50Sp2),
            b"10" => Some(DefaultApplVerId::FixLatest),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<DefaultApplVerId> {
        DefaultApplVerId::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            DefaultApplVerId::Fix27 => b"0",
            DefaultApplVerId::Fix30 => b"1",
            DefaultApplVerId::Fix40 => b"2",
            DefaultApplVerId::Fix41 => b"3",
            DefaultApplVerId::Fix42 => b"4",
            DefaultApplVerId::Fix43 => b"5",
            DefaultApplVerId::Fix44 => b"6",
            DefaultApplVerId::Fix50 => b"7",
            DefaultApplVerId::Fix50Sp1 => b"8",
            DefaultApplVerId::Fix50Sp2 => b"9",
            DefaultApplVerId::FixLatest => b"10",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for DefaultApplVerId {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<&FixStr> for DefaultApplVerId {
    type Error = SessionRejectReasonBase;

    fn try_from(input: &FixStr) -> Result<DefaultApplVerId, SessionRejectReasonBase> {
        match input.as_bytes() {
            b"0" => Ok(DefaultApplVerId::Fix27),
            b"1" => Ok(DefaultApplVerId::Fix30),
            b"2" => Ok(DefaultApplVerId::Fix40),
            b"3" => Ok(DefaultApplVerId::Fix41),
            b"4" => Ok(DefaultApplVerId::Fix42),
            b"5" => Ok(DefaultApplVerId::Fix43),
            b"6" => Ok(DefaultApplVerId::Fix44),
            b"7" => Ok(DefaultApplVerId::Fix50),
            b"8" => Ok(DefaultApplVerId::Fix50Sp1),
            b"9" => Ok(DefaultApplVerId::Fix50Sp2),
            b"10" => Ok(DefaultApplVerId::FixLatest),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<DefaultApplVerId> for &'static [u8] {
    fn from(input: DefaultApplVerId) -> &'static [u8] {
        input.as_bytes()
    }
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionStatus {
    #[default]
    ///Value "0"
    SessionActive,
    ///Value "1"
    SessionPasswordChanged,
    ///Value "2"
    SessionPasswordDueToExpire,
    ///Value "3"
    NewSessionPasswordDoesNotComplyWithPolicy,
    ///Value "4"
    SessionLogoutComplete,
    ///Value "5"
    InvalidUsernameOrPassword,
    ///Value "6"
    AccountLocked,
    ///Value "7"
    LogonsAreNotAllowedAtThisTime,
    ///Value "8"
    PasswordExpired,
    ///Value "9"
    ReceivedMsgSeqNumTooLow,
    ///Value "10"
    ReceivedNextExpectedMsgSeqNumTooHigh,
}
impl SessionStatus {
    pub const fn from_bytes(input: &[u8]) -> Option<SessionStatus> {
        match input {
            b"0" => Some(SessionStatus::SessionActive),
            b"1" => Some(SessionStatus::SessionPasswordChanged),
            b"2" => Some(SessionStatus::SessionPasswordDueToExpire),
            b"3" => Some(SessionStatus::NewSessionPasswordDoesNotComplyWithPolicy),
            b"4" => Some(SessionStatus::SessionLogoutComplete),
            b"5" => Some(SessionStatus::InvalidUsernameOrPassword),
            b"6" => Some(SessionStatus::AccountLocked),
            b"7" => Some(SessionStatus::LogonsAreNotAllowedAtThisTime),
            b"8" => Some(SessionStatus::PasswordExpired),
            b"9" => Some(SessionStatus::ReceivedMsgSeqNumTooLow),
            b"10" => Some(SessionStatus::ReceivedNextExpectedMsgSeqNumTooHigh),
            _ => None,
        }
    }

    pub const fn from_fix_str(input: &FixStr) -> Option<SessionStatus> {
        SessionStatus::from_bytes(input.as_bytes())
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            SessionStatus::SessionActive => b"0",
            SessionStatus::SessionPasswordChanged => b"1",
            SessionStatus::SessionPasswordDueToExpire => b"2",
            SessionStatus::NewSessionPasswordDoesNotComplyWithPolicy => b"3",
            SessionStatus::SessionLogoutComplete => b"4",
            SessionStatus::InvalidUsernameOrPassword => b"5",
            SessionStatus::AccountLocked => b"6",
            SessionStatus::LogonsAreNotAllowedAtThisTime => b"7",
            SessionStatus::PasswordExpired => b"8",
            SessionStatus::ReceivedMsgSeqNumTooLow => b"9",
            SessionStatus::ReceivedNextExpectedMsgSeqNumTooHigh => b"10",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }

    pub const fn as_int(&self) -> Int {
        match self {
            SessionStatus::SessionActive => 0i64,
            SessionStatus::SessionPasswordChanged => 1i64,
            SessionStatus::SessionPasswordDueToExpire => 2i64,
            SessionStatus::NewSessionPasswordDoesNotComplyWithPolicy => 3i64,
            SessionStatus::SessionLogoutComplete => 4i64,
            SessionStatus::InvalidUsernameOrPassword => 5i64,
            SessionStatus::AccountLocked => 6i64,
            SessionStatus::LogonsAreNotAllowedAtThisTime => 7i64,
            SessionStatus::PasswordExpired => 8i64,
            SessionStatus::ReceivedMsgSeqNumTooLow => 9i64,
            SessionStatus::ReceivedNextExpectedMsgSeqNumTooHigh => 10i64,
        }
    }
}
impl ToFixString for SessionStatus {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
impl TryFrom<Int> for SessionStatus {
    type Error = SessionRejectReasonBase;

    fn try_from(input: Int) -> Result<SessionStatus, SessionRejectReasonBase> {
        match input {
            0i64 => Ok(SessionStatus::SessionActive),
            1i64 => Ok(SessionStatus::SessionPasswordChanged),
            2i64 => Ok(SessionStatus::SessionPasswordDueToExpire),
            3i64 => Ok(SessionStatus::NewSessionPasswordDoesNotComplyWithPolicy),
            4i64 => Ok(SessionStatus::SessionLogoutComplete),
            5i64 => Ok(SessionStatus::InvalidUsernameOrPassword),
            6i64 => Ok(SessionStatus::AccountLocked),
            7i64 => Ok(SessionStatus::LogonsAreNotAllowedAtThisTime),
            8i64 => Ok(SessionStatus::PasswordExpired),
            9i64 => Ok(SessionStatus::ReceivedMsgSeqNumTooLow),
            10i64 => Ok(SessionStatus::ReceivedNextExpectedMsgSeqNumTooHigh),
            _ => Err(SessionRejectReasonBase::ValueIsIncorrect),
        }
    }
}
impl From<SessionStatus> for &'static [u8] {
    fn from(input: SessionStatus) -> &'static [u8] {
        input.as_bytes()
    }
}
impl MsgTypeValue for MsgType {
    fn raw_value(&self) -> MsgTypeField {
        MsgTypeField::from_bytes(self.as_bytes()).expect("generated MsgType values are valid")
    }
}
impl From<MsgTypeField> for MsgType {
    fn from(field: MsgTypeField) -> MsgType {
        MsgType::from_bytes(field.as_bytes()).expect("validated by MsgTypeField")
    }
}
impl From<EncryptMethodBase> for EncryptMethod {
    fn from(input: EncryptMethodBase) -> EncryptMethod {
        match input {
            EncryptMethodBase::None => EncryptMethod::None,
        }
    }
}
impl SessionRejectReasonValue for SessionRejectReason {
    fn raw_value(&self) -> Int {
        match self {
            SessionRejectReason::InvalidTagNumber => 0i64,
            SessionRejectReason::RequiredTagMissing => 1i64,
            SessionRejectReason::TagNotDefinedForThisMessageType => 2i64,
            SessionRejectReason::UndefinedTag => 3i64,
            SessionRejectReason::TagSpecifiedWithoutAValue => 4i64,
            SessionRejectReason::ValueIsIncorrect => 5i64,
            SessionRejectReason::IncorrectDataFormatForValue => 6i64,
            SessionRejectReason::DecryptionProblem => 7i64,
            SessionRejectReason::SignatureProblem => 8i64,
            SessionRejectReason::CompIdProblem => 9i64,
            SessionRejectReason::SendingTimeAccuracyProblem => 10i64,
            SessionRejectReason::InvalidMsgType => 11i64,
            SessionRejectReason::XmlValidationError => 12i64,
            SessionRejectReason::TagAppearsMoreThanOnce => 13i64,
            SessionRejectReason::TagSpecifiedOutOfRequiredOrder => 14i64,
            SessionRejectReason::RepeatingGroupFieldsOutOfOrder => 15i64,
            SessionRejectReason::IncorrectNumInGroupCountForRepeatingGroup => 16i64,
            SessionRejectReason::FieldDelimiterInFieldValue => 17i64,
            SessionRejectReason::InvalidUnsupportedAppVersion => 18i64,
        }
    }
}
impl From<SessionRejectReasonField> for SessionRejectReason {
    fn from(field: SessionRejectReasonField) -> SessionRejectReason {
        SessionRejectReason::try_from(field.into_inner()).expect("validated by field newtype")
    }
}
impl SessionStatusValue for SessionStatus {
    fn raw_value(&self) -> Int {
        match self {
            SessionStatus::SessionActive => 0i64,
            SessionStatus::SessionPasswordChanged => 1i64,
            SessionStatus::SessionPasswordDueToExpire => 2i64,
            SessionStatus::NewSessionPasswordDoesNotComplyWithPolicy => 3i64,
            SessionStatus::SessionLogoutComplete => 4i64,
            SessionStatus::InvalidUsernameOrPassword => 5i64,
            SessionStatus::AccountLocked => 6i64,
            SessionStatus::LogonsAreNotAllowedAtThisTime => 7i64,
            SessionStatus::PasswordExpired => 8i64,
            SessionStatus::ReceivedMsgSeqNumTooLow => 9i64,
            SessionStatus::ReceivedNextExpectedMsgSeqNumTooHigh => 10i64,
        }
    }
}
impl From<SessionStatusField> for SessionStatus {
    fn from(field: SessionStatusField) -> SessionStatus {
        SessionStatus::try_from(field.into_inner()).expect("validated by field newtype")
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct MsgTypeGrp {
    ///Tag 372.
    pub ref_msg_type: Option<FixString>,
    ///Tag 385.
    pub msg_direction: Option<MsgDirection>,
    ///Tag 1130.
    pub default_ver_indicator: Option<Boolean>,
}
#[allow(dead_code)]
impl MsgTypeGrp {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(ref_msg_type) = &self.ref_msg_type {
            serializer.output_mut().extend_from_slice(b"372=");
            serializer.serialize_string(ref_msg_type);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(msg_direction) = &self.msg_direction {
            serializer.output_mut().extend_from_slice(b"385=");
            serializer.serialize_enum(msg_direction);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(default_ver_indicator) = &self.default_ver_indicator {
            serializer.output_mut().extend_from_slice(b"1130=");
            serializer.serialize_boolean(default_ver_indicator);
            serializer.output_mut().push(b'\x01');
        }
    }

    pub(crate) fn deserialize(
        deserializer: &mut Deserializer,
        num_in_group_tag: u16,
        expected_tags: &[u16],
        last_run: bool,
    ) -> Result<MsgTypeGrp, DeserializeError> {
        let mut ref_msg_type: Option<FixString> = None;
        let mut msg_direction: Option<MsgDirection> = None;
        let mut default_ver_indicator: Option<Boolean> = None;
        if let Some(372u16) = deserializer.deserialize_tag_num()? {
            if ref_msg_type.is_some() {
                return Err(deserializer.reject(
                    Some(372u16),
                    SessionRejectReasonBase::TagAppearsMoreThanOnce,
                ));
            }
            ref_msg_type = Some(deserializer.deserialize_string()?);
        } else {
            return Err(
                deserializer.reject(Some(372u16), SessionRejectReasonBase::RequiredTagMissing)
            );
        };
        let mut processed_tags = Vec::with_capacity(expected_tags.len());
        let mut iter = expected_tags.iter();
        iter.next();
        processed_tags.push(372u16);
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                385u16 => {
                    if msg_direction.is_some() {
                        return Err(deserializer.reject(
                            Some(385u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    msg_direction = Some(deserializer.deserialize_char_enum()?);
                }
                1130u16 => {
                    if default_ver_indicator.is_some() {
                        return Err(deserializer.reject(
                            Some(1130u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    default_ver_indicator = Some(deserializer.deserialize_boolean()?);
                }
                tag => {
                    if tag == 372u16 && last_run || tag != 372u16 && !last_run {
                        return Err(deserializer.reject(
                            Some(num_in_group_tag),
                            SessionRejectReasonBase::IncorrectNumInGroupCountForRepeatingGroup,
                        ));
                    } else {
                        deserializer.put_tag(tag);
                        break;
                    }
                }
            }
            processed_tags.push(tag);
            let mut tag_in_order = false;
            for expected_tag in iter.by_ref() {
                if *expected_tag == tag {
                    tag_in_order = true;
                    break;
                }
            }
            if !tag_in_order {
                return Err(deserializer.repeating_group_fields_out_of_order(
                    expected_tags,
                    &processed_tags,
                    tag,
                ));
            }
        }
        Ok(MsgTypeGrp {
            ref_msg_type,
            msg_direction,
            default_ver_indicator,
        })
    }
}
use std::{borrow::Cow, fmt};

#[allow(unused_imports)]
use easyfix_core::base_messages::{
    AdminBase, EncryptMethodBase, HeaderBase, HeartbeatBase, LogonBase, LogoutBase, RejectBase,
    ResendRequestBase, SequenceResetBase, SessionRejectReasonBase, TestRequestBase,
};
pub use easyfix_core::message::MsgCat;
use easyfix_core::message::{HeaderAccess, SessionMessage};
#[allow(unused_imports)]
use easyfix_core::{
    basic_types::{
        Amt, Boolean, Char, Country, Currency, Data, DayOfMonth, Decimal, Exchange, FixStr,
        FixString, Float, Int, Language, Length, LocalMktDate, LocalMktTime, MonthYear,
        MsgTypeField, MsgTypeValue, MultipleCharValue, MultipleStringValue, NumInGroup, Percentage,
        Price, PriceOffset, Qty, SeqNum, SessionRejectReasonField, SessionRejectReasonValue,
        SessionStatusField, SessionStatusValue, TagNum, Tenor, TenorUnit, TimePrecision,
        ToFixString, TzTimeOnly, TzTimestamp, UtcDateOnly, UtcTimeOnly, UtcTimestamp, XmlData,
    },
    deserializer::{DeserializeError, Deserializer, RawMessage, raw_message},
    serializer::Serializer,
};
pub const BEGIN_STRING: &FixStr = unsafe { FixStr::from_ascii_unchecked(b"FIXT.1.1") };
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum FieldTag {
    BeginSeqNo = 7u16,
    BeginString = 8u16,
    BodyLength = 9u16,
    CheckSum = 10u16,
    ClOrdId = 11u16,
    CumQty = 14u16,
    EndSeqNo = 16u16,
    ExecId = 17u16,
    MsgSeqNum = 34u16,
    MsgType = 35u16,
    NewSeqNo = 36u16,
    OrderId = 37u16,
    OrderQty = 38u16,
    OrdStatus = 39u16,
    OrdType = 40u16,
    PossDupFlag = 43u16,
    Price = 44u16,
    RefSeqNum = 45u16,
    SenderCompId = 49u16,
    SenderSubId = 50u16,
    SendingTime = 52u16,
    Side = 54u16,
    Symbol = 55u16,
    TargetCompId = 56u16,
    TargetSubId = 57u16,
    Text = 58u16,
    TransactTime = 60u16,
    Signature = 89u16,
    SignatureLength = 93u16,
    RawDataLength = 95u16,
    RawData = 96u16,
    EncryptMethod = 98u16,
    HeartBtInt = 108u16,
    TestReqId = 112u16,
    OrigSendingTime = 122u16,
    GapFillFlag = 123u16,
    ResetSeqNumFlag = 141u16,
    ExecType = 150u16,
    LeavesQty = 151u16,
    RefTagId = 371u16,
    RefMsgType = 372u16,
    SessionRejectReason = 373u16,
    NoMsgTypes = 384u16,
    MsgDirection = 385u16,
    NextExpectedMsgSeqNum = 789u16,
    ApplVerId = 1128u16,
    DefaultVerIndicator = 1130u16,
    DefaultApplVerId = 1137u16,
    SessionStatus = 1409u16,
}
impl fmt::Display for FieldTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_fix_str().as_utf8())
    }
}
#[allow(dead_code)]
impl FieldTag {
    pub const fn from_tag_num(tag_num: TagNum) -> Option<FieldTag> {
        match tag_num {
            7u16 => Some(FieldTag::BeginSeqNo),
            8u16 => Some(FieldTag::BeginString),
            9u16 => Some(FieldTag::BodyLength),
            10u16 => Some(FieldTag::CheckSum),
            11u16 => Some(FieldTag::ClOrdId),
            14u16 => Some(FieldTag::CumQty),
            16u16 => Some(FieldTag::EndSeqNo),
            17u16 => Some(FieldTag::ExecId),
            34u16 => Some(FieldTag::MsgSeqNum),
            35u16 => Some(FieldTag::MsgType),
            36u16 => Some(FieldTag::NewSeqNo),
            37u16 => Some(FieldTag::OrderId),
            38u16 => Some(FieldTag::OrderQty),
            39u16 => Some(FieldTag::OrdStatus),
            40u16 => Some(FieldTag::OrdType),
            43u16 => Some(FieldTag::PossDupFlag),
            44u16 => Some(FieldTag::Price),
            45u16 => Some(FieldTag::RefSeqNum),
            49u16 => Some(FieldTag::SenderCompId),
            50u16 => Some(FieldTag::SenderSubId),
            52u16 => Some(FieldTag::SendingTime),
            54u16 => Some(FieldTag::Side),
            55u16 => Some(FieldTag::Symbol),
            56u16 => Some(FieldTag::TargetCompId),
            57u16 => Some(FieldTag::TargetSubId),
            58u16 => Some(FieldTag::Text),
            60u16 => Some(FieldTag::TransactTime),
            89u16 => Some(FieldTag::Signature),
            93u16 => Some(FieldTag::SignatureLength),
            95u16 => Some(FieldTag::RawDataLength),
            96u16 => Some(FieldTag::RawData),
            98u16 => Some(FieldTag::EncryptMethod),
            108u16 => Some(FieldTag::HeartBtInt),
            112u16 => Some(FieldTag::TestReqId),
            122u16 => Some(FieldTag::OrigSendingTime),
            123u16 => Some(FieldTag::GapFillFlag),
            141u16 => Some(FieldTag::ResetSeqNumFlag),
            150u16 => Some(FieldTag::ExecType),
            151u16 => Some(FieldTag::LeavesQty),
            371u16 => Some(FieldTag::RefTagId),
            372u16 => Some(FieldTag::RefMsgType),
            373u16 => Some(FieldTag::SessionRejectReason),
            384u16 => Some(FieldTag::NoMsgTypes),
            385u16 => Some(FieldTag::MsgDirection),
            789u16 => Some(FieldTag::NextExpectedMsgSeqNum),
            1128u16 => Some(FieldTag::ApplVerId),
            1130u16 => Some(FieldTag::DefaultVerIndicator),
            1137u16 => Some(FieldTag::DefaultApplVerId),
            1409u16 => Some(FieldTag::SessionStatus),
            _ => None,
        }
    }

    pub const fn as_bytes(&self) -> &'static [u8] {
        match self {
            FieldTag::BeginSeqNo => b"BeginSeqNo",
            FieldTag::BeginString => b"BeginString",
            FieldTag::BodyLength => b"BodyLength",
            FieldTag::CheckSum => b"CheckSum",
            FieldTag::ClOrdId => b"ClOrdId",
            FieldTag::CumQty => b"CumQty",
            FieldTag::EndSeqNo => b"EndSeqNo",
            FieldTag::ExecId => b"ExecId",
            FieldTag::MsgSeqNum => b"MsgSeqNum",
            FieldTag::MsgType => b"MsgType",
            FieldTag::NewSeqNo => b"NewSeqNo",
            FieldTag::OrderId => b"OrderId",
            FieldTag::OrderQty => b"OrderQty",
            FieldTag::OrdStatus => b"OrdStatus",
            FieldTag::OrdType => b"OrdType",
            FieldTag::PossDupFlag => b"PossDupFlag",
            FieldTag::Price => b"Price",
            FieldTag::RefSeqNum => b"RefSeqNum",
            FieldTag::SenderCompId => b"SenderCompId",
            FieldTag::SenderSubId => b"SenderSubId",
            FieldTag::SendingTime => b"SendingTime",
            FieldTag::Side => b"Side",
            FieldTag::Symbol => b"Symbol",
            FieldTag::TargetCompId => b"TargetCompId",
            FieldTag::TargetSubId => b"TargetSubId",
            FieldTag::Text => b"Text",
            FieldTag::TransactTime => b"TransactTime",
            FieldTag::Signature => b"Signature",
            FieldTag::SignatureLength => b"SignatureLength",
            FieldTag::RawDataLength => b"RawDataLength",
            FieldTag::RawData => b"RawData",
            FieldTag::EncryptMethod => b"EncryptMethod",
            FieldTag::HeartBtInt => b"HeartBtInt",
            FieldTag::TestReqId => b"TestReqId",
            FieldTag::OrigSendingTime => b"OrigSendingTime",
            FieldTag::GapFillFlag => b"GapFillFlag",
            FieldTag::ResetSeqNumFlag => b"ResetSeqNumFlag",
            FieldTag::ExecType => b"ExecType",
            FieldTag::LeavesQty => b"LeavesQty",
            FieldTag::RefTagId => b"RefTagId",
            FieldTag::RefMsgType => b"RefMsgType",
            FieldTag::SessionRejectReason => b"SessionRejectReason",
            FieldTag::NoMsgTypes => b"NoMsgTypes",
            FieldTag::MsgDirection => b"MsgDirection",
            FieldTag::NextExpectedMsgSeqNum => b"NextExpectedMsgSeqNum",
            FieldTag::ApplVerId => b"ApplVerId",
            FieldTag::DefaultVerIndicator => b"DefaultVerIndicator",
            FieldTag::DefaultApplVerId => b"DefaultApplVerId",
            FieldTag::SessionStatus => b"SessionStatus",
        }
    }

    pub const fn as_fix_str(&self) -> &'static FixStr {
        unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
    }
}
impl ToFixString for FieldTag {
    fn to_fix_string(&self) -> FixString {
        self.as_fix_str().to_owned()
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Header {
    ///Tag 8.
    pub begin_string: FixString,
    ///Tag 9.
    pub body_length: Length,
    ///Tag 1128.
    pub appl_ver_id: Option<ApplVerId>,
    ///Tag 49.
    pub sender_comp_id: FixString,
    ///Tag 56.
    pub target_comp_id: FixString,
    ///Tag 34.
    pub msg_seq_num: SeqNum,
    ///Tag 50.
    pub sender_sub_id: Option<FixString>,
    ///Tag 57.
    pub target_sub_id: Option<FixString>,
    ///Tag 43.
    pub poss_dup_flag: Option<Boolean>,
    ///Tag 52.
    pub sending_time: UtcTimestamp,
    ///Tag 122.
    pub orig_sending_time: Option<UtcTimestamp>,
}
#[allow(dead_code)]
impl Header {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(appl_ver_id) = &self.appl_ver_id {
            serializer.output_mut().extend_from_slice(b"1128=");
            serializer.serialize_enum(appl_ver_id);
            serializer.output_mut().push(b'\x01');
        }
        serializer.output_mut().extend_from_slice(b"49=");
        serializer.serialize_string(&self.sender_comp_id);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"56=");
        serializer.serialize_string(&self.target_comp_id);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"34=");
        serializer.serialize_seq_num(&self.msg_seq_num);
        serializer.output_mut().push(b'\x01');
        if let Some(sender_sub_id) = &self.sender_sub_id {
            serializer.output_mut().extend_from_slice(b"50=");
            serializer.serialize_string(sender_sub_id);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(target_sub_id) = &self.target_sub_id {
            serializer.output_mut().extend_from_slice(b"57=");
            serializer.serialize_string(target_sub_id);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(poss_dup_flag) = &self.poss_dup_flag {
            serializer.output_mut().extend_from_slice(b"43=");
            serializer.serialize_boolean(poss_dup_flag);
            serializer.output_mut().push(b'\x01');
        }
        serializer.output_mut().extend_from_slice(b"52=");
        serializer.serialize_utc_timestamp(&self.sending_time);
        serializer.output_mut().push(b'\x01');
        if let Some(orig_sending_time) = &self.orig_sending_time {
            serializer.output_mut().extend_from_slice(b"122=");
            serializer.serialize_utc_timestamp(orig_sending_time);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(
        deserializer: &mut Deserializer,
        begin_string: FixString,
        body_length: Length,
    ) -> Result<Header, DeserializeError> {
        let mut appl_ver_id: Option<ApplVerId> = None;
        let mut sender_comp_id: Option<FixString> = None;
        let mut target_comp_id: Option<FixString> = None;
        let mut msg_seq_num: Option<SeqNum> = None;
        let mut sender_sub_id: Option<FixString> = None;
        let mut target_sub_id: Option<FixString> = None;
        let mut poss_dup_flag: Option<Boolean> = None;
        let mut sending_time: Option<UtcTimestamp> = None;
        let mut orig_sending_time: Option<UtcTimestamp> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                8u16 => {
                    return Err(deserializer
                        .reject(Some(8u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                9u16 => {
                    return Err(deserializer
                        .reject(Some(9u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                35u16 => {
                    return Err(deserializer
                        .reject(Some(35u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                1128u16 => {
                    if appl_ver_id.is_some() {
                        return Err(deserializer.reject(
                            Some(1128u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    appl_ver_id = Some(deserializer.deserialize_string_enum()?);
                }
                49u16 => {
                    if sender_comp_id.is_some() {
                        return Err(deserializer
                            .reject(Some(49u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    sender_comp_id = Some(deserializer.deserialize_string()?);
                }
                56u16 => {
                    if target_comp_id.is_some() {
                        return Err(deserializer
                            .reject(Some(56u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    target_comp_id = Some(deserializer.deserialize_string()?);
                }
                34u16 => {
                    if msg_seq_num.is_some() {
                        return Err(deserializer
                            .reject(Some(34u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    let msg_seq_num_value = deserializer.deserialize_seq_num()?;
                    deserializer.set_seq_num(msg_seq_num_value);
                    msg_seq_num = Some(msg_seq_num_value);
                }
                50u16 => {
                    if sender_sub_id.is_some() {
                        return Err(deserializer
                            .reject(Some(50u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    sender_sub_id = Some(deserializer.deserialize_string()?);
                }
                57u16 => {
                    if target_sub_id.is_some() {
                        return Err(deserializer
                            .reject(Some(57u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    target_sub_id = Some(deserializer.deserialize_string()?);
                }
                43u16 => {
                    if poss_dup_flag.is_some() {
                        return Err(deserializer
                            .reject(Some(43u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    poss_dup_flag = Some(deserializer.deserialize_boolean()?);
                }
                52u16 => {
                    if sending_time.is_some() {
                        return Err(deserializer
                            .reject(Some(52u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    sending_time = Some(deserializer.deserialize_utc_timestamp()?);
                }
                122u16 => {
                    if orig_sending_time.is_some() {
                        return Err(deserializer.reject(
                            Some(122u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    orig_sending_time = Some(deserializer.deserialize_utc_timestamp()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        deserializer.put_tag(tag);
                        break;
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Header {
            begin_string,
            body_length,
            appl_ver_id,
            sender_comp_id: sender_comp_id.ok_or_else(|| {
                deserializer.reject(Some(49u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            target_comp_id: target_comp_id.ok_or_else(|| {
                deserializer.reject(Some(56u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            msg_seq_num: msg_seq_num.ok_or_else(|| {
                deserializer.reject(Some(34u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            sender_sub_id,
            target_sub_id,
            poss_dup_flag,
            sending_time: sending_time.ok_or_else(|| {
                deserializer.reject(Some(52u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            orig_sending_time,
        })
    }
}
impl<'a> From<&'a Header> for HeaderBase<'a> {
    fn from(header: &'a Header) -> Self {
        HeaderBase {
            begin_string: Cow::Borrowed(&header.begin_string),
            sender_comp_id: Cow::Borrowed(&header.sender_comp_id),
            target_comp_id: Cow::Borrowed(&header.target_comp_id),
            msg_seq_num: header.msg_seq_num,
            sending_time: header.sending_time,
            poss_dup_flag: header.poss_dup_flag,
            orig_sending_time: header.orig_sending_time,
            appl_ver_id: header
                .appl_ver_id
                .as_ref()
                .map(|v| Cow::Borrowed(v.as_fix_str())),
        }
    }
}
impl From<HeaderBase<'_>> for Header {
    fn from(base: HeaderBase<'_>) -> Header {
        Header {
            begin_string: base.begin_string.into_owned(),
            sender_comp_id: base.sender_comp_id.into_owned(),
            target_comp_id: base.target_comp_id.into_owned(),
            msg_seq_num: base.msg_seq_num,
            sending_time: base.sending_time,
            poss_dup_flag: base.poss_dup_flag,
            orig_sending_time: base.orig_sending_time,
            appl_ver_id: base.appl_ver_id.map(|v| {
                ApplVerId::from_fix_str(&v)
                    .expect("HeaderBase appl_ver_id must be a valid ApplVerId")
            }),
            ..Default::default()
        }
    }
}
impl HeaderAccess for Header {
    fn begin_string(&self) -> &FixStr {
        &self.begin_string
    }

    fn sender_comp_id(&self) -> &FixStr {
        &self.sender_comp_id
    }

    fn target_comp_id(&self) -> &FixStr {
        &self.target_comp_id
    }

    fn msg_seq_num(&self) -> SeqNum {
        self.msg_seq_num
    }

    fn sending_time(&self) -> UtcTimestamp {
        self.sending_time
    }

    fn poss_dup_flag(&self) -> Option<Boolean> {
        self.poss_dup_flag
    }

    fn orig_sending_time(&self) -> Option<UtcTimestamp> {
        self.orig_sending_time
    }

    fn appl_ver_id(&self) -> Option<&FixStr> {
        self.appl_ver_id.as_ref().map(|v| v.as_fix_str())
    }

    fn set_begin_string(&mut self, value: FixString) {
        self.begin_string = value;
    }

    fn set_sender_comp_id(&mut self, value: FixString) {
        self.sender_comp_id = value;
    }

    fn set_target_comp_id(&mut self, value: FixString) {
        self.target_comp_id = value;
    }

    fn set_msg_seq_num(&mut self, value: SeqNum) {
        self.msg_seq_num = value;
    }

    fn set_sending_time(&mut self, value: UtcTimestamp) {
        self.sending_time = value;
    }

    fn set_poss_dup_flag(&mut self, value: Option<Boolean>) {
        self.poss_dup_flag = value;
    }

    fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
        self.orig_sending_time = value;
    }

    fn set_appl_ver_id(&mut self, value: Option<FixString>) {
        self.appl_ver_id = value.map(|v| {
            ApplVerId::from_fix_str(&v)
                .expect("HeaderAccess::set_appl_ver_id: invalid ApplVerId value")
        });
    }
}
impl HeaderAccess for Message {
    fn begin_string(&self) -> &FixStr {
        &self.header.begin_string
    }

    fn sender_comp_id(&self) -> &FixStr {
        &self.header.sender_comp_id
    }

    fn target_comp_id(&self) -> &FixStr {
        &self.header.target_comp_id
    }

    fn msg_seq_num(&self) -> SeqNum {
        self.header.msg_seq_num
    }

    fn sending_time(&self) -> UtcTimestamp {
        self.header.sending_time
    }

    fn poss_dup_flag(&self) -> Option<Boolean> {
        self.header.poss_dup_flag
    }

    fn orig_sending_time(&self) -> Option<UtcTimestamp> {
        self.header.orig_sending_time
    }

    fn appl_ver_id(&self) -> Option<&FixStr> {
        self.header.appl_ver_id.as_ref().map(|v| v.as_fix_str())
    }

    fn set_begin_string(&mut self, value: FixString) {
        self.header.begin_string = value;
    }

    fn set_sender_comp_id(&mut self, value: FixString) {
        self.header.sender_comp_id = value;
    }

    fn set_target_comp_id(&mut self, value: FixString) {
        self.header.target_comp_id = value;
    }

    fn set_msg_seq_num(&mut self, value: SeqNum) {
        self.header.msg_seq_num = value;
    }

    fn set_sending_time(&mut self, value: UtcTimestamp) {
        self.header.sending_time = value;
    }

    fn set_poss_dup_flag(&mut self, value: Option<Boolean>) {
        self.header.poss_dup_flag = value;
    }

    fn set_orig_sending_time(&mut self, value: Option<UtcTimestamp>) {
        self.header.orig_sending_time = value;
    }

    fn set_appl_ver_id(&mut self, value: Option<FixString>) {
        self.header.appl_ver_id = value.map(|v| {
            ApplVerId::from_fix_str(&v)
                .expect("HeaderAccess::set_appl_ver_id: invalid ApplVerId value")
        });
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Trailer {
    ///Tag 89.
    pub signature: Option<Data>,
    ///Tag 10.
    pub check_sum: FixString,
}
#[allow(dead_code)]
impl Trailer {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(signature) = &self.signature {
            serializer.output_mut().extend_from_slice(b"93=");
            serializer.serialize_length(&(signature.len() as u16));
            serializer.output_mut().push(b'\x01');
            serializer.output_mut().extend_from_slice(b"89=");
            serializer.serialize_data(signature);
            serializer.output_mut().push(b'\x01');
        }
        serializer.serialize_checksum();
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Trailer, DeserializeError> {
        let mut signature_length: Option<Length> = None;
        let mut signature: Option<Data> = None;
        let check_sum = deserializer.check_sum();
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                93u16 => {
                    if signature_length.is_some() {
                        return Err(deserializer
                            .reject(Some(93u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    let len = deserializer.deserialize_length()?;
                    signature_length = Some(len);
                    if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                        deserializer
                            .reject(Some(89u16), SessionRejectReasonBase::RequiredTagMissing)
                    })? != 89u16
                    {
                        return Err(deserializer.reject(
                            Some(93u16),
                            SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                        ));
                    }
                    if signature.is_some() {
                        return Err(deserializer
                            .reject(Some(93u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    signature = Some(deserializer.deserialize_data(len as usize)?);
                }
                89u16 => {
                    return Err(deserializer.reject(
                        Some(tag),
                        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                    ));
                }
                10u16 => {
                    return Err(deserializer
                        .reject(Some(10u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Trailer {
            signature,
            check_sum,
        })
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Heartbeat {
    ///Tag 112.
    pub test_req_id: Option<FixString>,
}
#[allow(dead_code)]
impl Heartbeat {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(test_req_id) = &self.test_req_id {
            serializer.output_mut().extend_from_slice(b"112=");
            serializer.serialize_string(test_req_id);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut test_req_id: Option<FixString> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                112u16 => {
                    if test_req_id.is_some() {
                        return Err(deserializer.reject(
                            Some(112u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    test_req_id = Some(deserializer.deserialize_string()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::Heartbeat(Heartbeat { test_req_id })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::Heartbeat
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct TestRequest {
    ///Tag 112.
    pub test_req_id: FixString,
}
#[allow(dead_code)]
impl TestRequest {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"112=");
        serializer.serialize_string(&self.test_req_id);
        serializer.output_mut().push(b'\x01');
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut test_req_id: Option<FixString> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                112u16 => {
                    if test_req_id.is_some() {
                        return Err(deserializer.reject(
                            Some(112u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    test_req_id = Some(deserializer.deserialize_string()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::TestRequest(TestRequest {
            test_req_id: test_req_id.ok_or_else(|| {
                deserializer.reject(Some(112u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::TestRequest
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ResendRequest {
    ///Tag 7.
    pub begin_seq_no: SeqNum,
    ///Tag 16.
    pub end_seq_no: SeqNum,
}
#[allow(dead_code)]
impl ResendRequest {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"7=");
        serializer.serialize_seq_num(&self.begin_seq_no);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"16=");
        serializer.serialize_seq_num(&self.end_seq_no);
        serializer.output_mut().push(b'\x01');
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut begin_seq_no: Option<SeqNum> = None;
        let mut end_seq_no: Option<SeqNum> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                7u16 => {
                    if begin_seq_no.is_some() {
                        return Err(deserializer
                            .reject(Some(7u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    begin_seq_no = Some(deserializer.deserialize_seq_num()?);
                }
                16u16 => {
                    if end_seq_no.is_some() {
                        return Err(deserializer
                            .reject(Some(16u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    end_seq_no = Some(deserializer.deserialize_seq_num()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::ResendRequest(ResendRequest {
            begin_seq_no: begin_seq_no.ok_or_else(|| {
                deserializer.reject(Some(7u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            end_seq_no: end_seq_no.ok_or_else(|| {
                deserializer.reject(Some(16u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::ResendRequest
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Reject {
    ///Tag 45.
    pub ref_seq_num: SeqNum,
    ///Tag 371.
    pub ref_tag_id: Option<Int>,
    ///Tag 372.
    pub ref_msg_type: Option<FixString>,
    ///Tag 373.
    pub session_reject_reason: Option<SessionRejectReason>,
    ///Tag 58.
    pub text: Option<FixString>,
}
#[allow(dead_code)]
impl Reject {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"45=");
        serializer.serialize_seq_num(&self.ref_seq_num);
        serializer.output_mut().push(b'\x01');
        if let Some(ref_tag_id) = &self.ref_tag_id {
            serializer.output_mut().extend_from_slice(b"371=");
            serializer.serialize_int(ref_tag_id);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(ref_msg_type) = &self.ref_msg_type {
            serializer.output_mut().extend_from_slice(b"372=");
            serializer.serialize_string(ref_msg_type);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(session_reject_reason) = &self.session_reject_reason {
            serializer.output_mut().extend_from_slice(b"373=");
            serializer.serialize_enum(session_reject_reason);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(text) = &self.text {
            serializer.output_mut().extend_from_slice(b"58=");
            serializer.serialize_string(text);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut ref_seq_num: Option<SeqNum> = None;
        let mut ref_tag_id: Option<Int> = None;
        let mut ref_msg_type: Option<FixString> = None;
        let mut session_reject_reason: Option<SessionRejectReason> = None;
        let mut text: Option<FixString> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                45u16 => {
                    if ref_seq_num.is_some() {
                        return Err(deserializer
                            .reject(Some(45u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    ref_seq_num = Some(deserializer.deserialize_seq_num()?);
                }
                371u16 => {
                    if ref_tag_id.is_some() {
                        return Err(deserializer.reject(
                            Some(371u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    ref_tag_id = Some(deserializer.deserialize_int()?);
                }
                372u16 => {
                    if ref_msg_type.is_some() {
                        return Err(deserializer.reject(
                            Some(372u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    ref_msg_type = Some(deserializer.deserialize_string()?);
                }
                373u16 => {
                    if session_reject_reason.is_some() {
                        return Err(deserializer.reject(
                            Some(373u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    session_reject_reason = Some(deserializer.deserialize_int_enum()?);
                }
                58u16 => {
                    if text.is_some() {
                        return Err(deserializer
                            .reject(Some(58u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    text = Some(deserializer.deserialize_string()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::Reject(Reject {
            ref_seq_num: ref_seq_num.ok_or_else(|| {
                deserializer.reject(Some(45u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            ref_tag_id,
            ref_msg_type,
            session_reject_reason,
            text,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::Reject
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct SequenceReset {
    ///Tag 123.
    pub gap_fill_flag: Option<Boolean>,
    ///Tag 36.
    pub new_seq_no: SeqNum,
}
#[allow(dead_code)]
impl SequenceReset {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(gap_fill_flag) = &self.gap_fill_flag {
            serializer.output_mut().extend_from_slice(b"123=");
            serializer.serialize_boolean(gap_fill_flag);
            serializer.output_mut().push(b'\x01');
        }
        serializer.output_mut().extend_from_slice(b"36=");
        serializer.serialize_seq_num(&self.new_seq_no);
        serializer.output_mut().push(b'\x01');
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut gap_fill_flag: Option<Boolean> = None;
        let mut new_seq_no: Option<SeqNum> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                123u16 => {
                    if gap_fill_flag.is_some() {
                        return Err(deserializer.reject(
                            Some(123u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    gap_fill_flag = Some(deserializer.deserialize_boolean()?);
                }
                36u16 => {
                    if new_seq_no.is_some() {
                        return Err(deserializer
                            .reject(Some(36u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    new_seq_no = Some(deserializer.deserialize_seq_num()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::SequenceReset(SequenceReset {
            gap_fill_flag,
            new_seq_no: new_seq_no.ok_or_else(|| {
                deserializer.reject(Some(36u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::SequenceReset
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Logout {
    ///Tag 1409.
    pub session_status: Option<SessionStatus>,
    ///Tag 789.
    pub next_expected_msg_seq_num: Option<SeqNum>,
    ///Tag 58.
    pub text: Option<FixString>,
}
#[allow(dead_code)]
impl Logout {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        if let Some(session_status) = &self.session_status {
            serializer.output_mut().extend_from_slice(b"1409=");
            serializer.serialize_enum(session_status);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(next_expected_msg_seq_num) = &self.next_expected_msg_seq_num {
            serializer.output_mut().extend_from_slice(b"789=");
            serializer.serialize_seq_num(next_expected_msg_seq_num);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(text) = &self.text {
            serializer.output_mut().extend_from_slice(b"58=");
            serializer.serialize_string(text);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut session_status: Option<SessionStatus> = None;
        let mut next_expected_msg_seq_num: Option<SeqNum> = None;
        let mut text: Option<FixString> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                1409u16 => {
                    if session_status.is_some() {
                        return Err(deserializer.reject(
                            Some(1409u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    session_status = Some(deserializer.deserialize_int_enum()?);
                }
                789u16 => {
                    if next_expected_msg_seq_num.is_some() {
                        return Err(deserializer.reject(
                            Some(789u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    next_expected_msg_seq_num = Some(deserializer.deserialize_seq_num()?);
                }
                58u16 => {
                    if text.is_some() {
                        return Err(deserializer
                            .reject(Some(58u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    text = Some(deserializer.deserialize_string()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::Logout(Logout {
            session_status,
            next_expected_msg_seq_num,
            text,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::Logout
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Logon {
    ///Tag 98.
    pub encrypt_method: EncryptMethod,
    ///Tag 108.
    pub heart_bt_int: Int,
    ///Tag 96.
    pub raw_data: Option<Data>,
    ///Tag 141.
    pub reset_seq_num_flag: Option<Boolean>,
    ///Tag 789.
    pub next_expected_msg_seq_num: Option<SeqNum>,
    ///Tag 384.
    pub msg_type_grp: Option<Vec<MsgTypeGrp>>,
    ///Tag 1409.
    pub session_status: Option<SessionStatus>,
    ///Tag 1137.
    pub default_appl_ver_id: DefaultApplVerId,
    ///Tag 58.
    pub text: Option<FixString>,
}
#[allow(dead_code)]
impl Logon {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"98=");
        serializer.serialize_enum(&self.encrypt_method);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"108=");
        serializer.serialize_int(&self.heart_bt_int);
        serializer.output_mut().push(b'\x01');
        if let Some(raw_data) = &self.raw_data {
            serializer.output_mut().extend_from_slice(b"95=");
            serializer.serialize_length(&(raw_data.len() as u16));
            serializer.output_mut().push(b'\x01');
            serializer.output_mut().extend_from_slice(b"96=");
            serializer.serialize_data(raw_data);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(reset_seq_num_flag) = &self.reset_seq_num_flag {
            serializer.output_mut().extend_from_slice(b"141=");
            serializer.serialize_boolean(reset_seq_num_flag);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(next_expected_msg_seq_num) = &self.next_expected_msg_seq_num {
            serializer.output_mut().extend_from_slice(b"789=");
            serializer.serialize_seq_num(next_expected_msg_seq_num);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(msg_type_grp) = &self.msg_type_grp {
            serializer.output_mut().extend_from_slice(b"384=");
            serializer.serialize_num_in_group(&(msg_type_grp.len() as NumInGroup));
            serializer.output_mut().push(b'\x01');
            for entry in msg_type_grp {
                entry.serialize(serializer);
            }
        }
        if let Some(session_status) = &self.session_status {
            serializer.output_mut().extend_from_slice(b"1409=");
            serializer.serialize_enum(session_status);
            serializer.output_mut().push(b'\x01');
        }
        serializer.output_mut().extend_from_slice(b"1137=");
        serializer.serialize_enum(&self.default_appl_ver_id);
        serializer.output_mut().push(b'\x01');
        if let Some(text) = &self.text {
            serializer.output_mut().extend_from_slice(b"58=");
            serializer.serialize_string(text);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut encrypt_method: Option<EncryptMethod> = None;
        let mut heart_bt_int: Option<Int> = None;
        let mut raw_data_length: Option<Length> = None;
        let mut raw_data: Option<Data> = None;
        let mut reset_seq_num_flag: Option<Boolean> = None;
        let mut next_expected_msg_seq_num: Option<SeqNum> = None;
        let mut no_msg_types: Option<NumInGroup> = None;
        let mut msg_type_grp: Option<Vec<MsgTypeGrp>> = None;
        let mut session_status: Option<SessionStatus> = None;
        let mut default_appl_ver_id: Option<DefaultApplVerId> = None;
        let mut text: Option<FixString> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                98u16 => {
                    if encrypt_method.is_some() {
                        return Err(deserializer
                            .reject(Some(98u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    encrypt_method = Some(deserializer.deserialize_int_enum()?);
                }
                108u16 => {
                    if heart_bt_int.is_some() {
                        return Err(deserializer.reject(
                            Some(108u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    heart_bt_int = Some(deserializer.deserialize_int()?);
                }
                95u16 => {
                    if raw_data_length.is_some() {
                        return Err(deserializer
                            .reject(Some(95u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    let len = deserializer.deserialize_length()?;
                    raw_data_length = Some(len);
                    if deserializer.deserialize_tag_num()?.ok_or_else(|| {
                        deserializer
                            .reject(Some(96u16), SessionRejectReasonBase::RequiredTagMissing)
                    })? != 96u16
                    {
                        return Err(deserializer.reject(
                            Some(95u16),
                            SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                        ));
                    }
                    if raw_data.is_some() {
                        return Err(deserializer
                            .reject(Some(95u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    raw_data = Some(deserializer.deserialize_data(len as usize)?);
                }
                96u16 => {
                    return Err(deserializer.reject(
                        Some(tag),
                        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                    ));
                }
                141u16 => {
                    if reset_seq_num_flag.is_some() {
                        return Err(deserializer.reject(
                            Some(141u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    reset_seq_num_flag = Some(deserializer.deserialize_boolean()?);
                }
                789u16 => {
                    if next_expected_msg_seq_num.is_some() {
                        return Err(deserializer.reject(
                            Some(789u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    next_expected_msg_seq_num = Some(deserializer.deserialize_seq_num()?);
                }
                384u16 => {
                    if no_msg_types.is_some() {
                        return Err(deserializer.reject(
                            Some(384u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    let len = deserializer.deserialize_num_in_group()?;
                    no_msg_types = Some(len);
                    if msg_type_grp.is_some() {
                        return Err(deserializer.reject(
                            Some(384u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    let num_in_group_tag = 384u16;
                    let expected_tags = &[372u16, 385u16, 1130u16];
                    let mut msg_type_grp_local = Vec::with_capacity(len as usize);
                    let last_run = false;
                    for _ in 0..len - 1 {
                        msg_type_grp_local.push(MsgTypeGrp::deserialize(
                            deserializer,
                            num_in_group_tag,
                            expected_tags,
                            last_run,
                        )?);
                    }
                    let last_run = true;
                    msg_type_grp_local.push(MsgTypeGrp::deserialize(
                        deserializer,
                        num_in_group_tag,
                        expected_tags,
                        last_run,
                    )?);
                    msg_type_grp = Some(msg_type_grp_local);
                }
                1409u16 => {
                    if session_status.is_some() {
                        return Err(deserializer.reject(
                            Some(1409u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    session_status = Some(deserializer.deserialize_int_enum()?);
                }
                1137u16 => {
                    if default_appl_ver_id.is_some() {
                        return Err(deserializer.reject(
                            Some(1137u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    default_appl_ver_id = Some(deserializer.deserialize_string_enum()?);
                }
                58u16 => {
                    if text.is_some() {
                        return Err(deserializer
                            .reject(Some(58u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    text = Some(deserializer.deserialize_string()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::Logon(Logon {
            encrypt_method: encrypt_method.ok_or_else(|| {
                deserializer.reject(Some(98u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            heart_bt_int: heart_bt_int.ok_or_else(|| {
                deserializer.reject(Some(108u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            raw_data,
            reset_seq_num_flag,
            next_expected_msg_seq_num,
            msg_type_grp,
            session_status,
            default_appl_ver_id: default_appl_ver_id.ok_or_else(|| {
                deserializer.reject(Some(1137u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            text,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::Logon
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::Admin
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct NewOrderSingle {
    ///Tag 11.
    pub cl_ord_id: FixString,
    ///Tag 55.
    pub symbol: FixString,
    ///Tag 54.
    pub side: Side,
    ///Tag 60.
    pub transact_time: UtcTimestamp,
    ///Tag 38.
    pub order_qty: Qty,
    ///Tag 40.
    pub ord_type: OrdType,
    ///Tag 44.
    pub price: Option<Price>,
}
#[allow(dead_code)]
impl NewOrderSingle {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"11=");
        serializer.serialize_string(&self.cl_ord_id);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"55=");
        serializer.serialize_string(&self.symbol);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"54=");
        serializer.serialize_enum(&self.side);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"60=");
        serializer.serialize_utc_timestamp(&self.transact_time);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"38=");
        serializer.serialize_qty(&self.order_qty);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"40=");
        serializer.serialize_enum(&self.ord_type);
        serializer.output_mut().push(b'\x01');
        if let Some(price) = &self.price {
            serializer.output_mut().extend_from_slice(b"44=");
            serializer.serialize_price(price);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut cl_ord_id: Option<FixString> = None;
        let mut symbol: Option<FixString> = None;
        let mut side: Option<Side> = None;
        let mut transact_time: Option<UtcTimestamp> = None;
        let mut order_qty: Option<Qty> = None;
        let mut ord_type: Option<OrdType> = None;
        let mut price: Option<Price> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                11u16 => {
                    if cl_ord_id.is_some() {
                        return Err(deserializer
                            .reject(Some(11u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    cl_ord_id = Some(deserializer.deserialize_string()?);
                }
                55u16 => {
                    if symbol.is_some() {
                        return Err(deserializer
                            .reject(Some(55u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    symbol = Some(deserializer.deserialize_string()?);
                }
                54u16 => {
                    if side.is_some() {
                        return Err(deserializer
                            .reject(Some(54u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    side = Some(deserializer.deserialize_char_enum()?);
                }
                60u16 => {
                    if transact_time.is_some() {
                        return Err(deserializer
                            .reject(Some(60u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    transact_time = Some(deserializer.deserialize_utc_timestamp()?);
                }
                38u16 => {
                    if order_qty.is_some() {
                        return Err(deserializer
                            .reject(Some(38u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    order_qty = Some(deserializer.deserialize_qty()?);
                }
                40u16 => {
                    if ord_type.is_some() {
                        return Err(deserializer
                            .reject(Some(40u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    ord_type = Some(deserializer.deserialize_char_enum()?);
                }
                44u16 => {
                    if price.is_some() {
                        return Err(deserializer
                            .reject(Some(44u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    price = Some(deserializer.deserialize_price()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::NewOrderSingle(NewOrderSingle {
            cl_ord_id: cl_ord_id.ok_or_else(|| {
                deserializer.reject(Some(11u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            symbol: symbol.ok_or_else(|| {
                deserializer.reject(Some(55u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            side: side.ok_or_else(|| {
                deserializer.reject(Some(54u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            transact_time: transact_time.ok_or_else(|| {
                deserializer.reject(Some(60u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            order_qty: order_qty.ok_or_else(|| {
                deserializer.reject(Some(38u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            ord_type: ord_type.ok_or_else(|| {
                deserializer.reject(Some(40u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            price,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::NewOrderSingle
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::App
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ExecutionReport {
    ///Tag 37.
    pub order_id: FixString,
    ///Tag 17.
    pub exec_id: FixString,
    ///Tag 150.
    pub exec_type: ExecType,
    ///Tag 39.
    pub ord_status: OrdStatus,
    ///Tag 55.
    pub symbol: FixString,
    ///Tag 54.
    pub side: Side,
    ///Tag 151.
    pub leaves_qty: Qty,
    ///Tag 14.
    pub cum_qty: Qty,
    ///Tag 11.
    pub cl_ord_id: Option<FixString>,
    ///Tag 38.
    pub order_qty: Option<Qty>,
    ///Tag 44.
    pub price: Option<Price>,
    ///Tag 60.
    pub transact_time: Option<UtcTimestamp>,
}
#[allow(dead_code)]
impl ExecutionReport {
    pub(crate) fn serialize(&self, serializer: &mut Serializer) {
        serializer.output_mut().extend_from_slice(b"37=");
        serializer.serialize_string(&self.order_id);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"17=");
        serializer.serialize_string(&self.exec_id);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"150=");
        serializer.serialize_enum(&self.exec_type);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"39=");
        serializer.serialize_enum(&self.ord_status);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"55=");
        serializer.serialize_string(&self.symbol);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"54=");
        serializer.serialize_enum(&self.side);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"151=");
        serializer.serialize_qty(&self.leaves_qty);
        serializer.output_mut().push(b'\x01');
        serializer.output_mut().extend_from_slice(b"14=");
        serializer.serialize_qty(&self.cum_qty);
        serializer.output_mut().push(b'\x01');
        if let Some(cl_ord_id) = &self.cl_ord_id {
            serializer.output_mut().extend_from_slice(b"11=");
            serializer.serialize_string(cl_ord_id);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(order_qty) = &self.order_qty {
            serializer.output_mut().extend_from_slice(b"38=");
            serializer.serialize_qty(order_qty);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(price) = &self.price {
            serializer.output_mut().extend_from_slice(b"44=");
            serializer.serialize_price(price);
            serializer.output_mut().push(b'\x01');
        }
        if let Some(transact_time) = &self.transact_time {
            serializer.output_mut().extend_from_slice(b"60=");
            serializer.serialize_utc_timestamp(transact_time);
            serializer.output_mut().push(b'\x01');
        }
    }

    fn deserialize(deserializer: &mut Deserializer) -> Result<Box<Body>, DeserializeError> {
        let mut order_id: Option<FixString> = None;
        let mut exec_id: Option<FixString> = None;
        let mut exec_type: Option<ExecType> = None;
        let mut ord_status: Option<OrdStatus> = None;
        let mut symbol: Option<FixString> = None;
        let mut side: Option<Side> = None;
        let mut leaves_qty: Option<Qty> = None;
        let mut cum_qty: Option<Qty> = None;
        let mut cl_ord_id: Option<FixString> = None;
        let mut order_qty: Option<Qty> = None;
        let mut price: Option<Price> = None;
        let mut transact_time: Option<UtcTimestamp> = None;
        while let Some(tag) = deserializer.deserialize_tag_num()? {
            match tag {
                37u16 => {
                    if order_id.is_some() {
                        return Err(deserializer
                            .reject(Some(37u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    order_id = Some(deserializer.deserialize_string()?);
                }
                17u16 => {
                    if exec_id.is_some() {
                        return Err(deserializer
                            .reject(Some(17u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    exec_id = Some(deserializer.deserialize_string()?);
                }
                150u16 => {
                    if exec_type.is_some() {
                        return Err(deserializer.reject(
                            Some(150u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    exec_type = Some(deserializer.deserialize_char_enum()?);
                }
                39u16 => {
                    if ord_status.is_some() {
                        return Err(deserializer
                            .reject(Some(39u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    ord_status = Some(deserializer.deserialize_char_enum()?);
                }
                55u16 => {
                    if symbol.is_some() {
                        return Err(deserializer
                            .reject(Some(55u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    symbol = Some(deserializer.deserialize_string()?);
                }
                54u16 => {
                    if side.is_some() {
                        return Err(deserializer
                            .reject(Some(54u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    side = Some(deserializer.deserialize_char_enum()?);
                }
                151u16 => {
                    if leaves_qty.is_some() {
                        return Err(deserializer.reject(
                            Some(151u16),
                            SessionRejectReasonBase::TagAppearsMoreThanOnce,
                        ));
                    }
                    leaves_qty = Some(deserializer.deserialize_qty()?);
                }
                14u16 => {
                    if cum_qty.is_some() {
                        return Err(deserializer
                            .reject(Some(14u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    cum_qty = Some(deserializer.deserialize_qty()?);
                }
                11u16 => {
                    if cl_ord_id.is_some() {
                        return Err(deserializer
                            .reject(Some(11u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    cl_ord_id = Some(deserializer.deserialize_string()?);
                }
                38u16 => {
                    if order_qty.is_some() {
                        return Err(deserializer
                            .reject(Some(38u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    order_qty = Some(deserializer.deserialize_qty()?);
                }
                44u16 => {
                    if price.is_some() {
                        return Err(deserializer
                            .reject(Some(44u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    price = Some(deserializer.deserialize_price()?);
                }
                60u16 => {
                    if transact_time.is_some() {
                        return Err(deserializer
                            .reject(Some(60u16), SessionRejectReasonBase::TagAppearsMoreThanOnce));
                    }
                    transact_time = Some(deserializer.deserialize_utc_timestamp()?);
                }
                tag => {
                    if FieldTag::from_tag_num(tag).is_some() {
                        return Err(deserializer.reject(
                            Some(tag),
                            SessionRejectReasonBase::TagNotDefinedForThisMessageType,
                        ));
                    } else {
                        return Err(
                            deserializer.reject(Some(tag), SessionRejectReasonBase::UndefinedTag)
                        );
                    }
                }
            }
        }
        Ok(Box::new(Body::ExecutionReport(ExecutionReport {
            order_id: order_id.ok_or_else(|| {
                deserializer.reject(Some(37u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            exec_id: exec_id.ok_or_else(|| {
                deserializer.reject(Some(17u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            exec_type: exec_type.ok_or_else(|| {
                deserializer.reject(Some(150u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            ord_status: ord_status.ok_or_else(|| {
                deserializer.reject(Some(39u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            symbol: symbol.ok_or_else(|| {
                deserializer.reject(Some(55u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            side: side.ok_or_else(|| {
                deserializer.reject(Some(54u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            leaves_qty: leaves_qty.ok_or_else(|| {
                deserializer.reject(Some(151u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            cum_qty: cum_qty.ok_or_else(|| {
                deserializer.reject(Some(14u16), SessionRejectReasonBase::RequiredTagMissing)
            })?,
            cl_ord_id,
            order_qty,
            price,
            transact_time,
        })))
    }

    pub const fn msg_type(&self) -> MsgType {
        MsgType::ExecutionReport
    }

    pub const fn msg_cat(&self) -> MsgCat {
        MsgCat::App
    }
}
impl<'a> From<&'a Heartbeat> for HeartbeatBase<'a> {
    fn from(msg: &'a Heartbeat) -> Self {
        HeartbeatBase {
            test_req_id: msg.test_req_id.as_deref().map(Cow::Borrowed),
        }
    }
}
impl From<HeartbeatBase<'_>> for Heartbeat {
    fn from(base: HeartbeatBase<'_>) -> Heartbeat {
        Heartbeat {
            test_req_id: base.test_req_id.map(Cow::into_owned),
        }
    }
}
impl<'a> From<&'a TestRequest> for TestRequestBase<'a> {
    fn from(msg: &'a TestRequest) -> Self {
        TestRequestBase {
            test_req_id: Cow::Borrowed(&msg.test_req_id),
        }
    }
}
impl From<TestRequestBase<'_>> for TestRequest {
    fn from(base: TestRequestBase<'_>) -> TestRequest {
        TestRequest {
            test_req_id: base.test_req_id.into_owned(),
        }
    }
}
impl From<&ResendRequest> for ResendRequestBase {
    fn from(msg: &ResendRequest) -> Self {
        ResendRequestBase {
            begin_seq_no: msg.begin_seq_no,
            end_seq_no: msg.end_seq_no,
        }
    }
}
impl From<ResendRequestBase> for ResendRequest {
    fn from(base: ResendRequestBase) -> ResendRequest {
        ResendRequest {
            begin_seq_no: base.begin_seq_no,
            end_seq_no: base.end_seq_no,
        }
    }
}
impl From<&SequenceReset> for SequenceResetBase {
    fn from(msg: &SequenceReset) -> Self {
        SequenceResetBase {
            gap_fill_flag: msg.gap_fill_flag,
            new_seq_no: msg.new_seq_no,
        }
    }
}
impl From<SequenceResetBase> for SequenceReset {
    fn from(base: SequenceResetBase) -> SequenceReset {
        SequenceReset {
            gap_fill_flag: base.gap_fill_flag,
            new_seq_no: base.new_seq_no,
        }
    }
}
impl<'a> From<&'a Logout> for LogoutBase<'a> {
    fn from(msg: &'a Logout) -> Self {
        LogoutBase {
            session_status: msg
                .session_status
                .as_ref()
                .map(|v| SessionStatusField::from(*v)),
            text: msg.text.as_deref().map(Cow::Borrowed),
        }
    }
}
impl From<LogoutBase<'_>> for Logout {
    fn from(base: LogoutBase<'_>) -> Logout {
        Logout {
            session_status: base.session_status.map(SessionStatus::from),
            text: base.text.map(Cow::into_owned),
            ..Default::default()
        }
    }
}
impl<'a> From<&'a Reject> for RejectBase<'a> {
    fn from(msg: &'a Reject) -> Self {
        RejectBase {
            ref_seq_num: msg.ref_seq_num,
            ref_tag_id: msg.ref_tag_id,
            ref_msg_type: msg.ref_msg_type.as_deref().map(Cow::Borrowed),
            session_reject_reason: msg
                .session_reject_reason
                .as_ref()
                .map(|v| SessionRejectReasonField::from(*v)),
            text: msg.text.as_deref().map(Cow::Borrowed),
        }
    }
}
impl From<RejectBase<'_>> for Reject {
    fn from(base: RejectBase<'_>) -> Reject {
        Reject {
            ref_seq_num: base.ref_seq_num,
            ref_tag_id: base.ref_tag_id,
            ref_msg_type: base.ref_msg_type.map(Cow::into_owned),
            session_reject_reason: base.session_reject_reason.map(SessionRejectReason::from),
            text: base.text.map(Cow::into_owned),
        }
    }
}
impl<'a> From<&'a Logon> for LogonBase<'a> {
    fn from(msg: &'a Logon) -> Self {
        LogonBase {
            encrypt_method: Default::default(),
            encrypt_method_raw: msg.encrypt_method.as_int(),
            heart_bt_int: msg.heart_bt_int,
            reset_seq_num_flag: msg.reset_seq_num_flag,
            next_expected_msg_seq_num: msg.next_expected_msg_seq_num,
            default_appl_ver_id: Some(Cow::Borrowed(msg.default_appl_ver_id.as_fix_str())),
            session_status: msg
                .session_status
                .as_ref()
                .map(|v| SessionStatusField::from(*v)),
        }
    }
}
impl From<LogonBase<'_>> for Logon {
    fn from(base: LogonBase<'_>) -> Logon {
        Logon {
            encrypt_method: EncryptMethod::from(base.encrypt_method),
            heart_bt_int: base.heart_bt_int,
            reset_seq_num_flag: base.reset_seq_num_flag,
            next_expected_msg_seq_num: base.next_expected_msg_seq_num,
            default_appl_ver_id: base
                .default_appl_ver_id
                .map(|v| {
                    DefaultApplVerId::from_fix_str(&v)
                        .expect("LogonBase default_appl_ver_id must be a valid DefaultApplVerId")
                })
                .unwrap_or_default(),
            session_status: base.session_status.map(SessionStatus::from),
            ..Default::default()
        }
    }
}
impl From<AdminBase<'_>> for Body {
    fn from(admin: AdminBase<'_>) -> Self {
        match admin {
            AdminBase::Logon(base) => Body::Logon(base.into()),
            AdminBase::Logout(base) => Body::Logout(base.into()),
            AdminBase::Heartbeat(base) => Body::Heartbeat(base.into()),
            AdminBase::TestRequest(base) => Body::TestRequest(base.into()),
            AdminBase::ResendRequest(base) => Body::ResendRequest(base.into()),
            AdminBase::SequenceReset(base) => Body::SequenceReset(base.into()),
            AdminBase::Reject(base) => Body::Reject(base.into()),
        }
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Body {
    Heartbeat(Heartbeat),
    TestRequest(TestRequest),
    ResendRequest(ResendRequest),
    Reject(Reject),
    SequenceReset(SequenceReset),
    Logout(Logout),
    Logon(Logon),
    NewOrderSingle(NewOrderSingle),
    ExecutionReport(ExecutionReport),
}
#[allow(dead_code)]
impl Body {
    fn serialize(&self, serializer: &mut Serializer) {
        match self {
            Body::Heartbeat(msg) => msg.serialize(serializer),
            Body::TestRequest(msg) => msg.serialize(serializer),
            Body::ResendRequest(msg) => msg.serialize(serializer),
            Body::Reject(msg) => msg.serialize(serializer),
            Body::SequenceReset(msg) => msg.serialize(serializer),
            Body::Logout(msg) => msg.serialize(serializer),
            Body::Logon(msg) => msg.serialize(serializer),
            Body::NewOrderSingle(msg) => msg.serialize(serializer),
            Body::ExecutionReport(msg) => msg.serialize(serializer),
        }
    }

    fn deserialize(
        deserializer: &mut Deserializer,
        msg_type: MsgType,
    ) -> Result<Box<Body>, DeserializeError> {
        match msg_type {
            MsgType::Heartbeat => Ok(Heartbeat::deserialize(deserializer)?),
            MsgType::TestRequest => Ok(TestRequest::deserialize(deserializer)?),
            MsgType::ResendRequest => Ok(ResendRequest::deserialize(deserializer)?),
            MsgType::Reject => Ok(Reject::deserialize(deserializer)?),
            MsgType::SequenceReset => Ok(SequenceReset::deserialize(deserializer)?),
            MsgType::Logout => Ok(Logout::deserialize(deserializer)?),
            MsgType::Logon => Ok(Logon::deserialize(deserializer)?),
            MsgType::NewOrderSingle => Ok(NewOrderSingle::deserialize(deserializer)?),
            MsgType::ExecutionReport => Ok(ExecutionReport::deserialize(deserializer)?),
        }
    }

    pub const fn msg_type(&self) -> MsgType {
        match self {
            Body::Heartbeat(_) => MsgType::Heartbeat,
            Body::TestRequest(_) => MsgType::TestRequest,
            Body::ResendRequest(_) => MsgType::ResendRequest,
            Body::Reject(_) => MsgType::Reject,
            Body::SequenceReset(_) => MsgType::SequenceReset,
            Body::Logout(_) => MsgType::Logout,
            Body::Logon(_) => MsgType::Logon,
            Body::NewOrderSingle(_) => MsgType::NewOrderSingle,
            Body::ExecutionReport(_) => MsgType::ExecutionReport,
        }
    }

    pub const fn msg_cat(&self) -> MsgCat {
        match self {
            Body::Heartbeat(msg) => msg.msg_cat(),
            Body::TestRequest(msg) => msg.msg_cat(),
            Body::ResendRequest(msg) => msg.msg_cat(),
            Body::Reject(msg) => msg.msg_cat(),
            Body::SequenceReset(msg) => msg.msg_cat(),
            Body::Logout(msg) => msg.msg_cat(),
            Body::Logon(msg) => msg.msg_cat(),
            Body::NewOrderSingle(msg) => msg.msg_cat(),
            Body::ExecutionReport(msg) => msg.msg_cat(),
        }
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Body::Heartbeat(_) => "Heartbeat",
            Body::TestRequest(_) => "TestRequest",
            Body::ResendRequest(_) => "ResendRequest",
            Body::Reject(_) => "Reject",
            Body::SequenceReset(_) => "SequenceReset",
            Body::Logout(_) => "Logout",
            Body::Logon(_) => "Logon",
            Body::NewOrderSingle(_) => "NewOrderSingle",
            Body::ExecutionReport(_) => "ExecutionReport",
        }
    }

    pub fn try_as_admin_base(&self) -> Option<AdminBase<'_>> {
        match self {
            Body::Logon(msg) => Some(AdminBase::Logon(msg.into())),
            Body::Logout(msg) => Some(AdminBase::Logout(msg.into())),
            Body::Heartbeat(msg) => Some(AdminBase::Heartbeat(msg.into())),
            Body::TestRequest(msg) => Some(AdminBase::TestRequest(msg.into())),
            Body::ResendRequest(msg) => Some(AdminBase::ResendRequest(msg.into())),
            Body::SequenceReset(msg) => Some(AdminBase::SequenceReset(msg.into())),
            Body::Reject(msg) => Some(AdminBase::Reject(msg.into())),
            _ => None,
        }
    }
}
impl From<Heartbeat> for Body {
    fn from(msg: Heartbeat) -> Body {
        Body::Heartbeat(msg)
    }
}
impl From<TestRequest> for Body {
    fn from(msg: TestRequest) -> Body {
        Body::TestRequest(msg)
    }
}
impl From<ResendRequest> for Body {
    fn from(msg: ResendRequest) -> Body {
        Body::ResendRequest(msg)
    }
}
impl From<Reject> for Body {
    fn from(msg: Reject) -> Body {
        Body::Reject(msg)
    }
}
impl From<SequenceReset> for Body {
    fn from(msg: SequenceReset) -> Body {
        Body::SequenceReset(msg)
    }
}
impl From<Logout> for Body {
    fn from(msg: Logout) -> Body {
        Body::Logout(msg)
    }
}
impl From<Logon> for Body {
    fn from(msg: Logon) -> Body {
        Body::Logon(msg)
    }
}
impl From<NewOrderSingle> for Body {
    fn from(msg: NewOrderSingle) -> Body {
        Body::NewOrderSingle(msg)
    }
}
impl From<ExecutionReport> for Body {
    fn from(msg: ExecutionReport) -> Body {
        Body::ExecutionReport(msg)
    }
}
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Message {
    pub header: Header,
    pub body: Box<Body>,
    pub trailer: Trailer,
}
#[allow(dead_code)]
impl Message {
    pub fn deserialize(mut deserializer: Deserializer) -> Result<Box<Message>, DeserializeError> {
        let begin_string = deserializer.begin_string();
        if begin_string != BEGIN_STRING {
            return Err(DeserializeError::GarbledMessage(
                "begin string mismatch".into(),
            ));
        }
        let body_length = deserializer.body_length();
        let msg_type = if let Some(35) = deserializer.deserialize_tag_num().map_err(|e| {
            DeserializeError::GarbledMessage(format!("failed to parse MsgType<35>: {e}"))
        })? {
            let msg_type_range = deserializer.deserialize_msg_type()?;
            let msg_type_fixstr = deserializer.range_to_fixstr(msg_type_range);
            let Ok(msg_type) = MsgType::try_from(msg_type_fixstr) else {
                return Err(deserializer.reject(Some(35), SessionRejectReasonBase::InvalidMsgType));
            };
            msg_type
        } else {
            return Err(DeserializeError::GarbledMessage(
                "MsgType<35> not third tag".into(),
            ));
        };
        let header =
            Header::deserialize(&mut deserializer, begin_string, body_length).map_err(|err| {
                if let DeserializeError::Reject { reason, .. } = err
                    && reason == SessionRejectReasonBase::RequiredTagMissing
                    && let Ok(Some(tag)) = deserializer.deserialize_tag_num()
                {
                    deserializer.reject(
                        Some(tag),
                        SessionRejectReasonBase::TagSpecifiedOutOfRequiredOrder,
                    )
                } else {
                    err
                }
            })?;
        let body = Body::deserialize(&mut deserializer, msg_type)?;
        let trailer = Trailer::deserialize(&mut deserializer)?;
        Ok(Box::new(Message {
            header,
            body,
            trailer,
        }))
    }

    pub fn from_raw_message(raw_message: RawMessage) -> Result<Box<Message>, DeserializeError> {
        let deserializer = Deserializer::from_raw_message(raw_message);
        Message::deserialize(deserializer)
    }

    pub fn from_bytes(input: &[u8]) -> Result<Box<Message>, DeserializeError> {
        let (_, raw_msg) = raw_message(input)?;
        let deserializer = Deserializer::from_raw_message(raw_msg);
        Message::deserialize(deserializer)
    }

    pub fn dbg_fix_str(&self) -> impl fmt::Display {
        let mut output = self.serialize();
        for byte in output.iter_mut() {
            if *byte == b'\x01' {
                *byte = b'|';
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    }

    pub const fn msg_type(&self) -> MsgType {
        self.body.msg_type()
    }
}
impl SessionMessage for Message {
    fn from_raw_message(raw: RawMessage<'_>) -> Result<Self, DeserializeError> {
        let deserializer = Deserializer::from_raw_message(raw);
        Ok(*Message::deserialize(deserializer)?)
    }

    fn serialize(&self) -> Vec<u8> {
        let mut serializer = Serializer::new();
        serializer.output_mut().extend_from_slice(b"8=");
        serializer.serialize_string(&self.header.begin_string);
        serializer.output_mut().push(b'\x01');
        serializer.serialize_body_len();
        serializer.output_mut().extend_from_slice(b"35=");
        serializer.serialize_enum(&self.body.msg_type());
        serializer.output_mut().push(b'\x01');
        self.header.serialize(&mut serializer);
        self.body.serialize(&mut serializer);
        self.trailer.serialize(&mut serializer);
        serializer.take()
    }

    fn header(&self) -> HeaderBase<'_> {
        HeaderBase::from(&self.header)
    }

    fn try_as_admin(&self) -> Option<AdminBase<'_>> {
        self.body.try_as_admin_base()
    }

    fn msg_type(&self) -> MsgTypeField {
        self.body.msg_type().raw_value()
    }

    fn msg_cat(&self) -> MsgCat {
        self.body.msg_cat()
    }

    fn name(&self) -> &'static str {
        self.body.name()
    }

    fn from_admin(header: HeaderBase<'static>, admin: AdminBase<'static>) -> Self {
        Message {
            header: Header::from(header),
            body: Box::new(Body::from(admin)),
            trailer: Trailer::default(),
        }
    }
}
