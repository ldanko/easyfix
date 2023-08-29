#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Country {
    #[default]
    AD, // Andorra
    AE, // United Arab Emirates
    AF, // Afghanistan
    AG, // Antigua and Barbuda
    AI, // Anguilla
    AL, // Albania
    AM, // Armenia
    AO, // Angola
    AQ, // Antarctica
    AR, // Argentina
    AS, // American Samoa
    AT, // Austria
    AU, // Australia
    AW, // Aruba
    AX, // Åland Islands
    AZ, // Azerbaijan
    BA, // Bosnia and Herzegovina
    BB, // Barbados
    BD, // Bangladesh
    BE, // Belgium
    BF, // Burkina Faso
    BG, // Bulgaria
    BH, // Bahrain
    BI, // Burundi
    BJ, // Benin
    BL, // Saint Barthélemy
    BM, // Bermuda
    BN, // Brunei Darussalam
    BO, // Bolivia (Plurinational State of)
    BQ, // Bonaire, Sint Eustatius and Saba
    BR, // Brazil
    BS, // Bahamas
    BT, // Bhutan
    BV, // Bouvet Island
    BW, // Botswana
    BY, // Belarus
    BZ, // Belize
    CA, // Canada
    CC, // Cocos (Keeling) Islands
    CD, // Congo, Democratic Republic of the
    CF, // Central African Republic
    CG, // Congo
    CH, // Switzerland
    CI, // Côte d'Ivoire
    CK, // Cook Islands
    CL, // Chile
    CM, // Cameroon
    CN, // China
    CO, // Colombia
    CR, // Costa Rica
    CU, // Cuba
    CV, // Cabo Verde
    CW, // Curaçao
    CX, // Christmas Island
    CY, // Cyprus
    CZ, // Czechia
    DE, // Germany
    DJ, // Djibouti
    DK, // Denmark
    DM, // Dominica
    DO, // Dominican Republic
    DZ, // Algeria
    EC, // Ecuador
    EE, // Estonia
    EG, // Egypt
    EH, // Western Sahara
    ER, // Eritrea
    ES, // Spain
    ET, // Ethiopia
    FI, // Finland
    FJ, // Fiji
    FK, // Falkland Islands (Malvinas)
    FM, // Micronesia (Federated States of)
    FO, // Faroe Islands
    FR, // France
    GA, // Gabon
    GB, // United Kingdom of Great Britain and Northern Ireland
    GD, // Grenada
    GE, // Georgia
    GF, // French Guiana
    GG, // Guernsey
    GH, // Ghana
    GI, // Gibraltar
    GL, // Greenland
    GM, // Gambia
    GN, // Guinea
    GP, // Guadeloupe
    GQ, // Equatorial Guinea
    GR, // Greece
    GS, // South Georgia and the South Sandwich Islands
    GT, // Guatemala
    GU, // Guam
    GW, // Guinea-Bissau
    GY, // Guyana
    HK, // Hong Kong
    HM, // Heard Island and McDonald Islands
    HN, // Honduras
    HR, // Croatia
    HT, // Haiti
    HU, // Hungary
    ID, // Indonesia
    IE, // Ireland
    IL, // Israel
    IM, // Isle of Man
    IN, // India
    IO, // British Indian Ocean Territory
    IQ, // Iraq
    IR, // Iran (Islamic Republic of)
    IS, // Iceland
    IT, // Italy
    JE, // Jersey
    JM, // Jamaica
    JO, // Jordan
    JP, // Japan
    KE, // Kenya
    KG, // Kyrgyzstan
    KH, // Cambodia
    KI, // Kiribati
    KM, // Comoros
    KN, // Saint Kitts and Nevis
    KP, // Korea (Democratic People's Republic of)
    KR, // Korea, Republic of
    KW, // Kuwait
    KY, // Cayman Islands
    KZ, // Kazakhstan
    LA, // Lao People's Democratic Republic
    LB, // Lebanon
    LC, // Saint Lucia
    LI, // Liechtenstein
    LK, // Sri Lanka
    LR, // Liberia
    LS, // Lesotho
    LT, // Lithuania
    LU, // Luxembourg
    LV, // Latvia
    LY, // Libya
    MA, // Morocco
    MC, // Monaco
    MD, // Moldova, Republic of
    ME, // Montenegro
    MF, // Saint Martin (French part)
    MG, // Madagascar
    MH, // Marshall Islands
    MK, // North Macedonia
    ML, // Mali
    MM, // Myanmar
    MN, // Mongolia
    MO, // Macao
    MP, // Northern Mariana Islands
    MQ, // Martinique
    MR, // Mauritania
    MS, // Montserrat
    MT, // Malta
    MU, // Mauritius
    MV, // Maldives
    MW, // Malawi
    MX, // Mexico
    MY, // Malaysia
    MZ, // Mozambique
    NA, // Namibia
    NC, // New Caledonia
    NE, // Niger
    NF, // Norfolk Island
    NG, // Nigeria
    NI, // Nicaragua
    NL, // Netherlands
    NO, // Norway
    NP, // Nepal
    NR, // Nauru
    NU, // Niue
    NZ, // New Zealand
    OM, // Oman
    PA, // Panama
    PE, // Peru
    PF, // French Polynesia
    PG, // Papua New Guinea
    PH, // Philippines
    PK, // Pakistan
    PL, // Poland
    PM, // Saint Pierre and Miquelon
    PN, // Pitcairn
    PR, // Puerto Rico
    PS, // Palestine, State of
    PT, // Portugal
    PW, // Palau
    PY, // Paraguay
    QA, // Qatar
    RE, // Réunion
    RO, // Romania
    RS, // Serbia
    RU, // Russian Federation
    RW, // Rwanda
    SA, // Saudi Arabia
    SB, // Solomon Islands
    SC, // Seychelles
    SD, // Sudan
    SE, // Sweden
    SG, // Singapore
    SH, // Saint Helena, Ascension and Tristan da Cunha
    SI, // Slovenia
    SJ, // Svalbard and Jan Mayen
    SK, // Slovakia
    SL, // Sierra Leone
    SM, // San Marino
    SN, // Senegal
    SO, // Somalia
    SR, // Suriname
    SS, // South Sudan
    ST, // Sao Tome and Principe
    SV, // El Salvador
    SX, // Sint Maarten (Dutch part)
    SY, // Syrian Arab Republic
    SZ, // Eswatini
    TC, // Turks and Caicos Islands
    TD, // Chad
    TF, // French Southern Territories
    TG, // Togo
    TH, // Thailand
    TJ, // Tajikistan
    TK, // Tokelau
    TL, // Timor-Leste
    TM, // Turkmenistan
    TN, // Tunisia
    TO, // Tonga
    TR, // Turkey
    TT, // Trinidad and Tobago
    TV, // Tuvalu
    TW, // Taiwan, Province of China
    TZ, // Tanzania, United Republic of
    UA, // Ukraine
    UG, // Uganda
    UM, // United States Minor Outlying Islands
    US, // United States of America
    UY, // Uruguay
    UZ, // Uzbekistan
    VA, // Holy See
    VC, // Saint Vincent and the Grenadines
    VE, // Venezuela (Bolivarian Republic of)
    VG, // Virgin Islands (British)
    VI, // Virgin Islands (U.S.)
    VN, // Viet Nam
    VU, // Vanuatu
    WF, // Wallis and Futuna
    WS, // Samoa
    YE, // Yemen
    YT, // Mayotte
    ZA, // South Africa
    ZM, // Zambia
    ZW, // Zimbabwe
}

