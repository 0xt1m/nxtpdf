//! Document lifecycle and page-tree operations, all on top of `lopdf`.
//!
//! Page indices crossing the IPC boundary are **0-based**. `lopdf` numbers
//! pages from 1, so that conversion happens here and nowhere else.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Attributes a page may inherit from an ancestor node in the page tree.
/// These must be pushed down before the tree is flattened.
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Guards against a malformed file whose `/Parent` chain forms a cycle.
const MAX_TREE_DEPTH: usize = 64;

/// Inheritable attributes resolved for one page, ready to be written onto it.
type ResolvedAttributes<'a> = Vec<(ObjectId, Vec<(&'a [u8], Object)>)>;

// ---------------------------------------------------------------------------
// Types crossing the IPC boundary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    /// 0-based index in the current page order.
    pub index: usize,
    /// Width in PDF points, after rotation is applied.
    pub width_pt: f32,
    /// Height in PDF points, after rotation is applied.
    pub height_pt: f32,
    /// Clockwise rotation in degrees: 0, 90, 180, or 270.
    pub rotation: i64,
    /// Whether this page carries any form-field widgets.
    pub has_form_fields: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    /// Identifies this tab. Also namespaces its page-image URLs, so two
    /// documents can never collide in the webview's cache.
    pub id: u64,
    pub name: String,
    pub path: Option<String>,
    pub page_count: usize,
    pub dirty: bool,
    pub revision: u64,
    pub pdf_version: String,
    pub has_acro_form: bool,
    pub pages: Vec<PageInfo>,
}

/// Follows a `/Reference` to the object it names.
///
/// `Document::dereference` returns both the resolving object id and a `Result`.
/// Every call site here wants only the object, and wants a dangling reference
/// to behave like the reference itself rather than abort the operation — a
/// slightly broken PDF should still open.
pub fn resolve<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    doc.dereference(object)
        .map(|(_, resolved)| resolved)
        .unwrap_or(object)
}

// ---------------------------------------------------------------------------
// Opening and creating
// ---------------------------------------------------------------------------

pub fn open(path: &Path) -> AppResult<Document> {
    let mut doc = Document::load(path)?;
    // Some files in the wild carry no version marker at all.
    if doc.version.is_empty() {
        doc.version = "1.7".to_string();
    }

    // Forms filled by writing `/V` and setting `/NeedAppearances` carry no
    // drawable appearance. PDFium — which renders both our pages and our
    // printed output — ignores that flag, so such a document would open
    // looking entirely empty. Paint the missing appearances up front.
    let repaired = crate::pdf::forms::regenerate_missing_appearances(&mut doc);
    if repaired > 0 {
        log::info!("generated appearances for {repaired} filled field(s)");
    }

    // A widget is only drawn if its page lists it in /Annots. Some generators
    // write the field tree and leave that list empty, which makes the form
    // invisible everywhere except Acrobat, which quietly rebuilds it.
    let reattached = crate::pdf::forms::reattach_orphaned_widgets(&mut doc);
    if reattached > 0 {
        log::info!("re-attached {reattached} orphaned form widget(s) to their pages");
    }

    Ok(doc)
}

/// Builds a valid, empty single-page document (US Letter).
pub fn blank() -> AppResult<Document> {
    let mut doc = Document::with_version("1.7");

    let pages_id = doc.new_object_id();

    let mut page = Dictionary::new();
    page.set("Type", Object::Name(b"Page".to_vec()));
    page.set("Parent", Object::Reference(pages_id));
    page.set(
        "MediaBox",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(612),
            Object::Integer(792),
        ]),
    );
    page.set("Resources", Object::Dictionary(Dictionary::new()));
    let page_id = doc.add_object(Object::Dictionary(page));

    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
    pages.set("Count", Object::Integer(1));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(Object::Dictionary(catalog));

    doc.trailer.set("Root", Object::Reference(catalog_id));
    Ok(doc)
}

