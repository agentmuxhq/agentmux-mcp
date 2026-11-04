import { convertFileSrc } from "@tauri-apps/api/core";
import { FileText, Paperclip, X } from "lucide-react";
import type { Attachment } from "../lib/api";

const kb = (n: number) => (n > 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`);

/** Images, video, audio and PDFs attached to a message, shown and played in place. */
export function AttachmentList({ items }: { items: Attachment[] }) {
  return (
    <div className="mt-2 flex flex-wrap gap-2">
      {items.map((a) => {
        const src = convertFileSrc(a.path);
        const caption = a.caption ? <div className="mt-1 max-w-[320px] text-[11px] text-zinc-500">{a.caption}</div> : null;
        if (a.mime.startsWith("image/"))
          return (
            <figure key={a.id}>
              <a href={src} target="_blank" rel="noreferrer">
                <img src={src} alt={a.caption ?? a.name} className="max-h-64 max-w-[320px] rounded-md border border-white/10 object-contain" />
              </a>
              {caption}
            </figure>
          );
        if (a.mime.startsWith("video/"))
          return (
            <figure key={a.id}>
              <video src={src} controls preload="metadata" className="max-h-72 max-w-[420px] rounded-md border border-white/10" />
              {caption}
            </figure>
          );
        if (a.mime.startsWith("audio/"))
          return (
            <figure key={a.id} className="rounded-md border border-white/10 p-2">
              <div className="mb-1 text-[11px] text-zinc-400">{a.name}</div>
              <audio src={src} controls preload="metadata" />
              {caption}
            </figure>
          );
        return (
          <a key={a.id} href={src} target="_blank" rel="noreferrer" className="flex items-center gap-2 rounded-md border border-white/10 px-3 py-2 text-xs text-zinc-300 hover:bg-white/5">
            <FileText size={14} /> {a.name} <span className="text-zinc-600">{kb(a.size)}</span>
          </a>
        );
      })}
    </div>
  );
}

export interface PendingFile {
  name: string;
  mime: string;
  data: string; // base64
  size: number;
}

const ACCEPT = "image/*,audio/*,video/*,application/pdf";

/** Read picked files as base64 for sending. */
export async function readFiles(files: FileList | null): Promise<PendingFile[]> {
  if (!files) return [];
  const out: PendingFile[] = [];
  for (const f of Array.from(files)) {
    const data = await new Promise<string>((res, rej) => {
      const r = new FileReader();
      r.onload = () => res(String(r.result).split("base64,")[1] ?? "");
      r.onerror = () => rej(r.error);
      r.readAsDataURL(f);
    });
    out.push({ name: f.name, mime: f.type, data, size: f.size });
  }
  return out;
}

export function AttachButton({ onPick }: { onPick: (files: PendingFile[]) => void }) {
  return (
    <label className="flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-1 text-zinc-400 hover:bg-white/10 hover:text-white" title="Attach images, audio, video or PDFs">
      <Paperclip size={13} />
      <input type="file" multiple accept={ACCEPT} className="hidden" onChange={async (e) => (onPick(await readFiles(e.target.files)), (e.target.value = ""))} />
    </label>
  );
}

export function PendingChips({ files, onRemove }: { files: PendingFile[]; onRemove: (i: number) => void }) {
  if (!files.length) return null;
  return (
    <div className="mb-2 flex flex-wrap gap-1.5">
      {files.map((f, i) => (
        <span key={i} className="flex items-center gap-1 rounded-full border border-white/10 bg-white/5 px-2 py-0.5 text-[11px] text-zinc-300">
          {f.name} <span className="text-zinc-600">{kb(f.size)}</span>
          <button onClick={() => onRemove(i)} className="text-zinc-500 hover:text-white" title="Remove">
            <X size={11} />
          </button>
        </span>
      ))}
    </div>
  );
}
