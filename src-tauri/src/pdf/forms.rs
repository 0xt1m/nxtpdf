//! AcroForm support: discovery, filling, and field creation.
//!
//! # Appearance streams
//!
//! When a field value changes we set the value and flip the form's
//! `/NeedAppearances` flag to true, which asks the viewer to regenerate the
//! field's visual appearance from the value. This is correct per the spec and
//! is what every mainstream viewer honors.
//!
//! The tradeoff: the appearance is *not* baked into the file, so a renderer
//! that ignores `/NeedAppearances` shows the old visuals. Flattening a form to
//! static content requires generating real `/AP` streams — tracked separately
//! and deliberately out of scope for this draft.

use std::collections::HashMap;

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::pdf::document::{catalog_id, page_ids, resolve};

/// Guards against a malformed file whose field tree forms a cycle.
const MAX_FIELD_DEPTH: usize = 32;

// Field flag bits from the PDF spec (1-based bit numbers in the spec, so the
// shift is one less than the documented bit position).
const FLAG_READ_ONLY: i64 = 1 << 0; // bit 1
const FLAG_REQUIRED: i64 = 1 << 1; // bit 2
const FLAG_MULTILINE: i64 = 1 << 12; // bit 13, /Tx only
const FLAG_PASSWORD: i64 = 1 << 13; // bit 14, /Tx only
const FLAG_PUSHBUTTON: i64 = 1 << 16; // bit 17, /Btn only
const FLAG_RADIO: i64 = 1 << 15; // bit 16, /Btn only
const FLAG_COMBO: i64 = 1 << 17; // bit 18, /Ch only

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    PushButton,
    Choice,
    Signature,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    /// Fully qualified name (`parent.child`), the identifier used to set values.
    pub name: String,
    pub kind: FieldKind,
    /// Current value rendered as text. `None` when unset.
    pub value: Option<String>,
    /// 0-based page the first widget sits on, if it could be resolved.
    pub page_index: Option<usize>,
    /// Widget rectangle `[x0, y0, x1, y1]` in PDF user space (origin bottom-left).
    pub rect: Option<[f32; 4]>,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
    pub password: bool,
    pub max_length: Option<i64>,
    /// Text size in points. `Some(0.0)` means auto-size: the viewer shrinks
    /// the text to fit the box. `None` means the field declares no appearance.
    pub font_size: Option<f32>,
    /// Selectable values: choice options, or a checkbox/radio's "on" states.
    pub options: Vec<String>,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn acro_form_dict(doc: &Document) -> Option<&Dictionary> {
    let catalog = doc.get_dictionary(catalog_id(doc).ok()?).ok()?;
    let form = catalog.get(b"AcroForm").ok()?;
    resolve(doc, form).as_dict().ok()
}

pub fn has_acro_form(doc: &Document) -> bool {
    acro_form_dict(doc)
        .and_then(|form| form.get(b"Fields").ok())
        .and_then(|fields| resolve(doc, fields).as_array().ok())
        .is_some_and(|fields| !fields.is_empty())
}

/// Resolves an attribute that a field may inherit from an ancestor field.
fn field_attr(doc: &Document, field_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = field_id;

    for _ in 0..MAX_FIELD_DEPTH {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(resolve(doc, value).clone());
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => return None,
        }
    }
    None
}

fn field_flags(doc: &Document, field_id: ObjectId) -> i64 {
    field_attr(doc, field_id, b"Ff")
        .and_then(|object| object.as_i64().ok())
        .unwrap_or(0)
}

fn classify(doc: &Document, field_id: ObjectId) -> FieldKind {
    let Some(field_type) = field_attr(doc, field_id, b"FT") else {
        return FieldKind::Unknown;
    };
    let Ok(name) = field_type.as_name() else {
        return FieldKind::Unknown;
    };
    let flags = field_flags(doc, field_id);

    match name {
        b"Tx" => FieldKind::Text,
        b"Sig" => FieldKind::Signature,
        b"Ch" => FieldKind::Choice,
        b"Btn" => {
            if flags & FLAG_PUSHBUTTON != 0 {
                FieldKind::PushButton
            } else if flags & FLAG_RADIO != 0 {
                FieldKind::Radio
            } else {
                FieldKind::Checkbox
            }
        }
        _ => FieldKind::Unknown,
    }
}

/// Renders a field value object as display text.
fn value_to_string(doc: &Document, object: &Object) -> Option<String> {
    match resolve(doc, object) {
        Object::String(bytes, _) => Some(decode_pdf_text(bytes)),
        Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        Object::Integer(number) => Some(number.to_string()),
        Object::Real(number) => Some(number.to_string()),
        Object::Boolean(flag) => Some(flag.to_string()),
        Object::Array(items) => {
            // Multi-select choice fields carry an array of strings.
            let joined: Vec<String> = items
                .iter()
                .filter_map(|item| value_to_string(doc, item))
                .collect();
            Some(joined.join(", "))
        }
        _ => None,
    }
}

/// PDF text strings are either UTF-16BE (with a BOM) or PDFDocEncoding.
/// PDFDocEncoding matches Latin-1 across the range that matters here.
fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&byte| byte as char).collect()
    }
}