pub fn save_to_path(doc: &mut Document, path: &Path) -> AppResult<()> {
    doc.save(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Inspection
// ---------------------------------------------------------------------------

/// Page object ids in presentation order (index 0 == first page).
pub fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

pub fn page_count(doc: &Document) -> usize {
    doc.get_pages().len()
}

fn page_id_at(doc: &Document, index: usize) -> AppResult<ObjectId> {
    page_ids(doc)
        .get(index)
        .copied()
        .ok_or(AppError::PageOutOfRange(index))
}

/// Walks up `/Parent` links to resolve an attribute the page may inherit.
fn inherited(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;

    for _ in 0..MAX_TREE_DEPTH {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => return None,
        }
    }
    None
}

/// Returns `[x0, y0, x1, y1]` for the page's effective MediaBox.
fn media_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    const US_LETTER: [f32; 4] = [0.0, 0.0, 612.0, 792.0];

    let Some(object) = inherited(doc, page_id, b"MediaBox") else {
        return US_LETTER;
    };
    let resolved = resolve(doc, &object).clone();
    let Ok(array) = resolved.as_array() else {
        return US_LETTER;
    };
    if array.len() != 4 {
        return US_LETTER;
    }

    let mut out = US_LETTER;
    for (slot, value) in out.iter_mut().zip(array.iter()) {
        match resolve(doc, value).as_float() {
            Ok(number) => *slot = number,
            Err(_) => return US_LETTER,
        }
    }
    out
}

fn rotation(doc: &Document, page_id: ObjectId) -> i64 {
    inherited(doc, page_id, b"Rotate")
        .and_then(|object| object.as_i64().ok())
        .map(normalize_rotation)
        .unwrap_or(0)
}

/// PDF permits any multiple of 90, including negatives. Fold to 0/90/180/270.
fn normalize_rotation(degrees: i64) -> i64 {
    let wrapped = degrees % 360;
    let positive = if wrapped < 0 { wrapped + 360 } else { wrapped };
    // Snap to a legal quarter turn rather than trusting the file.
    (positive / 90) * 90
}

pub fn describe(
    doc: &Document,
    id: u64,
    name: String,
    path: Option<PathBuf>,
    dirty: bool,
    revision: u64,
) -> DocumentInfo {
    let ids = page_ids(doc);
    let pages = ids
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            let bounds = media_box(doc, id);
            let turn = rotation(doc, id);
            let width = (bounds[2] - bounds[0]).abs();
            let height = (bounds[3] - bounds[1]).abs();
            // A quarter turn swaps the page's visual dimensions.
            let quarter_turned = turn == 90 || turn == 270;

            PageInfo {
                index,
                width_pt: if quarter_turned { height } else { width },
                height_pt: if quarter_turned { width } else { height },
                rotation: turn,
                has_form_fields: page_has_widgets(doc, id),
            }
        })
        .collect();

    DocumentInfo {
        id,
        name,
        path: path.map(|p| p.to_string_lossy().into_owned()),
        page_count: ids.len(),
        dirty,
        revision,
        pdf_version: doc.version.clone(),
        has_acro_form: crate::pdf::forms::has_acro_form(doc),
        pages,
    }
}

