import { useEffect, useState } from 'react';
import { useStore } from '@/state/store';
import type { FormField } from '@/lib/types';

/** Fill in an existing form. */
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

  // Local draft so typing stays responsive; committed on blur, not per key.
  const [draft, setDraft] = useState(field.value ?? '');

  // Re-sync when the backend snapshot changes underneath us.
  useEffect(() => {
    setDraft(field.value ?? '');
  }, [field.value]);

  const disabled = busy || field.readOnly;

  function commit(value: string) {
    if (value === (field.value ?? '')) return;
    void setFieldValue(field.name, value);
  }

  return (
    <div
      className={`field-row${isSelected ? ' field-row--selected' : ''}`}
      onClick={onSelect}
    >
      <div className="field-row__header">
        <label className="field-row__name" htmlFor={`field-${field.name}`}>
          {field.name}
          {field.required && <span className="field-row__required">*</span>}
        </label>
        <span className="field-row__kind">{field.kind}</span>
      </div>

      {field.kind === 'text' &&
        (field.multiline ? (
          <textarea
            id={`field-${field.name}`}
            value={draft}
            rows={3}
            disabled={disabled}
            maxLength={field.maxLength ?? undefined}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => commit(draft)}
            onKeyDown={(event) => event.stopPropagation()}
          />
        ) : (
          <input
            id={`field-${field.name}`}
            type={field.password ? 'password' : 'text'}
            value={draft}
            disabled={disabled}
            maxLength={field.maxLength ?? undefined}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={() => commit(draft)}
            onKeyDown={(event) => {
              // Keep Delete and Ctrl+A local to this input.
              event.stopPropagation();
              if (event.key === 'Enter') event.currentTarget.blur();
            }}
          />
        ))}

      {field.kind === 'checkbox' && (
        <label className="checkbox">
          <input
            id={`field-${field.name}`}
            type="checkbox"
            checked={draft !== '' && draft !== 'Off'}
            disabled={disabled}
            onChange={(event) => {
              const next = event.target.checked ? 'On' : 'Off';
              setDraft(next);
              commit(next);
            }}
          />
          <span>{draft !== '' && draft !== 'Off' ? 'Checked' : 'Unchecked'}</span>
        </label>
      )}

      {(field.kind === 'choice' || field.kind === 'radio') && (
        <select
          id={`field-${field.name}`}
          value={draft}
          disabled={disabled}
          onChange={(event) => {
            setDraft(event.target.value);
            commit(event.target.value);
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
        {field.readOnly && <span className="hint">read-only</span>}
        {field.pageIndex !== null && (
          <span className="hint">page {field.pageIndex + 1}</span>
        )}
      </div>
    </div>
  );
}
