//! Reading and editing the text drawn on a page.
//!
//! # What this does
//!
//! Form fields ([`super::forms`]) are structured data the PDF format knows
//! about. Everything else on a page — headings, paragraphs, the labels printed
//! beside those fields — is just drawing commands in a content stream. There is
//! no "text object" to look up; there is a sequence of operators that set a
//! font, move to a position, and show some bytes.
//!
//! So editing page text means three things:
//!
//! 1. Walking the content stream while tracking the graphics and text state, so
//!    every show-text operator can be given a position and a size.
//! 2. Decoding the bytes it shows into readable text, which depends on the
//!    font's encoding.
//! 3. Writing replacement text back in a way the same font can render.
//!
//! # What it does not do
//!
//! Text does not reflow. A run is edited where it sits, so making it much
//! longer will run it into whatever is drawn to its right. This matches how
//! every other simple PDF editor behaves, and is the honest consequence of
//! there being no paragraph structure in the file to reflow *with*.

use std::collections::{HashMap, HashSet};

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::pdf::document::{page_ids, resolve};

/// A stretch of text drawn by one show-text operator.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRun {
    /// Position of the operator within the page's content, and the handle the
    /// front end passes back to edit it.
    pub id: usize,
    pub page_index: usize,
    pub text: String,
    /// Bounding box in PDF user space, as `[x0, y0, x1, y1]`.
    pub rect: [f32; 4],
    pub font_size: f32,
    /// The resource name of the font, e.g. `F1`. Shown in the UI for context.
    pub font_name: String,
    /// False when the run's font cannot render replacement text directly, so
    /// an edit would fall back to redrawing in a substituted font.
    pub exact_edit: bool,
}

// ---------------------------------------------------------------------------
// Matrices
// ---------------------------------------------------------------------------

/// A PDF transformation matrix `[a b c d e f]`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Matrix {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Matrix {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn new(a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> Self {
        Self { a, b, c, d, e, f }
    }

    fn translation(tx: f32, ty: f32) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    /// `self × other`, in PDF's row-vector convention.
    fn then(self, other: Self) -> Self {
        Self {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// Glyph widths for Helvetica, used when a font declares none of its own.
///
/// Indexed from character 32 (space) to 126 (`~`). A wrong width only shifts a
/// run's reported box slightly; it never affects what is written to the file.
const HELVETICA_WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// The characters WinAnsiEncoding puts in 0x80–0x9F, where Latin-1 has controls.
const WIN_ANSI_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

/// Everything needed to measure, decode and re-encode text in one font.
struct FontInfo {
    /// Character code to width, already divided by 1000.
    widths: HashMap<u32, f32>,
    default_width: f32,
    /// Glyph extents above and below the baseline, as a fraction of font size.
    ascent: f32,
    descent: f32,
    /// True for Type0 fonts, whose codes are two bytes rather than one.
    two_byte: bool,
    /// Character code to the text it represents.
    to_unicode: HashMap<u32, String>,
    /// The reverse, for writing text back. Empty when the font cannot be
    /// written to directly.
    from_unicode: HashMap<char, u8>,
    /// True when the font program travels inside the PDF.
    ///
    /// Embedded fonts are almost always subsets holding only the glyphs the
    /// document already uses, so a character that has an encoding may still
    /// have no outline to draw. Non-embedded fonts come from the viewer and
    /// have the full repertoire.
    embedded: bool,
}

impl FontInfo {
    /// A stand-in for a font that could not be read, so a malformed resource
    /// costs accuracy rather than losing the whole page.
    fn fallback() -> Self {
        let mut widths = HashMap::new();
        let mut to_unicode = HashMap::new();
        let mut from_unicode = HashMap::new();

        for code in 32u32..=126 {
            let ch = char::from_u32(code).unwrap_or(' ');
            widths.insert(
                code,
                f32::from(HELVETICA_WIDTHS[code as usize - 32]) / 1000.0,
            );
            to_unicode.insert(code, ch.to_string());
            from_unicode.insert(ch, code as u8);
        }

        Self {
            widths,
            default_width: 0.5,
            ascent: 0.72,
            descent: -0.21,
            two_byte: false,
            to_unicode,
            from_unicode,
            embedded: false,
        }
    }

    fn width_of(&self, code: u32) -> f32 {
        self.widths
            .get(&code)
            .copied()
            .unwrap_or(self.default_width)
    }

    fn decode(&self, bytes: &[u8]) -> String {
        let mut out = String::new();
        for code in self.codes(bytes) {
            match self.to_unicode.get(&code) {
                Some(text) => out.push_str(text),
                // An unmapped code still occupies space; a placeholder keeps
                // the text the same length as what is drawn.
                None => out.push('\u{FFFD}'),
            }
        }
        out
    }

    /// Splits raw string bytes into character codes.
    fn codes(&self, bytes: &[u8]) -> Vec<u32> {
        if self.two_byte {
            bytes
                .chunks(2)
                .map(|pair| match pair {
                    [high, low] => u32::from(*high) << 8 | u32::from(*low),
                    [only] => u32::from(*only),
                    _ => 0,
                })
                .collect()
        } else {
            bytes.iter().map(|&byte| u32::from(byte)).collect()
        }
    }

    /// Encodes text for this font, or `None` if any character has no code.
    fn encode(&self, text: &str) -> Option<Vec<u8>> {
        if self.from_unicode.is_empty() {
            return None;
        }
        text.chars()
            .map(|ch| self.from_unicode.get(&ch).copied())
            .collect()
    }
}

fn read_font(doc: &Document, font: &Dictionary) -> FontInfo {
    let subtype = font
        .get(b"Subtype")
        .and_then(Object::as_name)
        .unwrap_or(b"");

    if subtype == b"Type0" {
        read_type0_font(doc, font)
    } else {
        read_simple_font(doc, font)
    }
}

/// Reads a font whose character codes are single bytes.
fn read_simple_font(doc: &Document, font: &Dictionary) -> FontInfo {
    let mut info = FontInfo {
        widths: HashMap::new(),
        default_width: 0.5,
        ascent: 0.72,
        descent: -0.21,
        two_byte: false,
        to_unicode: HashMap::new(),
        from_unicode: HashMap::new(),
        embedded: false,
    };

    // --- Encoding: which character each byte stands for ---
    let mut table: Vec<char> = (0..256u32)
        .map(|code| match code {
            0x80..=0x9F => WIN_ANSI_HIGH[code as usize - 0x80],
            _ => char::from_u32(code).unwrap_or('\u{FFFD}'),
        })
        .collect();

    if let Ok(encoding) = font.get(b"Encoding") {
        if let Ok(dict) = resolve(doc, encoding).as_dict() {
            apply_differences(doc, dict, &mut table);
        }
    }

    for (code, &ch) in table.iter().enumerate() {
        if ch != '\u{FFFD}' {
            info.to_unicode.insert(code as u32, ch.to_string());
            // First code wins, so a duplicate mapping cannot shadow the
            // ordinary one further up the table.
            info.from_unicode.entry(ch).or_insert(code as u8);
        }
    }

    // A /ToUnicode map, when present, is authoritative for reading.
    if let Some(map) = read_to_unicode(doc, font) {
        for (code, text) in map {
            info.to_unicode.insert(code, text);
        }
    }

    // --- Widths ---
    let first = font
        .get(b"FirstChar")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as u32;

    let widths = font
        .get(b"Widths")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_array().ok().cloned())
        .unwrap_or_default();

    for (offset, width) in widths.iter().enumerate() {
        if let Ok(width) = resolve(doc, width).as_float() {
            info.widths.insert(first + offset as u32, width / 1000.0);
        }
    }

    if info.widths.is_empty() {
        // Not embedded and not measured: Helvetica is the closest thing to a
        // neutral guess, and only affects the reported box.
        for code in 32u32..=126 {
            info.widths.insert(
                code,
                f32::from(HELVETICA_WIDTHS[code as usize - 32]) / 1000.0,
            );
        }
    }

    read_descriptor(doc, font, &mut info);
    info
}

/// Reads a composite font, whose codes are two bytes under Identity encoding.
fn read_type0_font(doc: &Document, font: &Dictionary) -> FontInfo {
    let mut info = FontInfo {
        widths: HashMap::new(),
        default_width: 1.0,
        ascent: 0.75,
        descent: -0.25,
        two_byte: true,
        to_unicode: HashMap::new(),
        // Left empty: writing to a subset font would need its glyph mapping
        // rebuilt, so edits to these runs are redrawn in a substitute font.
        from_unicode: HashMap::new(),
        embedded: false,
    };

    if let Some(map) = read_to_unicode(doc, font) {
        info.to_unicode = map;
    }

    let descendants = font
        .get(b"DescendantFonts")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_array().ok().cloned())
        .unwrap_or_default();

    let Some(descendant) = descendants
        .first()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_dict().ok().cloned())
    else {
        return info;
    };

    if let Some(default) = descendant.get(b"DW").ok().and_then(|w| w.as_float().ok()) {
        info.default_width = default / 1000.0;
    }

    if let Some(array) = descendant
        .get(b"W")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_array().ok().cloned())
    {
        read_cid_widths(doc, &array, &mut info);
    }

    read_descriptor(doc, &descendant, &mut info);
    info
}

