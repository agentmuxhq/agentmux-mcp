import { useEffect, useRef, useState } from "react";

/** A name you can rename in place: double-click, type, Enter to save, Esc to cancel. */
export default function EditableTitle({
  value,
  onSave,
  className,
  title = "Double-click to rename",
}: {
  value: string;
  onSave: (next: string) => Promise<unknown>;
  className?: string;
  title?: string;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!editing) setDraft(value);
  }, [value, editing]);
  useEffect(() => {
    if (editing) input.current?.select();
  }, [editing]);

  const commit = async () => {
    const next = draft.trim();
    setEditing(false);
    if (next && next !== value) await onSave(next).catch(() => setDraft(value));
  };

  if (editing)
    return (
      <input
        ref={input}
        value={draft}
        maxLength={60}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") (setDraft(value), setEditing(false));
        }}
        onClick={(e) => e.stopPropagation()}
        className={`min-w-[8rem] select-text rounded border border-white/20 bg-black/50 px-1.5 py-0.5 outline-none focus:border-white/40 ${className ?? ""}`}
      />
    );
  return (
    <span className={className} title={title} onDoubleClick={(e) => (e.stopPropagation(), setEditing(true))}>
      {value}
    </span>
  );
}
