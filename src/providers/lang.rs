//! ISO code conversion.
//!
//! TMDB speaks ISO 639-1 (`en`) and ISO 3166-1 alpha-2 (`US`). Sonarr and Radarr
//! expect ISO 639-2/T (`eng`) and alpha-3 (`usa`). Unknown codes pass through
//! lowercased rather than being dropped: a wrong-looking code is more useful to
//! a client than a missing one.

/// ISO 639-1 → ISO 639-2/T.
pub fn iso_639_1_to_3(code: &str) -> String {
    let lower = code.trim().to_ascii_lowercase();

    let mapped = match lower.as_str() {
        "aa" => "aar",
        "ab" => "abk",
        "af" => "afr",
        "ak" => "aka",
        "am" => "amh",
        "ar" => "ara",
        "as" => "asm",
        "az" => "aze",
        "ba" => "bak",
        "be" => "bel",
        "bg" => "bul",
        "bm" => "bam",
        "bn" => "ben",
        "bo" => "bod",
        "br" => "bre",
        "bs" => "bos",
        "ca" => "cat",
        "ce" => "che",
        "co" => "cos",
        "cs" => "ces",
        "cy" => "cym",
        "da" => "dan",
        "de" => "deu",
        "dv" => "div",
        "dz" => "dzo",
        "el" => "ell",
        "en" => "eng",
        "eo" => "epo",
        "es" => "spa",
        "et" => "est",
        "eu" => "eus",
        "fa" => "fas",
        "ff" => "ful",
        "fi" => "fin",
        "fj" => "fij",
        "fo" => "fao",
        "fr" => "fra",
        "fy" => "fry",
        "ga" => "gle",
        "gd" => "gla",
        "gl" => "glg",
        "gn" => "grn",
        "gu" => "guj",
        "ha" => "hau",
        "he" => "heb",
        "hi" => "hin",
        "hr" => "hrv",
        "ht" => "hat",
        "hu" => "hun",
        "hy" => "hye",
        "id" => "ind",
        "ig" => "ibo",
        "is" => "isl",
        "it" => "ita",
        "iu" => "iku",
        "ja" => "jpn",
        "jv" => "jav",
        "ka" => "kat",
        "kk" => "kaz",
        "km" => "khm",
        "kn" => "kan",
        "ko" => "kor",
        "ku" => "kur",
        "ky" => "kir",
        "la" => "lat",
        "lb" => "ltz",
        "lo" => "lao",
        "lt" => "lit",
        "lv" => "lav",
        "mg" => "mlg",
        "mi" => "mri",
        "mk" => "mkd",
        "ml" => "mal",
        "mn" => "mon",
        "mr" => "mar",
        "ms" => "msa",
        "mt" => "mlt",
        "my" => "mya",
        "nb" => "nob",
        "ne" => "nep",
        "nl" => "nld",
        "nn" => "nno",
        "no" => "nor",
        "ny" => "nya",
        "or" => "ori",
        "pa" => "pan",
        "pl" => "pol",
        "ps" => "pus",
        "pt" => "por",
        "qu" => "que",
        "rm" => "roh",
        "ro" => "ron",
        "ru" => "rus",
        "rw" => "kin",
        "sa" => "san",
        "sd" => "snd",
        "se" => "sme",
        "si" => "sin",
        "sk" => "slk",
        "sl" => "slv",
        "sm" => "smo",
        "sn" => "sna",
        "so" => "som",
        "sq" => "sqi",
        "sr" => "srp",
        "ss" => "ssw",
        "st" => "sot",
        "su" => "sun",
        "sv" => "swe",
        "sw" => "swa",
        "ta" => "tam",
        "te" => "tel",
        "tg" => "tgk",
        "th" => "tha",
        "ti" => "tir",
        "tk" => "tuk",
        "tl" => "tgl",
        "tn" => "tsn",
        "to" => "ton",
        "tr" => "tur",
        "ts" => "tso",
        "tt" => "tat",
        "ug" => "uig",
        "uk" => "ukr",
        "ur" => "urd",
        "uz" => "uzb",
        "ve" => "ven",
        "vi" => "vie",
        "wo" => "wol",
        "xh" => "xho",
        "yi" => "yid",
        "yo" => "yor",
        "zh" => "zho",
        "zu" => "zul",
        other => other,
    };

    mapped.to_string()
}

