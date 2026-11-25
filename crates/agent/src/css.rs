//! Sanitizer untuk "CSS bebas terbatas" dari agen.
//!
//! Aturan:
//! - Hanya rule biasa, `@media`, dan `@keyframes`. At-rule lain ditolak.
//! - Selektor hanya karakter aman (tanpa escape `\`), lalu diberi awalan
//!   `.panel-root`; `:root`, `html`, `body` dipetakan ke `.panel-root`.
//! - Properti dari allowlist (plus custom property `--x`).
//! - Nilai: `url()`, `image-set()`, `expression`, `@`, dan blok `{}` ditolak;
//!   fungsi hanya dari allowlist. `position: fixed` ditolak.
//! - Output disusun ulang dari token, jadi komentar dan trik penutup hilang.
use cssparser::{Delimiter, ParseError, Parser, ToCss, Token};

use crate::design::MAX_CUSTOM_CSS;

pub const SCOPE: &str = ".panel-root";

const PROPERTIES: &[&str] = &[
    "color", "background", "background-color", "background-image", "background-size",
    "background-position", "background-repeat", "background-blend-mode", "opacity",
    "border", "border-top", "border-right", "border-bottom", "border-left", "border-color",
    "border-width", "border-style", "border-radius", "border-top-left-radius",
    "border-top-right-radius", "border-bottom-left-radius", "border-bottom-right-radius",
    "outline", "outline-color", "outline-offset", "outline-width", "outline-style",
    "box-shadow", "text-shadow", "filter", "backdrop-filter", "mix-blend-mode",
    "font", "font-family", "font-size", "font-weight", "font-style", "font-variant-numeric",
    "line-height", "letter-spacing", "text-align", "text-transform", "text-decoration",
    "text-overflow", "white-space", "word-break", "overflow-wrap",
    "margin", "margin-top", "margin-right", "margin-bottom", "margin-left", "margin-inline",
    "margin-block", "padding", "padding-top", "padding-right", "padding-bottom",
    "padding-left", "padding-inline", "padding-block", "gap", "row-gap", "column-gap",
    "width", "min-width", "max-width", "height", "min-height", "max-height", "aspect-ratio",
    "display", "flex", "flex-direction", "flex-wrap", "flex-grow", "flex-shrink",
    "flex-basis", "justify-content", "justify-items", "justify-self", "align-items",
    "align-content", "align-self", "place-items", "place-content", "place-self", "order",
    "grid", "grid-template-columns", "grid-template-rows", "grid-template-areas",
    "grid-area", "grid-column", "grid-row", "grid-auto-flow", "grid-auto-rows",
    "grid-auto-columns", "position", "top", "right", "bottom", "left", "inset", "z-index",
    "overflow", "overflow-x", "overflow-y", "visibility", "cursor", "transform",
    "transform-origin", "transition", "transition-property", "transition-duration",
    "transition-timing-function", "transition-delay", "animation", "animation-name",
    "animation-duration", "animation-timing-function", "animation-delay",
    "animation-iteration-count", "animation-direction", "animation-fill-mode",
    "accent-color", "caret-color", "scrollbar-color", "scrollbar-width", "list-style",
    "content", "vertical-align", "object-fit", "table-layout", "border-collapse",
    "border-spacing", "color-scheme",
];

const FUNCTIONS: &[&str] = &[
    "rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color-mix",
    "calc", "min", "max", "clamp", "var", "linear-gradient", "radial-gradient",
    "conic-gradient", "repeating-linear-gradient", "repeating-radial-gradient", "repeat",
    "minmax", "fit-content", "cubic-bezier", "steps", "translate", "translatex",
    "translatey", "scale", "scalex", "scaley", "rotate", "skew", "blur", "brightness",
    "contrast", "drop-shadow", "grayscale", "saturate", "sepia", "hue-rotate", "invert",
];

const SELECTOR_FUNCTIONS: &[&str] = &[
    "not", "is", "where", "has", "nth-child", "nth-last-child", "nth-of-type",
];

