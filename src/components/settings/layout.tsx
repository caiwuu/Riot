import type { ReactNode } from "react";

/**
 * 设置页的版式件。
 *
 * `[取舍]` 分区正文一律走这四件套，不再各写各的 `section > h2 + 裸表单`。
 * 之前每个分区自己排版，结果同一页里标签有的在左有的在上、输入框宽度
 * 各不相同 —— 看着像九个人分别做的九个页面。这里把"一行设置长什么样"
 * 收成一个决定：左边是名字和说明，右边是控件，行装在卡片里。
 */

/** 分区页头：大标题 + 一句话说明。每个分区顶上都该有，用户得知道自己在哪。 */
export function PaneHead({ title, desc }: { title: string; desc?: ReactNode }) {
  return (
    <header className="pane-head">
      <h1>{title}</h1>
      {desc ? <p>{desc}</p> : null}
    </header>
  );
}

/** 一组设置。标题是小号灰字；`action` 挂在标题右端（刷新、添加这类）。 */
export function Group({
  title,
  desc,
  action,
  children,
}: {
  title?: ReactNode;
  desc?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="set-group">
      {title || action ? (
        <div className="set-group-head">
          {title ? <h2>{title}</h2> : <span />}
          {action}
        </div>
      ) : null}
      {desc ? <p className="set-group-desc">{desc}</p> : null}
      {children}
    </section>
  );
}

/** 卡片。里面的 `Row` 之间自动出分隔线，不用各自画边。 */
export function Card({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={className ? `set-card ${className}` : "set-card"}>{children}</div>;
}

/**
 * 卡片里的一行：左边名字 + 说明，右边控件。
 *
 * `stack` 给"控件本身要占满一行"的字段用（长路径、多行文本）——
 * 硬塞进右列的话，输入框会被挤成一条缝。
 */
export function Row({
  title,
  desc,
  children,
  stack,
  htmlFor,
}: {
  title: ReactNode;
  desc?: ReactNode;
  children?: ReactNode;
  stack?: boolean;
  htmlFor?: string;
}) {
  const label = htmlFor ? (
    <label className="set-row-title" htmlFor={htmlFor}>
      {title}
    </label>
  ) : (
    <span className="set-row-title">{title}</span>
  );
  return (
    <div className={stack ? "set-row stack" : "set-row"}>
      <div className="set-row-text">
        {label}
        {desc ? <p className="set-row-desc">{desc}</p> : null}
      </div>
      {children ? <div className="set-row-ctl">{children}</div> : null}
    </div>
  );
}

/** 整行都是自定义内容的卡片行（列表、进度、说明块）。 */
export function CardBlock({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={className ? `set-block ${className}` : "set-block"}>{children}</div>;
}
