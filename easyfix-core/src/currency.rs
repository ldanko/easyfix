//! ISO 4217 currency codes, as the three-character FIX `Currency` datatype.

use crate::{basic_types::FixStr, fix_str};

/// An ISO 4217 three-letter currency code.
///
/// The default is [`Unspecified`](Self::Unspecified), with empty text views
/// and zero-filled byte views. FIX serialization rejects it; Serde represents
/// it as an empty string in both directions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
pub enum Currency {
    /// No currency has been specified.
    #[default]
    #[cfg_attr(
        any(feature = "serde-serialize", feature = "serde-deserialize"),
        serde(rename = "")
    )]
    Unspecified,
    /// United Arab Emirates dirham
    AED,
    /// Afghan afghani
    AFN,
    /// Albanian lek
    ALL,
    /// Armenian dram
    AMD,
    /// Netherlands Antillean guilder
    ANG,
    /// Angolan kwanza
    AOA,
    /// Argentine peso
    ARS,
    /// Australian dollar
    AUD,
    /// Aruban florin
    AWG,
    /// Azerbaijani manat
    AZN,
    /// Bosnia and Herzegovina convertible mark
    BAM,
    /// Barbados dollar
    BBD,
    /// Bangladeshi taka
    BDT,
    /// Bulgarian lev
    BGN,
    /// Bahraini dinar
    BHD,
    /// Burundian franc
    BIF,
    /// Bermudian dollar
    BMD,
    /// Brunei dollar
    BND,
    /// Boliviano
    BOB,
    /// Bolivian Mvdol (funds code)
    BOV,
    /// Brazilian real
    BRL,
    /// Bahamian dollar
    BSD,
    /// Bhutanese ngultrum
    BTN,
    /// Botswana pula
    BWP,
    /// Belarusian ruble
    BYN,
    /// Belize dollar
    BZD,
    /// Canadian dollar
    CAD,
    /// Congolese franc
    CDF,
    /// WIR euro (complementary currency)
    CHE,
    /// Swiss franc
    CHF,
    /// WIR franc (complementary currency)
    CHW,
    /// Unidad de Fomento (funds code)
    CLF,
    /// Chilean peso
    CLP,
    /// Chinese yuan[8]
    CNY,
    /// Colombian peso
    COP,
    /// Unidad de Valor Real (UVR) (funds code)[9]
    COU,
    /// Costa Rican colon
    CRC,
    /// Cuban convertible peso
    CUC,
    /// Cuban peso
    CUP,
    /// Cape Verdean escudo
    CVE,
    /// Czech koruna
    CZK,
    /// Djiboutian franc
    DJF,
    /// Danish krone
    DKK,
    /// Dominican peso
    DOP,
    /// Algerian dinar
    DZD,
    /// Egyptian pound
    EGP,
    /// Eritrean nakfa
    ERN,
    /// Ethiopian birr
    ETB,
    /// Euro
    EUR,
    /// Fiji dollar
    FJD,
    /// Falkland Islands pound
    FKP,
    /// Pound sterling
    GBP,
    /// Georgian lari
    GEL,
    /// Ghanaian cedi
    GHS,
    /// Gibraltar pound
    GIP,
    /// Gambian dalasi
    GMD,
    /// Guinean franc
    GNF,
    /// Guatemalan quetzal
    GTQ,
    /// Guyanese dollar
    GYD,
    /// Hong Kong dollar
    HKD,
    /// Honduran lempira
    HNL,
    /// Croatian kuna
    HRK,
    /// Haitian gourde
    HTG,
    /// Hungarian forint
    HUF,
    /// Indonesian rupiah
    IDR,
    /// Israeli new shekel
    ILS,
    /// Indian rupee
    INR,
    /// Iraqi dinar
    IQD,
    /// Iranian rial
    IRR,
    /// Icelandic krona (plural: kronur)
    ISK,
    /// Jamaican dollar
    JMD,
    /// Jordanian dinar
    JOD,
    /// Japanese yen
    JPY,
    /// Kenyan shilling
    KES,
    /// Kyrgyzstani som
    KGS,
    /// Cambodian riel
    KHR,
    /// Comoro franc
    KMF,
    /// North Korean won
    KPW,
    /// South Korean won
    KRW,
    /// Kuwaiti dinar
    KWD,
    /// Cayman Islands dollar
    KYD,
    /// Kazakhstani tenge
    KZT,
    /// Lao kip
    LAK,
    /// Lebanese pound
    LBP,
    /// Sri Lankan rupee
    LKR,
    /// Liberian dollar
    LRD,
    /// Lesotho loti
    LSL,
    /// Libyan dinar
    LYD,
    /// Moroccan dirham
    MAD,
    /// Moldovan leu
    MDL,
    /// Malagasy ariary
    MGA,
    /// Macedonian denar
    MKD,
    /// Myanmar kyat
    MMK,
    /// Mongolian togrog
    MNT,
    /// Macanese pataca
    MOP,
    /// Mauritanian ouguiya
    MRU,
    /// Mauritian rupee
    MUR,
    /// Maldivian rufiyaa
    MVR,
    /// Malawian kwacha
    MWK,
    /// Mexican peso
    MXN,
    /// Mexican Unidad de Inversion (UDI) (funds code)
    MXV,
    /// Malaysian ringgit
    MYR,
    /// Mozambican metical
    MZN,
    /// Namibian dollar
    NAD,
    /// Nigerian naira
    NGN,
    /// Nicaraguan cordoba
    NIO,
    /// Norwegian krone
    NOK,
    /// Nepalese rupee
    NPR,
    /// New Zealand dollar
    NZD,
    /// Omani rial
    OMR,
    /// Panamanian balboa
    PAB,
    /// Peruvian sol
    PEN,
    /// Papua New Guinean kina
    PGK,
    /// Philippine peso[13]
    PHP,
    /// Pakistani rupee
    PKR,
    /// Polish zloty
    PLN,
    /// Paraguayan guarani
    PYG,
    /// Qatari riyal
    QAR,
    /// Romanian leu
    RON,
    /// Serbian dinar
    RSD,
    /// Russian ruble
    RUB,
    /// Rwandan franc
    RWF,
    /// Saudi riyal
    SAR,
    /// Solomon Islands dollar
    SBD,
    /// Seychelles rupee
    SCR,
    /// Sudanese pound
    SDG,
    /// Swedish krona (plural: kronor)
    SEK,
    /// Singapore dollar
    SGD,
    /// Saint Helena pound
    SHP,
    /// Sierra Leonean leone
    SLE,
    /// Somali shilling
    SOS,
    /// Surinamese dollar
    SRD,
    /// South Sudanese pound
    SSP,
    /// Sao Tome and Principe dobra
    STN,
    /// Salvadoran colon
    SVC,
    /// Syrian pound
    SYP,
    /// Swazi lilangeni
    SZL,
    /// Thai baht
    THB,
    /// Tajikistani somoni
    TJS,
    /// Turkmenistan manat
    TMT,
    /// Tunisian dinar
    TND,
    /// Tongan pa'anga
    TOP,
    /// Turkish lira
    TRY,
    /// Trinidad and Tobago dollar
    TTD,
    /// New Taiwan dollar
    TWD,
    /// Tanzanian shilling
    TZS,
    /// Ukrainian hryvnia
    UAH,
    /// Ugandan shilling
    UGX,
    /// United States dollar
    USD,
    /// United States dollar (next day) (funds code)
    USN,
    /// Uruguay Peso en Unidades Indexadas (URUIURUI) (funds code)
    UYI,
    /// Uruguayan peso
    UYU,
    /// Unidad previsional[15]
    UYW,
    /// Uzbekistan som
    UZS,
    /// Venezuelan bolivar digital[16]
    VED,
    /// Venezuelan bolivar soberano[13]
    VES,
    /// Vietnamese dong
    VND,
    /// Vanuatu vatu
    VUV,
    /// Samoan tala
    WST,
    /// CFA franc BEAC
    XAF,
    /// Silver (one troy ounce)
    XAG,
    /// Gold (one troy ounce)
    XAU,
    /// European Composite Unit (EURCO) (bond market unit)
    XBA,
    /// European Monetary Unit (E.M.U.-6) (bond market unit)
    XBB,
    /// European Unit of Account 9 (E.U.A.-9) (bond market unit)
    XBC,
    /// European Unit of Account 17 (E.U.A.-17) (bond market unit)
    XBD,
    /// East Caribbean dollar
    XCD,
    /// Special drawing rights
    XDR,
    /// CFA franc BCEAO
    XOF,
    /// Palladium (one troy ounce)
    XPD,
    /// CFP franc (franc Pacifique)
    XPF,
    /// Platinum (one troy ounce)
    XPT,
    /// SUCRE
    XSU,
    /// Code reserved for testing
    XTS,
    /// ADB Unit of Account
    XUA,
    /// No currency
    XXX,
    /// Yemeni rial
    YER,
    /// South African rand
    ZAR,
    /// Zambian kwacha
    ZMW,
    /// Zimbabwean dollar
    ZWL,
}