fn page_has_widgets(doc: &Document, page_id: ObjectId) -> bool {
    let Ok(dict) = doc.get_dictionary(page_id) else {
        return false;
    };
    let Ok(annots) = dict.get(b"Annots") else {
        return false;
    };
    let Ok(array) = resolve(doc, annots).as_array() else {
        return false;
    };

    array.iter().any(|entry| {
        resolve(doc, entry)
            .as_dict()
            .ok()
            .and_then(|annot| annot.get(b"Subtype").ok())
            .and_then(|subtype| subtype.as_name().ok())
            .is_some_and(|name| name == b"Widget")
    })
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// Rotates one page by `delta` degrees (a multiple of 90), clockwise.
pub fn rotate_page(doc: &mut Document, index: usize, delta: i64) -> AppResult<()> {
    if delta % 90 != 0 {
        return Err(AppError::InvalidInput(
            "Rotation must be a multiple of 90 degrees.".into(),
        ));
    }

    let page_id = page_id_at(doc, index)?;
    let next = normalize_rotation(rotation(doc, page_id) + delta);

    let dict = doc
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    dict.set("Rotate", Object::Integer(next));
    Ok(())
}

pub fn delete_pages(doc: &mut Document, indices: &[usize]) -> AppResult<()> {
    let total = page_count(doc);

    if let Some(&bad) = indices.iter().find(|&&i| i >= total) {
        return Err(AppError::PageOutOfRange(bad));
    }

    let unique: HashSet<usize> = indices.iter().copied().collect();
    if unique.len() >= total {
        return Err(AppError::InvalidInput(
            "A document must keep at least one page.".into(),
        ));
    }

    // lopdf numbers pages from 1.
    let one_based: Vec<u32> = unique.iter().map(|&i| (i + 1) as u32).collect();
    doc.delete_pages(&one_based);
    Ok(())
}

/// Rewrites the page tree so pages appear in `order` (a permutation of 0..n).
///
/// This flattens any nested page tree into a single `Pages` node. Inheritable
/// attributes are pushed down onto each page first so nothing is lost.
pub fn reorder_pages(doc: &mut Document, order: &[usize]) -> AppResult<()> {
    let ids = page_ids(doc);
    let total = ids.len();

    if order.len() != total {
        return Err(AppError::InvalidInput(format!(
            "Expected an order for all {total} pages, got {}.",
            order.len()
        )));
    }

    let mut seen = vec![false; total];
    for &index in order {
        let slot = seen.get_mut(index).ok_or(AppError::PageOutOfRange(index))?;
        if *slot {
            return Err(AppError::InvalidInput(format!(
                "Page {index} appears more than once in the new order."
            )));
        }
        *slot = true;
    }

    push_down_inherited(doc, &ids)?;

    let root_id = root_pages_id(doc)?;
    let kids: Vec<Object> = order
        .iter()
        .map(|&index| Object::Reference(ids[index]))
        .collect();

    for &id in &ids {
        if let Ok(dict) = doc.get_object_mut(id).and_then(Object::as_dict_mut) {
            dict.set("Parent", Object::Reference(root_id));
        }
    }

    let pages = doc
        .get_object_mut(root_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;
    pages.set("Kids", Object::Array(kids));
    pages.set("Count", Object::Integer(total as i64));

    // Intermediate tree nodes are now unreachable.
    doc.prune_objects();
    Ok(())
}

/// Convenience wrapper: move a single page to a new index.
pub fn move_page(doc: &mut Document, from: usize, to: usize) -> AppResult<()> {
    let total = page_count(doc);
    if from >= total {
        return Err(AppError::PageOutOfRange(from));
    }
    if to >= total {
        return Err(AppError::PageOutOfRange(to));
    }

    let mut order: Vec<usize> = (0..total).collect();
    let page = order.remove(from);
    order.insert(to, page);
    reorder_pages(doc, &order)
}

/// Copies inheritable attributes from ancestors onto each page dictionary.
fn push_down_inherited(doc: &mut Document, page_ids: &[ObjectId]) -> AppResult<()> {
    // Resolve everything up front: `inherited` borrows the document immutably.
    let mut resolved: ResolvedAttributes = Vec::new();

    for &page_id in page_ids {
        let mut attributes = Vec::new();
        for key in INHERITABLE {
            let already_present = doc
                .get_dictionary(page_id)
                .map(|dict| dict.has(key))
                .unwrap_or(false);

            if !already_present {
                if let Some(value) = inherited(doc, page_id, key) {
                    attributes.push((key, value));
                }
            }
        }
        resolved.push((page_id, attributes));
    }

    for (page_id, attributes) in resolved {
        let dict = doc
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .map_err(AppError::Pdf)?;
        for (key, value) in attributes {
            dict.set(key.to_vec(), value);
        }
    }

    Ok(())
}

pub fn root_pages_id(doc: &Document) -> AppResult<ObjectId> {
    let catalog_ref = doc
        .trailer
        .get(b"Root")
        .map_err(|_| AppError::InvalidInput("Document has no /Root catalog.".into()))?;

    let catalog = resolve(doc, catalog_ref).as_dict().map_err(AppError::Pdf)?;

    catalog
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| AppError::InvalidInput("Catalog has no /Pages tree.".into()))
}

pub fn catalog_id(doc: &Document) -> AppResult<ObjectId> {
    doc.trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| AppError::InvalidInput("Document has no /Root catalog.".into()))
}

// ---------------------------------------------------------------------------
// Merge / extract
// ---------------------------------------------------------------------------

/// Appends every page of `other` to the end of `doc`.
///
/// Object ids in `other` are renumbered above `doc`'s highest id so the two
/// object graphs cannot collide.
pub fn append_document(doc: &mut Document, mut other: Document) -> AppResult<()> {
    let offset = doc.max_id + 1;
    other.renumber_objects_with(offset);
    doc.max_id = other.max_id;

    let incoming_pages = page_ids(&other);
    push_down_inherited(&mut other, &incoming_pages)?;

    let root_id = root_pages_id(doc)?;

    // Move every object across; page dictionaries get re-parented below.
    for (id, object) in std::mem::take(&mut other.objects) {
        doc.objects.insert(id, object);
    }

    for &page_id in &incoming_pages {
        if let Ok(dict) = doc.get_object_mut(page_id).and_then(Object::as_dict_mut) {
            dict.set("Parent", Object::Reference(root_id));
        }
    }

    let pages = doc
        .get_object_mut(root_id)
        .and_then(Object::as_dict_mut)
        .map_err(AppError::Pdf)?;

    let mut kids = pages
        .get(b"Kids")
        .and_then(Object::as_array)
        .cloned()
        .unwrap_or_default();
    kids.extend(incoming_pages.iter().map(|&id| Object::Reference(id)));

    let count = kids.len() as i64;
    pages.set("Kids", Object::Array(kids));
    pages.set("Count", Object::Integer(count));

    // The merged file's own catalog, outlines, and tree nodes are now orphaned.
    doc.prune_objects();
    Ok(())
}

