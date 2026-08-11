import { memo, useState } from "react";
import ReactMarkdown from "react-markdown";
import rehypeHighlight from "rehype-highlight";
import remarkGfm from "remark-gfm";

import "highlight.js/styles/github-dark-dimmed.css";

/**
 * 助手输出的 Markdown 渲染。
 *
 * `[约束]` memo 是性能要求，不是锦上添花。流式输出时 transcript 每帧
 * 重渲染，没有 memo 的话每条历史消息都要重新 parse 一遍 markdown ——
 * 长对话里打字机效果会肉眼可见地掉帧。有 memo 后只有正在流式的那一段
 * 反复 parse，它通常只有几百字。
 *
 * 用户消息**不走这里**：用户输入的 `# 标题` 就是字面上的井号标题，
 * 按 markdown 渲染等于篡改他说的话。
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[[rehypeHighlight, { ignoreMissing: true, detect: false }]]}
        components={{
          pre: CodeBlock,
          // 裸链接一律新窗口，别把应用本身导航走 —— webview 里没有后退按钮
          a: ({ children, href }) => (
            <a href={href} target="_blank" rel="noreferrer">
              {children}
            </a>
          ),
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});

/** 代码块：语言标签 + 复制按钮。 */
function CodeBlock(props: React.HTMLAttributes<HTMLPreElement>) {
  const [copied, setCopied] = useState(false);

  const child = props.children as React.ReactElement<{
    className?: string;
    children?: React.ReactNode;
  }> | null;
  const lang =
    child?.props?.className
      ?.split(" ")
      .find((c) => c.startsWith("language-"))
      ?.slice("language-".length) ?? "";

  const copy = () => {
    const text = extractText(child?.props?.children);
    void navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="codeblock">
      <div className="codeblock-bar">
        <span className="codeblock-lang">{lang}</span>
        <button type="button" className="codeblock-copy" onClick={copy}>
          {copied ? "已复制" : "复制"}
        </button>
      </div>
      <pre {...props} />
    </div>
  );
}

/** 从 React 节点树里抠出纯文本。高亮后的代码是嵌套 span，得递归。 */
function extractText(node: React.ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(extractText).join("");
  if (typeof node === "object" && "props" in node) {
    return extractText((node.props as { children?: React.ReactNode }).children);
  }
  return "";
}
