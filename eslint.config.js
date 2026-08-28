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

export default tseslint.config(
  { ignores: ["dist", "src/bridge/generated.ts", "vendor", "website", "scripts"] },

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
      "no-restricted-syntax": [
        "error",
        {
          selector: "ImportExpression[source.value=/^@tauri-apps/]",
          message: BRIDGE_ONLY,
        },
      ],

      // 下划线前缀 = 有意不用（解构丢弃、占位参数）。
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrors: "none" },
      ],
    },
  },

  // bridge 是唯一的例外，它的职责就是把宿主 API 包起来。
  {
    files: ["src/bridge/**/*.ts"],
    rules: {
      "no-restricted-imports": "off",
      "no-restricted-syntax": "off",
    },
  },

  // Web Worker 里没有 window，globals 与主线程不同。
  {
    files: ["src/lib/*.worker.ts"],
    languageOptions: { globals: { self: "readonly", postMessage: "readonly" } },
  },
);
