import { useStore, type AddableKind } from '@/state/store';

/**
 * The add-a-field strip.
 *
 * One click drops a field onto the current page with a generated name, ready
 * to be renamed in the Fields panel. Fields cascade down the page so repeated
 * clicks do not land on top of each other, and each one can be dragged
 * wherever it belongs.
 */
const TOOLS: { kind: AddableKind; label: string; glyph: string; hint: string }[] = [
  { kind: 'text', label: 'Text', glyph: 'I', hint: 'Add a text field' },
  { kind: 'checkbox', label: 'Checkbox', glyph: '☑', hint: 'Add a checkbox' },
  { kind: 'choice', label: 'Dropdown', glyph: '▾', hint: 'Add a dropdown' },
];

export function FieldToolbar() {
  const doc = useStore((s) => s.doc);
  const busy = useStore((s) => s.busy);
  const currentPage = useStore((s) => s.currentPage);
  const addField = useStore((s) => s.addField);

  if (!doc) return null;

  return (
    <div className="field-toolbar">
      <span className="field-toolbar__label">Add to page {currentPage + 1}</span>

      {TOOLS.map((tool) => (
        <button
          key={tool.kind}
          className="field-toolbar__button"
          disabled={busy}
          title={tool.hint}
          onClick={() => void addField(tool.kind)}
        >
          <span className="field-toolbar__glyph" aria-hidden="true">
            {tool.glyph}
          </span>
          {tool.label}
        </button>
      ))}

      <span className="field-toolbar__hint">Drag a field on the page to move it</span>
    </div>
  );
}