/// Produces a new document containing only `indices`, in the order given.
pub fn extract_pages(doc: &Document, indices: &[usize]) -> AppResult<Document> {
    if indices.is_empty() {
        return Err(AppError::InvalidInput(
            "Select at least one page to extract.".into(),
        ));
    }

    let total = page_count(doc);
    if let Some(&bad) = indices.iter().find(|&&i| i >= total) {
        return Err(AppError::PageOutOfRange(bad));
    }

    // Round-trip through bytes so the copy owns an independent object graph,
    // then drop the pages we do not want. Far safer than hand-copying an
    // arbitrary object subgraph and its shared resources.
    let mut buffer = Vec::new();
    let mut source = doc.clone();
    source.save_to(&mut buffer)?;

    let mut copy = Document::load_mem(&buffer)?;
    let keep: HashSet<usize> = indices.iter().copied().collect();
    let drop_list: Vec<u32> = (0..total)
        .filter(|i| !keep.contains(i))
        .map(|i| (i + 1) as u32)
        .collect();

    if !drop_list.is_empty() {
        copy.delete_pages(&drop_list);
    }

    // Deleting preserves ascending order; re-apply the caller's ordering if it
    // differs from that.
    let mut sorted: Vec<usize> = keep.iter().copied().collect();
    sorted.sort_unstable();
    if sorted != indices {
        let rank: BTreeMap<usize, usize> = sorted
            .iter()
            .enumerate()
            .map(|(position, &page)| (page, position))
            .collect();
        let order: Vec<usize> = indices
            .iter()
            .filter_map(|page| rank.get(page).copied())
            .collect();
        reorder_pages(&mut copy, &order)?;
    }

    copy.prune_objects();
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi_page(count: usize) -> Document {
        let mut doc = blank().expect("blank document");
        for _ in 1..count {
            let extra = blank().expect("blank document");
            append_document(&mut doc, extra).expect("append");
        }
        doc
    }

    #[test]
    fn blank_document_has_one_page() {
        let doc = blank().unwrap();
        assert_eq!(page_count(&doc), 1);
    }

    #[test]
    fn append_grows_the_page_count() {
        let doc = multi_page(3);
        assert_eq!(page_count(&doc), 3);
    }

    #[test]
    fn rotation_wraps_and_normalizes() {
        assert_eq!(normalize_rotation(-90), 270);
        assert_eq!(normalize_rotation(450), 90);
        assert_eq!(normalize_rotation(360), 0);
    }

    #[test]
    fn rotate_page_accumulates() {
        let mut doc = blank().unwrap();
        rotate_page(&mut doc, 0, 90).unwrap();
        rotate_page(&mut doc, 0, 270).unwrap();
        let id = page_ids(&doc)[0];
        assert_eq!(rotation(&doc, id), 0);
    }

    #[test]
    fn rotate_rejects_non_quarter_turns() {
        let mut doc = blank().unwrap();
        assert!(rotate_page(&mut doc, 0, 45).is_err());
    }

    #[test]
    fn cannot_delete_every_page() {
        let mut doc = multi_page(2);
        assert!(delete_pages(&mut doc, &[0, 1]).is_err());
    }

    #[test]
    fn delete_removes_the_requested_page() {
        let mut doc = multi_page(3);
        delete_pages(&mut doc, &[1]).unwrap();
        assert_eq!(page_count(&doc), 2);
    }

    #[test]
    fn reorder_rejects_duplicates() {
        let mut doc = multi_page(3);
        assert!(reorder_pages(&mut doc, &[0, 0, 1]).is_err());
    }

    #[test]
    fn reorder_rejects_wrong_length() {
        let mut doc = multi_page(3);
        assert!(reorder_pages(&mut doc, &[0, 1]).is_err());
    }

    #[test]
    fn move_page_preserves_count() {
        let mut doc = multi_page(4);
        move_page(&mut doc, 0, 3).unwrap();
        assert_eq!(page_count(&doc), 4);
    }

    #[test]
    fn extract_keeps_only_requested_pages() {
        let doc = multi_page(4);
        let subset = extract_pages(&doc, &[1, 2]).unwrap();
        assert_eq!(page_count(&subset), 2);
    }
}
