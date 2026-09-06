// @ts-check
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

/**
 * 前端 lint。存在的首要理由是把 `src/bridge/index.ts` 顶部那条 `[约束]`
 * 变成机器可查的规则 —— 在此之前它只是一句注释，而 App.tsx 里已经漏进过
 * 一处直连 `@tauri-apps/plugin-notification`。
 *
 * 宿主 API 的限制必须同时拦静态和动态两种写法：`no-restricted-imports`
 * 只看 `import ... from`，而 bridge 之外的逃逸恰恰是 `await import(...)`。
 * 动态那半靠下面的 `no-restricted-syntax` 选择器。
 */

/** bridge 之外一律不许碰宿主 API。两条规则共用这段说明。 */
const BRIDGE_ONLY =
  "宿主 API 只能在 src/bridge/ 里调用，其余代码 import bridge 导出的函数。" +
  "绕过这层，前端就无法脱离 Tauri 单独跑起来（调试、组件测试全部失效），mock 也无处下手。";

const NO_TAURI_DYNAMIC_IMPORT = {
  selector: "ImportExpression[source.value=/^@tauri-apps/]",
  message: BRIDGE_ONLY,
};

/**
 * 界面文案一律走 `src/i18n` 的 `t()`，代码里不许写死中文。
 *
 * 只拦汉字：这是"漏了没翻"最可靠的信号 —— 英文字面量分不清是文案还是
 * 标识符（className、事件名、URL），汉字一定是给人看的。日志（console.*）
 * 用英文写，不进词典。词典目录本身是唯一的例外。
 */
const NO_HARDCODED_TEXT =
  "界面文案要放进 src/i18n/messages 并用 t() 取，代码里不写死中文；日志请用英文。";
const HAN = "/[\\u4e00-\\u9fff\\u3400-\\u4dbf\\uff01-\\uff5e\\u3000-\\u303f]/";
const NO_HARDCODED_TEXT_RULES = [
  { selector: `Literal[value=${HAN}]`, message: NO_HARDCODED_TEXT },
  { selector: `TemplateElement[value.cooked=${HAN}]`, message: NO_HARDCODED_TEXT },
  { selector: `JSXText[value=${HAN}]`, message: NO_HARDCODED_TEXT },
];

export default tseslint.config(
  // 非前端源码一律排除。少一条的代价不是"多几条告警"而是这条命令没法用：
  // `target` 里是 Rust 的构建产物（打包进去的第三方 JS），`pnpm exec eslint .`
  // 会在那里刨出九千多条错误、跑掉十几秒，真正的告警全被冲走。
  // `src-tauri` 和 `public` 同理 —— 那边的 JS 不归这套规则管。
  {
    ignores: [
      "dist",
      "src/bridge/generated.ts",
      "vendor",
      "website",
      "scripts",
      "target",
      "src-tauri",
      "public",
    ],
  },

  js.configs.recommended,
  ...tseslint.configs.recommended,

  {
    files: ["src/**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    languageOptions: {
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    rules: {
      ...reactHooks.configs.recommended.rules,

      // 依赖数组：这批规则从来没跑过，存量有 10 处 disable 注释压着一个
      // 不存在的规则。先开成 warn 让新代码受约束，存量另行清理。
      "react-hooks/exhaustive-deps": "warn",

      "no-restricted-imports": [
        "error",
        { patterns: [{ group: ["@tauri-apps/*"], message: BRIDGE_ONLY }] },
      ],
      "no-restricted-syntax": ["error", NO_TAURI_DYNAMIC_IMPORT, ...NO_HARDCODED_TEXT_RULES],

      // 下划线前缀 = 有意不用（解构丢弃、占位参数）。
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrors: "none" },
      ],
    },
  },

  // bridge 是唯一的例外，它的职责就是把宿主 API 包起来。文案的规则照常。
  {
    files: ["src/bridge/**/*.ts"],
    rules: {
      "no-restricted-imports": "off",
      "no-restricted-syntax": ["error", ...NO_HARDCODED_TEXT_RULES],
    },
  },

  // 词典就是放中文的地方；语言清单里各语言的本名同理。
  {
    files: ["src/i18n/messages/**/*.ts", "src/i18n/locales.ts"],
    rules: {
      "no-restricted-syntax": ["error", NO_TAURI_DYNAMIC_IMPORT],
    },
  },

  // Web Worker 里没有 window，globals 与主线程不同。
  {
    files: ["src/lib/*.worker.ts"],
    languageOptions: { globals: { self: "readonly", postMessage: "readonly" } },
  },
);
