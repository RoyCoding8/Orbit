import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

export default function MarkdownPreview({ value }: { value: string }) {
  const metadata = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(value);
  return (
    <div className="markdown-preview" tabIndex={0} aria-label="Rendered Markdown">
      {metadata && <details className="markdown-metadata"><summary>File metadata</summary><pre>{metadata[1]}</pre></details>}
      <Markdown remarkPlugins={[remarkGfm]} skipHtml components={{
        a: ({ href, children }) => href && /^(https?:|mailto:)/i.test(href)
          ? <a href={href} target="_blank" rel="noreferrer">{children}</a>
          : <span className="markdown-reference" title={href}>{children}</span>,
        img: ({ alt, src }) => <span className="markdown-image-reference" title={src}>Image: {alt || src || "attachment"}</span>,
      }}>{metadata ? value.slice(metadata[0].length) : value}</Markdown>
      {!value.trim() && <p className="quiet-copy">Nothing to preview yet.</p>}
    </div>
  );
}
