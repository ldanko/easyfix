//! ISO 639-1 language codes, as the two-character FIX `Language` datatype.

use crate::{basic_types::FixStr, fix_str};

// Current alpha-2 entries from the registry linked by ISO:
// https://www.iso.org/iso-639-language-code
// https://www.loc.gov/standards/iso639-2/ISO-639-2_utf-8.txt
// Retrieved 2026-10-17. Deprecated codes are excluded.

/// A language identified by an ISO 639-1 alpha-2 code.
///
/// Codes use lowercase ASCII letters (ISO 639-1:2002 section 4.1).
/// Only codes represented by the variants are accepted.
///
/// The default is [`Unspecified`](Self::Unspecified), with empty text views
/// and zero-filled byte views. FIX serialization rejects it; Serde represents
/// it as an empty string in both directions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde-serialize", derive(serde::Serialize))]
#[cfg_attr(feature = "serde-deserialize", derive(serde::Deserialize))]
#[cfg_attr(
    any(feature = "serde-serialize", feature = "serde-deserialize"),
    serde(rename_all = "lowercase")
)]
pub enum Language {
    /// No language has been specified.
    #[default]
    #[cfg_attr(
        any(feature = "serde-serialize", feature = "serde-deserialize"),
        serde(rename = "")
    )]
    Unspecified,
    /// Afar
    AA,
    /// Abkhazian
    AB,
    /// Avestan
    AE,
    /// Afrikaans
    AF,
    /// Akan
    AK,
    /// Amharic
    AM,
    /// Aragonese
    AN,
    /// Arabic
    AR,
    /// Assamese
    AS,
    /// Avaric
    AV,
    /// Aymara
    AY,
    /// Azerbaijani
    AZ,
    /// Bashkir
    BA,
    /// Belarusian
    BE,
    /// Bulgarian
    BG,
    /// Bislama
    BI,
    /// Bambara
    BM,
    /// Bengali
    BN,
    /// Tibetan
    BO,
    /// Breton
    BR,
    /// Bosnian
    BS,
    /// Catalan; Valencian
    CA,
    /// Chechen
    CE,
    /// Chamorro
    CH,
    /// Corsican
    CO,
    /// Cree
    CR,
    /// Czech
    CS,
    /// Church Slavic; Old Slavonic; Church Slavonic; Old Bulgarian; Old Church Slavonic
    CU,
    /// Chuvash
    CV,
    /// Welsh
    CY,
    /// Danish
    DA,
    /// German
    DE,
    /// Divehi; Dhivehi; Maldivian
    DV,
    /// Dzongkha
    DZ,
    /// Ewe
    EE,
    /// Modern Greek (1453-)
    EL,
    /// English
    EN,
    /// Esperanto
    EO,
    /// Spanish; Castilian
    ES,
    /// Estonian
    ET,
    /// Basque
    EU,
    /// Persian
    FA,
    /// Fulah
    FF,
    /// Finnish
    FI,
    /// Fijian
    FJ,
    /// Faroese
    FO,
    /// French
    FR,
    /// Western Frisian
    FY,
    /// Irish
    GA,
    /// Gaelic; Scottish Gaelic
    GD,
    /// Galician
    GL,
    /// Guarani
    GN,
    /// Gujarati
    GU,
    /// Manx
    GV,
    /// Hausa
    HA,
    /// Hebrew
    HE,
    /// Hindi
    HI,
    /// Hiri Motu
    HO,
    /// Croatian
    HR,
    /// Haitian; Haitian Creole
    HT,
    /// Hungarian
    HU,
    /// Armenian
    HY,
    /// Herero
    HZ,
    /// Interlingua (International Auxiliary Language Association)
    IA,
    /// Indonesian
    ID,
    /// Interlingue; Occidental
    IE,
    /// Igbo
    IG,
    /// Sichuan Yi; Nuosu
    II,
    /// Inupiaq
    IK,
    /// Ido
    IO,
    /// Icelandic
    IS,
    /// Italian
    IT,
    /// Inuktitut
    IU,
    /// Japanese
    JA,
    /// Javanese
    JV,
    /// Georgian
    KA,
    /// Kongo
    KG,
    /// Kikuyu; Gikuyu
    KI,
    /// Kuanyama; Kwanyama
    KJ,
    /// Kazakh
    KK,
    /// Kalaallisut; Greenlandic
    KL,
    /// Central Khmer
    KM,
    /// Kannada
    KN,
    /// Korean
    KO,
    /// Kanuri
    KR,
    /// Kashmiri
    KS,
    /// Kurdish
    KU,
    /// Komi
    KV,
    /// Cornish
    KW,
    /// Kirghiz; Kyrgyz
    KY,
    /// Latin
    LA,
    /// Luxembourgish; Letzeburgesch
    LB,
    /// Ganda
    LG,
    /// Limburgan; Limburger; Limburgish
    LI,
    /// Lingala
    LN,
    /// Lao
    LO,
    /// Lithuanian
    LT,
    /// Luba-Katanga
    LU,
    /// Latvian
    LV,
    /// Malagasy
    MG,
    /// Marshallese
    MH,
    /// Maori
    MI,
    /// Macedonian
    MK,
    /// Malayalam
    ML,
    /// Mongolian
    MN,
    /// Marathi
    MR,
    /// Malay
    MS,
    /// Maltese
    MT,
    /// Burmese
    MY,
    /// Nauru
    NA,
    /// Norwegian Bokmal
    NB,
    /// North Ndebele
    ND,
    /// Nepali
    NE,
    /// Ndonga
    NG,
    /// Dutch; Flemish
    NL,
    /// Norwegian Nynorsk
    NN,
    /// Norwegian
    NO,
    /// South Ndebele
    NR,
    /// Navajo; Navaho
    NV,
    /// Chichewa; Chewa; Nyanja
    NY,
    /// Occitan (post 1500)
    OC,
    /// Ojibwa
    OJ,
    /// Oromo
    OM,
    /// Oriya
    OR,
    /// Ossetian; Ossetic
    OS,
    /// Panjabi; Punjabi
    PA,
    /// Pali
    PI,
    /// Polish
    PL,
    /// Pushto; Pashto
    PS,
    /// Portuguese
    PT,
    /// Quechua
    QU,
    /// Romansh
    RM,
    /// Rundi
    RN,
    /// Romanian; Moldavian; Moldovan
    RO,
    /// Russian
    RU,
    /// Kinyarwanda
    RW,
    /// Sanskrit
    SA,
    /// Sardinian
    SC,
    /// Sindhi
    SD,
    /// Northern Sami
    SE,
    /// Sango
    SG,
    /// Sinhala; Sinhalese
    SI,
    /// Slovak
    SK,
    /// Slovenian
    SL,
    /// Samoan
    SM,
    /// Shona
    SN,
    /// Somali
    SO,
    /// Albanian
    SQ,
    /// Serbian
    SR,
    /// Swati
    SS,
    /// Sotho, Southern
    ST,
    /// Sundanese
    SU,
    /// Swedish
    SV,
    /// Swahili
    SW,
    /// Tamil
    TA,
    /// Telugu
    TE,
    /// Tajik
    TG,
    /// Thai
    TH,
    /// Tigrinya
    TI,
    /// Turkmen
    TK,
    /// Tagalog
    TL,
    /// Tswana
    TN,
    /// Tonga (Tonga Islands)
    TO,
    /// Turkish
    TR,
    /// Tsonga
    TS,
    /// Tatar
    TT,
    /// Twi
    TW,
    /// Tahitian
    TY,
    /// Uighur; Uyghur
    UG,
    /// Ukrainian
    UK,
    /// Urdu
    UR,
    /// Uzbek
    UZ,
    /// Venda
    VE,
    /// Vietnamese
    VI,
    /// Volapuk
    VO,
    /// Walloon
    WA,
    /// Wolof
    WO,
    /// Xhosa
    XH,
    /// Yiddish
    YI,
    /// Yoruba
    YO,
    /// Zhuang; Chuang
    ZA,
    /// Chinese
    ZH,
    /// Zulu
    ZU,
}

