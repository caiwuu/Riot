/** 欢迎页（没有活跃会话时的主区）。从 App.tsx 拆出。 */

import { basename, parentOf, tildify } from "../pathDisplay";
import { FolderIcon } from "./icons";

/**
 * 欢迎页的插画。
 *
 * 手写 SVG 而不是位图：描边直接引用主题变量，换主题不用重新导出；
 * 任何 DPI 都锐利，不用准备 @2x/@3x；整体不到 2KB，不进构建产物的
 * 资源表。位图在这三件事上都要额外维护，而它换来的表现力这里用不上。
 */
function WelcomeArt() {
  return (
    <svg className="welcome-art" viewBox="0 0 200 128" fill="none" aria-hidden>
      {/* 这里曾经有一圈径向渐变光晕。删掉了：在纯色深底上，大面积的
          低不透明度柔光会因为 8 位色深出现色带，看起来像一块脏斑而不是
          发光。这套界面是平的、低对比的，本来就没有光源可言。 */}

      {/* 往后叠的两层：会话堆在同一个项目下 */}
      <rect
        x="54" y="18" width="92" height="58" rx="9"
        stroke="var(--border-strong)" strokeWidth="1.5" opacity="0.5"
      />
      <rect
        x="44" y="27" width="112" height="64" rx="10"
        fill="var(--bg)" stroke="var(--border-strong)" strokeWidth="1.5" opacity="0.85"
      />

      {/* 最前面那层：当前会话 */}
      <rect
        x="32" y="36" width="136" height="72" rx="11"
        fill="var(--bg-card)" stroke="var(--border-strong)" strokeWidth="1.5"
      />
      {/* 顶边一道高光，让最前面这层有厚度 */}
      <path d="M43.5 36.75h113" stroke="var(--text)" strokeWidth="1.2" opacity="0.07" />
      <path d="M32 51h136" stroke="var(--border)" strokeWidth="1.5" />
      <circle cx="43" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.7" />
      <circle cx="50" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.5" />
      <circle cx="57" cy="43.5" r="1.75" fill="var(--text-faint)" opacity="0.35" />

      {/* 提示符 + 三行"代码" */}
      <path
        d="M44 62.5l4 3-4 3" stroke="var(--text-faint)"
        strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round"
      />
      <g fill="var(--text-faint)">
        <rect x="55" y="63.5" width="48" height="4" rx="2" opacity="0.5" />
        <rect x="44" y="78" width="74" height="4" rx="2" opacity="0.3" />
        <rect x="44" y="92" width="38" height="4" rx="2" opacity="0.3" />
      </g>
      {/* 光标。唯一会动的东西，一点生气就够了 */}
      <rect className="wa-caret" x="86" y="90" width="2.5" height="8" rx="1.25" fill="var(--ok)" />
    </svg>
  );
}

/** 欢迎页最多列几个最近项目。再多就该去侧边栏找了。 */
const RECENT_LIMIT = 4;

export function Welcome({
  projects,
  missing,
  onNewSession,
  onOpenProject,
}: {
  projects: string[];
  missing: ReadonlySet<string>;
  onNewSession: (root: string) => void;
  onOpenProject: () => void;
}) {
  const recent = projects.slice(0, RECENT_LIMIT);

  return (
    <div className="welcome">
      <WelcomeArt />
      <h1>Riot</h1>
      <p>每个会话绑定一个项目目录。</p>

      {/* 按钮标签只放短动词。之前这里是「在 codeTest 开新会话」——
          把一句话塞进按钮，目录名还在中间，名字一长按钮就跟着变形。
          项目本身是数据，该列出来让人挑，不该编进标签里。 */}
      <button className="primary big" onClick={onOpenProject}>
        打开目录…
      </button>

      {recent.length > 0 ? (
        <div className="recent">
          <div className="recent-label">最近</div>
          {recent.map((root) => (
            <button
              key={root}
              className={missing.has(root) ? "recent-row gone" : "recent-row"}
              onClick={() => onNewSession(root)}
            >
              <FolderIcon />
              <span className="recent-name">{basename(root)}</span>
              {/* 只显示父目录。完整路径的最后一段就是左边那个名字，
                  重复一遍既占地方又要截断。失效项改说「找不到」，
                  父目录还在也帮不上忙。 */}
              <span className="recent-path">
                {missing.has(root) ? "找不到这个目录" : tildify(parentOf(root))}
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
