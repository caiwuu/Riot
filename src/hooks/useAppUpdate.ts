import { useCallback, useEffect, useState } from "react";

import { appVersion, checkUpdate, type UpdateInfo } from "../bridge";

const DISMISSED = "riot.update.dismissed";

/**
 * 启动查一次 GitHub Release；关于页的「检查更新」走同一份状态。
 *
 * 启动失败保持安静 —— 没网不该挡进应用。手动点检查才把错误亮出来。
 */
export function useAppUpdate(ready: boolean) {
  const [version, setVersion] = useState("");
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [dismissed, setDismissed] = useState(
    () => (typeof localStorage === "undefined" ? "" : (localStorage.getItem(DISMISSED) ?? "")),
  );

  useEffect(() => {
    void appVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  const check = useCallback(async (opts?: { quiet?: boolean }) => {
    setChecking(true);
    if (!opts?.quiet) setError(null);
    try {
      const got = await checkUpdate();
      setInfo(got);
      setVersion(got.current);
      setError(null);
      return got;
    } catch (e) {
      if (!opts?.quiet) setError(String(e));
      return null;
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (!ready) return;
    void check({ quiet: true });
  }, [ready, check]);

  const banner = info?.newer && info.latest && info.latest !== dismissed ? info : null;

  const dismiss = useCallback(() => {
    if (!info?.latest) return;
    localStorage.setItem(DISMISSED, info.latest);
    setDismissed(info.latest);
  }, [info?.latest]);

  return { version, info, error, checking, check, banner, dismiss };
}