impl Language {
    /// Read an assigned code, returning `None` for unknown codes or invalid lengths.
    /// Empty input and zero-filled byte arrays are not codes.
    pub const fn from_bytes(input: &[u8]) -> Option<Self> {
        match input {
            b"aa" => Some(Self::AA),
            b"ab" => Some(Self::AB),
            b"ae" => Some(Self::AE),
            b"af" => Some(Self::AF),
            b"ak" => Some(Self::AK),
            b"am" => Some(Self::AM),
            b"an" => Some(Self::AN),
            b"ar" => Some(Self::AR),
            b"as" => Some(Self::AS),
            b"av" => Some(Self::AV),
            b"ay" => Some(Self::AY),
            b"az" => Some(Self::AZ),
            b"ba" => Some(Self::BA),
            b"be" => Some(Self::BE),
            b"bg" => Some(Self::BG),
            b"bi" => Some(Self::BI),
            b"bm" => Some(Self::BM),
            b"bn" => Some(Self::BN),
            b"bo" => Some(Self::BO),
            b"br" => Some(Self::BR),
            b"bs" => Some(Self::BS),
            b"ca" => Some(Self::CA),
            b"ce" => Some(Self::CE),
            b"ch" => Some(Self::CH),
            b"co" => Some(Self::CO),
            b"cr" => Some(Self::CR),
            b"cs" => Some(Self::CS),
            b"cu" => Some(Self::CU),
            b"cv" => Some(Self::CV),
            b"cy" => Some(Self::CY),
            b"da" => Some(Self::DA),
            b"de" => Some(Self::DE),
            b"dv" => Some(Self::DV),
            b"dz" => Some(Self::DZ),
            b"ee" => Some(Self::EE),
            b"el" => Some(Self::EL),
            b"en" => Some(Self::EN),
            b"eo" => Some(Self::EO),
            b"es" => Some(Self::ES),
            b"et" => Some(Self::ET),
            b"eu" => Some(Self::EU),
            b"fa" => Some(Self::FA),
            b"ff" => Some(Self::FF),
            b"fi" => Some(Self::FI),
            b"fj" => Some(Self::FJ),
            b"fo" => Some(Self::FO),
            b"fr" => Some(Self::FR),
            b"fy" => Some(Self::FY),
            b"ga" => Some(Self::GA),
            b"gd" => Some(Self::GD),
            b"gl" => Some(Self::GL),
            b"gn" => Some(Self::GN),
            b"gu" => Some(Self::GU),
            b"gv" => Some(Self::GV),
            b"ha" => Some(Self::HA),
            b"he" => Some(Self::HE),
            b"hi" => Some(Self::HI),
            b"ho" => Some(Self::HO),
            b"hr" => Some(Self::HR),
            b"ht" => Some(Self::HT),
            b"hu" => Some(Self::HU),
            b"hy" => Some(Self::HY),
            b"hz" => Some(Self::HZ),
            b"ia" => Some(Self::IA),
            b"id" => Some(Self::ID),
            b"ie" => Some(Self::IE),
            b"ig" => Some(Self::IG),
            b"ii" => Some(Self::II),
            b"ik" => Some(Self::IK),
            b"io" => Some(Self::IO),
            b"is" => Some(Self::IS),
            b"it" => Some(Self::IT),
            b"iu" => Some(Self::IU),
            b"ja" => Some(Self::JA),
            b"jv" => Some(Self::JV),
            b"ka" => Some(Self::KA),
            b"kg" => Some(Self::KG),
            b"ki" => Some(Self::KI),
            b"kj" => Some(Self::KJ),
            b"kk" => Some(Self::KK),
            b"kl" => Some(Self::KL),
            b"km" => Some(Self::KM),
            b"kn" => Some(Self::KN),
            b"ko" => Some(Self::KO),
            b"kr" => Some(Self::KR),
            b"ks" => Some(Self::KS),
            b"ku" => Some(Self::KU),
            b"kv" => Some(Self::KV),
            b"kw" => Some(Self::KW),
            b"ky" => Some(Self::KY),
            b"la" => Some(Self::LA),
            b"lb" => Some(Self::LB),
            b"lg" => Some(Self::LG),
            b"li" => Some(Self::LI),
            b"ln" => Some(Self::LN),
            b"lo" => Some(Self::LO),
            b"lt" => Some(Self::LT),
            b"lu" => Some(Self::LU),
            b"lv" => Some(Self::LV),
            b"mg" => Some(Self::MG),
            b"mh" => Some(Self::MH),
            b"mi" => Some(Self::MI),
            b"mk" => Some(Self::MK),
            b"ml" => Some(Self::ML),
            b"mn" => Some(Self::MN),
            b"mr" => Some(Self::MR),
            b"ms" => Some(Self::MS),
            b"mt" => Some(Self::MT),
            b"my" => Some(Self::MY),
            b"na" => Some(Self::NA),
            b"nb" => Some(Self::NB),
            b"nd" => Some(Self::ND),
            b"ne" => Some(Self::NE),
            b"ng" => Some(Self::NG),
            b"nl" => Some(Self::NL),
            b"nn" => Some(Self::NN),
            b"no" => Some(Self::NO),
            b"nr" => Some(Self::NR),
            b"nv" => Some(Self::NV),
            b"ny" => Some(Self::NY),
            b"oc" => Some(Self::OC),
            b"oj" => Some(Self::OJ),
            b"om" => Some(Self::OM),
            b"or" => Some(Self::OR),
            b"os" => Some(Self::OS),
            b"pa" => Some(Self::PA),
            b"pi" => Some(Self::PI),
            b"pl" => Some(Self::PL),
            b"ps" => Some(Self::PS),
            b"pt" => Some(Self::PT),
            b"qu" => Some(Self::QU),
            b"rm" => Some(Self::RM),
            b"rn" => Some(Self::RN),
            b"ro" => Some(Self::RO),
            b"ru" => Some(Self::RU),
            b"rw" => Some(Self::RW),
            b"sa" => Some(Self::SA),
            b"sc" => Some(Self::SC),
            b"sd" => Some(Self::SD),
            b"se" => Some(Self::SE),
            b"sg" => Some(Self::SG),
            b"si" => Some(Self::SI),
            b"sk" => Some(Self::SK),
            b"sl" => Some(Self::SL),
            b"sm" => Some(Self::SM),
            b"sn" => Some(Self::SN),
            b"so" => Some(Self::SO),
            b"sq" => Some(Self::SQ),
            b"sr" => Some(Self::SR),
            b"ss" => Some(Self::SS),
            b"st" => Some(Self::ST),
            b"su" => Some(Self::SU),
            b"sv" => Some(Self::SV),
            b"sw" => Some(Self::SW),
            b"ta" => Some(Self::TA),
            b"te" => Some(Self::TE),
            b"tg" => Some(Self::TG),
            b"th" => Some(Self::TH),
            b"ti" => Some(Self::TI),
            b"tk" => Some(Self::TK),
            b"tl" => Some(Self::TL),
            b"tn" => Some(Self::TN),
            b"to" => Some(Self::TO),
            b"tr" => Some(Self::TR),
            b"ts" => Some(Self::TS),
            b"tt" => Some(Self::TT),
            b"tw" => Some(Self::TW),
            b"ty" => Some(Self::TY),
            b"ug" => Some(Self::UG),
            b"uk" => Some(Self::UK),
            b"ur" => Some(Self::UR),
            b"uz" => Some(Self::UZ),
            b"ve" => Some(Self::VE),
            b"vi" => Some(Self::VI),
            b"vo" => Some(Self::VO),
            b"wa" => Some(Self::WA),
            b"wo" => Some(Self::WO),
            b"xh" => Some(Self::XH),
            b"yi" => Some(Self::YI),
            b"yo" => Some(Self::YO),
            b"za" => Some(Self::ZA),
            b"zh" => Some(Self::ZH),
            b"zu" => Some(Self::ZU),
            _ => None,
        }
    }