/// Parses the `/W` array, which comes in `c [w1 w2 …]` and `cFirst cLast w`
/// forms, freely mixed.
fn read_cid_widths(doc: &Document, array: &[Object], info: &mut FontInfo) {
    let mut index = 0;
    while index < array.len() {
        let Ok(first) = resolve(doc, &array[index]).as_i64() else {
            break;
        };
        let Some(next) = array
            .get(index + 1)
            .map(|value| resolve(doc, value).clone())
        else {
            break;
        };

        match next {
            Object::Array(widths) => {
                for (offset, width) in widths.iter().enumerate() {
                    if let Ok(width) = resolve(doc, width).as_float() {
                        info.widths
                            .insert((first + offset as i64) as u32, width / 1000.0);
                    }
                }
                index += 2;
            }
            _ => {
                let (Ok(last), Some(Ok(width))) = (
                    next.as_i64(),
                    array
                        .get(index + 2)
                        .map(|value| resolve(doc, value).as_float()),
                ) else {
                    break;
                };
                for code in first..=last.min(first + 65_535) {
                    info.widths.insert(code as u32, width / 1000.0);
                }
                index += 3;
            }
        }
    }
}

fn read_descriptor(doc: &Document, font: &Dictionary, info: &mut FontInfo) {
    let Some(descriptor) = font
        .get(b"FontDescriptor")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_dict().ok().cloned())
    else {
        return;
    };

    if let Ok(ascent) = descriptor.get(b"Ascent").and_then(Object::as_float) {
        if ascent > 0.0 {
            info.ascent = ascent / 1000.0;
        }
    }
    if let Ok(descent) = descriptor.get(b"Descent").and_then(Object::as_float) {
        if descent < 0.0 {
            info.descent = descent / 1000.0;
        }
    }
    if let Ok(missing) = descriptor.get(b"MissingWidth").and_then(Object::as_float) {
        info.default_width = missing / 1000.0;
    }

    info.embedded =
        descriptor.has(b"FontFile") || descriptor.has(b"FontFile2") || descriptor.has(b"FontFile3");
}

/// Applies an `/Encoding` dictionary's `/Differences` array to a code table.
fn apply_differences(doc: &Document, encoding: &Dictionary, table: &mut [char]) {
    let Some(differences) = encoding
        .get(b"Differences")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_array().ok().cloned())
    else {
        return;
    };

    let mut code = 0usize;
    for entry in differences {
        match resolve(doc, &entry) {
            Object::Integer(start) => code = *start as usize,
            Object::Real(start) => code = *start as usize,
            Object::Name(name) => {
                if code < table.len() {
                    if let Some(ch) = glyph_name_to_char(name) {
                        table[code] = ch;
                    }
                }
                code += 1;
            }
            _ => {}
        }
    }
}

/// Resolves the glyph names that actually appear in `/Differences` arrays.
///
/// The full Adobe Glyph List runs to thousands of entries; the names below plus
/// the `uniXXXX` form cover what real documents use in practice.
fn glyph_name_to_char(name: &[u8]) -> Option<char> {
    let name = std::str::from_utf8(name).ok()?;

    // uniXXXX and uXXXX[XX] spell the code point out directly.
    if let Some(hex) = name.strip_prefix("uni").or_else(|| name.strip_prefix('u')) {
        if hex.len() >= 4 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        }
    }

    // A single letter or digit is its own name.
    let mut chars = name.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        if only.is_ascii_alphanumeric() {
            return Some(only);
        }
    }

    let named = match name {
        "space" => ' ',
        "exclam" => '!',
        "quotedbl" => '"',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "ampersand" => '&',
        "quotesingle" => '\'',
        "parenleft" => '(',
        "parenright" => ')',
        "asterisk" => '*',
        "plus" => '+',
        "comma" => ',',
        "hyphen" | "minus" => '-',
        "period" => '.',
        "slash" => '/',
        "zero" => '0',
        "one" => '1',
        "two" => '2',
        "three" => '3',
        "four" => '4',
        "five" => '5',
        "six" => '6',
        "seven" => '7',
        "eight" => '8',
        "nine" => '9',
        "colon" => ':',
        "semicolon" => ';',
        "less" => '<',
        "equal" => '=',
        "greater" => '>',
        "question" => '?',
        "at" => '@',
        "bracketleft" => '[',
        "backslash" => '\\',
        "bracketright" => ']',
        "asciicircum" => '^',
        "underscore" => '_',
        "grave" => '`',
        "braceleft" => '{',
        "bar" => '|',
        "braceright" => '}',
        "asciitilde" => '~',
        "quoteleft" | "quoteright" => '\'',
        "quotedblleft" => '\u{201C}',
        "quotedblright" => '\u{201D}',
        "endash" => '\u{2013}',
        "emdash" => '\u{2014}',
        "bullet" => '\u{2022}',
        "eacute" => '\u{00E9}',
        "egrave" => '\u{00E8}',
        "agrave" => '\u{00E0}',
        "ccedilla" => '\u{00E7}',
        "adieresis" => '\u{00E4}',
        "odieresis" => '\u{00F6}',
        "udieresis" => '\u{00FC}',
        "germandbls" => '\u{00DF}',
        "degree" => '\u{00B0}',
        "copyright" => '\u{00A9}',
        "registered" => '\u{00AE}',
        "trademark" => '\u{2122}',
        _ => return None,
    };
    Some(named)
}

/// Reads a `/ToUnicode` CMap into a code-to-text map.
fn read_to_unicode(doc: &Document, font: &Dictionary) -> Option<HashMap<u32, String>> {
    let stream = resolve(doc, font.get(b"ToUnicode").ok()?)
        .as_stream()
        .ok()?;
    let bytes = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    Some(parse_cmap(&String::from_utf8_lossy(&bytes)))
}

