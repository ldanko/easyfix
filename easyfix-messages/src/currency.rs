#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub enum Currency {
    #[default]
    AED, // United Arab Emirates dirham
    AFN, // Afghan afghani
    ALL, // Albanian lek
    AMD, // Armenian dram
    ANG, // Netherlands Antillean guilder
    AOA, // Angolan kwanza
    ARS, // Argentine peso
    AUD, // Australian dollar
    AWG, // Aruban florin
    AZN, // Azerbaijani manat
    BAM, // Bosnia and Herzegovina convertible mark
    BBD, // Barbados dollar
    BDT, // Bangladeshi taka
    BGN, // Bulgarian lev
    BHD, // Bahraini dinar
    BIF, // Burundian franc
    BMD, // Bermudian dollar
    BND, // Brunei dollar
    BOB, // Boliviano
    BOV, // Bolivian Mvdol (funds code)
    BRL, // Brazilian real
    BSD, // Bahamian dollar
    BTN, // Bhutanese ngultrum
    BWP, // Botswana pula
    BYN, // Belarusian ruble
    BZD, // Belize dollar
    CAD, // Canadian dollar
    CDF, // Congolese franc
    CHE, // WIR euro (complementary currency)
    CHF, // Swiss franc
    CHW, // WIR franc (complementary currency)
    CLF, // Unidad de Fomento (funds code)
    CLP, // Chilean peso
    CNY, // Chinese yuan[8]
    COP, // Colombian peso
    COU, // Unidad de Valor Real (UVR) (funds code)[9]
    CRC, // Costa Rican colon
    CUC, // Cuban convertible peso
    CUP, // Cuban peso
    CVE, // Cape Verdean escudo
    CZK, // Czech koruna
    DJF, // Djiboutian franc
    DKK, // Danish krone
    DOP, // Dominican peso
    DZD, // Algerian dinar
    EGP, // Egyptian pound
    ERN, // Eritrean nakfa
    ETB, // Ethiopian birr
    EUR, // Euro
    FJD, // Fiji dollar
    FKP, // Falkland Islands pound
    GBP, // Pound sterling
    GEL, // Georgian lari
    GHS, // Ghanaian cedi
    GIP, // Gibraltar pound
    GMD, // Gambian dalasi
    GNF, // Guinean franc
    GTQ, // Guatemalan quetzal
    GYD, // Guyanese dollar
    HKD, // Hong Kong dollar
    HNL, // Honduran lempira
    HRK, // Croatian kuna
    HTG, // Haitian gourde
    HUF, // Hungarian forint
    IDR, // Indonesian rupiah
    ILS, // Israeli new shekel
    INR, // Indian rupee
    IQD, // Iraqi dinar
    IRR, // Iranian rial
    ISK, // Icelandic króna (plural: krónur)
    JMD, // Jamaican dollar
    JOD, // Jordanian dinar
    JPY, // Japanese yen
    KES, // Kenyan shilling
    KGS, // Kyrgyzstani som
    KHR, // Cambodian riel
    KMF, // Comoro franc
    KPW, // North Korean won
    KRW, // South Korean won
    KWD, // Kuwaiti dinar
    KYD, // Cayman Islands dollar
    KZT, // Kazakhstani tenge
    LAK, // Lao kip
    LBP, // Lebanese pound
    LKR, // Sri Lankan rupee
    LRD, // Liberian dollar
    LSL, // Lesotho loti
    LYD, // Libyan dinar
    MAD, // Moroccan dirham
    MDL, // Moldovan leu
    MGA, // Malagasy ariary
    MKD, // Macedonian denar
    MMK, // Myanmar kyat
    MNT, // Mongolian tögrög
    MOP, // Macanese pataca
    MRU, // Mauritanian ouguiya
    MUR, // Mauritian rupee
    MVR, // Maldivian rufiyaa
    MWK, // Malawian kwacha
    MXN, // Mexican peso
    MXV, // Mexican Unidad de Inversion (UDI) (funds code)
    MYR, // Malaysian ringgit
    MZN, // Mozambican metical
    NAD, // Namibian dollar
    NGN, // Nigerian naira
    NIO, // Nicaraguan córdoba
    NOK, // Norwegian krone
    NPR, // Nepalese rupee
    NZD, // New Zealand dollar
    OMR, // Omani rial
    PAB, // Panamanian balboa
    PEN, // Peruvian sol
    PGK, // Papua New Guinean kina
    PHP, // Philippine peso[13]
    PKR, // Pakistani rupee
    PLN, // Polish złoty
    PYG, // Paraguayan guaraní
    QAR, // Qatari riyal
    RON, // Romanian leu
    RSD, // Serbian dinar
    RUB, // Russian ruble
    RWF, // Rwandan franc
    SAR, // Saudi riyal
    SBD, // Solomon Islands dollar
    SCR, // Seychelles rupee
    SDG, // Sudanese pound
    SEK, // Swedish krona (plural: kronor)
    SGD, // Singapore dollar
    SHP, // Saint Helena pound
    SLE, // Sierra Leonean leone
    SOS, // Somali shilling
    SRD, // Surinamese dollar
    SSP, // South Sudanese pound
    STN, // São Tomé and Príncipe dobra
    SVC, // Salvadoran colón
    SYP, // Syrian pound
    SZL, // Swazi lilangeni
    THB, // Thai baht
    TJS, // Tajikistani somoni
    TMT, // Turkmenistan manat
    TND, // Tunisian dinar
    TOP, // Tongan paʻanga
    TRY, // Turkish lira
    TTD, // Trinidad and Tobago dollar
    TWD, // New Taiwan dollar
    TZS, // Tanzanian shilling
    UAH, // Ukrainian hryvnia
    UGX, // Ugandan shilling
    USD, // United States dollar
    USN, // United States dollar (next day) (funds code)
    UYI, // Uruguay Peso en Unidades Indexadas (URUIURUI) (funds code)
    UYU, // Uruguayan peso
    UYW, // Unidad previsional[15]
    UZS, // Uzbekistan som
    VED, // Venezuelan bolívar digital[16]
    VES, // Venezuelan bolívar soberano[13]
    VND, // Vietnamese đồng
    VUV, // Vanuatu vatu
    WST, // Samoan tala
    XAF, // CFA franc BEAC
    XAG, // Silver (one troy ounce)
    XAU, // Gold (one troy ounce)
    XBA, // European Composite Unit (EURCO) (bond market unit)
    XBB, // European Monetary Unit (E.M.U.-6) (bond market unit)
    XBC, // European Unit of Account 9 (E.U.A.-9) (bond market unit)
    XBD, // European Unit of Account 17 (E.U.A.-17) (bond market unit)
    XCD, // East Caribbean dollar
    XDR, // Special drawing rights
    XOF, // CFA franc BCEAO
    XPD, // Palladium (one troy ounce)
    XPF, // CFP franc (franc Pacifique)
    XPT, // Platinum (one troy ounce)
    XSU, // SUCRE
    XTS, // Code reserved for testing
    XUA, // ADB Unit of Account
    XXX, // No currency
    YER, // Yemeni rial
    ZAR, // South African rand
    ZMW, // Zambian kwacha
    ZWL, // Zimbabwean dollar
}

impl Currency {
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

    pub const fn to_bytes(&self) -> &'static [u8] {
        match self {
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