/// Encodes a Rust string as a PDF text string, using UTF-16BE when the text
/// contains anything Latin-1 cannot represent.
fn encode_pdf_text(text: &str) -> Vec<u8> {
    if text.chars().all(|c| (c as u32) < 0x100) {
        text.chars().map(|c| c as u32 as u8).collect()
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        bytes
    }
}

/// The default-appearance string that governs a field's text.
///
/// `/DA` looks like `/Helv 12 Tf 0 g`: a font, a size, the `Tf` operator, then
/// colour. A size of `0` is the spec's way of saying "auto-size to fit".
fn appearance_string(doc: &Document, field_id: ObjectId) -> Option<String> {
    let object = field_attr(doc, field_id, b"DA").or_else(|| {
        // Fall back to the form-wide default when the field declares none.
        let form = acro_form_dict(doc)?;
        form.get(b"DA").ok().map(|value| resolve(doc, value).clone())
    })?;

    match object {
        Object::String(bytes, _) => Some(decode_pdf_text(&bytes)),
        _ => None,
    }
}

fn font_size_from_appearance(appearance: &str) -> Option<f32> {
    let tokens: Vec<&str> = appearance.split_whitespace().collect();
    let operator = tokens.iter().position(|token| *token == "Tf")?;
    // The size is the operand immediately before `Tf`.
    tokens.get(operator.checked_sub(1)?)?.parse().ok()
}

/// Formats a size without a trailing `.0`, which keeps `/DA` tidy.
fn format_size(size: f32) -> String {
    if (size - size.round()).abs() < f32::EPSILON {
        format!("{}", size.round() as i64)
    } else {
        format!("{size}")
    }
}

/// Rewrites an appearance string with a new size, preserving font and colour.
fn appearance_with_size(appearance: Option<&str>, size: f32) -> String {
    if let Some(existing) = appearance {
        let mut tokens: Vec<String> = existing.split_whitespace().map(String::from).collect();
        if let Some(operator) = tokens.iter().position(|token| token == "Tf") {
            if operator >= 1 {
                tokens[operator - 1] = format_size(size);
                return tokens.join(" ");
            }
        }
    }
    format!("/Helv {} Tf 0 g", format_size(size))
}

/// Sets a field's text size. `0.0` selects auto-sizing.
pub fn set_field_font_size(doc: &mut Document, name: &str, size: f32) -> AppResult<()> {
    if !(0.0..=144.0).contains(&size) {
        return Err(AppError::InvalidInput(
            "Font size must be between 0 (auto) and 144.".into(),
        ));
    }

    let field_id = find_field(doc, name).ok_or_else(|| AppError::FieldNotFound(name.to_string()))?;
    let updated = appearance_with_size(appearance_string(doc, field_id).as_deref(), size);

    // Write it on the field and on every widget, since either may carry /DA.
    let mut targets = field_widgets(doc, field_id);
    if !targets.contains(&field_id) {
        targets.push(field_id);
    }

    for target in targets {
        if let Ok(dict) = doc.get_object_mut(target).and_then(Object::as_dict_mut) {
            dict.set(
                "DA",
                Object::String(encode_pdf_text(&updated), StringFormat::Literal),
            );
            // The baked appearance no longer matches the requested size.
            dict.remove(b"AP");
        }
    }

    set_need_appearances(doc, true)?;
    Ok(())
}

fn partial_name(doc: &Document, field_id: ObjectId) -> Option<String> {
    let dict = doc.get_dictionary(field_id).ok()?;
    let title = dict.get(b"T").ok()?;
    match resolve(doc, title) {
        Object::String(bytes, _) => Some(decode_pdf_text(bytes)),
        _ => None,
    }
}

/// The on-state names a checkbox or radio widget accepts, taken from its
/// normal-appearance dictionary. Everything except `/Off` is an "on" state.
fn widget_on_states(doc: &Document, widget_id: ObjectId) -> Vec<String> {
    let Ok(widget) = doc.get_dictionary(widget_id) else {
        return Vec::new();
    };
    let Ok(appearance) = widget.get(b"AP") else {
        return Vec::new();
    };
    let Ok(appearance) = resolve(doc, appearance).as_dict() else {
        return Vec::new();
    };
    let Ok(normal) = appearance.get(b"N") else {
        return Vec::new();
    };
    let Ok(normal) = resolve(doc, normal).as_dict() else {
        return Vec::new();
    };

    normal
        .iter()
        .map(|(key, _)| String::from_utf8_lossy(key).into_owned())
        .filter(|state| state != "Off")
        .collect()
}

fn choice_options(doc: &Document, field_id: ObjectId) -> Vec<String> {
    let Some(options) = field_attr(doc, field_id, b"Opt") else {
        return Vec::new();
    };
    let Ok(array) = options.as_array() else {
        return Vec::new();
    };

    array
        .iter()
        .filter_map(|entry| match resolve(doc, entry) {
            // An option may be a plain string, or [export_value, display_text].
            Object::Array(pair) => pair.last().and_then(|item| value_to_string(doc, item)),
            other => value_to_string(doc, other),
        })
        .collect()
}

/// A field is *terminal* when it has no children that are themselves fields.
/// Children without a `/T` entry are widget annotations, not sub-fields.
fn child_fields(doc: &Document, field_id: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_dictionary(field_id) else {
        return Vec::new();
    };
    let Ok(kids) = dict.get(b"Kids") else {
        return Vec::new();
    };
    let Ok(kids) = resolve(doc, kids).as_array() else {
        return Vec::new();
    };

    kids.iter()
        .filter_map(|kid| kid.as_reference().ok())
        .filter(|&kid_id| {
            doc.get_dictionary(kid_id)
                .map(|kid| kid.has(b"T"))
                .unwrap_or(false)
        })
        .collect()
}

