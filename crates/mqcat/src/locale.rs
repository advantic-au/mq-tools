//! Host terminal encoding detection.
//!
//! When `mqcat` produces from an interactive terminal, the bytes it reads are in
//! whatever encoding the console uses, not necessarily UTF-8. Two groups of
//! encodings matter and they need opposite handling:
//!
//! * Encodings that [`encoding_rs`] implements (UTF-8, the ANSI/Windows pages,
//!   and the CJK multi-byte pages). These are transcoded to UTF-8 and the
//!   message is tagged with CCSID 1208, so consumers read correct text.
//! * OEM/DOS code pages. IBM MQ converts these itself when the message carries
//!   the matching CCSID, and transcoding would discard that information. The
//!   bytes are therefore sent unchanged and the CCSID is set to the OEM page.
//!
//! The two groups are disjoint in practice: `encoding_rs` implements the WHATWG
//! Encoding Standard, which omits OEM pages, so an OEM code page never resolves
//! to an encoding here. Detection on Windows therefore checks the OEM pages
//! explicitly before falling back to the label lookup.

/// Encoding the host terminal is expected to use, or `None` when the bytes are
/// already UTF-8 or the encoding could not be determined.
///
/// `None` means "send the input unchanged", so an unrecognised locale never
/// causes a lossy conversion.
#[cfg(windows)]
#[must_use]
pub fn terminal_encoding() -> Option<&'static encoding_rs::Encoding> {
    use encoding_rs::{BIG5, EUC_KR, GBK, IBM866, SHIFT_JIS, UTF_8};

    let encoding = match console_code_page()? {
        65001 => UTF_8,
        866 => IBM866,
        874 => encoding_rs::WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1200 | 1201 => return None,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        _ => return None,
    };
    Some(encoding)
}

/// Encoding the host terminal is expected to use, or `None` when the bytes are
/// already UTF-8 or the encoding could not be determined.
///
/// On POSIX systems the locale environment variables are consulted in the same
/// order as the C library: `LC_ALL`, then `LC_CTYPE`, then `LANG`.
#[cfg(not(windows))]
#[must_use]
pub fn terminal_encoding() -> Option<&'static encoding_rs::Encoding> {
    let codeset = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(|value| codeset_from_locale(&value).map(str::to_owned))?;

    encoding_rs::Encoding::for_label_no_replacement(codeset.as_bytes())
}

/// Return whether `encoding` is UTF-8, in which case no conversion is needed.
#[must_use]
pub fn is_utf8(encoding: &encoding_rs::Encoding) -> bool {
    encoding == encoding_rs::UTF_8
}

/// MQ CCSID for an OEM/DOS code page, or `None` when the page is not one of the
/// OEM pages IBM MQ recognises.
///
/// These CCSIDs are passed through to the message descriptor so that the queue
/// manager performs the character-set conversion. The numbers are the code page
/// numbers, which IBM MQ uses directly for these pages.
#[must_use]
pub const fn oem_ccsid_for_code_page(code_page: u32) -> Option<i32> {
    match code_page {
        437 => Some(437),
        850 => Some(850),
        852 => Some(852),
        855 => Some(855),
        857 => Some(857),
        858 => Some(858),
        860 => Some(860),
        861 => Some(861),
        862 => Some(862),
        863 => Some(863),
        864 => Some(864),
        865 => Some(865),
        _ => None,
    }
}

/// Active console input code page on Windows.
///
/// Returns `None` on other platforms, where a console code page is not part of
/// the terminal model. POSIX users with OEM-encoded data set `--ccsid` instead.
#[cfg(windows)]
#[must_use]
pub fn console_code_page() -> Option<u32> {
    let code_page = unsafe { windows_sys::Win32::System::Console::GetConsoleCP() };
    (code_page != 0).then_some(code_page)
}

/// Active console input code page on Windows.
///
/// Returns `None` on other platforms, where a console code page is not part of
/// the terminal model. POSIX users with OEM-encoded data set `--ccsid` instead.
#[cfg(not(windows))]
#[must_use]
pub const fn console_code_page() -> Option<u32> {
    None
}

/// Extract the codeset from a POSIX locale value such as `en_GB.ISO-8859-1`.
///
/// A `@modifier` suffix is stripped before the codeset is taken. A value with no
/// codeset (`C`, `POSIX`, or an empty string) yields `None`, because such a
/// locale does not name an encoding.
fn codeset_from_locale(value: &str) -> Option<&str> {
    let without_modifier = value.split('@').next().unwrap_or_default();
    without_modifier
        .split_once('.')
        .map(|(_, codeset)| codeset)
        .filter(|codeset| !codeset.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_codeset_is_extracted() {
        assert_eq!(codeset_from_locale("en_GB.ISO-8859-1"), Some("ISO-8859-1"));
        assert_eq!(codeset_from_locale("de_DE.UTF-8"), Some("UTF-8"));
        assert_eq!(codeset_from_locale("ja_JP.eucJP"), Some("eucJP"));
    }

    #[test]
    fn locale_modifier_is_stripped() {
        // The `@euro` modifier is not part of the codeset name.
        assert_eq!(codeset_from_locale("de_DE.UTF-8@euro"), Some("UTF-8"));
    }

    #[test]
    fn locale_without_codeset_is_none() {
        assert_eq!(codeset_from_locale("C"), None);
        assert_eq!(codeset_from_locale("POSIX"), None);
        assert_eq!(codeset_from_locale(""), None);
        assert_eq!(codeset_from_locale("en_GB."), None);
    }

    #[test]
    fn utf8_is_recognised() {
        assert!(is_utf8(encoding_rs::UTF_8));
        assert!(!is_utf8(encoding_rs::WINDOWS_1252));
    }

    #[test]
    fn replacement_labels_are_not_treated_as_encodings() {
        // These labels resolve to the WHATWG replacement encoding, which would
        // turn every byte into U+FFFD. The locale path must not use them, so it
        // asks for labels without a replacement fallback.
        for label in [b"replacement".as_slice(), b"iso-2022-kr", b"hz-gb-2312"] {
            assert!(encoding_rs::Encoding::for_label(label).is_some());
            assert!(encoding_rs::Encoding::for_label_no_replacement(label).is_none());
        }
    }

    #[test]
    fn oem_code_pages_map_to_their_ccsid() {
        assert_eq!(oem_ccsid_for_code_page(437), Some(437));
        assert_eq!(oem_ccsid_for_code_page(850), Some(850));
        assert_eq!(oem_ccsid_for_code_page(865), Some(865));
    }

    #[test]
    fn ansi_code_pages_are_not_oem() {
        // These are handled by encoding_rs and transcoded to UTF-8 instead.
        assert_eq!(oem_ccsid_for_code_page(1252), None);
        assert_eq!(oem_ccsid_for_code_page(65001), None);
        assert_eq!(oem_ccsid_for_code_page(0), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_codeset_resolves_to_an_encoding() {
        // The ISO-8859-1 label maps to the WHATWG superset windows-1252.
        let encoding = encoding_rs::Encoding::for_label_no_replacement(b"ISO-8859-1").unwrap();
        assert_eq!(encoding, encoding_rs::WINDOWS_1252);
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_has_no_console_code_page() {
        assert_eq!(console_code_page(), None);
    }
}
