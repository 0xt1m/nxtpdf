import { useEffect, useRef, useState } from 'react';
import { useStore } from '@/state/store';
import type { FormField } from '@/lib/types';

/** Fill in an existing form, and rename or remove its fields. */
export function FieldsPanel() {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  const selectedFields = useStore((s) => s.selectedFields);
  const busy = useStore((s) => s.busy);
  const setCurrentPage = useStore((s) => s.setCurrentPage);
  const selectField = useStore((s) => s.selectField);
  const clearFieldSelection = useStore((s) => s.clearFieldSelection);
  const deleteSelectedFields = useStore((s) => s.deleteSelectedFields);

  if (!doc) return null;

  if (fields.length === 0) {
    return (
      <div className="panel__empty">
        <p>This document has no form fields.</p>
        <p className="hint">Switch to the Design tab to add some.</p>
      </div>
    );
  }

  return (
    <>
      {selectedFields.length > 0 && (
        <div className="panel__selection-bar">
          <span>{selectedFields.length} selected</span>
          <div>
            <button onClick={clearFieldSelection} disabled={busy}>
              None
            </button>
            <button
              className="button--danger"
              onClick={deleteSelectedFields}
              disabled={busy}
              title="Delete selected fields (Del)"
            >
              Delete
            </button>
          </div>
        </div>
      )}

      <div className="field-list">
        {fields.map((field) => (
          <FieldRow
            key={field.name}
            field={field}
            busy={busy}
            isSelected={selectedFields.includes(field.name)}
            onSelect={(event) => {
              selectField(field.name, {
                toggle: event.ctrlKey || event.metaKey,
                range: event.shiftKey,
              });
              if (field.pageIndex !== null) setCurrentPage(field.pageIndex);
            }}
          />
        ))}
      </div>
    </>
  );
}

interface FieldRowProps {
  field: FormField;
  busy: boolean;
  isSelected: boolean;
  onSelect: (event: React.MouseEvent) => void;
}

function FieldRow({ field, busy, isSelected, onSelect }: FieldRowProps) {
  const setFieldValue = useStore((s) => s.setFieldValue);
  const renameField = useStore((s) => s.renameField);

  // Local drafts so typing stays responsive; committed on blur, not per key.
  const [draft, setDraft] = useState(field.value ?? '');
  const [nameDraft, setNameDraft] = useState(() => partialName(field.name));
  const rowRef = useRef<HTMLDivElement>(null);

  // Re-sync when the backend snapshot changes underneath us.
  useEffect(() => {
    setDraft(field.value ?? '');
  }, [field.value]);

  useEffect(() => {
    setNameDraft(partialName(field.name));
  }, [field.name]);

  // Clicking a field on the page selects it here, which is useless if the row
  // is scrolled out of sight.
  useEffect(() => {
    if (isSelected) {
      rowRef.current?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  }, [isSelected]);

  const disabled = busy || field.readOnly;

  function commitValue(value: string) {
    if (value === (field.value ?? '')) return;
    void setFieldValue(field.name, value);
  }

  function commitName() {
    const next = nameDraft.trim();
    if (next === '' || next === partialName(field.name)) {
      // Reject an empty rename by snapping back rather than erroring.
      setNameDraft(partialName(field.name));
      return;
    }
    void renameField(field.name, next);
  }

  return (
    <div
      ref={rowRef}
      className={`field-row${isSelected ? ' field-row--selected' : ''}`}
      onClick={onSelect}
    >
      <div className="field-row__header">
        <input
          className="field-row__name-input"
          value={nameDraft}
          disabled={busy}
          aria-label={`Name of field ${field.name}`}
          title="Click to rename"
          onChange={(event) => setNameDraft(event.target.value)}
          onBlur={commitName}
          onKeyDown={(event) => {
            // Keep Delete and Ctrl+A local to this input.
            event.stopPropagation();
            if (event.key === 'Enter') event.currentTarget.blur();
            if (event.key === 'Escape') {
              setNameDraft(partialName(field.name));
              event.currentTarget.blur();
            }
          }}
        />
        <span className="field-row__kind">{field.kind}</span>
      </div>

      {field.kind === 'text' &&
        (field.multiline ? (
          <textarea
            value={draft}
            rows={3}
            disabled={disabled}
            maxLength={field.maxLength ?? undefined}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => commitValue(draft)}
            onKeyDown={(event) => event.stopPropagation()}
          />
        ) : (
          <input
            type={field.password ? 'password' : 'text'}
            value={draft}
            disabled={disabled}
            maxLength={field.maxLength ?? undefined}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => commitValue(draft)}
            onKeyDown={(event) => {
              event.stopPropagation();
              if (event.key === 'Enter') event.currentTarget.blur();
            }}
          />
        ))}

      {field.kind === 'checkbox' && (
        <label className="checkbox">
          <input
            type="checkbox"
            checked={draft !== '' && draft !== 'Off'}
            disabled={disabled}
            onChange={(event) => {
              const next = event.target.checked ? 'On' : 'Off';
              setDraft(next);
              commitValue(next);
            }}
          />
          <span>{draft !== '' && draft !== 'Off' ? 'Checked' : 'Unchecked'}</span>
        </label>
      )}

      {(field.kind === 'choice' || field.kind === 'radio') && (
        <select
          value={draft}
          disabled={disabled}
          onChange={(event) => {
            setDraft(event.target.value);
            commitValue(event.target.value);
          }}
        >
          <option value="">— none —</option>
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      )}

      {field.kind === 'signature' && (
        <p className="hint">Signature fields cannot be filled in this version.</p>
      )}

      {field.kind === 'pushButton' && <p className="hint">Push button — no value.</p>}

      <div className="field-row__footer">
        {field.required && <span className="hint">required</span>}
        {field.readOnly && <span className="hint">read-only</span>}
        {field.pageIndex !== null && (
          <span className="hint">page {field.pageIndex + 1}</span>
        )}
      </div>
    </div>
  );
}

/**
 * The editable part of a field name.
 *
 * A nested field is fully qualified as `parent.child`, but only the last
 * segment belongs to the field itself — renaming must not let the user
 * rewrite its parent.
 */
function partialName(qualified: string): string {
  const index = qualified.lastIndexOf('.');
  return index === -1 ? qualified : qualified.slice(index + 1);
}