impl Currency {
    /// Read an assigned code, returning `None` for unknown codes or invalid lengths.
    /// Empty input and zero-filled byte arrays are not codes.
    pub const fn from_bytes(input: &[u8]) -> Option<Currency> {
        match input {
            b"AED" => Some(Currency::AED),
            b"AFN" => Some(Currency::AFN),
            b"ALL" => Some(Currency::ALL),
            b"AMD" => Some(Currency::AMD),
            b"ANG" => Some(Currency::ANG),
            b"AOA" => Some(Currency::AOA),
            b"ARS" => Some(Currency::ARS),
            b"AUD" => Some(Currency::AUD),
            b"AWG" => Some(Currency::AWG),
            b"AZN" => Some(Currency::AZN),
            b"BAM" => Some(Currency::BAM),
            b"BBD" => Some(Currency::BBD),
            b"BDT" => Some(Currency::BDT),
            b"BGN" => Some(Currency::BGN),
            b"BHD" => Some(Currency::BHD),
            b"BIF" => Some(Currency::BIF),
            b"BMD" => Some(Currency::BMD),
            b"BND" => Some(Currency::BND),
            b"BOB" => Some(Currency::BOB),
            b"BOV" => Some(Currency::BOV),
            b"BRL" => Some(Currency::BRL),
            b"BSD" => Some(Currency::BSD),
            b"BTN" => Some(Currency::BTN),
            b"BWP" => Some(Currency::BWP),
            b"BYN" => Some(Currency::BYN),
            b"BZD" => Some(Currency::BZD),
            b"CAD" => Some(Currency::CAD),
            b"CDF" => Some(Currency::CDF),
            b"CHE" => Some(Currency::CHE),
            b"CHF" => Some(Currency::CHF),
            b"CHW" => Some(Currency::CHW),
            b"CLF" => Some(Currency::CLF),
            b"CLP" => Some(Currency::CLP),
            b"CNY" => Some(Currency::CNY),
            b"COP" => Some(Currency::COP),
            b"COU" => Some(Currency::COU),
            b"CRC" => Some(Currency::CRC),
            b"CUC" => Some(Currency::CUC),
            b"CUP" => Some(Currency::CUP),
            b"CVE" => Some(Currency::CVE),
            b"CZK" => Some(Currency::CZK),
            b"DJF" => Some(Currency::DJF),
            b"DKK" => Some(Currency::DKK),
            b"DOP" => Some(Currency::DOP),
            b"DZD" => Some(Currency::DZD),
            b"EGP" => Some(Currency::EGP),
            b"ERN" => Some(Currency::ERN),
            b"ETB" => Some(Currency::ETB),
            b"EUR" => Some(Currency::EUR),
            b"FJD" => Some(Currency::FJD),
            b"FKP" => Some(Currency::FKP),
            b"GBP" => Some(Currency::GBP),
            b"GEL" => Some(Currency::GEL),
            b"GHS" => Some(Currency::GHS),
            b"GIP" => Some(Currency::GIP),
            b"GMD" => Some(Currency::GMD),
            b"GNF" => Some(Currency::GNF),
            b"GTQ" => Some(Currency::GTQ),
            b"GYD" => Some(Currency::GYD),
            b"HKD" => Some(Currency::HKD),
            b"HNL" => Some(Currency::HNL),
            b"HRK" => Some(Currency::HRK),
            b"HTG" => Some(Currency::HTG),
            b"HUF" => Some(Currency::HUF),
            b"IDR" => Some(Currency::IDR),
            b"ILS" => Some(Currency::ILS),
            b"INR" => Some(Currency::INR),
            b"IQD" => Some(Currency::IQD),
            b"IRR" => Some(Currency::IRR),
            b"ISK" => Some(Currency::ISK),
            b"JMD" => Some(Currency::JMD),
            b"JOD" => Some(Currency::JOD),
            b"JPY" => Some(Currency::JPY),
            b"KES" => Some(Currency::KES),
            b"KGS" => Some(Currency::KGS),
            b"KHR" => Some(Currency::KHR),
            b"KMF" => Some(Currency::KMF),
            b"KPW" => Some(Currency::KPW),
            b"KRW" => Some(Currency::KRW),
            b"KWD" => Some(Currency::KWD),
            b"KYD" => Some(Currency::KYD),
            b"KZT" => Some(Currency::KZT),
            b"LAK" => Some(Currency::LAK),
            b"LBP" => Some(Currency::LBP),
            b"LKR" => Some(Currency::LKR),
            b"LRD" => Some(Currency::LRD),
            b"LSL" => Some(Currency::LSL),
            b"LYD" => Some(Currency::LYD),
            b"MAD" => Some(Currency::MAD),
            b"MDL" => Some(Currency::MDL),
            b"MGA" => Some(Currency::MGA),
            b"MKD" => Some(Currency::MKD),
            b"MMK" => Some(Currency::MMK),
            b"MNT" => Some(Currency::MNT),
            b"MOP" => Some(Currency::MOP),
            b"MRU" => Some(Currency::MRU),
            b"MUR" => Some(Currency::MUR),
            b"MVR" => Some(Currency::MVR),
            b"MWK" => Some(Currency::MWK),
            b"MXN" => Some(Currency::MXN),
            b"MXV" => Some(Currency::MXV),
            b"MYR" => Some(Currency::MYR),
            b"MZN" => Some(Currency::MZN),
            b"NAD" => Some(Currency::NAD),
            b"NGN" => Some(Currency::NGN),
            b"NIO" => Some(Currency::NIO),
            b"NOK" => Some(Currency::NOK),
            b"NPR" => Some(Currency::NPR),
            b"NZD" => Some(Currency::NZD),
            b"OMR" => Some(Currency::OMR),
            b"PAB" => Some(Currency::PAB),
            b"PEN" => Some(Currency::PEN),
            b"PGK" => Some(Currency::PGK),
            b"PHP" => Some(Currency::PHP),
            b"PKR" => Some(Currency::PKR),
            b"PLN" => Some(Currency::PLN),
            b"PYG" => Some(Currency::PYG),
            b"QAR" => Some(Currency::QAR),
            b"RON" => Some(Currency::RON),
            b"RSD" => Some(Currency::RSD),
            b"RUB" => Some(Currency::RUB),
            b"RWF" => Some(Currency::RWF),
            b"SAR" => Some(Currency::SAR),
            b"SBD" => Some(Currency::SBD),
            b"SCR" => Some(Currency::SCR),
            b"SDG" => Some(Currency::SDG),
            b"SEK" => Some(Currency::SEK),
            b"SGD" => Some(Currency::SGD),
            b"SHP" => Some(Currency::SHP),
            b"SLE" => Some(Currency::SLE),
            b"SOS" => Some(Currency::SOS),
            b"SRD" => Some(Currency::SRD),
            b"SSP" => Some(Currency::SSP),
            b"STN" => Some(Currency::STN),
            b"SVC" => Some(Currency::SVC),
            b"SYP" => Some(Currency::SYP),
            b"SZL" => Some(Currency::SZL),
            b"THB" => Some(Currency::THB),
            b"TJS" => Some(Currency::TJS),
            b"TMT" => Some(Currency::TMT),
            b"TND" => Some(Currency::TND),
            b"TOP" => Some(Currency::TOP),
            b"TRY" => Some(Currency::TRY),
            b"TTD" => Some(Currency::TTD),
            b"TWD" => Some(Currency::TWD),
            b"TZS" => Some(Currency::TZS),
            b"UAH" => Some(Currency::UAH),
            b"UGX" => Some(Currency::UGX),
            b"USD" => Some(Currency::USD),
            b"USN" => Some(Currency::USN),
            b"UYI" => Some(Currency::UYI),
            b"UYU" => Some(Currency::UYU),
            b"UYW" => Some(Currency::UYW),
            b"UZS" => Some(Currency::UZS),
            b"VED" => Some(Currency::VED),
            b"VES" => Some(Currency::VES),
            b"VND" => Some(Currency::VND),
            b"VUV" => Some(Currency::VUV),
            b"WST" => Some(Currency::WST),
            b"XAF" => Some(Currency::XAF),
            b"XAG" => Some(Currency::XAG),
            b"XAU" => Some(Currency::XAU),
            b"XBA" => Some(Currency::XBA),
            b"XBB" => Some(Currency::XBB),
            b"XBC" => Some(Currency::XBC),
            b"XBD" => Some(Currency::XBD),
            b"XCD" => Some(Currency::XCD),
            b"XDR" => Some(Currency::XDR),
            b"XOF" => Some(Currency::XOF),
            b"XPD" => Some(Currency::XPD),
            b"XPF" => Some(Currency::XPF),
            b"XPT" => Some(Currency::XPT),
            b"XSU" => Some(Currency::XSU),
            b"XTS" => Some(Currency::XTS),
            b"XUA" => Some(Currency::XUA),
            b"XXX" => Some(Currency::XXX),
            b"YER" => Some(Currency::YER),
            b"ZAR" => Some(Currency::ZAR),
            b"ZMW" => Some(Currency::ZMW),
            b"ZWL" => Some(Currency::ZWL),
            _ => None,
        }
    }