/// A token from a CMap section.
enum CMapToken {
    Hex(String),
    ArrayStart,
    ArrayEnd,
}

/// Splits a CMap section into hex strings and array brackets.
///
/// The brackets matter: a `bfrange` destination may be either a single value or
/// a bracketed list of them, and a parser that flattens the list loses track of
/// where one entry ends and the next begins — which silently corrupts every
/// mapping after the first list.
fn cmap_tokens(section: &str) -> Vec<CMapToken> {
    let mut tokens = Vec::new();
    let mut rest = section;

    while let Some(next) = rest.find(['<', '[', ']']) {
        match &rest[next..next + 1] {
            "[" => {
                tokens.push(CMapToken::ArrayStart);
                rest = &rest[next + 1..];
            }
            "]" => {
                tokens.push(CMapToken::ArrayEnd);
                rest = &rest[next + 1..];
            }
            _ => {
                let after = &rest[next + 1..];
                let Some(close) = after.find('>') else { break };
                tokens.push(CMapToken::Hex(after[..close].trim().to_string()));
                rest = &after[close + 1..];
            }
        }
    }
    tokens
}

/// Returns the text of every section between `marker` and `end`.
fn cmap_sections<'a>(text: &'a str, marker: &str, end: &str) -> Vec<&'a str> {
    let mut sections = Vec::new();
    let mut rest = text;

    while let Some(start) = rest.find(marker) {
        let after = &rest[start + marker.len()..];
        let section = match after.find(end) {
            Some(stop) => &after[..stop],
            None => after,
        };
        sections.push(section);
        rest = &after[section.len()..];
    }
    sections
}

/// Extracts the `bfchar` and `bfrange` mappings from a CMap.
///
/// This reads only what a `/ToUnicode` map needs rather than implementing
/// PostScript, but it does respect both `bfrange` destination forms.
fn parse_cmap(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();

    // bfchar: <src> <dst> pairs.
    for section in cmap_sections(text, "beginbfchar", "endbfchar") {
        let hex: Vec<String> = cmap_tokens(section)
            .into_iter()
            .filter_map(|token| match token {
                CMapToken::Hex(value) => Some(value),
                _ => None,
            })
            .collect();

        for pair in hex.chunks(2) {
            if let [source, destination] = pair {
                if let (Some(code), Some(text)) =
                    (hex_to_code(source), utf16_hex_to_text(destination))
                {
                    map.insert(code, text);
                }
            }
        }
    }

    // bfrange: <lo> <hi> <dst>, or <lo> <hi> [<dst> <dst> …].
    for section in cmap_sections(text, "beginbfrange", "endbfrange") {
        let tokens = cmap_tokens(section);
        let mut index = 0;

        while index < tokens.len() {
            let (Some(CMapToken::Hex(low)), Some(CMapToken::Hex(high))) =
                (tokens.get(index), tokens.get(index + 1))
            else {
                index += 1;
                continue;
            };
            let (Some(first), Some(last)) = (hex_to_code(low), hex_to_code(high)) else {
                index += 2;
                continue;
            };
            // A malformed file can name an enormous range; cap the work.
            let span = last.saturating_sub(first).min(65_535);

            match tokens.get(index + 2) {
                Some(CMapToken::Hex(destination)) => {
                    for offset in 0..=span {
                        if let Some(text) = utf16_hex_offset(destination, offset) {
                            map.insert(first + offset, text);
                        }
                    }
                    index += 3;
                }
                Some(CMapToken::ArrayStart) => {
                    let mut cursor = index + 3;
                    let mut offset = 0u32;

                    while let Some(CMapToken::Hex(destination)) = tokens.get(cursor) {
                        if let Some(text) = utf16_hex_to_text(destination) {
                            map.insert(first + offset, text);
                        }
                        cursor += 1;
                        offset += 1;
                    }
                    // Step past the closing bracket, if it is there.
                    index = match tokens.get(cursor) {
                        Some(CMapToken::ArrayEnd) => cursor + 1,
                        _ => cursor,
                    };
                }
                _ => index += 2,
            }
        }
    }

    map
}

fn hex_to_code(hex: &str) -> Option<u32> {
    u32::from_str_radix(hex, 16).ok()
}

/// Decodes a CMap destination and advances its last code unit by `offset`,
/// which is how a `bfrange` spells consecutive destinations.
fn utf16_hex_offset(hex: &str, offset: u32) -> Option<String> {
    let text = utf16_hex_to_text(hex)?;
    if offset == 0 {
        return Some(text);
    }

    let mut units: Vec<u16> = text.encode_utf16().collect();
    let last = units.last_mut()?;
    *last = last.checked_add(u16::try_from(offset).ok()?)?;
    String::from_utf16(&units).ok()
}

/// Decodes a hex string of UTF-16BE code units, which is how CMaps spell text.
fn utf16_hex_to_text(hex: &str) -> Option<String> {
    let units: Vec<u16> = hex
        .as_bytes()
        .chunks(4)
        .filter(|chunk| chunk.len() == 4)
        .filter_map(|chunk| u16::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok())
        .collect();

    if units.is_empty() {
        return None;
    }
    String::from_utf16(&units).ok()
}

// ---------------------------------------------------------------------------
// Walking the content stream
// ---------------------------------------------------------------------------

/// The text state carried between operators.
#[derive(Clone)]
struct TextState {
    font: Vec<u8>,
    size: f32,
    char_spacing: f32,
    word_spacing: f32,
    horizontal_scale: f32,
    leading: f32,
    rise: f32,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            font: Vec::new(),
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
        }
    }
}

/// A show-text operator found during the walk.
struct FoundRun {
    /// The stream it was drawn from; `None` is the page's own content.
    stream: Option<ObjectId>,
    /// Index of the operation within that stream's decoded content.
    operation: usize,
    text: String,
    rect: [f32; 4],
    font_size: f32,
    font_name: String,
    exact_edit: bool,
}

fn operand_float(operands: &[Object], index: usize) -> f32 {
    operands
        .get(index)
        .and_then(|value| value.as_float().ok())
        .unwrap_or(0.0)
}

/// What one pass over a page's content found.
#[derive(Default)]
struct Walked {
    runs: Vec<FoundRun>,
    /// Per font, the character codes the page already draws.
    ///
    /// For an embedded subset this is the best available evidence of which
    /// glyphs the font program actually contains.
    ///
    /// Keyed by the font object itself, not by the resource name that reached
    /// it: the same name means different fonts in different scopes, and the
    /// same font is often reached by different names. Pooling by object is the
    /// only grouping that matches what the font program actually contains.
    used_codes: HashMap<ObjectId, HashSet<u32>>,
}

/// How deep a chain of nested form XObjects is followed.
const MAX_XOBJECT_DEPTH: usize = 8;

/// The resources and fonts in effect for one content stream.
struct Scope {
    resources: Dictionary,
    fonts: HashMap<Vec<u8>, FontInfo>,
    /// The object each font name resolves to, for pooling used codes.
    font_ids: HashMap<Vec<u8>, ObjectId>,
}