/// Widget annotations belonging to a terminal field. A field with a single
/// widget usually merges the two dictionaries, in which case the field *is*
/// the widget.
fn field_widgets(doc: &Document, field_id: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_dictionary(field_id) else {
        return Vec::new();
    };

    if let Ok(kids) = dict.get(b"Kids") {
        if let Ok(kids) = resolve(doc, kids).as_array() {
            let widgets: Vec<ObjectId> = kids
                .iter()
                .filter_map(|kid| kid.as_reference().ok())
                .filter(|&kid_id| {
                    doc.get_dictionary(kid_id)
                        .map(|kid| !kid.has(b"T"))
                        .unwrap_or(false)
                })
                .collect();
            if !widgets.is_empty() {
                return widgets;
            }
        }
    }

    // Merged field/widget.
    if dict.has(b"Rect") {
        return vec![field_id];
    }
    Vec::new()
}

fn widget_rect(doc: &Document, widget_id: ObjectId) -> Option<[f32; 4]> {
    let dict = doc.get_dictionary(widget_id).ok()?;
    let rect = resolve(doc, dict.get(b"Rect").ok()?).clone();
    let array = rect.as_array().ok()?;
    if array.len() != 4 {
        return None;
    }

    let mut out = [0.0f32; 4];
    for (slot, value) in out.iter_mut().zip(array.iter()) {
        *slot = resolve(doc, value).as_float().ok()?;
    }

    // PDF allows either corner order; normalize to lower-left / upper-right.
    Some([
        out[0].min(out[2]),
        out[1].min(out[3]),
        out[0].max(out[2]),
        out[1].max(out[3]),
    ])
}

/// Maps every page object id to its 0-based index, for widget lookups.
fn page_index_map(doc: &Document) -> HashMap<ObjectId, usize> {
    page_ids(doc)
        .into_iter()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect()
}

fn widget_page(
    doc: &Document,
    widget_id: ObjectId,
    pages: &HashMap<ObjectId, usize>,
) -> Option<usize> {
    let dict = doc.get_dictionary(widget_id).ok()?;

    // Preferred: the widget's own back-pointer to its page.
    if let Ok(page_ref) = dict.get(b"P").and_then(Object::as_reference) {
        if let Some(&index) = pages.get(&page_ref) {
            return Some(index);
        }
    }

    // Fallback: some writers omit /P, so scan page /Annots arrays for it.
    for (&page_id, &index) in pages {
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        let Ok(annots) = page.get(b"Annots") else {
            continue;
        };
        let Ok(annots) = resolve(doc, annots).as_array() else {
            continue;
        };
        if annots
            .iter()
            .filter_map(|entry| entry.as_reference().ok())
            .any(|id| id == widget_id)
        {
            return Some(index);
        }
    }

    None
}

/// Enumerates every terminal field in the document, in tree order.
pub fn list_fields(doc: &Document) -> Vec<FormField> {
    let Some(form) = acro_form_dict(doc) else {
        return Vec::new();
    };
    let Ok(roots) = form.get(b"Fields") else {
        return Vec::new();
    };
    let Ok(roots) = resolve(doc, roots).as_array() else {
        return Vec::new();
    };

    let root_ids: Vec<ObjectId> = roots
        .iter()
        .filter_map(|entry| entry.as_reference().ok())
        .collect();

    let pages = page_index_map(doc);
    let mut out = Vec::new();
    for root in root_ids {
        collect_fields(doc, root, String::new(), &pages, &mut out, 0);
    }
    out
}

fn collect_fields(
    doc: &Document,
    field_id: ObjectId,
    prefix: String,
    pages: &HashMap<ObjectId, usize>,
    out: &mut Vec<FormField>,
    depth: usize,
) {
    if depth > MAX_FIELD_DEPTH {
        return;
    }

    let name = match partial_name(doc, field_id) {
        Some(part) if prefix.is_empty() => part,
        Some(part) => format!("{prefix}.{part}"),
        None => prefix.clone(),
    };

    let children = child_fields(doc, field_id);
    if !children.is_empty() {
        for child in children {
            collect_fields(doc, child, name.clone(), pages, out, depth + 1);
        }
        return;
    }

    let kind = classify(doc, field_id);
    let flags = field_flags(doc, field_id);
    let widgets = field_widgets(doc, field_id);
    let first_widget = widgets.first().copied();

    let options = match kind {
        FieldKind::Choice => choice_options(doc, field_id),
        FieldKind::Checkbox | FieldKind::Radio => widgets
            .iter()
            .flat_map(|&widget| widget_on_states(doc, widget))
            .collect(),
        _ => Vec::new(),
    };

    out.push(FormField {
        name,
        kind,
        value: field_attr(doc, field_id, b"V")
            .as_ref()
            .and_then(|object| value_to_string(doc, object)),
        page_index: first_widget.and_then(|widget| widget_page(doc, widget, pages)),
        rect: first_widget.and_then(|widget| widget_rect(doc, widget)),
        read_only: flags & FLAG_READ_ONLY != 0,
        required: flags & FLAG_REQUIRED != 0,
        multiline: kind == FieldKind::Text && flags & FLAG_MULTILINE != 0,
        password: kind == FieldKind::Text && flags & FLAG_PASSWORD != 0,
        max_length: field_attr(doc, field_id, b"MaxLen").and_then(|object| object.as_i64().ok()),
        font_size: appearance_string(doc, field_id)
            .as_deref()
            .and_then(font_size_from_appearance),
        options,
    });
}

