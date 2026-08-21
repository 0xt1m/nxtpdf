import { ChevronDown, SquareCheck, Type } from 'lucide-react';
import { useStore, type AddableKind } from '@/state/store';

/**
 * The add-a-field strip.
 *
 * One click drops a field onto the current page with a generated name, ready
 * to be renamed in the Fields panel. Fields cascade down the page so repeated
 * clicks do not land on top of each other, and each one can be dragged
 * wherever it belongs.
 */
const TOOLS: {
  kind: AddableKind;
  label: string;
  Icon: typeof Type;
  hint: string;
}[] = [
  { kind: 'text', label: 'Text', Icon: Type, hint: 'Add a text field' },
  { kind: 'checkbox', label: 'Checkbox', Icon: SquareCheck, hint: 'Add a checkbox' },
  { kind: 'choice', label: 'Dropdown', Icon: ChevronDown, hint: 'Add a dropdown' },
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

      {TOOLS.map(({ kind, label, Icon, hint }) => (
        <button
          key={kind}
          className="field-toolbar__button"
          disabled={busy}
          title={hint}
          onClick={() => void addField(kind)}
        >
          <Icon size={15} />
          {label}
        </button>
      ))}

      <span className="field-toolbar__hint">Drag a field on the page to move it</span>
    </div>
  );
}