type Error = ParseError<String>;

fn fail<T>(_p: &Parser<'_>, msg: impl Into<String>) -> Result<T, Error> {
    Err(ParseError::custom(msg.into()))
}

/// Sanitasi CSS. Mengembalikan CSS bersih atau pesan error untuk agen.
pub fn sanitize(css: &str) -> Result<String, String> {
    if css.len() > MAX_CUSTOM_CSS {
        return Err("CSS khusus maksimal 8 KB.".into());
    }
    let mut parser = Parser::new(css);
    let mut out = String::new();
    rules(&mut parser, &mut out, 0).map_err(|e| match e.kind {
        cssparser::ParseErrorKind::Custom(msg) => format!("CSS ditolak: {msg}"),
        other => format!("CSS tidak valid: {other}"),
    })?;
    if out.len() > MAX_CUSTOM_CSS {
        return Err("CSS khusus maksimal 8 KB.".into());
    }
    Ok(out)
}

fn rules<'i>(p: &mut Parser<'i>, out: &mut String, depth: u8) -> Result<(), Error> {
    loop {
        p.skip_whitespace();
        if p.is_exhausted() {
            return Ok(());
        }
        let state = p.state();
        if let Token::AtKeyword(name) = p.next()?.clone() {
            let name = name.to_ascii_lowercase();
            match name.as_str() {
                "media" if depth == 0 => {
                    let prelude = p.parse_until_before(Delimiter::CurlyBracketBlock, |p| media_query(p))?;
                    p.expect_curly_bracket_block()?;
                    out.push_str(&format!("@media {prelude}{{"));
                    p.parse_nested_block(|p| rules(p, out, depth + 1))?;
                    out.push('}');
                }
                "keyframes" if depth == 0 => {
                    let ident = p.expect_ident_cloned()?;
                    if !simple_ident(&ident) {
                        return fail(p, "Nama keyframes tidak valid.");
                    }
                    p.expect_curly_bracket_block()?;
                    out.push_str(&format!("@keyframes {ident}{{"));
                    p.parse_nested_block(|p| keyframes(p, out))?;
                    out.push('}');
                }
                _ => return fail(p, format!("@{name} tidak diizinkan.")),
            }
            continue;
        }
        p.reset(&state);
        let start = p.position();
        p.parse_until_before(Delimiter::CurlyBracketBlock, |p| {
            while p.next_including_whitespace().is_ok() {}
            Ok::<_, Error>(())
        })?;
        let selector = scope_selector(p.slice_from(start)).or_else(|msg| fail(p, msg))?;
        p.expect_curly_bracket_block()?;
        out.push_str(&selector);
        out.push('{');
        p.parse_nested_block(|p| declarations(p, out))?;
        out.push('}');
    }
}

fn keyframes<'i>(p: &mut Parser<'i>, out: &mut String) -> Result<(), Error> {
    loop {
        p.skip_whitespace();
        if p.is_exhausted() {
            return Ok(());
        }
        let start = p.position();
        p.parse_until_before(Delimiter::CurlyBracketBlock, |p| {
            while let Ok(token) = p.next() {
                match token {
                    Token::Ident(i) if matches!(i.to_ascii_lowercase().as_str(), "from" | "to") => {}
                    Token::Percentage { .. } | Token::Comma => {}
                    _ => return fail(p, "Selektor keyframes hanya from, to, atau persen."),
                }
            }
            Ok(())
        })?;
        let selector = p.slice_from(start).trim().to_string();
        p.expect_curly_bracket_block()?;
        out.push_str(&selector);
        out.push('{');
        p.parse_nested_block(|p| declarations(p, out))?;
        out.push('}');
    }
}

fn media_query<'i>(p: &mut Parser<'i>) -> Result<String, Error> {
    let mut text = String::new();
    media_tokens(p, &mut text)?;
    Ok(text.trim().to_string())
}