/// The countries works are filed under, as ISO 3166-1 codes them — alpha-2,
/// then alpha-3 in lower case — with the English name TMDB gives each.
const COUNTRIES: &[(&str, &str, &str)] = &[
    ("AE", "are", "United Arab Emirates"),
    ("AR", "arg", "Argentina"),
    ("AT", "aut", "Austria"),
    ("AU", "aus", "Australia"),
    ("BA", "bih", "Bosnia and Herzegovina"),
    ("BD", "bgd", "Bangladesh"),
    ("BE", "bel", "Belgium"),
    ("BG", "bgr", "Bulgaria"),
    ("BO", "bol", "Bolivia"),
    ("BR", "bra", "Brazil"),
    ("BY", "blr", "Belarus"),
    ("CA", "can", "Canada"),
    ("CH", "che", "Switzerland"),
    ("CL", "chl", "Chile"),
    ("CN", "chn", "China"),
    ("CO", "col", "Colombia"),
    ("CR", "cri", "Costa Rica"),
    ("CU", "cub", "Cuba"),
    ("CY", "cyp", "Cyprus"),
    ("CZ", "cze", "Czech Republic"),
    ("DE", "deu", "Germany"),
    ("DK", "dnk", "Denmark"),
    ("DO", "dom", "Dominican Republic"),
    ("DZ", "dza", "Algeria"),
    ("EC", "ecu", "Ecuador"),
    ("EE", "est", "Estonia"),
    ("EG", "egy", "Egypt"),
    ("ES", "esp", "Spain"),
    ("FI", "fin", "Finland"),
    ("FR", "fra", "France"),
    ("GB", "gbr", "United Kingdom"),
    ("GE", "geo", "Georgia"),
    ("GR", "grc", "Greece"),
    ("GT", "gtm", "Guatemala"),
    ("HK", "hkg", "Hong Kong"),
    ("HR", "hrv", "Croatia"),
    ("HU", "hun", "Hungary"),
    ("ID", "idn", "Indonesia"),
    ("IE", "irl", "Ireland"),
    ("IL", "isr", "Israel"),
    ("IN", "ind", "India"),
    ("IQ", "irq", "Iraq"),
    ("IR", "irn", "Iran"),
    ("IS", "isl", "Iceland"),
    ("IT", "ita", "Italy"),
    ("JM", "jam", "Jamaica"),
    ("JO", "jor", "Jordan"),
    ("JP", "jpn", "Japan"),
    ("KE", "ken", "Kenya"),
    ("KR", "kor", "South Korea"),
    ("KZ", "kaz", "Kazakhstan"),
    ("LB", "lbn", "Lebanon"),
    ("LK", "lka", "Sri Lanka"),
    ("LT", "ltu", "Lithuania"),
    ("LU", "lux", "Luxembourg"),
    ("LV", "lva", "Latvia"),
    ("MA", "mar", "Morocco"),
    ("MK", "mkd", "North Macedonia"),
    ("MT", "mlt", "Malta"),
    ("MX", "mex", "Mexico"),
    ("MY", "mys", "Malaysia"),
    ("NG", "nga", "Nigeria"),
    ("NL", "nld", "Netherlands"),
    ("NO", "nor", "Norway"),
    ("NP", "npl", "Nepal"),
    ("NZ", "nzl", "New Zealand"),
    ("PE", "per", "Peru"),
    ("PH", "phl", "Philippines"),
    ("PK", "pak", "Pakistan"),
    ("PL", "pol", "Poland"),
    ("PR", "pri", "Puerto Rico"),
    ("PT", "prt", "Portugal"),
    ("PY", "pry", "Paraguay"),
    ("QA", "qat", "Qatar"),
    ("RO", "rou", "Romania"),
    ("RS", "srb", "Serbia"),
    ("RU", "rus", "Russia"),
    ("SA", "sau", "Saudi Arabia"),
    ("SE", "swe", "Sweden"),
    ("SG", "sgp", "Singapore"),
    ("SI", "svn", "Slovenia"),
    ("SK", "svk", "Slovakia"),
    ("SN", "sen", "Senegal"),
    ("TH", "tha", "Thailand"),
    ("TN", "tun", "Tunisia"),
    ("TR", "tur", "Turkey"),
    ("TW", "twn", "Taiwan"),
    ("UA", "ukr", "Ukraine"),
    ("US", "usa", "United States of America"),
    ("UY", "ury", "Uruguay"),
    ("VE", "ven", "Venezuela"),
    ("VN", "vnm", "Vietnam"),
    ("ZA", "zaf", "South Africa"),
];

