import { useStore } from '@/state/store';
import type { FieldKind } from '@/lib/types';

/** Field kinds this draft can create. */
const CREATABLE: { kind: FieldKind; label: string; hint: string }[] = [
  { kind: 'text', label: 'Text', hint: 'Single or multi-line text entry' },
  { kind: 'checkbox', label: 'Checkbox', hint: 'A single on/off tick box' },
  { kind: 'choice', label: 'Dropdown', hint: 'Pick one of a fixed list' },
];

export interface DraftField {
  kind: FieldKind;
  name: string;
  multiline: boolean;
  required: boolean;
  fontSize: number;
  optionsText: string;
}

export const EMPTY_DRAFT: DraftField = {
  kind: 'text',
  name: '',
  multiline: false,
  required: false,
  fontSize: 10,
  optionsText: '',
};

interface DesignPanelProps {
  draft: DraftField;
  onDraftChange: (draft: DraftField) => void;
  /** True while the viewer is waiting for a rectangle to be drawn. */
  drawing: boolean;
  onToggleDrawing: () => void;
}

export function DesignPanel({
  draft,
  onDraftChange,
  drawing,
  onToggleDrawing,
}: DesignPanelProps) {
  const doc = useStore((s) => s.doc);
  const fields = useStore((s) => s.fields);
  const currentPage = useStore((s) => s.currentPage);

  if (!doc) return null;

  const trimmedName = draft.name.trim();
  const nameTaken = fields.some((field) => field.name === trimmedName);
  const nameHasDot = trimmedName.includes('.');
  const nameValid = trimmedName.length > 0 && !nameTaken && !nameHasDot;

  function update(patch: Partial<DraftField>) {
    onDraftChange({ ...draft, ...patch });
  }

  return (
    <div className="design-panel">
      <section className="design-panel__section">
        <h3>Field type</h3>
        <div className="kind-picker">
          {CREATABLE.map((option) => (
            <button
              key={option.kind}
              className={`kind-picker__option${
                draft.kind === option.kind ? ' kind-picker__option--active' : ''
              }`}
              onClick={() => update({ kind: option.kind })}
              title={option.hint}
            >
              {option.label}
            </button>
          ))}
        </div>
      </section>

      <section className="design-panel__section">
        <h3>Properties</h3>

        <label className="form-control">
          <span>Name</span>
          <input
            type="text"
            value={draft.name}
            placeholder="e.g. full_name"
            onChange={(event) => update({ name: event.target.value })}
          />
        </label>
        {trimmedName.length > 0 && nameTaken && (
          <p className="form-error">A field with this name already exists.</p>
        )}
        {nameHasDot && (
          <p className="form-error">
            Names cannot contain “.” — it separates parent and child fields.
          </p>
        )}

        {draft.kind === 'text' && (
          <>
            <label className="checkbox">
              <input
                type="checkbox"
                checked={draft.multiline}
                onChange={(event) => update({ multiline: event.target.checked })}
              />
              <span>Multi-line</span>
            </label>

            <label className="form-control">
              <span>Font size (0 = auto)</span>
              <input
                type="number"
                min={0}
                max={72}
                value={draft.fontSize}
                onChange={(event) => update({ fontSize: Number(event.target.value) })}
              />
            </label>
          </>
        )}

        {draft.kind === 'choice' && (
          <label className="form-control">
            <span>Options (one per line)</span>
            <textarea
              rows={4}
              value={draft.optionsText}
              placeholder={'Yes\nNo\nMaybe'}
              onChange={(event) => update({ optionsText: event.target.value })}
            />
          </label>
        )}

        <label className="checkbox">
          <input
            type="checkbox"
            checked={draft.required}
            onChange={(event) => update({ required: event.target.checked })}
          />
          <span>Required</span>
        </label>
      </section>

      <section className="design-panel__section">
        <button
          className={drawing ? 'button--danger' : 'button--primary'}
          disabled={!nameValid}
          onClick={onToggleDrawing}
        >
          {drawing ? 'Cancel' : 'Place on page'}
        </button>

        <p className="hint">
          {drawing
            ? `Drag a rectangle on page ${currentPage + 1} to place the field.`
            : nameValid
              ? 'Then drag a rectangle on the page.'
              : 'Give the field a unique name first.'}
        </p>
      </section>

      <section className="design-panel__section">
        <h3>Not in this draft</h3>
        <p className="hint">
          Radio groups, digital signatures, and flattening a filled form to static
          content. Field values rely on <code>/NeedAppearances</code>, so a viewer that
          ignores that flag shows stale visuals.
        </p>
      </section>
    </div>
  );
}