impl Country {
    pub const fn from_bytes(input: &[u8]) -> Option<Country> {
        match input {
            b"AD" => Some(Country::AD),
            b"AE" => Some(Country::AE),
            b"AF" => Some(Country::AF),
            b"AG" => Some(Country::AG),
            b"AI" => Some(Country::AI),
            b"AL" => Some(Country::AL),
            b"AM" => Some(Country::AM),
            b"AO" => Some(Country::AO),
            b"AQ" => Some(Country::AQ),
            b"AR" => Some(Country::AR),
            b"AS" => Some(Country::AS),
            b"AT" => Some(Country::AT),
            b"AU" => Some(Country::AU),
            b"AW" => Some(Country::AW),
            b"AX" => Some(Country::AX),
            b"AZ" => Some(Country::AZ),
            b"BA" => Some(Country::BA),
            b"BB" => Some(Country::BB),
            b"BD" => Some(Country::BD),
            b"BE" => Some(Country::BE),
            b"BF" => Some(Country::BF),
            b"BG" => Some(Country::BG),
            b"BH" => Some(Country::BH),
            b"BI" => Some(Country::BI),
            b"BJ" => Some(Country::BJ),
            b"BL" => Some(Country::BL),
            b"BM" => Some(Country::BM),
            b"BN" => Some(Country::BN),
            b"BO" => Some(Country::BO),
            b"BQ" => Some(Country::BQ),
            b"BR" => Some(Country::BR),
            b"BS" => Some(Country::BS),
            b"BT" => Some(Country::BT),
            b"BV" => Some(Country::BV),
            b"BW" => Some(Country::BW),
            b"BY" => Some(Country::BY),
            b"BZ" => Some(Country::BZ),
            b"CA" => Some(Country::CA),
            b"CC" => Some(Country::CC),
            b"CD" => Some(Country::CD),
            b"CF" => Some(Country::CF),
            b"CG" => Some(Country::CG),
            b"CH" => Some(Country::CH),
            b"CI" => Some(Country::CI),
            b"CK" => Some(Country::CK),
            b"CL" => Some(Country::CL),
            b"CM" => Some(Country::CM),
            b"CN" => Some(Country::CN),
            b"CO" => Some(Country::CO),
            b"CR" => Some(Country::CR),
            b"CU" => Some(Country::CU),
            b"CV" => Some(Country::CV),
            b"CW" => Some(Country::CW),
            b"CX" => Some(Country::CX),
            b"CY" => Some(Country::CY),
            b"CZ" => Some(Country::CZ),
            b"DE" => Some(Country::DE),
            b"DJ" => Some(Country::DJ),
            b"DK" => Some(Country::DK),
            b"DM" => Some(Country::DM),
            b"DO" => Some(Country::DO),
            b"DZ" => Some(Country::DZ),
            b"EC" => Some(Country::EC),
            b"EE" => Some(Country::EE),
            b"EG" => Some(Country::EG),
            b"EH" => Some(Country::EH),
            b"ER" => Some(Country::ER),
            b"ES" => Some(Country::ES),
            b"ET" => Some(Country::ET),
            b"FI" => Some(Country::FI),
            b"FJ" => Some(Country::FJ),
            b"FK" => Some(Country::FK),
            b"FM" => Some(Country::FM),
            b"FO" => Some(Country::FO),
            b"FR" => Some(Country::FR),
            b"GA" => Some(Country::GA),
            b"GB" => Some(Country::GB),
            b"GD" => Some(Country::GD),
            b"GE" => Some(Country::GE),
            b"GF" => Some(Country::GF),
            b"GG" => Some(Country::GG),
            b"GH" => Some(Country::GH),
            b"GI" => Some(Country::GI),
            b"GL" => Some(Country::GL),
            b"GM" => Some(Country::GM),
            b"GN" => Some(Country::GN),
            b"GP" => Some(Country::GP),
            b"GQ" => Some(Country::GQ),
            b"GR" => Some(Country::GR),
            b"GS" => Some(Country::GS),
            b"GT" => Some(Country::GT),
            b"GU" => Some(Country::GU),
            b"GW" => Some(Country::GW),
            b"GY" => Some(Country::GY),
            b"HK" => Some(Country::HK),
            b"HM" => Some(Country::HM),
            b"HN" => Some(Country::HN),
            b"HR" => Some(Country::HR),
            b"HT" => Some(Country::HT),
            b"HU" => Some(Country::HU),
            b"ID" => Some(Country::ID),
            b"IE" => Some(Country::IE),
            b"IL" => Some(Country::IL),
            b"IM" => Some(Country::IM),
            b"IN" => Some(Country::IN),
            b"IO" => Some(Country::IO),
            b"IQ" => Some(Country::IQ),
            b"IR" => Some(Country::IR),
            b"IS" => Some(Country::IS),
            b"IT" => Some(Country::IT),
            b"JE" => Some(Country::JE),
            b"JM" => Some(Country::JM),
            b"JO" => Some(Country::JO),
            b"JP" => Some(Country::JP),
            b"KE" => Some(Country::KE),
            b"KG" => Some(Country::KG),
            b"KH" => Some(Country::KH),
            b"KI" => Some(Country::KI),
            b"KM" => Some(Country::KM),
            b"KN" => Some(Country::KN),
            b"KP" => Some(Country::KP),
            b"KR" => Some(Country::KR),
            b"KW" => Some(Country::KW),
            b"KY" => Some(Country::KY),
            b"KZ" => Some(Country::KZ),
            b"LA" => Some(Country::LA),
            b"LB" => Some(Country::LB),
            b"LC" => Some(Country::LC),
            b"LI" => Some(Country::LI),
            b"LK" => Some(Country::LK),
            b"LR" => Some(Country::LR),
            b"LS" => Some(Country::LS),
            b"LT" => Some(Country::LT),
            b"LU" => Some(Country::LU),
            b"LV" => Some(Country::LV),
            b"LY" => Some(Country::LY),
            b"MA" => Some(Country::MA),
            b"MC" => Some(Country::MC),
            b"MD" => Some(Country::MD),
            b"ME" => Some(Country::ME),
            b"MF" => Some(Country::MF),
            b"MG" => Some(Country::MG),
            b"MH" => Some(Country::MH),
            b"MK" => Some(Country::MK),
            b"ML" => Some(Country::ML),
            b"MM" => Some(Country::MM),
            b"MN" => Some(Country::MN),
            b"MO" => Some(Country::MO),
            b"MP" => Some(Country::MP),
            b"MQ" => Some(Country::MQ),
            b"MR" => Some(Country::MR),
            b"MS" => Some(Country::MS),
            b"MT" => Some(Country::MT),
            b"MU" => Some(Country::MU),
            b"MV" => Some(Country::MV),
            b"MW" => Some(Country::MW),
            b"MX" => Some(Country::MX),
            b"MY" => Some(Country::MY),
            b"MZ" => Some(Country::MZ),
            b"NA" => Some(Country::NA),
            b"NC" => Some(Country::NC),
            b"NE" => Some(Country::NE),
            b"NF" => Some(Country::NF),
            b"NG" => Some(Country::NG),
            b"NI" => Some(Country::NI),
            b"NL" => Some(Country::NL),
            b"NO" => Some(Country::NO),
            b"NP" => Some(Country::NP),
            b"NR" => Some(Country::NR),
            b"NU" => Some(Country::NU),
            b"NZ" => Some(Country::NZ),
            b"OM" => Some(Country::OM),
            b"PA" => Some(Country::PA),
            b"PE" => Some(Country::PE),
            b"PF" => Some(Country::PF),
            b"PG" => Some(Country::PG),
            b"PH" => Some(Country::PH),
            b"PK" => Some(Country::PK),
            b"PL" => Some(Country::PL),
            b"PM" => Some(Country::PM),
            b"PN" => Some(Country::PN),
            b"PR" => Some(Country::PR),
            b"PS" => Some(Country::PS),
            b"PT" => Some(Country::PT),
            b"PW" => Some(Country::PW),
            b"PY" => Some(Country::PY),
            b"QA" => Some(Country::QA),
            b"RE" => Some(Country::RE),
            b"RO" => Some(Country::RO),
            b"RS" => Some(Country::RS),
            b"RU" => Some(Country::RU),
            b"RW" => Some(Country::RW),
            b"SA" => Some(Country::SA),
            b"SB" => Some(Country::SB),
            b"SC" => Some(Country::SC),
            b"SD" => Some(Country::SD),
            b"SE" => Some(Country::SE),
            b"SG" => Some(Country::SG),
            b"SH" => Some(Country::SH),
            b"SI" => Some(Country::SI),
            b"SJ" => Some(Country::SJ),
            b"SK" => Some(Country::SK),
            b"SL" => Some(Country::SL),
            b"SM" => Some(Country::SM),
            b"SN" => Some(Country::SN),
            b"SO" => Some(Country::SO),
            b"SR" => Some(Country::SR),
            b"SS" => Some(Country::SS),
            b"ST" => Some(Country::ST),
            b"SV" => Some(Country::SV),
            b"SX" => Some(Country::SX),
            b"SY" => Some(Country::SY),
            b"SZ" => Some(Country::SZ),
            b"TC" => Some(Country::TC),
            b"TD" => Some(Country::TD),
            b"TF" => Some(Country::TF),
            b"TG" => Some(Country::TG),
            b"TH" => Some(Country::TH),
            b"TJ" => Some(Country::TJ),
            b"TK" => Some(Country::TK),
            b"TL" => Some(Country::TL),
            b"TM" => Some(Country::TM),
            b"TN" => Some(Country::TN),
            b"TO" => Some(Country::TO),
            b"TR" => Some(Country::TR),
            b"TT" => Some(Country::TT),
            b"TV" => Some(Country::TV),
            b"TW" => Some(Country::TW),
            b"TZ" => Some(Country::TZ),
            b"UA" => Some(Country::UA),
            b"UG" => Some(Country::UG),
            b"UM" => Some(Country::UM),
            b"US" => Some(Country::US),
            b"UY" => Some(Country::UY),
            b"UZ" => Some(Country::UZ),
            b"VA" => Some(Country::VA),
            b"VC" => Some(Country::VC),
            b"VE" => Some(Country::VE),
            b"VG" => Some(Country::VG),
            b"VI" => Some(Country::VI),
            b"VN" => Some(Country::VN),
            b"VU" => Some(Country::VU),
            b"WF" => Some(Country::WF),
            b"WS" => Some(Country::WS),
            b"YE" => Some(Country::YE),
            b"YT" => Some(Country::YT),
            b"ZA" => Some(Country::ZA),
            b"ZM" => Some(Country::ZM),
            b"ZW" => Some(Country::ZW),
            _ => None,
        }
    }

