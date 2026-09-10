/* ============================================================
   Riot 官网交互脚本
   ============================================================ */

/**
 * 下载地址跟着 GitHub 最新正式 Release 走。
 * 发 Riot_0.1.1 时资产会变成 Riot_0.1.1_aarch64.dmg，写死 0.1.0 就会下到旧包。
 * 没拉到接口时退回 /releases/latest 页面，不要链到某个过期文件名。
 */
const RELEASES_PAGE = "https://github.com/caiwuu/Riot/releases/latest";
const RELEASES_API = "https://api.github.com/repos/caiwuu/Riot/releases/latest";
const RELEASE_CACHE = "riot.latest-release.v3"; // 换键即可作废所有旧格式缓存
const RELEASE_TTL = 10 * 60 * 1000; // 缓存 10 分钟：长开的标签页里也能拿到新版本

const DOWNLOADS = {
  mac: {
    url: RELEASES_PAGE,
    label: "下载 macOS 版",
    meta: ".dmg · 仅 Apple Silicon",
    size: 0,
  },
  win: {
    url: RELEASES_PAGE,
    label: "下载 Windows 版",
    meta: "NSIS 安装包 · x64",
    size: 0,
  },
};

/** 全站统一版本号，注入 HTML 里所有 .js-version 占位。拉到最新 Release 后再改。 */
let VERSION = "";

function versionLabel(tag) {
  const v = String(tag || "")
    .replace(/^Riot_/i, "")
    .replace(/^v/i, "");
  return v ? `v${v}` : "";
}

function pickAsset(assets, suffix) {
  return (assets || []).find(
    (a) => typeof a.name === "string" && a.name.endsWith(suffix),
  );
}

function assetInfo(assets, suffix) {
  const a = pickAsset(assets, suffix);
  if (!a) return { url: "", size: 0 };
  return { url: a.browser_download_url, size: Number(a.size) || 0 };
}

function formatSize(bytes) {
  if (!bytes) return "";
  return `约 ${Math.round(bytes / (1024 * 1024))} MB`;
}

async function loadLatestRelease() {
  try {
    const cached = sessionStorage.getItem(RELEASE_CACHE);
    if (cached) {
      const { t, data } = JSON.parse(cached);
      // 只信任 TTL 内的缓存；过期就重新请求，保证发新版后很快能看到
      if (data && Date.now() - t < RELEASE_TTL) return data;
    }
  } catch {
    /* 隐私模式 / 禁用存储 / 旧格式 */
  }
  const res = await fetch(RELEASES_API, {
    headers: { Accept: "application/vnd.github+json" },
  });
  if (!res.ok) throw new Error(`releases/latest ${res.status}`);
  const data = await res.json();
  const slim = {
    tag: data.tag_name,
    mac: assetInfo(data.assets, "_aarch64.dmg"),
    win: assetInfo(data.assets, "_x64-setup.exe"),
  };
  try {
    sessionStorage.setItem(
      RELEASE_CACHE,
      JSON.stringify({ t: Date.now(), data: slim }),
    );
  } catch {
    /* ignore */
  }
  return slim;
}

async function initDownloads() {
  try {
    const latest = await loadLatestRelease();
    if (latest.mac?.url) {
      DOWNLOADS.mac.url = latest.mac.url;
      DOWNLOADS.mac.size = latest.mac.size;
    }
    if (latest.win?.url) {
      DOWNLOADS.win.url = latest.win.url;
      DOWNLOADS.win.size = latest.win.size;
    }
    VERSION = versionLabel(latest.tag);
  } catch {
    /* 没网就停在 Release 页 */
  }
  applyOS();
  initVersion();
  initDownloadMeta();
}

const OS_ICONS = {
  mac: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M17.05 20.28c-.98.95-2.05.8-3.08.35-1.09-.46-2.09-.48-3.24 0-1.44.62-2.2.44-3.06-.35C2.79 15.25 3.51 7.59 9.05 7.31c1.35.07 2.29.74 3.08.8 1.18-.24 2.31-.93 3.57-.84 1.51.12 2.65.72 3.4 1.8-3.12 1.87-2.38 5.98.48 7.13-.57 1.5-1.31 2.99-2.54 4.09l.01-.01zM12.03 7.25c-.15-2.23 1.66-4.07 3.74-4.25.29 2.58-2.34 4.5-3.74 4.25z"/></svg>',
  win: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M3 5.5 10.5 4.4v7.1H3V5.5ZM3 12.5h7.5v7.1L3 18.5v-6ZM11.5 4.25 21 3v8.5h-9.5V4.25ZM21 12.5V21l-9.5-1.35V12.5H21Z"/></svg>',
};