// ---------------------------------------------------------------------------
// Filling
// ---------------------------------------------------------------------------

/// Locates a terminal field by fully qualified name.
fn find_field(doc: &Document, name: &str) -> Option<ObjectId> {
    let form = acro_form_dict(doc)?;
    let roots = resolve(doc, form.get(b"Fields").ok()?).as_array().ok()?;
    let root_ids: Vec<ObjectId> = roots
        .iter()
        .filter_map(|entry| entry.as_reference().ok())
        .collect();

    for root in root_ids {
        if let Some(found) = search_field(doc, root, String::new(), name, 0) {
            return Some(found);
        }
    }
    None
}

fn search_field(
    doc: &Document,
    field_id: ObjectId,
    prefix: String,
    target: &str,
    depth: usize,
) -> Option<ObjectId> {
    if depth > MAX_FIELD_DEPTH {
        return None;
    }

    let name = match partial_name(doc, field_id) {
        Some(part) if prefix.is_empty() => part,
        Some(part) => format!("{prefix}.{part}"),
        None => prefix.clone(),
    };

    let children = child_fields(doc, field_id);
    if children.is_empty() {
        return (name == target).then_some(field_id);
    }

    // A non-terminal name can never match a terminal field, but keep walking.
    for child in children {
        if let Some(found) = search_field(doc, child, name.clone(), target, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// Sets a field's value. `value` is interpreted per field kind:
///
/// * text / choice — used verbatim
/// * checkbox      — any of `"", "Off", "false"` clears it; anything else ticks it
/// * radio         — must match one of the group's export values
pub fn set_field_value(doc: &mut Document, name: &str, value: &str) -> AppResult<()> {
    if !has_acro_form(doc) {
        return Err(AppError::NoAcroForm);
    }

    let field_id =
        find_field(doc, name).ok_or_else(|| AppError::FieldNotFound(name.to_string()))?;
    let kind = classify(doc, field_id);

    if field_flags(doc, field_id) & FLAG_READ_ONLY != 0 {
        return Err(AppError::InvalidInput(format!(
            "Field \"{name}\" is read-only."
        )));
    }

    match kind {
        FieldKind::Text | FieldKind::Choice => {
            let encoded = encode_pdf_text(value);
            let dict = doc
                .get_object_mut(field_id)
                .and_then(Object::as_dict_mut)
                .map_err(AppError::Pdf)?;
            dict.set("V", Object::String(encoded, StringFormat::Literal));
            // A stale appearance stream would otherwise mask the new value.
            dict.remove(b"AP");
        }

        FieldKind::Checkbox | FieldKind::Radio => {
            let widgets = field_widgets(doc, field_id);
            let states: Vec<String> = widgets
                .iter()
                .flat_map(|&widget| widget_on_states(doc, widget))
                .collect();

            let off = || "Off".to_string();
            let target = if matches!(value, "" | "Off" | "off" | "false" | "0") {
                off()
            } else if kind == FieldKind::Radio {
                // Radios need an exact export value.
                states
                    .iter()
                    .find(|state| state.as_str() == value)
                    .cloned()
                    .ok_or_else(|| {
                        AppError::InvalidInput(format!(
                            "\"{value}\" is not a valid option for \"{name}\". Valid: {}",
                            if states.is_empty() {
                                "<none found>".to_string()
                            } else {
                                states.join(", ")
                            }
                        ))
                    })?
            } else {
                // Checkboxes accept any truthy input; use the widget's own
                // on-state name, which is often "Yes" or "1" rather than "On".
                states.first().cloned().unwrap_or_else(|| "On".to_string())
            };

            let name_object = Object::Name(target.clone().into_bytes());

            let dict = doc
                .get_object_mut(field_id)
                .and_then(Object::as_dict_mut)
                .map_err(AppError::Pdf)?;
            dict.set("V", name_object.clone());

            // /AS selects which appearance state each widget displays. For a
            // radio group only the matching widget turns on.
            for widget_id in widgets {
                let widget_states = widget_on_states(doc, widget_id);
                let shows_target = widget_states.contains(&target);
                let widget_state = if shows_target {
                    Object::Name(target.clone().into_bytes())
                } else {
                    Object::Name(b"Off".to_vec())
                };

                if let Ok(widget) = doc.get_object_mut(widget_id).and_then(Object::as_dict_mut) {
                    widget.set("AS", widget_state);
                }
            }
        }

        FieldKind::Signature => {
            return Err(AppError::InvalidInput(
                "Signature fields cannot be filled as text.".into(),
            ));
        }
        FieldKind::PushButton => {
            return Err(AppError::InvalidInput("Push buttons hold no value.".into()));
        }
        FieldKind::Unknown => {
            return Err(AppError::InvalidInput(format!(
                "Field \"{name}\" has an unrecognized type."
            )));
        }
    }

    set_need_appearances(doc, true)?;
    Ok(())
}

/// Asks viewers to rebuild field appearances from their values.
fn set_need_appearances(doc: &mut Document, value: bool) -> AppResult<()> {
    let catalog = doc
        .get_dictionary(catalog_id(doc)?)
        .map_err(AppError::Pdf)?;
    let form_ref = catalog.get(b"AcroForm").ok().cloned();

    let Some(form_ref) = form_ref else {
        return Ok(());
    };

    match form_ref {
        Object::Reference(id) => {
            if let Ok(form) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
                form.set("NeedAppearances", Object::Boolean(value));
            }
        }
        Object::Dictionary(_) => {
            // Inline AcroForm: rewrite it on the catalog.
            let catalog_id = catalog_id(doc)?;
            if let Ok(catalog) = doc.get_object_mut(catalog_id).and_then(Object::as_dict_mut) {
                if let Ok(Object::Dictionary(form)) = catalog.get_mut(b"AcroForm") {
                    form.set("NeedAppearances", Object::Boolean(value));
                }
            }
        }
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Field creation
// ---------------------------------------------------------------------------

/// Where and what to create. `rect` is in PDF user space, origin bottom-left.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewField {
    pub page_index: usize,
    pub name: String,
    pub kind: FieldKind,
    pub rect: [f32; 4],
    #[serde(default)]
    pub font_size: Option<f32>,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub max_length: Option<i64>,
    /// Options for a choice field; ignored otherwise.
    #[serde(default)]
    pub options: Vec<String>,
}

/// Ensures the catalog has an `/AcroForm` with a `/Fields` array and a default
/// resource dictionary containing Helvetica. Returns the AcroForm object id.
fn ensure_acro_form(doc: &mut Document) -> AppResult<ObjectId> {
    let catalog_id = catalog_id(doc)?;

    let existing = doc
        .get_dictionary(catalog_id)
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok())
        .and_then(|form| form.as_reference().ok());

    if let Some(form_id) = existing {
        return Ok(form_id);
    }

    // Build a fresh AcroForm, carrying over an inline one if present.
    let helvetica = doc.add_object(Object::Dictionary(dictionary_from([
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Helvetica".to_vec())),
        ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
    ])));

    let mut fonts = Dictionary::new();
    fonts.set("Helv", Object::Reference(helvetica));

    let mut resources = Dictionary::new();
    resources.set("Font", Object::Dictionary(fonts));

    let mut form = Dictionary::new();
    form.set("Fields", Object::Array(Vec::new()));
    form.set("DR", Object::Dictionary(resources));
    form.set(
        "DA",
        Object::String(b"/Helv 0 Tf 0 g".to_vec(), StringFormat::Literal),
    );
    form.set("NeedAppearances", Object::Boolean(true));

    let form_id = doc.add_object(Object::Dictionary(form));

    let catalog = doc
        .get_object_mut(catalog_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    catalog.set("AcroForm", Object::Reference(form_id));

    Ok(form_id)
}

fn dictionary_from<const N: usize>(entries: [(&str, Object); N]) -> Dictionary {
    let mut dict = Dictionary::new();
    for (key, value) in entries {
        dict.set(key, value);
    }
    dict
}

/// Creates a merged field/widget annotation on a page.
pub fn create_field(doc: &mut Document, spec: &NewField) -> AppResult<()> {
    if spec.name.trim().is_empty() {
        return Err(AppError::InvalidInput("Field name cannot be empty.".into()));
    }
    if spec.name.contains('.') {
        return Err(AppError::InvalidInput(
            "Field names cannot contain '.' — it separates parent and child names.".into(),
        ));
    }

    let [x0, y0, x1, y1] = spec.rect;
    if (x1 - x0).abs() < 1.0 || (y1 - y0).abs() < 1.0 {
        return Err(AppError::InvalidInput(
            "Field rectangle is too small to be usable.".into(),
        ));
    }

    let pages = page_ids(doc);
    let page_id = *pages
        .get(spec.page_index)
        .ok_or(AppError::PageOutOfRange(spec.page_index))?;

    if list_fields(doc).iter().any(|field| field.name == spec.name) {
        return Err(AppError::InvalidInput(format!(
            "A field named \"{}\" already exists.",
            spec.name
        )));
    }

    let form_id = ensure_acro_form(doc)?;
    let font_size = spec.font_size.unwrap_or(0.0); // 0 == auto-size
    let height = (y1 - y0).abs();

    let mut flags = 0i64;
    if spec.required {
        flags |= FLAG_REQUIRED;
    }

    let mut widget = Dictionary::new();
    widget.set("Type", Object::Name(b"Annot".to_vec()));
    widget.set("Subtype", Object::Name(b"Widget".to_vec()));
    widget.set(
        "T",
        Object::String(encode_pdf_text(&spec.name), StringFormat::Literal),
    );
    widget.set(
        "Rect",
        Object::Array(vec![
            Object::Real(x0.min(x1)),
            Object::Real(y0.min(y1)),
            Object::Real(x0.max(x1)),
            Object::Real(y0.max(y1)),
        ]),
    );
    // Bit 3 (value 4) = Print: without it the field never appears on paper.
    widget.set("F", Object::Integer(4));
    widget.set("P", Object::Reference(page_id));
    widget.set(
        "DA",
        Object::String(
            format!("/Helv {font_size} Tf 0 g").into_bytes(),
            StringFormat::Literal,
        ),
    );

    match spec.kind {
        FieldKind::Text => {
            widget.set("FT", Object::Name(b"Tx".to_vec()));
            if spec.multiline {
                flags |= FLAG_MULTILINE;
            }
            if let Some(max) = spec.max_length {
                widget.set("MaxLen", Object::Integer(max));
            }
            widget.set("V", Object::String(Vec::new(), StringFormat::Literal));
        }

        FieldKind::Checkbox => {
            widget.set("FT", Object::Name(b"Btn".to_vec()));
            widget.set("V", Object::Name(b"Off".to_vec()));
            widget.set("AS", Object::Name(b"Off".to_vec()));
            // ZapfDingbats "4" is the conventional check glyph.
            widget.set(
                "MK",
                Object::Dictionary(dictionary_from([
                    ("BC", Object::Array(vec![Object::Integer(0)])),
                    ("CA", Object::String(b"4".to_vec(), StringFormat::Literal)),
                ])),
            );
            // Declare the two appearance states so viewers know the on-value.
            let on_stream = checkbox_appearance(doc, height, true);
            let off_stream = checkbox_appearance(doc, height, false);
            let mut normal = Dictionary::new();
            normal.set("Yes", Object::Reference(on_stream));
            normal.set("Off", Object::Reference(off_stream));
            widget.set(
                "AP",
                Object::Dictionary(dictionary_from([("N", Object::Dictionary(normal))])),
            );
        }

        FieldKind::Choice => {
            widget.set("FT", Object::Name(b"Ch".to_vec()));
            flags |= FLAG_COMBO;
            widget.set(
                "Opt",
                Object::Array(
                    spec.options
                        .iter()
                        .map(|option| {
                            Object::String(encode_pdf_text(option), StringFormat::Literal)
                        })
                        .collect(),
                ),
            );
            widget.set("V", Object::String(Vec::new(), StringFormat::Literal));
        }

        FieldKind::Signature => {
            widget.set("FT", Object::Name(b"Sig".to_vec()));
        }

        FieldKind::Radio | FieldKind::PushButton | FieldKind::Unknown => {
            return Err(AppError::InvalidInput(format!(
                "Creating {:?} fields is not supported yet.",
                spec.kind
            )));
        }
    }

    if flags != 0 {
        widget.set("Ff", Object::Integer(flags));
    }

    let widget_id = doc.add_object(Object::Dictionary(widget));

    // Link it into the page's annotation list...
    let page = doc
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    let mut annots = page
        .get(b"Annots")
        .and_then(Object::as_array)
        .cloned()
        .unwrap_or_default();
    annots.push(Object::Reference(widget_id));
    page.set("Annots", Object::Array(annots));

    // ...and into the form's field list.
    let form = doc
        .get_object_mut(form_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    let mut fields = form
        .get(b"Fields")
        .and_then(Object::as_array)
        .cloned()
        .unwrap_or_default();
    fields.push(Object::Reference(widget_id));
    form.set("Fields", Object::Array(fields));
    form.set("NeedAppearances", Object::Boolean(true));

    Ok(())
}

/// Minimal appearance stream for a checkbox in its on/off state.
fn checkbox_appearance(doc: &mut Document, size: f32, checked: bool) -> ObjectId {
    let box_size = size.max(1.0);
    let mut content = format!("q 0.5 w 0 0 {box_size} {box_size} re S Q\n");

    if checked {
        // A simple vector tick, inset from the border.
        let inset = box_size * 0.22;
        let mid = box_size * 0.45;
        content.push_str(&format!(
            "q 1.2 w {inset} {mid} m {mid} {inset} l {} {} l S Q\n",
            box_size - inset,
            box_size - inset
        ));
    }

    let mut stream_dict = Dictionary::new();
    stream_dict.set("Type", Object::Name(b"XObject".to_vec()));
    stream_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    stream_dict.set(
        "BBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(box_size),
            Object::Real(box_size),
        ]),
    );
    stream_dict.set("Resources", Object::Dictionary(Dictionary::new()));

    let stream = lopdf::Stream::new(stream_dict, content.into_bytes());
    doc.add_object(Object::Stream(stream))
}

/// Moves or resizes a field's widget.
///
/// `rect` is in PDF user space, origin bottom-left. Only the first widget is
/// touched: a field with several widgets (a radio group) has no single
/// position to set.
pub fn set_field_rect(doc: &mut Document, name: &str, rect: [f32; 4]) -> AppResult<()> {
    let [x0, y0, x1, y1] = rect;
    if (x1 - x0).abs() < 1.0 || (y1 - y0).abs() < 1.0 {
        return Err(AppError::InvalidInput(
            "Field rectangle is too small to be usable.".into(),
        ));
    }

    let field_id = find_field(doc, name).ok_or_else(|| AppError::FieldNotFound(name.to_string()))?;
    let widget_id = field_widgets(doc, field_id).first().copied().ok_or_else(|| {
        AppError::InvalidInput(format!("Field \"{name}\" has no widget to move."))
    })?;

    let dict = doc
        .get_object_mut(widget_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    dict.set(
        "Rect",
        Object::Array(vec![
            Object::Real(x0.min(x1)),
            Object::Real(y0.min(y1)),
            Object::Real(x0.max(x1)),
            Object::Real(y0.max(y1)),
        ]),
    );

    Ok(())
}

/// Renames a field.
///
/// `new_partial_name` replaces the field's own `/T` entry. For a field nested
/// under a parent the fully qualified name keeps its prefix, so renaming
/// `address.city` to `town` yields `address.town`.
pub fn rename_field(doc: &mut Document, name: &str, new_partial_name: &str) -> AppResult<()> {
    let trimmed = new_partial_name.trim();

    if trimmed.is_empty() {
        return Err(AppError::InvalidInput("Field name cannot be empty.".into()));
    }
    if trimmed.contains('.') {
        return Err(AppError::InvalidInput(
            "Field names cannot contain '.' — it separates parent and child names.".into(),
        ));
    }

    let field_id = find_field(doc, name).ok_or_else(|| AppError::FieldNotFound(name.to_string()))?;

    // Preserve any parent prefix when checking for a collision.
    let qualified = match name.rsplit_once('.') {
        Some((prefix, _)) => format!("{prefix}.{trimmed}"),
        None => trimmed.to_string(),
    };

    if qualified != name && list_fields(doc).iter().any(|field| field.name == qualified) {
        return Err(AppError::InvalidInput(format!(
            "A field named \"{qualified}\" already exists."
        )));
    }

    let dict = doc
        .get_object_mut(field_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    dict.set(
        "T",
        Object::String(encode_pdf_text(trimmed), StringFormat::Literal),
    );

    Ok(())
}

/// Removes a field and every widget it owns.
pub fn delete_field(doc: &mut Document, name: &str) -> AppResult<()> {
    let field_id =
        find_field(doc, name).ok_or_else(|| AppError::FieldNotFound(name.to_string()))?;
    let widgets = field_widgets(doc, field_id);

    let mut doomed: Vec<ObjectId> = widgets;
    if !doomed.contains(&field_id) {
        doomed.push(field_id);
    }

    // Unlink from every page's /Annots.
    for page_id in page_ids(doc) {
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        let Ok(annots) = page.get(b"Annots").and_then(Object::as_array) else {
            continue;
        };

        let filtered: Vec<Object> = annots
            .iter()
            .filter(|entry| match entry.as_reference() {
                Ok(id) => !doomed.contains(&id),
                Err(_) => true,
            })
            .cloned()
            .collect();

        if filtered.len() != annots.len() {
            if let Ok(page) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
                page.set("Annots", Object::Array(filtered));
            }
        }
    }

    // Unlink from /AcroForm /Fields.
    if let Some(form_id) = acro_form_id(doc) {
        if let Ok(form) = doc.get_dictionary(form_id) {
            if let Ok(fields) = form.get(b"Fields").and_then(Object::as_array) {
                let filtered: Vec<Object> = fields
                    .iter()
                    .filter(|entry| match entry.as_reference() {
                        Ok(id) => !doomed.contains(&id),
                        Err(_) => true,
                    })
                    .cloned()
                    .collect();

                if let Ok(form) = doc.get_object_mut(form_id).and_then(Object::as_dict_mut) {
                    form.set("Fields", Object::Array(filtered));
                }
            }
        }
    }

    for id in doomed {
        doc.objects.remove(&id);
    }

    Ok(())
}

fn acro_form_id(doc: &Document) -> Option<ObjectId> {
    doc.get_dictionary(catalog_id(doc).ok()?)
        .ok()?
        .get(b"AcroForm")
        .ok()?
        .as_reference()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::document::blank;

    fn text_field(name: &str) -> NewField {
        NewField {
            page_index: 0,
            name: name.to_string(),
            kind: FieldKind::Text,
            rect: [72.0, 600.0, 300.0, 620.0],
            font_size: Some(10.0),
            multiline: false,
            required: false,
            max_length: None,
            options: Vec::new(),
        }
    }

    #[test]
    fn blank_document_has_no_form() {
        let doc = blank().unwrap();
        assert!(!has_acro_form(&doc));
        assert!(list_fields(&doc).is_empty());
    }

    #[test]
    fn creating_a_field_establishes_the_form() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("full_name")).unwrap();

        assert!(has_acro_form(&doc));
        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "full_name");
        assert_eq!(fields[0].kind, FieldKind::Text);
        assert_eq!(fields[0].page_index, Some(0));
    }

    #[test]
    fn duplicate_names_are_rejected() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("email")).unwrap();
        assert!(create_field(&mut doc, &text_field("email")).is_err());
    }

    #[test]
    fn dotted_names_are_rejected() {
        let mut doc = blank().unwrap();
        assert!(create_field(&mut doc, &text_field("a.b")).is_err());
    }

    #[test]
    fn degenerate_rectangles_are_rejected() {
        let mut doc = blank().unwrap();
        let mut spec = text_field("tiny");
        spec.rect = [10.0, 10.0, 10.2, 10.2];
        assert!(create_field(&mut doc, &spec).is_err());
    }

    #[test]
    fn text_values_round_trip() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("city")).unwrap();
        set_field_value(&mut doc, "city", "Zürich").unwrap();

        let fields = list_fields(&doc);
        assert_eq!(fields[0].value.as_deref(), Some("Zürich"));
    }

    #[test]
    fn unknown_field_names_error() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("known")).unwrap();
        assert!(set_field_value(&mut doc, "unknown", "x").is_err());
    }

    #[test]
    fn checkbox_toggles_between_states() {
        let mut doc = blank().unwrap();
        let mut spec = text_field("agree");
        spec.kind = FieldKind::Checkbox;
        spec.rect = [72.0, 500.0, 90.0, 518.0];
        create_field(&mut doc, &spec).unwrap();

        set_field_value(&mut doc, "agree", "true").unwrap();
        assert_eq!(list_fields(&doc)[0].value.as_deref(), Some("Yes"));

        set_field_value(&mut doc, "agree", "Off").unwrap();
        assert_eq!(list_fields(&doc)[0].value.as_deref(), Some("Off"));
    }

    #[test]
    fn renaming_a_field_changes_its_name() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("old_name")).unwrap();
        set_field_value(&mut doc, "old_name", "keep me").unwrap();

        rename_field(&mut doc, "old_name", "new_name").unwrap();

        let fields = list_fields(&doc);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "new_name");
        // Renaming must not disturb the value.
        assert_eq!(fields[0].value.as_deref(), Some("keep me"));
    }

    #[test]
    fn renaming_onto_an_existing_name_is_rejected() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("first")).unwrap();

        let mut second = text_field("second");
        second.rect = [72.0, 500.0, 300.0, 520.0];
        create_field(&mut doc, &second).unwrap();

        assert!(rename_field(&mut doc, "first", "second").is_err());
    }

    #[test]
    fn renaming_rejects_empty_and_dotted_names() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("field")).unwrap();
        assert!(rename_field(&mut doc, "field", "   ").is_err());
        assert!(rename_field(&mut doc, "field", "a.b").is_err());
    }

    #[test]
    fn renaming_an_unknown_field_errors() {
        let mut doc = blank().unwrap();
        assert!(rename_field(&mut doc, "nope", "x").is_err());
    }

    #[test]
    fn moving_a_field_updates_its_rectangle() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("movable")).unwrap();

        set_field_rect(&mut doc, "movable", [100.0, 100.0, 260.0, 124.0]).unwrap();

        let rect = list_fields(&doc)[0].rect.unwrap();
        assert_eq!(rect, [100.0, 100.0, 260.0, 124.0]);
    }

    #[test]
    fn moving_normalizes_inverted_rectangles() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("movable")).unwrap();

        // Corners given upper-right first should still store lower-left first.
        set_field_rect(&mut doc, "movable", [260.0, 124.0, 100.0, 100.0]).unwrap();

        let rect = list_fields(&doc)[0].rect.unwrap();
        assert_eq!(rect, [100.0, 100.0, 260.0, 124.0]);
    }

    #[test]
    fn moving_rejects_a_degenerate_rectangle() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("movable")).unwrap();
        assert!(set_field_rect(&mut doc, "movable", [10.0, 10.0, 10.2, 10.2]).is_err());
    }

    #[test]
    fn reads_font_size_from_an_appearance_string() {
        assert_eq!(font_size_from_appearance("/Helv 12 Tf 0 g"), Some(12.0));
        assert_eq!(font_size_from_appearance("/Helv 0 Tf 0 g"), Some(0.0));
        assert_eq!(font_size_from_appearance("0 g"), None);
        assert_eq!(font_size_from_appearance("Tf"), None);
    }

    #[test]
    fn rewrites_size_but_keeps_font_and_colour() {
        assert_eq!(
            appearance_with_size(Some("/Arial 8 Tf 1 0 0 rg"), 14.0),
            "/Arial 14 Tf 1 0 0 rg"
        );
    }

    #[test]
    fn builds_an_appearance_string_when_none_exists() {
        assert_eq!(appearance_with_size(None, 11.0), "/Helv 11 Tf 0 g");
    }

    #[test]
    fn zero_means_auto_size() {
        assert_eq!(appearance_with_size(Some("/Helv 12 Tf 0 g"), 0.0), "/Helv 0 Tf 0 g");
    }

    #[test]
    fn font_size_round_trips_through_a_field() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("sized")).unwrap();

        set_field_font_size(&mut doc, "sized", 18.0).unwrap();
        assert_eq!(list_fields(&doc)[0].font_size, Some(18.0));

        set_field_font_size(&mut doc, "sized", 0.0).unwrap();
        assert_eq!(list_fields(&doc)[0].font_size, Some(0.0));
    }

    #[test]
    fn font_size_is_range_checked() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("sized")).unwrap();
        assert!(set_field_font_size(&mut doc, "sized", -1.0).is_err());
        assert!(set_field_font_size(&mut doc, "sized", 500.0).is_err());
    }

    #[test]
    fn deleting_a_field_removes_it() {
        let mut doc = blank().unwrap();
        create_field(&mut doc, &text_field("temp")).unwrap();
        delete_field(&mut doc, "temp").unwrap();
        assert!(list_fields(&doc).is_empty());
    }

    #[test]
    fn utf16_encoding_round_trips() {
        let text = "日本語";
        let encoded = encode_pdf_text(text);
        assert_eq!(decode_pdf_text(&encoded), text);
    }

    #[test]
    fn latin1_stays_single_byte() {
        let encoded = encode_pdf_text("Cafe");
        assert_eq!(encoded.len(), 4);
        assert_eq!(decode_pdf_text(&encoded), "Cafe");
    }
}
