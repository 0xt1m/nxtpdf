import { ChevronDown, PenLine, SquareCheck, Type } from 'lucide-react';
import { useStore, type AddableKind } from '@/state/store';

/**
 * The add-a-field strip, plus the switch into editing the page's own text.
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
  const pendingField = useStore((s) => s.pendingField);
  const armField = useStore((s) => s.armField);
  const textMode = useStore((s) => s.textMode);
  const toggleTextMode = useStore((s) => s.toggleTextMode);

  if (!doc) return null;

  const armed = TOOLS.find((tool) => tool.kind === pendingField);

  return (
    <div className="field-toolbar">
      <span className="field-toolbar__label">Add to page {currentPage + 1}</span>

      {TOOLS.map(({ kind, label, Icon, hint }) => (
        <button
          key={kind}
          className={`field-toolbar__button${
            pendingField === kind ? ' field-toolbar__button--armed' : ''
          }`}
          disabled={busy || textMode}
          aria-pressed={pendingField === kind}
          title={hint}
          onClick={() => armField(kind)}
        >
          <Icon size={15} />
          {label}
        </button>
      ))}

      <span className="field-toolbar__divider" />

      {/*
        A mode rather than an always-on overlay. A page carries far more text
        than it does fields, so leaving every label clickable would bury the
        fields underneath them.
      */}
      <button
        className={`field-toolbar__button${
          textMode ? ' field-toolbar__button--armed' : ''
        }`}
        disabled={busy}
        aria-pressed={textMode}
        title="Edit the text printed on the page, outside any form field"
        onClick={toggleTextMode}
      >
        <PenLine size={15} />
        Edit page text
      </button>

      <span className="field-toolbar__hint">
        {textMode
          ? 'Click any text on the page to edit it — Esc to leave'
          : armed
            ? `Drag on the page to place the ${armed.label.toLowerCase()} — Esc to cancel`
            : 'Drag a field on the page to move it'}
      </span>
    </div>
  );
}
