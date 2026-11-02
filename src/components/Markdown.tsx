import { useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Check, Copy } from "lucide-react";

/**
 * Renders agent messages as Markdown (GitHub flavour: tables, task lists, strikethrough).
 * Messages come from other agents, so raw HTML is never rendered, images are shown as their alt
 * text (nothing is fetched), and links don't navigate the app window: clicking one copies it.
 */

function copy(text: string, done: () => void) {
  navigator.clipboard?.writeText(text).then(done).catch(() => {});
}

function CodeBlock({ lang, code }: { lang: string | null; code: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="my-2 overflow-hidden rounded-md border border-white/10 bg-black/50">
      <div className="flex items-center gap-2 border-b border-white/10 px-3 py-1 text-[10.5px] text-zinc-500">
        <span className="font-mono">{lang || "code"}</span>
        <button
          className="ml-auto flex items-center gap-1 hover:text-zinc-200"
          title="Copy code"
          onClick={() =>
            copy(code, () => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
          }
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre className="overflow-x-auto p-3 font-mono text-[11.5px] leading-relaxed text-zinc-200">
        <code>{code}</code>
      </pre>
    </div>
  );
}

function SafeLink({ href, children }: { href?: string; children?: ReactNode }) {
  const [copied, setCopied] = useState(false);
  return (
    <span
      role="link"
      tabIndex={0}
      title={href ? `${href}\nClick to copy` : undefined}
      onClick={() => href && copy(href, () => (setCopied(true), setTimeout(() => setCopied(false), 1200)))}
      className="cursor-pointer text-sky-300 underline decoration-sky-300/40 underline-offset-2 hover:decoration-sky-300"
    >
      {children}
      {copied && <span className="ml-1 text-[10px] text-emerald-300">copied</span>}
    </span>
  );
}

const components: Components = {
  p: ({ children }) => <p className="my-1.5 first:mt-0 last:mb-0">{children}</p>,
  h1: ({ children }) => <h3 className="mb-1.5 mt-3 font-display text-[15px] font-semibold first:mt-0">{children}</h3>,
  h2: ({ children }) => <h3 className="mb-1.5 mt-3 font-display text-[14px] font-semibold first:mt-0">{children}</h3>,
  h3: ({ children }) => <h4 className="mb-1 mt-2.5 font-display text-[13px] font-semibold first:mt-0">{children}</h4>,
  h4: ({ children }) => <h4 className="mb-1 mt-2 text-[13px] font-semibold first:mt-0">{children}</h4>,
  strong: ({ children }) => <strong className="font-semibold text-zinc-50">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  del: ({ children }) => <del className="text-zinc-500">{children}</del>,
  ul: ({ children }) => <ul className="my-1.5 list-disc space-y-0.5 pl-5 marker:text-zinc-500">{children}</ul>,
  ol: ({ children }) => <ol className="my-1.5 list-decimal space-y-0.5 pl-5 marker:text-zinc-500">{children}</ol>,
  li: ({ children }) => <li className="pl-0.5">{children}</li>,
  blockquote: ({ children }) => <blockquote className="my-2 border-l-2 border-white/20 pl-3 text-zinc-400">{children}</blockquote>,
  hr: () => <hr className="my-3 border-white/10" />,
  a: ({ href, children }) => <SafeLink href={href}>{children}</SafeLink>,
  img: ({ alt }) => <span className="text-zinc-500">[image{alt ? `: ${alt}` : ""}]</span>,
  table: ({ children }) => (
    <div className="my-2 overflow-x-auto">
      <table className="w-full border-collapse text-[12px]">{children}</table>
    </div>
  ),
  th: ({ children }) => <th className="border border-white/10 bg-white/5 px-2 py-1 text-left font-semibold">{children}</th>,
  td: ({ children }) => <td className="border border-white/10 px-2 py-1 align-top">{children}</td>,
  // Fenced blocks arrive as <pre><code class="language-x">; inline code has no <pre> parent.
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children }) => {
    const text = String(children ?? "");
    const lang = /language-([\w+-]+)/.exec(className ?? "")?.[1] ?? null;
    const block = lang !== null || text.includes("\n");
    if (block) return <CodeBlock lang={lang} code={text.replace(/\n$/, "")} />;
    return <code className="rounded bg-white/10 px-1 py-px font-mono text-[11.5px] text-zinc-100">{children}</code>;
  },
};

export default function Markdown({ text, className }: { text: string; className?: string }) {
  return (
    <div className={`select-text break-words ${className ?? ""}`}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components} skipHtml>
        {text}
      </ReactMarkdown>
    </div>
  );
}