impl Scope {
    fn new(doc: &Document, resources: Dictionary) -> Self {
        let declared = resources
            .get(b"Font")
            .ok()
            .map(|value| resolve(doc, value).clone())
            .and_then(|value| value.as_dict().ok().cloned())
            .unwrap_or_default();

        let mut fonts = HashMap::new();
        let mut font_ids = HashMap::new();

        for (name, value) in declared.iter() {
            if let Ok(id) = value.as_reference() {
                font_ids.insert(name.clone(), id);
            }
            let font = resolve(doc, value)
                .as_dict()
                .ok()
                .cloned()
                .unwrap_or_default();
            fonts.insert(name.clone(), read_font(doc, &font));
        }

        Self {
            resources,
            fonts,
            font_ids,
        }
    }
}

/// The resources a page draws with, including any it inherits.
fn page_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    let Ok((inline, referenced)) = doc.get_page_resources(page_id) else {
        return Dictionary::new();
    };

    let mut merged = Dictionary::new();
    // Referenced dictionaries come from ancestors; anything the page states
    // inline is nearer and wins.
    for id in referenced {
        if let Ok(dict) = doc.get_dictionary(id) {
            for (key, value) in dict.iter() {
                merged.set(key.clone(), value.clone());
            }
        }
    }
    if let Some(dict) = inline {
        for (key, value) in dict.iter() {
            merged.set(key.clone(), value.clone());
        }
    }
    merged
}

/// Walks a page's content, reporting every stretch of text it draws.
fn walk(doc: &Document, page_id: ObjectId) -> Walked {
    let Ok(content) = doc.get_and_decode_page_content(page_id) else {
        return Walked::default();
    };

    let scope = Scope::new(doc, page_resources(doc, page_id));
    let mut walked = Walked::default();
    let mut seen = Vec::new();

    walk_stream(
        doc,
        &content,
        &scope,
        None,
        Matrix::IDENTITY,
        0,
        &mut seen,
        &mut walked,
    );
    walked
}

/// Walks one content stream, descending into the form XObjects it draws.
///
/// `stream` names the object the operations live in, so an edit can be written
/// back to the right place. `None` means the page's own content.
#[allow(clippy::too_many_arguments)]
fn walk_stream(
    doc: &Document,
    content: &Content<Vec<Operation>>,
    scope: &Scope,
    stream: Option<ObjectId>,
    initial_ctm: Matrix,
    depth: usize,
    seen: &mut Vec<ObjectId>,
    walked: &mut Walked,
) {
    let mut ctm = initial_ctm;
    // The text state is part of the graphics state, so `q`/`Q` save and
    // restore it along with the matrix. Saving only the matrix left the font
    // from inside a `q` block in effect after the matching `Q`, which made
    // later text be read through the wrong font entirely: a run of ordinary
    // single-byte spaces came back as unmapped two-byte codes, and the label
    // beside it decoded to replacement characters.
    //
    // The text *matrix* is deliberately not saved: `BT` resets it, and the
    // spec does not place it in the graphics state.
    let mut stack: Vec<(Matrix, TextState)> = Vec::new();
    let mut text = TextState::default();
    let mut matrix = Matrix::IDENTITY;
    let mut line_matrix = Matrix::IDENTITY;
    let fallback = FontInfo::fallback();

    for (index, operation) in content.operations.iter().enumerate() {
        let operands = &operation.operands;

        match operation.operator.as_str() {
            "q" => stack.push((ctm, text.clone())),
            "Q" => {
                if let Some((saved_ctm, saved_text)) = stack.pop() {
                    ctm = saved_ctm;
                    text = saved_text;
                }
            }
            "cm" => {
                let next = Matrix::new(
                    operand_float(operands, 0),
                    operand_float(operands, 1),
                    operand_float(operands, 2),
                    operand_float(operands, 3),
                    operand_float(operands, 4),
                    operand_float(operands, 5),
                );
                ctm = next.then(ctm);
            }

            "Do" => {
                if depth < MAX_XOBJECT_DEPTH {
                    if let Some(name) = operands.first().and_then(|v| v.as_name().ok()) {
                        walk_xobject(doc, scope, name, ctm, depth, seen, walked);
                    }
                }
            }

            "BT" => {
                matrix = Matrix::IDENTITY;
                line_matrix = Matrix::IDENTITY;
            }

            "Tf" => {
                text.font = operands
                    .first()
                    .and_then(|value| value.as_name().ok())
                    .map(<[u8]>::to_vec)
                    .unwrap_or_default();
                text.size = operand_float(operands, 1);
            }
            "Tc" => text.char_spacing = operand_float(operands, 0),
            "Tw" => text.word_spacing = operand_float(operands, 0),
            "Tz" => text.horizontal_scale = operand_float(operands, 0) / 100.0,
            "TL" => text.leading = operand_float(operands, 0),
            "Ts" => text.rise = operand_float(operands, 0),

            "Td" => {
                line_matrix =
                    Matrix::translation(operand_float(operands, 0), operand_float(operands, 1))
                        .then(line_matrix);
                matrix = line_matrix;
            }
            "TD" => {
                text.leading = -operand_float(operands, 1);
                line_matrix =
                    Matrix::translation(operand_float(operands, 0), operand_float(operands, 1))
                        .then(line_matrix);
                matrix = line_matrix;
            }
            "Tm" => {
                line_matrix = Matrix::new(
                    operand_float(operands, 0),
                    operand_float(operands, 1),
                    operand_float(operands, 2),
                    operand_float(operands, 3),
                    operand_float(operands, 4),
                    operand_float(operands, 5),
                );
                matrix = line_matrix;
            }
            "T*" => {
                line_matrix = Matrix::translation(0.0, -text.leading).then(line_matrix);
                matrix = line_matrix;
            }

            "Tj" | "TJ" | "'" | "\"" => {
                // The quote operators start a new line before showing anything.
                if operation.operator == "'" || operation.operator == "\"" {
                    if operation.operator == "\"" {
                        text.word_spacing = operand_float(operands, 0);
                        text.char_spacing = operand_float(operands, 1);
                    }
                    line_matrix = Matrix::translation(0.0, -text.leading).then(line_matrix);
                    matrix = line_matrix;
                }

                let font = scope.fonts.get(&text.font).unwrap_or(&fallback);
                let shown = shown_parts(operation);
                if shown.is_empty() {
                    continue;
                }

                let mut label = String::new();
                let mut advance = 0.0f32;

                for part in &shown {
                    match part {
                        Shown::Text(bytes) => {
                            label.push_str(&font.decode(bytes));
                            advance += measure(bytes, font, &text);
                            if let Some(&id) = scope.font_ids.get(&text.font) {
                                walked
                                    .used_codes
                                    .entry(id)
                                    .or_default()
                                    .extend(font.codes(bytes));
                            }
                        }
                        // A kerning number nudges the pen without drawing.
                        Shown::Adjust(amount) => {
                            advance -= amount / 1000.0 * text.size * text.horizontal_scale;
                        }
                    }
                }

                if !label.trim().is_empty() {
                    let render = matrix.then(ctm);
                    let (x0, y0) = render.apply(0.0, text.rise + font.descent * text.size);
                    let (x1, y1) = render.apply(advance, text.rise + font.ascent * text.size);

                    walked.runs.push(FoundRun {
                        stream,
                        operation: index,
                        text: label,
                        rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)],
                        font_size: (text.size * render.d.abs().max(0.001)).abs(),
                        font_name: String::from_utf8_lossy(&text.font).into_owned(),
                        exact_edit: !font.from_unicode.is_empty(),
                    });
                }

                matrix = Matrix::translation(advance, 0.0).then(matrix);
            }

            _ => {}
        }
    }
}

