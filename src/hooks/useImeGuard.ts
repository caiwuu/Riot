import { type KeyboardEvent, useCallback, useRef } from "react";

export interface ImeGuard {
  onCompositionStart: () => void;
  onCompositionEnd: () => void;
  /** 这一记按键是不是 IME 组字的一部分。是的话别拿它当快捷键。 */
  isComposing: (e: KeyboardEvent<Element>) => boolean;
}

/**
 * 中文/日文输入法组字期间的回车防误触。
 *
 * 拼音打字时回车是"确认候选、上屏"，不是"提交"。只判 `isComposing` 不够：
 * 确认候选那一下的 keydown 常常排在 compositionend 之后到达，此时
 * `isComposing` 已经翻回 false —— 用户刚把词打上屏，这一记回车就把半句话
 * 提交了出去。
 *
 * 三重保险：`isComposing`（标准字段）、`keyCode === 229`（IME 处理中的占位
 * 码，部分 WebView 上比前者准）、外加一个自己维护的 ref 盖住 compositionend
 * 之后的那一拍。
 */
export function useImeGuard(): ImeGuard {
  const composing = useRef(false);

  const onCompositionStart = useCallback(() => {
    composing.current = true;
  }, []);

  const onCompositionEnd = useCallback(() => {
    // compositionend 与确认用的 Enter 可能跨到下一个宏任务，microtask 不够。
    setTimeout(() => {
      composing.current = false;
    }, 0);
  }, []);

  const isComposing = useCallback(
    (e: KeyboardEvent<Element>) =>
      e.nativeEvent.isComposing || e.keyCode === 229 || composing.current,
    [],
  );

  return { onCompositionStart, onCompositionEnd, isComposing };
}