    /// Whether no currency has been specified.
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Unspecified)
    }

    /// Return the three-letter currency code, or three zero bytes if unspecified.
    pub const fn to_bytes(self) -> [u8; 3] {
        *self.as_bytes()
    }

    /// Borrow the three-letter currency code as a string.
    /// Returns an empty string if unspecified.
    pub const fn as_str(&self) -> &'static str {
        self.as_fix_str().as_utf8()
    }

    /// Borrow the three-letter currency code as a FIX string.
    /// Returns an empty string if unspecified.
    pub const fn as_fix_str(&self) -> &'static FixStr {
        if self.is_empty() {
            fix_str!("")
        } else {
            // SAFETY: Specified codes contain only uppercase ASCII letters.
            unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
        }
    }

    /// Borrow the three-letter currency code, or three zero bytes if unspecified.
    pub const fn as_bytes(&self) -> &'static [u8; 3] {
        match self {
            Currency::Unspecified => &[0; 3],
            Currency::AED => b"AED",
            Currency::AFN => b"AFN",
            Currency::ALL => b"ALL",
            Currency::AMD => b"AMD",
            Currency::ANG => b"ANG",
            Currency::AOA => b"AOA",
            Currency::ARS => b"ARS",
            Currency::AUD => b"AUD",
            Currency::AWG => b"AWG",
            Currency::AZN => b"AZN",
            Currency::BAM => b"BAM",
            Currency::BBD => b"BBD",
            Currency::BDT => b"BDT",
            Currency::BGN => b"BGN",
            Currency::BHD => b"BHD",
            Currency::BIF => b"BIF",
            Currency::BMD => b"BMD",
            Currency::BND => b"BND",
            Currency::BOB => b"BOB",
            Currency::BOV => b"BOV",
            Currency::BRL => b"BRL",
            Currency::BSD => b"BSD",
            Currency::BTN => b"BTN",
            Currency::BWP => b"BWP",
            Currency::BYN => b"BYN",
            Currency::BZD => b"BZD",
            Currency::CAD => b"CAD",
            Currency::CDF => b"CDF",
            Currency::CHE => b"CHE",
            Currency::CHF => b"CHF",
            Currency::CHW => b"CHW",
            Currency::CLF => b"CLF",
            Currency::CLP => b"CLP",
            Currency::CNY => b"CNY",
            Currency::COP => b"COP",
            Currency::COU => b"COU",
            Currency::CRC => b"CRC",
            Currency::CUC => b"CUC",
            Currency::CUP => b"CUP",
            Currency::CVE => b"CVE",
            Currency::CZK => b"CZK",
            Currency::DJF => b"DJF",
            Currency::DKK => b"DKK",
            Currency::DOP => b"DOP",
            Currency::DZD => b"DZD",
            Currency::EGP => b"EGP",
            Currency::ERN => b"ERN",
            Currency::ETB => b"ETB",
            Currency::EUR => b"EUR",
            Currency::FJD => b"FJD",
            Currency::FKP => b"FKP",
            Currency::GBP => b"GBP",
            Currency::GEL => b"GEL",
            Currency::GHS => b"GHS",
            Currency::GIP => b"GIP",
            Currency::GMD => b"GMD",
            Currency::GNF => b"GNF",
            Currency::GTQ => b"GTQ",
            Currency::GYD => b"GYD",
            Currency::HKD => b"HKD",
            Currency::HNL => b"HNL",
            Currency::HRK => b"HRK",
            Currency::HTG => b"HTG",
            Currency::HUF => b"HUF",
            Currency::IDR => b"IDR",
            Currency::ILS => b"ILS",
            Currency::INR => b"INR",
            Currency::IQD => b"IQD",
            Currency::IRR => b"IRR",
            Currency::ISK => b"ISK",
            Currency::JMD => b"JMD",
            Currency::JOD => b"JOD",
            Currency::JPY => b"JPY",
            Currency::KES => b"KES",
            Currency::KGS => b"KGS",
            Currency::KHR => b"KHR",
            Currency::KMF => b"KMF",
            Currency::KPW => b"KPW",
            Currency::KRW => b"KRW",
            Currency::KWD => b"KWD",
            Currency::KYD => b"KYD",
            Currency::KZT => b"KZT",
            Currency::LAK => b"LAK",
            Currency::LBP => b"LBP",
            Currency::LKR => b"LKR",
            Currency::LRD => b"LRD",
            Currency::LSL => b"LSL",
            Currency::LYD => b"LYD",
            Currency::MAD => b"MAD",
            Currency::MDL => b"MDL",
            Currency::MGA => b"MGA",
            Currency::MKD => b"MKD",
            Currency::MMK => b"MMK",
            Currency::MNT => b"MNT",
            Currency::MOP => b"MOP",
            Currency::MRU => b"MRU",
            Currency::MUR => b"MUR",
            Currency::MVR => b"MVR",
            Currency::MWK => b"MWK",
            Currency::MXN => b"MXN",
            Currency::MXV => b"MXV",
            Currency::MYR => b"MYR",
            Currency::MZN => b"MZN",
            Currency::NAD => b"NAD",
            Currency::NGN => b"NGN",
            Currency::NIO => b"NIO",
            Currency::NOK => b"NOK",
            Currency::NPR => b"NPR",
            Currency::NZD => b"NZD",
            Currency::OMR => b"OMR",
            Currency::PAB => b"PAB",
            Currency::PEN => b"PEN",
            Currency::PGK => b"PGK",
            Currency::PHP => b"PHP",
            Currency::PKR => b"PKR",
            Currency::PLN => b"PLN",
            Currency::PYG => b"PYG",
            Currency::QAR => b"QAR",
            Currency::RON => b"RON",
            Currency::RSD => b"RSD",
            Currency::RUB => b"RUB",
            Currency::RWF => b"RWF",
            Currency::SAR => b"SAR",
            Currency::SBD => b"SBD",
            Currency::SCR => b"SCR",
            Currency::SDG => b"SDG",
            Currency::SEK => b"SEK",
            Currency::SGD => b"SGD",
            Currency::SHP => b"SHP",
            Currency::SLE => b"SLE",
            Currency::SOS => b"SOS",
            Currency::SRD => b"SRD",
            Currency::SSP => b"SSP",
            Currency::STN => b"STN",
            Currency::SVC => b"SVC",
            Currency::SYP => b"SYP",
            Currency::SZL => b"SZL",
            Currency::THB => b"THB",
            Currency::TJS => b"TJS",
            Currency::TMT => b"TMT",
            Currency::TND => b"TND",
            Currency::TOP => b"TOP",
            Currency::TRY => b"TRY",
            Currency::TTD => b"TTD",
            Currency::TWD => b"TWD",
            Currency::TZS => b"TZS",
            Currency::UAH => b"UAH",
            Currency::UGX => b"UGX",
            Currency::USD => b"USD",
            Currency::USN => b"USN",
            Currency::UYI => b"UYI",
            Currency::UYU => b"UYU",
            Currency::UYW => b"UYW",
            Currency::UZS => b"UZS",
            Currency::VED => b"VED",
            Currency::VES => b"VES",
            Currency::VND => b"VND",
            Currency::VUV => b"VUV",
            Currency::WST => b"WST",
            Currency::XAF => b"XAF",
            Currency::XAG => b"XAG",
            Currency::XAU => b"XAU",
            Currency::XBA => b"XBA",
            Currency::XBB => b"XBB",
            Currency::XBC => b"XBC",
            Currency::XBD => b"XBD",
            Currency::XCD => b"XCD",
            Currency::XDR => b"XDR",
            Currency::XOF => b"XOF",
            Currency::XPD => b"XPD",
            Currency::XPF => b"XPF",
            Currency::XPT => b"XPT",
            Currency::XSU => b"XSU",
            Currency::XTS => b"XTS",
            Currency::XUA => b"XUA",
            Currency::XXX => b"XXX",
            Currency::YER => b"YER",
            Currency::ZAR => b"ZAR",
            Currency::ZMW => b"ZMW",
            Currency::ZWL => b"ZWL",
        }
    }
}