/// Follows a `Do` into a form XObject.
///
/// Text drawn this way is ordinary page text to whoever is reading it — a
/// value stamped into a form, most often — so it has to be reachable. Image
/// XObjects have no text and are skipped.
fn walk_xobject(
    doc: &Document,
    scope: &Scope,
    name: &[u8],
    ctm: Matrix,
    depth: usize,
    seen: &mut Vec<ObjectId>,
    walked: &mut Walked,
) {
    let Some(xobjects) = scope
        .resources
        .get(b"XObject")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_dict().ok().cloned())
    else {
        return;
    };

    let Ok(id) = xobjects.get(name).and_then(Object::as_reference) else {
        return;
    };
    // A form that draws itself, directly or through a chain, would otherwise
    // recurse until the stack ran out.
    if seen.contains(&id) {
        return;
    }

    let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
        return;
    };
    let is_form = stream
        .dict
        .get(b"Subtype")
        .and_then(Object::as_name)
        .is_ok_and(|subtype| subtype == b"Form");
    if !is_form {
        return;
    }

    // The form's own matrix maps its space onto the space that drew it.
    let form_matrix = stream
        .dict
        .get(b"Matrix")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_array().ok().cloned())
        .filter(|values| values.len() == 6)
        .map(|values| {
            let at = |index: usize| {
                resolve(doc, &values[index])
                    .as_float()
                    .unwrap_or(if index == 0 || index == 3 { 1.0 } else { 0.0 })
            };
            Matrix::new(at(0), at(1), at(2), at(3), at(4), at(5))
        })
        .unwrap_or(Matrix::IDENTITY);

    let bytes = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let Ok(content) = Content::decode(&bytes) else {
        return;
    };

    // A form without its own resources inherits the ones that drew it.
    let inner_resources = stream
        .dict
        .get(b"Resources")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_dict().ok().cloned())
        .unwrap_or_else(|| scope.resources.clone());
    let inner = Scope::new(doc, inner_resources);

    seen.push(id);
    walk_stream(
        doc,
        &content,
        &inner,
        Some(id),
        form_matrix.then(ctm),
        depth + 1,
        seen,
        walked,
    );
    seen.pop();
}

/// A group of runs that read as one piece of text.
struct MergedRun {
    /// The stream the operations live in; `None` is the page's own content.
    stream: Option<ObjectId>,
    /// Every operation the group covers, in content order.
    operations: Vec<usize>,
    text: String,
    rect: [f32; 4],
    font_size: f32,
    font_name: String,
    exact_edit: bool,
}

/// Joins runs that a reader would see as a single stretch of text.
///
/// Producers routinely split one word across several show-text operators to
/// apply kerning — "MILEAGE" arrives as "MIL", "E", "AGE". Presenting those as
/// three separate things to edit would be an accurate description of the file
/// and a useless one for the person reading the page.
///
/// The rule is deterministic so that reading and editing group identically.
fn merge(runs: Vec<FoundRun>) -> Vec<MergedRun> {
    /// Gap between runs, as a fraction of font size, that reads as a space.
    ///
    /// Comfortably above the kerning adjustments producers apply within a word
    /// and below the width of a space in any ordinary face.
    const SPACE_GAP: f32 = 0.18;

    let mut merged: Vec<MergedRun> = Vec::new();

    for run in runs {
        let joins = merged.last().is_some_and(|last| {
            let gap = run.rect[0] - last.rect[2];
            let size = last.font_size.max(1.0);

            // Never across streams: the two would have to be edited in
            // different places.
            last.stream == run.stream
                && last.font_name == run.font_name
                && (last.font_size - run.font_size).abs() < 0.01
                && last.exact_edit == run.exact_edit
                // Same line, allowing for rounding in the baseline.
                && (last.rect[1] - run.rect[1]).abs() < size * 0.15
                // Adjacent: a small overlap is kerning, a small gap is a space
                // that was drawn as spacing rather than as a character.
                && gap > -size * 0.4
                && gap < size * 0.4
        });

        if joins {
            let last = merged.last_mut().expect("checked above");

            // A word space is often drawn as a gap rather than as a space
            // character. Without this, "SALES TAX" reads back as "SALESTAX" -
            // and would be written back that way too.
            let gap = run.rect[0] - last.rect[2];
            let separated = gap >= last.font_size.max(1.0) * SPACE_GAP;
            if separated
                && !last.text.ends_with(' ')
                && !run.text.starts_with(' ')
                && !last.text.is_empty()
            {
                last.text.push(' ');
            }

            last.operations.push(run.operation);
            last.text.push_str(&run.text);
            last.rect[1] = last.rect[1].min(run.rect[1]);
            last.rect[2] = run.rect[2].max(last.rect[2]);
            last.rect[3] = last.rect[3].max(run.rect[3]);
            continue;
        }

        merged.push(MergedRun {
            stream: run.stream,
            operations: vec![run.operation],
            text: run.text,
            rect: run.rect,
            font_size: run.font_size,
            font_name: run.font_name,
            exact_edit: run.exact_edit,
        });
    }

    merged
}

/// One piece of a show-text operator's operands.
enum Shown<'a> {
    Text(&'a [u8]),
    Adjust(f32),
}

