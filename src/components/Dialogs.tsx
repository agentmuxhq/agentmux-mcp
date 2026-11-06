import { useEffect, useRef, useState } from "react";
import { Modal, btn, btnPrimary } from "./ui";

// In-app replacements for window.confirm / window.prompt, which the macOS webview Tauri uses
// doesn't show (confirm answers "no" and prompt answers nothing, silently). Mount <DialogHost />
// once; then `await ask(...)` / `await promptText(...)` from anywhere.

interface Req {
  kind: "ask" | "prompt";
  title: string;
  message: string;
  confirm: string;
  danger?: boolean;
  initial?: string;
  placeholder?: string;
  /** prompt only: the answer must equal this (type-the-name confirmations). */
  mustMatch?: string;
  resolve: (v: string | null) => void;
}

let show: ((r: Req) => void) | null = null;

export function ask(o: {
  title: string;
  message: string;
  confirm?: string;
  danger?: boolean;
}): Promise<boolean> {
  return new Promise((resolve) => {
    if (!show) return resolve(false);
    show({
      kind: "ask",
      confirm: o.confirm ?? "OK",
      ...o,
      resolve: (v) => resolve(v !== null),
    });
  });
}

export function promptText(o: {
  title: string;
  message: string;
  confirm?: string;
  danger?: boolean;
  initial?: string;
  placeholder?: string;
  mustMatch?: string;
}): Promise<string | null> {
  return new Promise((resolve) => {
    if (!show) return resolve(null);
    show({ kind: "prompt", confirm: o.confirm ?? "OK", ...o, resolve });
  });
}

export function DialogHost() {
  const [req, setReq] = useState<Req | null>(null);
  const [text, setText] = useState("");
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    show = (r) => {
      setText(r.initial ?? "");
      setReq(r);
    };
    return () => {
      show = null;
    };
  }, []);
  useEffect(() => {
    if (req?.kind === "prompt") setTimeout(() => input.current?.select(), 0);
  }, [req]);

  if (!req) return null;
  const done = (v: string | null) => {
    setReq(null);
    req.resolve(v);
  };
  const ok =
    req.kind === "ask" ||
    (req.mustMatch !== undefined
      ? text.trim() === req.mustMatch
      : text.trim() !== "");
  const submit = () => ok && done(req.kind === "ask" ? "" : text.trim());

  return (
    <div className="relative z-[60]">
      <Modal title={req.title} onClose={() => done(null)}>
        <div
          className="space-y-4 text-sm"
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
            if (e.key === "Escape") done(null);
          }}
        >
          <div className="whitespace-pre-wrap text-xs leading-relaxed text-zinc-400">
            {req.message}
          </div>
          {req.kind === "prompt" && (
            <input
              ref={input}
              autoFocus
              value={text}
              placeholder={req.placeholder ?? req.mustMatch}
              onChange={(e) => setText(e.target.value)}
              className="w-full select-text rounded-md border border-white/10 bg-black/40 px-2 py-1.5 text-xs text-zinc-200 outline-none focus:border-white/30"
            />
          )}
          <div className="flex justify-end gap-2">
            <button className={btn} onClick={() => done(null)}>
              Cancel
            </button>
            <button
              autoFocus={req.kind === "ask"}
              disabled={!ok}
              onClick={submit}
              className={
                req.danger
                  ? "rounded-md bg-red-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-red-500 disabled:opacity-40"
                  : btnPrimary
              }
            >
              {req.confirm}
            </button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
