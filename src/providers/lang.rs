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

/// ISO 3166-1 alpha-2 → alpha-3, lowercased.
pub fn iso_3166_2_to_3(code: &str) -> String {
    let upper = code.trim().to_ascii_uppercase();

    let mapped = match upper.as_str() {
        "AE" => "are",
        "AR" => "arg",
        "AT" => "aut",
        "AU" => "aus",
        "BE" => "bel",
        "BG" => "bgr",
        "BR" => "bra",
        "CA" => "can",
        "CH" => "che",
        "CL" => "chl",
        "CN" => "chn",
        "CO" => "col",
        "CZ" => "cze",
        "DE" => "deu",
        "DK" => "dnk",
        "EE" => "est",
        "EG" => "egy",
        "ES" => "esp",
        "FI" => "fin",
        "FR" => "fra",
        "GB" => "gbr",
        "GR" => "grc",
        "HK" => "hkg",
        "HR" => "hrv",
        "HU" => "hun",
        "ID" => "idn",
        "IE" => "irl",
        "IL" => "isr",
        "IN" => "ind",
        "IR" => "irn",
        "IS" => "isl",
        "IT" => "ita",
        "JP" => "jpn",
        "KR" => "kor",
        "LT" => "ltu",
        "LU" => "lux",
        "LV" => "lva",
        "MX" => "mex",
        "MY" => "mys",
        "NL" => "nld",
        "NO" => "nor",
        "NZ" => "nzl",
        "PE" => "per",
        "PH" => "phl",
        "PL" => "pol",
        "PT" => "prt",
        "RO" => "rou",
        "RS" => "srb",
        "RU" => "rus",
        "SA" => "sau",
        "SE" => "swe",
        "SG" => "sgp",
        "SI" => "svn",
        "SK" => "svk",
        "TH" => "tha",
        "TR" => "tur",
        "TW" => "twn",
        "UA" => "ukr",
        "US" => "usa",
        "VN" => "vnm",
        "ZA" => "zaf",
        other => return other.to_ascii_lowercase(),
    };

    mapped.to_string()
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
    fn locales_reduce_to_their_language() {
        assert_eq!(base_language("fr-FR"), "fr");
        assert_eq!(base_language("pt_BR"), "pt");
        assert_eq!(base_language("en"), "en");
    }
}