/// Flattens a show-text operator's operands into drawable pieces.
fn shown_parts(operation: &Operation) -> Vec<Shown<'_>> {
    match operation.operator.as_str() {
        "Tj" => match operation.operands.first() {
            Some(Object::String(bytes, _)) => vec![Shown::Text(bytes)],
            _ => Vec::new(),
        },
        // The string is the last operand: ' takes only it, " takes two numbers
        // before it.
        "'" | "\"" => match operation.operands.last() {
            Some(Object::String(bytes, _)) => vec![Shown::Text(bytes)],
            _ => Vec::new(),
        },
        "TJ" => match operation.operands.first() {
            Some(Object::Array(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Object::String(bytes, _) => Some(Shown::Text(bytes)),
                    Object::Integer(value) => Some(Shown::Adjust(*value as f32)),
                    Object::Real(value) => Some(Shown::Adjust(*value)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The width of a string in unscaled text space.
fn measure(bytes: &[u8], font: &FontInfo, state: &TextState) -> f32 {
    let mut total = 0.0;

    for code in font.codes(bytes) {
        total += font.width_of(code) * state.size + state.char_spacing;
        // Word spacing applies to the single-byte code 32 only.
        if code == 32 && !font.two_byte {
            total += state.word_spacing;
        }
    }

    total * state.horizontal_scale
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

fn page_id_at(doc: &Document, index: usize) -> AppResult<ObjectId> {
    page_ids(doc)
        .get(index)
        .copied()
        .ok_or(AppError::PageOutOfRange(index))
}

/// Lists every stretch of text drawn on a page, in the order it is drawn.
pub fn list_text_runs(doc: &Document, page_index: usize) -> AppResult<Vec<TextRun>> {
    let page_id = page_id_at(doc, page_index)?;
    let walked = walk(doc, page_id);

    Ok(merge(walked.runs)
        .into_iter()
        .enumerate()
        .map(|(position, run)| TextRun {
            // The run's position in reading order, not an operator index.
            //
            // Traversal and merging are both deterministic, so an edit rebuilds
            // the same list and resolves this back to the same operators. An
            // operator index stopped working once text inside a form XObject
            // became addressable: two runs in different streams can share one.
            id: position,
            page_index,
            text: run.text,
            rect: run.rect,
            font_size: run.font_size,
            font_name: run.font_name,
            exact_edit: run.exact_edit,
        })
        .collect())
}

/// How an edit was applied, so the caller can say what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditOutcome {
    /// Rewritten in the document's own font; visually seamless.
    InPlace,
    /// Covered and redrawn in a substituted font, because the original could
    /// not spell the new text.
    Redrawn,
}

/// The decoded operations of the stream a run lives in.
fn stream_content(
    doc: &Document,
    page_id: ObjectId,
    stream: Option<ObjectId>,
) -> AppResult<Content<Vec<Operation>>> {
    let unreadable = || AppError::Render("This page's contents could not be read.".into());

    match stream {
        None => doc
            .get_and_decode_page_content(page_id)
            .map_err(|_| unreadable()),
        Some(id) => {
            let stream = doc
                .get_object(id)
                .and_then(Object::as_stream)
                .map_err(|_| unreadable())?;
            let bytes = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
            Content::decode(&bytes).map_err(|_| unreadable())
        }
    }
}

/// Writes operations back to the stream they came from.
fn write_stream_content(
    doc: &mut Document,
    page_id: ObjectId,
    stream: Option<ObjectId>,
    content: Content<Vec<Operation>>,
) -> AppResult<()> {
    let bytes = content
        .encode()
        .map_err(|error| AppError::Render(format!("Could not rewrite the page: {error}")))?;

    match stream {
        None => write_content(doc, page_id, bytes),
        Some(id) => {
            let Ok(existing) = doc.get_object_mut(id).and_then(Object::as_stream_mut) else {
                return Err(AppError::Render("That text could not be rewritten.".into()));
            };
            // The replacement is plain text, so any filter the original
            // carried no longer describes it.
            existing.dict.remove(b"Filter");
            existing.dict.remove(b"DecodeParms");
            existing.set_content(bytes);
            Ok(())
        }
    }
}

/// The resources and fonts available to the stream a run lives in.
fn stream_scope(doc: &Document, page_id: ObjectId, stream: Option<ObjectId>) -> Scope {
    let resources = match stream {
        None => page_resources(doc, page_id),
        Some(id) => doc
            .get_object(id)
            .and_then(Object::as_stream)
            .ok()
            .and_then(|stream| stream.dict.get(b"Resources").ok().cloned())
            .map(|value| resolve(doc, &value).clone())
            .and_then(|value| value.as_dict().ok().cloned())
            // A form without its own resources uses the page's.
            .unwrap_or_else(|| page_resources(doc, page_id)),
    };

    Scope::new(doc, resources)
}

/// Replaces the text of one run.
///
/// Where the run's own font can spell the replacement, the string is rewritten
/// in place and the result is indistinguishable from the original typesetting.
/// Where it cannot — a subset font with no glyph for a new character, or a
/// composite font whose glyph mapping we would have to rebuild — the old text
/// is removed and redrawn in Helvetica, which is visibly a substitution but
/// keeps the document readable.
pub fn set_text_run(
    doc: &mut Document,
    page_index: usize,
    run_id: usize,
    new_text: &str,
) -> AppResult<EditOutcome> {
    let page_id = page_id_at(doc, page_index)?;

    let walked = walk(doc, page_id);
    let used_codes = walked.used_codes;
    let merged = merge(walked.runs);

    let run = merged
        .get(run_id)
        .ok_or_else(|| AppError::Render("That text is no longer on the page.".into()))?;

    let content = stream_content(doc, page_id, run.stream)?;
    let scope = stream_scope(doc, page_id, run.stream);
    let (fonts, font_ids) = (scope.fonts, scope.font_ids);
    let first = run.operations[0];

    let writable = font_for_run(&content, &fonts, first).and_then(|font| {
        let bytes = font.encode(new_text)?;

        // An embedded font is a subset: having an encoding for a character
        // does not mean the font program carries its outline. The codes the
        // page already draws are the only evidence available that a glyph is
        // really there, so an edit is only written in place when it stays
        // within them.
        if font.embedded {
            let id = font_ids.get(&font_name_at(&content, first)).copied()?;
            let used = used_codes.get(&id)?;
            if !font.codes(&bytes).iter().all(|code| used.contains(code)) {
                return None;
            }
        }
        Some(bytes)
    });

    // lopdf parses an inline image into an operation holding a stream, which
    // does not survive being written back as inline-image syntax. Such a
    // stream is only ever appended to, never rewritten.
    let has_inline_image = content
        .operations
        .iter()
        .any(|operation| operation.operator == "BI");

    if has_inline_image {
        // The old text stays where it is, hidden under a patch.
        cover_and_redraw(doc, page_id, run, new_text, true)?;
        return Ok(EditOutcome::Redrawn);
    }

    let stream = run.stream;
    let operations_to_clear: Vec<usize> = run.operations.clone();
    let mut operations = content.operations;

    match writable {
        Some(bytes) => {
            replace_shown_text(&mut operations[first], bytes);

            // The group's remaining operators drew the rest of the old text.
            // Emptying them leaves the positioning they carry intact.
            for &index in &operations_to_clear[1..] {
                clear_shown_text(&mut operations[index]);
            }

            write_stream_content(doc, page_id, stream, Content { operations })?;
            Ok(EditOutcome::InPlace)
        }
        None => {
            // Delete the original text rather than hiding it. A patch would
            // have to guess the page's background colour, and would leave the
            // old words in the file for anyone who looked.
            for &index in &operations_to_clear {
                clear_shown_text(&mut operations[index]);
            }

            let redraw = MergedRun {
                stream,
                operations: operations_to_clear,
                text: String::new(),
                rect: run.rect,
                font_size: run.font_size,
                font_name: run.font_name.clone(),
                exact_edit: run.exact_edit,
            };

            write_stream_content(doc, page_id, stream, Content { operations })?;
            // Drawn onto the page itself: the run's rectangle is already in
            // page space, so it lands correctly whichever stream drew it.
            cover_and_redraw(doc, page_id, &redraw, new_text, false)?;
            Ok(EditOutcome::Redrawn)
        }
    }
}

/// The resource name of the font in effect at an operation.
fn font_name_at(content: &Content<Vec<Operation>>, operation: usize) -> Vec<u8> {
    let mut current = Vec::new();

    for op in content.operations.iter().take(operation) {
        if op.operator == "Tf" {
            current = op
                .operands
                .first()
                .and_then(|value| value.as_name().ok())
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
        }
    }
    current
}

/// Finds the font in effect at a given operation, by replaying the `Tf`
/// operators that precede it.
fn font_for_run<'a>(
    content: &Content<Vec<Operation>>,
    fonts: &'a HashMap<Vec<u8>, FontInfo>,
    operation: usize,
) -> Option<&'a FontInfo> {
    let mut current: Option<&[u8]> = None;

    for op in content.operations.iter().take(operation) {
        if op.operator == "Tf" {
            current = op.operands.first().and_then(|value| value.as_name().ok());
        }
    }

    fonts.get(current?)
}

/// Swaps the string a show-text operator draws, keeping the operator itself.
///
/// A `TJ` array collapses to a single string: its numbers are kerning between
/// the old glyphs, which means nothing once the text has changed.
fn replace_shown_text(operation: &mut Operation, bytes: Vec<u8>) {
    let replacement = Object::String(bytes, StringFormat::Literal);

    match operation.operator.as_str() {
        "TJ" => {
            operation.operands = vec![Object::Array(vec![replacement])];
        }
        "'" | "\"" => {
            if let Some(last) = operation.operands.last_mut() {
                *last = replacement;
            }
        }
        _ => operation.operands = vec![replacement],
    }
}

/// Empties a show-text operator without removing it.
///
/// The operator is kept because `'` and `\"` also start a new line, and
/// dropping one would move everything drawn after it.
fn clear_shown_text(operation: &mut Operation) {
    let empty = Object::String(Vec::new(), StringFormat::Literal);

    match operation.operator.as_str() {
        "TJ" => operation.operands = vec![Object::Array(Vec::new())],
        "'" | "\"" => {
            if let Some(last) = operation.operands.last_mut() {
                *last = empty;
            }
        }
        _ => operation.operands = vec![empty],
    }
}

/// Draws replacement text in Helvetica where a run used to be.
///
/// `cover` paints a white patch first, for the one case where the original
/// text could not be deleted from the content stream.
fn cover_and_redraw(
    doc: &mut Document,
    page_id: ObjectId,
    run: &MergedRun,
    new_text: &str,
    cover: bool,
) -> AppResult<()> {
    ensure_helvetica(doc, page_id)?;

    let [x0, y0, x1, y1] = run.rect;
    let size = if run.font_size > 0.5 {
        run.font_size
    } else {
        10.0
    };
    // Sit the baseline where the original text sat, not on the box floor.
    let baseline = y0 + (y1 - y0) * 0.22;

    let mut operations = vec![Operation::new("q", vec![])];

    if cover {
        // White is the best available guess. Over shading or an image this
        // patch will be visible - which is why it is the last resort.
        operations.extend([
            Operation::new("g", vec![Object::Real(1.0)]),
            Operation::new(
                "re",
                vec![
                    Object::Real(x0 - 0.5),
                    Object::Real(y0 - 0.5),
                    Object::Real(x1 - x0 + 1.0),
                    Object::Real(y1 - y0 + 1.0),
                ],
            ),
            Operation::new("f", vec![]),
        ]);
    }

    operations.extend([
        Operation::new("BT", vec![]),
        Operation::new(
            "Tf",
            vec![
                Object::Name(HELVETICA_RESOURCE.to_vec()),
                Object::Real(size),
            ],
        ),
        Operation::new("g", vec![Object::Real(0.0)]),
        Operation::new("Td", vec![Object::Real(x0), Object::Real(baseline)]),
        Operation::new(
            "Tj",
            vec![Object::String(
                encode_win_ansi(new_text),
                StringFormat::Literal,
            )],
        ),
        Operation::new("ET", vec![]),
        Operation::new("Q", vec![]),
    ]);

    doc.add_to_page_content(page_id, Content { operations })
        .map_err(|error| AppError::Render(format!("Could not update the page: {error}")))?;
    Ok(())
}

/// Encodes text for the substituted Helvetica, dropping what it cannot spell.
fn encode_win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| if (ch as u32) < 256 { ch as u8 } else { b'?' })
        .collect()
}

/// The resource name the redraw path uses for its substituted font.
const HELVETICA_RESOURCE: &[u8] = b"NxHelv";

/// Adds Helvetica to a page's resources, returning the font object.
fn ensure_helvetica(doc: &mut Document, page_id: ObjectId) -> AppResult<ObjectId> {
    let font_id = doc.add_object(Object::Dictionary(
        [
            ("Type", Object::Name(b"Font".to_vec())),
            ("Subtype", Object::Name(b"Type1".to_vec())),
            ("BaseFont", Object::Name(b"Helvetica".to_vec())),
            ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
        ]
        .into_iter()
        .map(|(key, value)| (key.as_bytes().to_vec(), value))
        .collect::<Dictionary>(),
    ));

    let existing = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| page.get(b"Resources").ok().cloned());

    let mut resources = match existing {
        Some(Object::Dictionary(dict)) => dict,
        Some(Object::Reference(id)) => doc.get_dictionary(id).cloned().unwrap_or_default(),
        _ => Dictionary::new(),
    };

    let mut font_dict = resources
        .get(b"Font")
        .ok()
        .map(|value| resolve(doc, value).clone())
        .and_then(|value| value.as_dict().ok().cloned())
        .unwrap_or_default();

    font_dict.set(HELVETICA_RESOURCE.to_vec(), Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(font_dict));

    if let Ok(page) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
        page.set("Resources", Object::Dictionary(resources));
    }

    Ok(font_id)
}