fn media_tokens<'i>(p: &mut Parser<'i>, text: &mut String) -> Result<(), Error> {
    while let Ok(token) = p.next_including_whitespace() {
        match token.clone() {
            Token::ParenthesisBlock => {
                text.push('(');
                p.parse_nested_block(|p| media_tokens(p, text))?;
                text.push(')');
            }
            t @ (Token::Ident(_) | Token::Number { .. } | Token::Dimension { .. }
            | Token::Colon | Token::Comma | Token::WhiteSpace(_)
            | Token::Delim('<' | '>' | '=' | '/')) => text.push_str(&t.to_css_string()),
            _ => return fail(p, "Media query hanya boleh berisi fitur sederhana."),
        }
    }
    Ok(())
}

fn declarations<'i>(p: &mut Parser<'i>, out: &mut String) -> Result<(), Error> {
    loop {
        p.skip_whitespace();
        if p.is_exhausted() {
            return Ok(());
        }
        let name = match p.next()?.clone() {
            Token::Semicolon => continue,
            Token::Ident(name) => name.to_string(),
            _ => return fail(p, "Deklarasi harus diawali nama properti."),
        };
        let property = name.to_ascii_lowercase();
        let custom = property.starts_with("--") && simple_ident(&property[2..]);
        if !custom && !PROPERTIES.contains(&property.as_str()) {
            return fail(p, format!("Properti `{property}` tidak diizinkan."));
        }
        p.expect_colon()?;
        let value = p.parse_until_before(Delimiter::Semicolon, |p| {
            let mut value = String::new();
            value_tokens(p, &mut value, &property)?;
            Ok::<_, Error>(value.trim().to_string())
        })?;
        if value.is_empty() {
            return fail(p, format!("Nilai `{property}` kosong."));
        }
        if property == "position" && !matches!(value.as_str(), "static" | "relative" | "absolute" | "sticky") {
            return fail(p, "position hanya static, relative, absolute, atau sticky.");
        }
        out.push_str(&property);
        out.push(':');
        out.push_str(&value);
        out.push(';');
        if !p.is_exhausted() {
            p.expect_semicolon()?;
        }
    }
}

fn value_tokens<'i>(p: &mut Parser<'i>, out: &mut String, property: &str) -> Result<(), Error> {
    while let Ok(token) = p.next_including_whitespace() {
        let token = token.clone();
        match &token {
            Token::Function(name) => {
                let name = name.to_ascii_lowercase();
                if !FUNCTIONS.contains(&name.as_str()) {
                    return fail(p, format!("Fungsi `{name}()` tidak diizinkan."));
                }
                out.push_str(&name);
                out.push('(');
                p.parse_nested_block(|p| value_tokens(p, out, property))?;
                out.push(')');
            }
            Token::ParenthesisBlock => {
                out.push('(');
                p.parse_nested_block(|p| value_tokens(p, out, property))?;
                out.push(')');
            }
            Token::Ident(ident) => {
                let lower = ident.to_ascii_lowercase();
                if lower.contains("expression") || lower.starts_with("-moz-binding") {
                    return fail(p, "Nilai berbahaya ditolak.");
                }
                out.push_str(&token.to_css_string());
            }
            Token::QuotedString(text) => {
                let allowed = matches!(property, "font-family" | "content" | "grid-template-areas" | "font");
                if !allowed || text.chars().count() > 80 || text.chars().any(char::is_control) {
                    return fail(p, "Teks dalam CSS hanya untuk font-family, content, atau grid-template-areas (maks. 80 karakter).");
                }
                out.push_str(&token.to_css_string());
            }
            Token::Number { .. } | Token::Percentage { .. } | Token::Dimension { .. }
            | Token::Hash(_) | Token::IDHash(_) | Token::Comma | Token::WhiteSpace(_)
            | Token::Delim('/' | '+' | '-' | '*' | '!' | '.') => out.push_str(&token.to_css_string()),
            Token::Comment(_) => {}
            Token::UnquotedUrl(_) | Token::BadUrl(_) => return fail(p, "url() tidak diizinkan."),
            _ => return fail(p, format!("Token `{}` tidak diizinkan dalam nilai.", token.to_css_string())),
        }
    }
    Ok(())
}