/// ISO 3166-1 alpha-2 → alpha-3, lowercased. A code this does not know
/// passes through, lowercased.
pub fn iso_3166_2_to_3(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();
    COUNTRIES
        .iter()
        .find(|(two, _, _)| *two == upper)
        .map_or_else(
            || upper.to_ascii_lowercase(),
            |(_, three, _)| (*three).to_string(),
        )
}

/// ISO 3166-1 alpha-3 → alpha-2, upper case, as clients match countries;
/// nothing for one this does not know, rather than a guess.
pub fn iso_3166_3_to_2(code: &str) -> Option<&'static str> {
    let lower = code.trim().to_ascii_lowercase();
    COUNTRIES
        .iter()
        .find(|(_, three, _)| *three == lower)
        .map(|(two, _, _)| *two)
}

/// A country's name in English, as TMDB writes it, by its alpha-2 code: what
/// a Kodi document names a country of production by.
pub fn country_name(code: &str) -> Option<&'static str> {
    let upper = code.trim().to_ascii_uppercase();
    COUNTRIES
        .iter()
        .find(|(two, _, _)| *two == upper)
        .map(|(_, _, name)| *name)
}

/// The language part of a TMDB locale: `fr-FR` → `fr`.
pub fn base_language(locale: &str) -> &str {
    locale.split(['-', '_']).next().unwrap_or(locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_languages_map_to_iso_639_2t() {
        assert_eq!(iso_639_1_to_3("en"), "eng");
        assert_eq!(iso_639_1_to_3("fr"), "fra");
        assert_eq!(iso_639_1_to_3("ja"), "jpn");
        assert_eq!(iso_639_1_to_3("ZH"), "zho");
    }

    #[test]
    fn unknown_languages_pass_through_lowercased() {
        assert_eq!(iso_639_1_to_3("xx"), "xx");
        assert_eq!(iso_639_1_to_3("QQ"), "qq");
    }

    #[test]
    fn known_countries_map_to_alpha3() {
        assert_eq!(iso_3166_2_to_3("US"), "usa");
        assert_eq!(iso_3166_2_to_3("gb"), "gbr");
        assert_eq!(iso_3166_2_to_3("JP"), "jpn");
    }

    #[test]
    fn unknown_countries_pass_through_lowercased() {
        assert_eq!(iso_3166_2_to_3("ZZ"), "zz");
    }

    #[test]
    fn a_country_reads_back_from_alpha_3_and_is_named() {
        assert_eq!(iso_3166_3_to_2("usa"), Some("US"));
        assert_eq!(iso_3166_3_to_2(" GBR "), Some("GB"));
        assert_eq!(iso_3166_3_to_2("xyz"), None);
        assert_eq!(country_name("us"), Some("United States of America"));
        assert_eq!(country_name("KR"), Some("South Korea"));
        assert_eq!(country_name("ZZ"), None);
        // Every one both ways.
        for (two, three, _) in COUNTRIES {
            assert_eq!(iso_3166_2_to_3(two), *three);
            assert_eq!(iso_3166_3_to_2(three), Some(*two));
        }
    }

    #[test]
    fn locales_reduce_to_their_language() {
        assert_eq!(base_language("fr-FR"), "fr");
        assert_eq!(base_language("pt_BR"), "pt");
        assert_eq!(base_language("en"), "en");
    }
}