/// Replaces a page's content with a single stream holding `bytes`.
fn write_content(doc: &mut Document, page_id: ObjectId, bytes: Vec<u8>) -> AppResult<()> {
    let stream_id = doc.add_object(Object::Stream(Stream::new(Dictionary::new(), bytes)));

    if let Ok(page) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
        page.set("Contents", Object::Reference(stream_id));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::document::blank;

    /// Builds a one-page document whose content stream is `body`, drawing in
    /// Helvetica under the resource name `F1`.
    fn document_drawing(body: &str) -> Document {
        let mut doc = blank().expect("blank");
        let page_id = page_ids(&doc)[0];

        let font_id = doc.add_object(Object::Dictionary(
            [
                ("Type", Object::Name(b"Font".to_vec())),
                ("Subtype", Object::Name(b"Type1".to_vec())),
                ("BaseFont", Object::Name(b"Helvetica".to_vec())),
                ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
            ]
            .into_iter()
            .map(|(key, value)| (key.as_bytes().to_vec(), value))
            .collect::<Dictionary>(),
        ));

        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("Font", Object::Dictionary(fonts));

        let content_id = doc.add_object(Object::Stream(Stream::new(
            Dictionary::new(),
            body.as_bytes().to_vec(),
        )));

        let page = doc
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .expect("page");
        page.set("Resources", Object::Dictionary(resources));
        page.set("Contents", Object::Reference(content_id));

        doc
    }

    #[test]
    fn a_single_run_is_found_with_its_position() {
        let doc = document_drawing("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hello");
        assert!((runs[0].rect[0] - 72.0).abs() < 0.01, "{:?}", runs[0].rect);
        assert!((runs[0].font_size - 12.0).abs() < 0.01);
    }

    #[test]
    fn adjacent_runs_in_one_font_read_as_one() {
        // "MIL" "E" "AGE" split across operators, as kerning produces.
        let doc = document_drawing("BT /F1 12 Tf 72 700 Td (MIL) Tj (E) Tj (AGE) Tj ET");
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "MILEAGE");
    }

    #[test]
    fn runs_on_different_lines_stay_separate() {
        let doc = document_drawing("BT /F1 12 Tf 72 700 Td (First) Tj 0 -20 Td (Second) Tj ET");
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "First");
        assert_eq!(runs[1].text, "Second");
    }

    #[test]
    fn a_gap_wide_enough_to_be_a_space_becomes_one() {
        // Td offsets from the start of the line, not from the pen: "SALES" is
        // 38.7pt wide at 12pt, so 42 leaves a 3.3pt gap — a word space, and
        // narrower than the limit past which runs stop merging at all.
        let doc = document_drawing("BT /F1 12 Tf 72 700 Td (SALES) Tj 42 0 Td (TAX) Tj ET");
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "SALES TAX");
    }

    #[test]
    fn editing_rewrites_the_text_in_place() {
        let mut doc = document_drawing("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        let id = list_text_runs(&doc, 0).unwrap()[0].id;

        let outcome = set_text_run(&mut doc, 0, id, "Goodbye").expect("edit");
        assert_eq!(outcome, EditOutcome::InPlace);

        let runs = list_text_runs(&doc, 0).expect("runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Goodbye");
    }

    #[test]
    fn editing_a_merged_run_replaces_all_of_it() {
        let mut doc = document_drawing("BT /F1 12 Tf 72 700 Td (MIL) Tj (E) Tj (AGE) Tj ET");
        let id = list_text_runs(&doc, 0).unwrap()[0].id;

        set_text_run(&mut doc, 0, id, "ODOMETER").expect("edit");

        let runs = list_text_runs(&doc, 0).expect("runs");
        assert_eq!(runs.len(), 1, "leftover fragments: {runs:?}");
        assert_eq!(runs[0].text, "ODOMETER");
    }

    #[test]
    fn a_tj_array_collapses_to_the_new_text() {
        let mut doc = document_drawing("BT /F1 12 Tf 72 700 Td [(A) -120 (B)] TJ ET");
        assert_eq!(list_text_runs(&doc, 0).unwrap()[0].text, "AB");

        let id = list_text_runs(&doc, 0).unwrap()[0].id;
        set_text_run(&mut doc, 0, id, "CD").expect("edit");

        assert_eq!(list_text_runs(&doc, 0).unwrap()[0].text, "CD");
    }

    #[test]
    fn an_unknown_run_is_refused() {
        let mut doc = document_drawing("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        assert!(set_text_run(&mut doc, 0, 9999, "nope").is_err());
    }

    #[test]
    fn editing_leaves_other_text_untouched() {
        let mut doc = document_drawing("BT /F1 12 Tf 72 700 Td (First) Tj 0 -20 Td (Second) Tj ET");
        let id = list_text_runs(&doc, 0).unwrap()[0].id;

        set_text_run(&mut doc, 0, id, "Changed").expect("edit");

        let runs = list_text_runs(&doc, 0).expect("runs");
        assert_eq!(runs[0].text, "Changed");
        assert_eq!(runs[1].text, "Second");
    }

    #[test]
    fn text_state_operators_do_not_break_the_walk() {
        let doc = document_drawing(
            "BT /F1 10 Tf 2 Tc 1 Tw 100 Tz 14 TL 72 700 Td (Spaced out) Tj T* (Next line) Tj ET",
        );
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "Spaced out");
        assert_eq!(runs[1].text, "Next line");
    }

    /// `q`/`Q` save and restore the text state, not just the matrix.
    ///
    /// A real form set a two-byte font inside a `q` block, then drew ordinary
    /// single-byte text after the matching `Q` without naming a font again.
    /// Keeping the inner font in effect read that text through the wrong one
    /// entirely, and a label beside it decoded to replacement characters.
    #[test]
    fn the_font_set_inside_a_q_block_does_not_outlive_it() {
        let doc = document_drawing(concat!(
            "BT /F1 20 Tf 72 700 Td (Outer) Tj ET ",
            "q BT /F1 8 Tf 72 680 Td (Inner) Tj ET Q ",
            "BT 72 660 Td (After) Tj ET",
        ));
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 3);
        assert!((runs[0].font_size - 20.0).abs() < 0.01, "{:?}", runs[0]);
        assert!((runs[1].font_size - 8.0).abs() < 0.01, "{:?}", runs[1]);
        assert!(
            (runs[2].font_size - 20.0).abs() < 0.01,
            "the size from inside the block leaked out: {:?}",
            runs[2]
        );
    }

    #[test]
    fn character_spacing_is_restored_with_the_graphics_state() {
        // Wide spacing inside the block must not widen the run after it.
        let doc = document_drawing(concat!(
            "q BT /F1 10 Tf 20 Tc 72 700 Td (AB) Tj ET Q ",
            "BT /F1 10 Tf 72 680 Td (AB) Tj ET",
        ));
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 2);
        let spaced = runs[0].rect[2] - runs[0].rect[0];
        let plain = runs[1].rect[2] - runs[1].rect[0];
        assert!(spaced > plain + 30.0, "spaced={spaced} plain={plain}");
    }

    /// An unbalanced `Q` appears in real files; it must not panic or reset the
    /// state to something arbitrary.
    #[test]
    fn a_stray_restore_is_survivable() {
        let doc = document_drawing("Q BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        let runs = list_text_runs(&doc, 0).expect("runs");

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "Hello");
    }

    #[test]
    fn a_page_with_no_text_yields_nothing() {
        let doc = document_drawing("0 0 1 RG 10 10 m 100 100 l S");
        assert!(list_text_runs(&doc, 0).unwrap().is_empty());
    }

    #[test]
    fn a_missing_page_is_refused() {
        let doc = document_drawing("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        assert!(list_text_runs(&doc, 7).is_err());
    }

    // --- CMap parsing -----------------------------------------------------

    #[test]
    fn bfchar_entries_are_read() {
        let map = parse_cmap("beginbfchar <0041> <0061> <0042> <0062> endbfchar");
        assert_eq!(map.get(&0x41).map(String::as_str), Some("a"));
        assert_eq!(map.get(&0x42).map(String::as_str), Some("b"));
    }

    #[test]
    fn a_bfrange_walks_its_destination() {
        let map = parse_cmap("beginbfrange <0003> <0005> <0041> endbfrange");
        assert_eq!(map.get(&3).map(String::as_str), Some("A"));
        assert_eq!(map.get(&4).map(String::as_str), Some("B"));
        assert_eq!(map.get(&5).map(String::as_str), Some("C"));
    }

    /// A bracketed destination list is the form that, when flattened, silently
    /// corrupts every mapping after it — which showed up as a title reading
    /// "BILL OF S!LE".
    #[test]
    fn a_bfrange_list_maps_each_entry_and_keeps_its_place() {
        let map = parse_cmap(
            "beginbfrange <0003> <0005> [<0058> <0059> <005A>] <0010> <0011> <0041> endbfrange",
        );

        assert_eq!(map.get(&3).map(String::as_str), Some("X"));
        assert_eq!(map.get(&4).map(String::as_str), Some("Y"));
        assert_eq!(map.get(&5).map(String::as_str), Some("Z"));
        // The entry after the list must still line up.
        assert_eq!(map.get(&0x10).map(String::as_str), Some("A"));
        assert_eq!(map.get(&0x11).map(String::as_str), Some("B"));
    }

    #[test]
    fn glyph_names_resolve_to_characters() {
        assert_eq!(glyph_name_to_char(b"space"), Some(' '));
        assert_eq!(glyph_name_to_char(b"A"), Some('A'));
        assert_eq!(glyph_name_to_char(b"seven"), Some('7'));
        assert_eq!(glyph_name_to_char(b"uni20AC"), Some('\u{20AC}'));
        assert_eq!(glyph_name_to_char(b"nosuchglyph"), None);
    }

    // --- Matrices ---------------------------------------------------------

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let matrix = Matrix::new(2.0, 0.0, 0.0, 3.0, 10.0, 20.0);
        assert_eq!(matrix.then(Matrix::IDENTITY), matrix);
    }

    #[test]
    fn scaling_then_translating_applies_in_order() {
        let scaled = Matrix::new(2.0, 0.0, 0.0, 2.0, 0.0, 0.0);
        let moved = Matrix::translation(5.0, 7.0);

        let (x, y) = scaled.then(moved).apply(1.0, 1.0);
        assert!((x - 7.0).abs() < 0.001, "{x}");
        assert!((y - 9.0).abs() < 0.001, "{y}");
    }
}