    /// Whether no language has been specified.
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Unspecified)
    }

    /// Return the two-letter language code, or two zero bytes if unspecified.
    pub const fn to_bytes(self) -> [u8; 2] {
        *self.as_bytes()
    }

    /// Borrow the two-letter language code as a string.
    /// Returns an empty string if unspecified.
    pub const fn as_str(&self) -> &'static str {
        self.as_fix_str().as_utf8()
    }

    /// Borrow the two-letter language code as a FIX string.
    /// Returns an empty string if unspecified.
    pub const fn as_fix_str(&self) -> &'static FixStr {
        if self.is_empty() {
            fix_str!("")
        } else {
            // SAFETY: Specified codes contain only lowercase ASCII letters.
            unsafe { FixStr::from_ascii_unchecked(self.as_bytes()) }
        }
    }

    /// Borrow the two-letter language code, or two zero bytes if unspecified.
    pub const fn as_bytes(&self) -> &'static [u8; 2] {
        match self {
            Self::Unspecified => &[0; 2],
            Self::AA => b"aa",
            Self::AB => b"ab",
            Self::AE => b"ae",
            Self::AF => b"af",
            Self::AK => b"ak",
            Self::AM => b"am",
            Self::AN => b"an",
            Self::AR => b"ar",
            Self::AS => b"as",
            Self::AV => b"av",
            Self::AY => b"ay",
            Self::AZ => b"az",
            Self::BA => b"ba",
            Self::BE => b"be",
            Self::BG => b"bg",
            Self::BI => b"bi",
            Self::BM => b"bm",
            Self::BN => b"bn",
            Self::BO => b"bo",
            Self::BR => b"br",
            Self::BS => b"bs",
            Self::CA => b"ca",
            Self::CE => b"ce",
            Self::CH => b"ch",
            Self::CO => b"co",
            Self::CR => b"cr",
            Self::CS => b"cs",
            Self::CU => b"cu",
            Self::CV => b"cv",
            Self::CY => b"cy",
            Self::DA => b"da",
            Self::DE => b"de",
            Self::DV => b"dv",
            Self::DZ => b"dz",
            Self::EE => b"ee",
            Self::EL => b"el",
            Self::EN => b"en",
            Self::EO => b"eo",
            Self::ES => b"es",
            Self::ET => b"et",
            Self::EU => b"eu",
            Self::FA => b"fa",
            Self::FF => b"ff",
            Self::FI => b"fi",
            Self::FJ => b"fj",
            Self::FO => b"fo",
            Self::FR => b"fr",
            Self::FY => b"fy",
            Self::GA => b"ga",
            Self::GD => b"gd",
            Self::GL => b"gl",
            Self::GN => b"gn",
            Self::GU => b"gu",
            Self::GV => b"gv",
            Self::HA => b"ha",
            Self::HE => b"he",
            Self::HI => b"hi",
            Self::HO => b"ho",
            Self::HR => b"hr",
            Self::HT => b"ht",
            Self::HU => b"hu",
            Self::HY => b"hy",
            Self::HZ => b"hz",
            Self::IA => b"ia",
            Self::ID => b"id",
            Self::IE => b"ie",
            Self::IG => b"ig",
            Self::II => b"ii",
            Self::IK => b"ik",
            Self::IO => b"io",
            Self::IS => b"is",
            Self::IT => b"it",
            Self::IU => b"iu",
            Self::JA => b"ja",
            Self::JV => b"jv",
            Self::KA => b"ka",
            Self::KG => b"kg",
            Self::KI => b"ki",
            Self::KJ => b"kj",
            Self::KK => b"kk",
            Self::KL => b"kl",
            Self::KM => b"km",
            Self::KN => b"kn",
            Self::KO => b"ko",
            Self::KR => b"kr",
            Self::KS => b"ks",
            Self::KU => b"ku",
            Self::KV => b"kv",
            Self::KW => b"kw",
            Self::KY => b"ky",
            Self::LA => b"la",
            Self::LB => b"lb",
            Self::LG => b"lg",
            Self::LI => b"li",
            Self::LN => b"ln",
            Self::LO => b"lo",
            Self::LT => b"lt",
            Self::LU => b"lu",
            Self::LV => b"lv",
            Self::MG => b"mg",
            Self::MH => b"mh",
            Self::MI => b"mi",
            Self::MK => b"mk",
            Self::ML => b"ml",
            Self::MN => b"mn",
            Self::MR => b"mr",
            Self::MS => b"ms",
            Self::MT => b"mt",
            Self::MY => b"my",
            Self::NA => b"na",
            Self::NB => b"nb",
            Self::ND => b"nd",
            Self::NE => b"ne",
            Self::NG => b"ng",
            Self::NL => b"nl",
            Self::NN => b"nn",
            Self::NO => b"no",
            Self::NR => b"nr",
            Self::NV => b"nv",
            Self::NY => b"ny",
            Self::OC => b"oc",
            Self::OJ => b"oj",
            Self::OM => b"om",
            Self::OR => b"or",
            Self::OS => b"os",
            Self::PA => b"pa",
            Self::PI => b"pi",
            Self::PL => b"pl",
            Self::PS => b"ps",
            Self::PT => b"pt",
            Self::QU => b"qu",
            Self::RM => b"rm",
            Self::RN => b"rn",
            Self::RO => b"ro",
            Self::RU => b"ru",
            Self::RW => b"rw",
            Self::SA => b"sa",
            Self::SC => b"sc",
            Self::SD => b"sd",
            Self::SE => b"se",
            Self::SG => b"sg",
            Self::SI => b"si",
            Self::SK => b"sk",
            Self::SL => b"sl",
            Self::SM => b"sm",
            Self::SN => b"sn",
            Self::SO => b"so",
            Self::SQ => b"sq",
            Self::SR => b"sr",
            Self::SS => b"ss",
            Self::ST => b"st",
            Self::SU => b"su",
            Self::SV => b"sv",
            Self::SW => b"sw",
            Self::TA => b"ta",
            Self::TE => b"te",
            Self::TG => b"tg",
            Self::TH => b"th",
            Self::TI => b"ti",
            Self::TK => b"tk",
            Self::TL => b"tl",
            Self::TN => b"tn",
            Self::TO => b"to",
            Self::TR => b"tr",
            Self::TS => b"ts",
            Self::TT => b"tt",
            Self::TW => b"tw",
            Self::TY => b"ty",
            Self::UG => b"ug",
            Self::UK => b"uk",
            Self::UR => b"ur",
            Self::UZ => b"uz",
            Self::VE => b"ve",
            Self::VI => b"vi",
            Self::VO => b"vo",
            Self::WA => b"wa",
            Self::WO => b"wo",
            Self::XH => b"xh",
            Self::YI => b"yi",
            Self::YO => b"yo",
            Self::ZA => b"za",
            Self::ZH => b"zh",
            Self::ZU => b"zu",
        }
    }
}