fn simple_ident(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validasi selektor dan beri awalan `.panel-root`.
pub fn scope_selector(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 300 {
        return Err("Selektor kosong atau terlalu panjang.".into());
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || " \t\n_-.#:>+~*()[]=,\"'^$|%".contains(c);
    if !raw.chars().all(safe) {
        return Err(format!("Selektor `{raw}` berisi karakter yang tidak diizinkan."));
    }
    // Fungsi selektor hanya dari allowlist (`:hover` bukan fungsi, aman).
    let mut rest = raw;
    while let Some(pos) = rest.find('(') {
        let before = &rest[..pos];
        let name: String = before.chars().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<Vec<_>>().into_iter().rev().collect();
        if !SELECTOR_FUNCTIONS.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(format!("Selektor `:{name}()` tidak diizinkan."));
        }
        rest = &rest[pos + 1..];
    }
    let mut depth = 0i32;
    let mut parts = vec![String::new()];
    for c in raw.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return Err("Kurung selektor tidak seimbang.".into());
        }
        if c == ',' && depth == 0 {
            parts.push(String::new());
        } else {
            parts.last_mut().expect("part").push(c);
        }
    }
    if depth != 0 {
        return Err("Kurung selektor tidak seimbang.".into());
    }
    let scoped: Vec<String> = parts
        .iter()
        .map(|part| {
            let part = part.split_whitespace().collect::<Vec<_>>().join(" ");
            for root in [":root", "html", "body"] {
                if part == root {
                    return SCOPE.to_string();
                }
                if let Some(rest) = part.strip_prefix(&format!("{root} ")) {
                    return format!("{SCOPE} {rest}");
                }
            }
            format!("{SCOPE} {part}")
        })
        .collect();
    if scoped.iter().any(|s| s.trim() == SCOPE.to_string() + " ") {
        return Err("Selektor kosong.".into());
    }
    Ok(scoped.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_and_keeps_safe_css() {
        let out = sanitize(":root{--accent:#22d3ee} .card, body h2:hover { border-radius: 14px; box-shadow: 0 2px 8px rgba(0,0,0,.3) }\n@media (max-width: 640px) { .sidebar { display: none } }").unwrap();
        assert!(out.contains(".panel-root{--accent:#22d3ee;}"), "{out}");
        assert!(out.contains(".panel-root .card,.panel-root h2:hover{border-radius:14px;"), "{out}");
        assert!(out.contains("@media (max-width: 640px){.panel-root .sidebar{display:none;}}"), "{out}");
    }

    #[test]
    fn rejects_dangerous() {
        for css in [
            ".a{background:url(https://evil/x)}",
            ".a{background:url('https://evil/x')}",
            ".a{background:u\\72l(x)}",
            ".a{background-image:image-set('x' 1x)}",
            "@import 'x.css';",
            "@font-face{font-family:x;src:url(x)}",
            ".a{position:fixed;inset:0}",
            ".a{behavior:x}",
            ".a{width:expression(alert(1))}",
            ".a{color:red}} .b{",
            ".a\\,b{color:red}",
            "@media screen { @media print { .a{color:red} } }",
            ".a:nth-child(2) .b:-moz-any(x){color:red}",
        ] {
            assert!(sanitize(css).is_err(), "should reject: {css} -> {:?}", sanitize(css));
        }
    }

    #[test]
    fn strips_comments_and_allows_keyframes() {
        let out = sanitize("/* </style><script> */ @keyframes pulse { from { opacity: 1 } 50% { opacity: .4 } to { opacity: 1 } } .dot { animation: pulse 2s infinite }").unwrap();
        assert!(!out.contains("script"));
        assert!(out.starts_with("@keyframes pulse{from{opacity:1;}50%{opacity:0.4;}to{opacity:1;}}"), "{out}");
    }
}