/** Touch devices should see desktop download options, not a macOS installer. */
function detectOS() {
  const platform =
    navigator.userAgentData?.platform || navigator.platform || "";
  const ua = navigator.userAgent || "";
  if (
    /Android|iPhone|iPad|iPod/i.test(ua) ||
    (/Mac/i.test(platform) && navigator.maxTouchPoints > 1)
  )
    return null;
  if (/mac/i.test(platform) || /Mac OS X|Macintosh/i.test(ua)) return "mac";
  if (/win/i.test(platform) || /Windows/i.test(ua)) return "win";
  return null;
}

function applyOS() {
  document.querySelectorAll(".js-dl-mac").forEach((link) => {
    link.href = DOWNLOADS.mac.url;
  });
  document.querySelectorAll(".js-dl-win").forEach((link) => {
    link.href = DOWNLOADS.win.url;
  });
  const os = detectOS();
  if (!os) return;
  const download = DOWNLOADS[os];
  document.querySelectorAll(".js-primary-download").forEach((link) => {
    link.href = download.url;
  });
  document.getElementById("hero-download-label").textContent = download.label;
  document.getElementById("hero-os-icon").innerHTML = OS_ICONS[os];
  document.getElementById("hero-download-meta").textContent = download.meta;
  document.getElementById("download-detect").textContent =
    os === "mac"
      ? "macOS 安装包适用于 Apple Silicon 芯片。"
      : "Windows 安装包适用于 x64 设备。";
  const card = document.getElementById(
    os === "mac" ? "dl-card-mac" : "dl-card-win",
  );
  card.classList.add("recommended");
  card.querySelector(".dl-recommend").hidden = false;
}

function initNav() {
  const nav = document.getElementById("nav");
  const onScroll = () => nav.classList.toggle("scrolled", window.scrollY > 32);
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();
}

function initMobileMenu() {
  const burger = document.getElementById("nav-burger");
  const menu = document.getElementById("mobile-menu");
  const desktop = window.matchMedia("(min-width: 701px)");
  const setOpen = (open) => {
    menu.hidden = !open;
    burger.setAttribute("aria-expanded", String(open));
    burger.setAttribute("aria-label", open ? "关闭菜单" : "打开菜单");
  };
  burger.addEventListener("click", () => setOpen(menu.hidden));
  menu.querySelectorAll("a").forEach((link) =>
    link.addEventListener("click", () => {
      setOpen(false);
      if (link.hash) {
        const target = document.getElementById(link.hash.slice(1));
        if (target) {
          target.setAttribute("tabindex", "-1");
          target.focus({ preventScroll: true });
        }
      } else {
        burger.focus();
      }
    }),
  );
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !menu.hidden) {
      setOpen(false);
      burger.focus();
    }
  });
  document.addEventListener("click", (event) => {
    if (
      !menu.hidden &&
      !menu.contains(event.target) &&
      !burger.contains(event.target)
    )
      setOpen(false);
  });
  document.addEventListener("focusin", (event) => {
    if (
      !menu.hidden &&
      !menu.contains(event.target) &&
      !burger.contains(event.target)
    )
      setOpen(false);
  });
  desktop.addEventListener("change", () => {
    if (desktop.matches) setOpen(false);
  });
}

/** 页脚年份 */
function initYear() {
  const el = document.getElementById("year");
  if (el) el.textContent = String(new Date().getFullYear());
}

/** 统一注入版本号。没拉到 Release 时占位藏着，避免文案里写死某一个版本。 */
function initVersion() {
  document.querySelectorAll(".js-version").forEach((el) => {
    el.textContent = VERSION;
  });
  document.querySelectorAll(".js-ver").forEach((el) => {
    el.hidden = !VERSION;
  });
}

/** 下载卡片底下的「v0.1.1 · 约 158 MB」跟资产走，包变大了不用改文案。 */
function initDownloadMeta() {
  document.querySelectorAll(".dl-meta[data-os]").forEach((el) => {
    const os = el.getAttribute("data-os");
    const parts = [];
    if (VERSION) parts.push(VERSION);
    if (DOWNLOADS[os]?.size) parts.push(formatSize(DOWNLOADS[os].size));
    el.textContent = parts.join(" · ");
  });
}

applyOS();
initDownloads();
initNav();
initMobileMenu();
initYear();