    pub const fn to_bytes(&self) -> &'static [u8] {
        match self {
            Country::AD => b"AD",
            Country::AE => b"AE",
            Country::AF => b"AF",
            Country::AG => b"AG",
            Country::AI => b"AI",
            Country::AL => b"AL",
            Country::AM => b"AM",
            Country::AO => b"AO",
            Country::AQ => b"AQ",
            Country::AR => b"AR",
            Country::AS => b"AS",
            Country::AT => b"AT",
            Country::AU => b"AU",
            Country::AW => b"AW",
            Country::AX => b"AX",
            Country::AZ => b"AZ",
            Country::BA => b"BA",
            Country::BB => b"BB",
            Country::BD => b"BD",
            Country::BE => b"BE",
            Country::BF => b"BF",
            Country::BG => b"BG",
            Country::BH => b"BH",
            Country::BI => b"BI",
            Country::BJ => b"BJ",
            Country::BL => b"BL",
            Country::BM => b"BM",
            Country::BN => b"BN",
            Country::BO => b"BO",
            Country::BQ => b"BQ",
            Country::BR => b"BR",
            Country::BS => b"BS",
            Country::BT => b"BT",
            Country::BV => b"BV",
            Country::BW => b"BW",
            Country::BY => b"BY",
            Country::BZ => b"BZ",
            Country::CA => b"CA",
            Country::CC => b"CC",
            Country::CD => b"CD",
            Country::CF => b"CF",
            Country::CG => b"CG",
            Country::CH => b"CH",
            Country::CI => b"CI",
            Country::CK => b"CK",
            Country::CL => b"CL",
            Country::CM => b"CM",
            Country::CN => b"CN",
            Country::CO => b"CO",
            Country::CR => b"CR",
            Country::CU => b"CU",
            Country::CV => b"CV",
            Country::CW => b"CW",
            Country::CX => b"CX",
            Country::CY => b"CY",
            Country::CZ => b"CZ",
            Country::DE => b"DE",
            Country::DJ => b"DJ",
            Country::DK => b"DK",
            Country::DM => b"DM",
            Country::DO => b"DO",
            Country::DZ => b"DZ",
            Country::EC => b"EC",
            Country::EE => b"EE",
            Country::EG => b"EG",
            Country::EH => b"EH",
            Country::ER => b"ER",
            Country::ES => b"ES",
            Country::ET => b"ET",
            Country::FI => b"FI",
            Country::FJ => b"FJ",
            Country::FK => b"FK",
            Country::FM => b"FM",
            Country::FO => b"FO",
            Country::FR => b"FR",
            Country::GA => b"GA",
            Country::GB => b"GB",
            Country::GD => b"GD",
            Country::GE => b"GE",
            Country::GF => b"GF",
            Country::GG => b"GG",
            Country::GH => b"GH",
            Country::GI => b"GI",
            Country::GL => b"GL",
            Country::GM => b"GM",
            Country::GN => b"GN",
            Country::GP => b"GP",
            Country::GQ => b"GQ",
            Country::GR => b"GR",
            Country::GS => b"GS",
            Country::GT => b"GT",
            Country::GU => b"GU",
            Country::GW => b"GW",
            Country::GY => b"GY",
            Country::HK => b"HK",
            Country::HM => b"HM",
            Country::HN => b"HN",
            Country::HR => b"HR",
            Country::HT => b"HT",
            Country::HU => b"HU",
            Country::ID => b"ID",
            Country::IE => b"IE",
            Country::IL => b"IL",
            Country::IM => b"IM",
            Country::IN => b"IN",
            Country::IO => b"IO",
            Country::IQ => b"IQ",
            Country::IR => b"IR",
            Country::IS => b"IS",
            Country::IT => b"IT",
            Country::JE => b"JE",
            Country::JM => b"JM",
            Country::JO => b"JO",
            Country::JP => b"JP",
            Country::KE => b"KE",
            Country::KG => b"KG",
            Country::KH => b"KH",
            Country::KI => b"KI",
            Country::KM => b"KM",
            Country::KN => b"KN",
            Country::KP => b"KP",
            Country::KR => b"KR",
            Country::KW => b"KW",
            Country::KY => b"KY",
            Country::KZ => b"KZ",
            Country::LA => b"LA",
            Country::LB => b"LB",
            Country::LC => b"LC",
            Country::LI => b"LI",
            Country::LK => b"LK",
            Country::LR => b"LR",
            Country::LS => b"LS",
            Country::LT => b"LT",
            Country::LU => b"LU",
            Country::LV => b"LV",
            Country::LY => b"LY",
            Country::MA => b"MA",
            Country::MC => b"MC",
            Country::MD => b"MD",
            Country::ME => b"ME",
            Country::MF => b"MF",
            Country::MG => b"MG",
            Country::MH => b"MH",
            Country::MK => b"MK",
            Country::ML => b"ML",
            Country::MM => b"MM",
            Country::MN => b"MN",
            Country::MO => b"MO",
            Country::MP => b"MP",
            Country::MQ => b"MQ",
            Country::MR => b"MR",
            Country::MS => b"MS",
            Country::MT => b"MT",
            Country::MU => b"MU",
            Country::MV => b"MV",
            Country::MW => b"MW",
            Country::MX => b"MX",
            Country::MY => b"MY",
            Country::MZ => b"MZ",
            Country::NA => b"NA",
            Country::NC => b"NC",
            Country::NE => b"NE",
            Country::NF => b"NF",
            Country::NG => b"NG",
            Country::NI => b"NI",
            Country::NL => b"NL",
            Country::NO => b"NO",
            Country::NP => b"NP",
            Country::NR => b"NR",
            Country::NU => b"NU",
            Country::NZ => b"NZ",
            Country::OM => b"OM",
            Country::PA => b"PA",
            Country::PE => b"PE",
            Country::PF => b"PF",
            Country::PG => b"PG",
            Country::PH => b"PH",
            Country::PK => b"PK",
            Country::PL => b"PL",
            Country::PM => b"PM",
            Country::PN => b"PN",
            Country::PR => b"PR",
            Country::PS => b"PS",
            Country::PT => b"PT",
            Country::PW => b"PW",
            Country::PY => b"PY",
            Country::QA => b"QA",
            Country::RE => b"RE",
            Country::RO => b"RO",
            Country::RS => b"RS",
            Country::RU => b"RU",
            Country::RW => b"RW",
            Country::SA => b"SA",
            Country::SB => b"SB",
            Country::SC => b"SC",
            Country::SD => b"SD",
            Country::SE => b"SE",
            Country::SG => b"SG",
            Country::SH => b"SH",
            Country::SI => b"SI",
            Country::SJ => b"SJ",
            Country::SK => b"SK",
            Country::SL => b"SL",
            Country::SM => b"SM",
            Country::SN => b"SN",
            Country::SO => b"SO",
            Country::SR => b"SR",
            Country::SS => b"SS",
            Country::ST => b"ST",
            Country::SV => b"SV",
            Country::SX => b"SX",
            Country::SY => b"SY",
            Country::SZ => b"SZ",
            Country::TC => b"TC",
            Country::TD => b"TD",
            Country::TF => b"TF",
            Country::TG => b"TG",
            Country::TH => b"TH",
            Country::TJ => b"TJ",
            Country::TK => b"TK",
            Country::TL => b"TL",
            Country::TM => b"TM",
            Country::TN => b"TN",
            Country::TO => b"TO",
            Country::TR => b"TR",
            Country::TT => b"TT",
            Country::TV => b"TV",
            Country::TW => b"TW",
            Country::TZ => b"TZ",
            Country::UA => b"UA",
            Country::UG => b"UG",
            Country::UM => b"UM",
            Country::US => b"US",
            Country::UY => b"UY",
            Country::UZ => b"UZ",
            Country::VA => b"VA",
            Country::VC => b"VC",
            Country::VE => b"VE",
            Country::VG => b"VG",
            Country::VI => b"VI",
            Country::VN => b"VN",
            Country::VU => b"VU",
            Country::WF => b"WF",
            Country::WS => b"WS",
            Country::YE => b"YE",
            Country::YT => b"YT",
            Country::ZA => b"ZA",
            Country::ZM => b"ZM",
            Country::ZW => b"ZW",
        }
    }
}
