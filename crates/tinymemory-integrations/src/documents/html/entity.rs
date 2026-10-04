//! HTML entity decoding.
//!
//! Covers the named entities that actually appear in prose plus the numeric
//! forms, and leaves anything else alone. An unrecognised entity is passed
//! through verbatim rather than dropped: a literal `&foo;` in the output is a
//! visible, fixable wart, whereas a silently deleted one is a hole in the text
//! nobody notices.

/// Decode HTML entities in `text`.
pub(super) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        // An entity is short; a '&' with no ';' within that window is a
        // literal ampersand, which is far more common than a malformed entity.
        // The window has to land on a char boundary, or slicing panics on a
        // multibyte character sitting across the 12-byte mark.
        let mut window = tail.len().min(12);
        while window > 0 && !tail.is_char_boundary(window) {
            window -= 1;
        }
        let Some(end) = tail[..window].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        match decode_one(&tail[1..end]) {
            Some(decoded) => out.push_str(&decoded),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Decode the body of a single entity — what sits between `&` and `;`.
fn decode_one(body: &str) -> Option<String> {
    if let Some(digits) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        let code = u32::from_str_radix(digits, 16).ok()?;
        return char::from_u32(code).map(String::from);
    }
    if let Some(digits) = body.strip_prefix('#') {
        let code = digits.parse::<u32>().ok()?;
        return char::from_u32(code).map(String::from);
    }
    let literal = match body {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" | "#39" => "'",
        "nbsp" => " ",
        "hellip" => "…",
        "mdash" => "—",
        "ndash" => "–",
        "lsquo" => "\u{2018}",
        "rsquo" => "\u{2019}",
        "ldquo" => "\u{201C}",
        "rdquo" => "\u{201D}",
        "copy" => "©",
        "reg" => "®",
        "trade" => "™",
        "deg" => "°",
        "middot" => "·",
        "bull" => "•",
        _ => return None,
    };
    Some(literal.to_string())
}

#[cfg(test)]
#[path = "entity_tests.rs"]
mod tests;
