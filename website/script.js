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
const RELEASE_CACHE = "riot.latest-release.v2";

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

function versionLabel (tag) {
  const v = String(tag || "")
    .replace(/^Riot_/i, "")
    .replace(/^v/i, "");
  return v ? `v${v}` : "";
}

function pickAsset (assets, suffix) {
  return (assets || []).find((a) => typeof a.name === "string" && a.name.endsWith(suffix));
}

function assetInfo (assets, suffix) {
  const a = pickAsset(assets, suffix);
  if (!a) return { url: "", size: 0 };
  return { url: a.browser_download_url, size: Number(a.size) || 0 };
}

function formatSize (bytes) {
  if (!bytes) return "";
  return `约 ${Math.round(bytes / (1024 * 1024))} MB`;
}

async function loadLatestRelease () {
  try {
    const cached = sessionStorage.getItem(RELEASE_CACHE);
    if (cached) return JSON.parse(cached);
  } catch {
    /* 隐私模式 / 禁用存储 */
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
    sessionStorage.setItem(RELEASE_CACHE, JSON.stringify(slim));
  } catch {
    /* ignore */
  }
  return slim;
}

async function initDownloads () {
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

const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** 检测访问者操作系统，返回 'mac' | 'win' | null */
function detectOS () {
  const platform =
    (navigator.userAgentData && navigator.userAgentData.platform) ||
    navigator.platform ||
    "";
  const ua = navigator.userAgent || "";

  if (/mac/i.test(platform) || /Mac OS X|Macintosh/i.test(ua)) return "mac";
  if (/win/i.test(platform) || /Windows/i.test(ua)) return "win";
  return null;
}

/** 根据系统更新主下载按钮与下载区状态 */
function applyOS () {
  const os = detectOS();

  // 下载卡片按钮始终指向各自平台
  document.querySelectorAll(".js-dl-mac").forEach((a) => (a.href = DOWNLOADS.mac.url));
  document.querySelectorAll(".js-dl-win").forEach((a) => (a.href = DOWNLOADS.win.url));

  if (!os) return; // 未识别的系统：主按钮保持锚向下载区

  const dl = DOWNLOADS[os];

  // Hero 主按钮与导航下载按钮 → 直接下载对应平台安装包
  document.querySelectorAll(".js-primary-download").forEach((a) => {
    a.href = dl.url;
    a.setAttribute("download", "");
  });

  const heroLabel = document.getElementById("hero-download-label");
  if (heroLabel) heroLabel.textContent = dl.label;

  const heroIcon = document.getElementById("hero-os-icon");
  if (heroIcon) heroIcon.innerHTML = OS_ICONS[os];

  const heroMeta = document.getElementById("hero-download-meta");
  if (heroMeta) heroMeta.textContent = dl.meta;

  // 下载区：高亮推荐卡片
  const detect = document.getElementById("download-detect");
  if (detect) {
    detect.innerHTML =
      os === "mac"
        ? "检测到你正在使用 <strong>macOS</strong>，已为你标出推荐版本。"
        : "检测到你正在使用 <strong>Windows</strong>，已为你标出推荐版本。";
  }

  const card = document.getElementById(os === "mac" ? "dl-card-mac" : "dl-card-win");
  if (card) {
    card.classList.add("recommended");
    const badge = card.querySelector(".dl-recommend");
    if (badge) badge.hidden = false;
  }
}

/** 导航滚动状态：滚离顶部后加深底色并显出细线 */
function initNav () {
  const nav = document.getElementById("nav");
  const onScroll = () => nav.classList.toggle("scrolled", window.scrollY > 32);
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();
}

/** 移动端菜单 */
function initMobileMenu () {
  const burger = document.getElementById("nav-burger");
  const menu = document.getElementById("mobile-menu");
  if (!burger || !menu) return;

  burger.addEventListener("click", () => {
    const open = menu.classList.toggle("open");
    burger.setAttribute("aria-expanded", String(open));
    burger.setAttribute("aria-label", open ? "关闭菜单" : "打开菜单");
  });

  // 点击菜单项后收起
  menu.querySelectorAll("a").forEach((a) =>
    a.addEventListener("click", () => {
      menu.classList.remove("open");
      burger.setAttribute("aria-expanded", "false");
    })
  );
}

/** 滚动显现动画 */
function initReveal () {
  const items = document.querySelectorAll("[data-reveal]");
  if (!("IntersectionObserver" in window)) {
    items.forEach((el) => el.classList.add("revealed"));
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add("revealed");
          observer.unobserve(entry.target);
        }
      });
    },
    { threshold: 0.12, rootMargin: "0px 0px -6% 0px" }
  );

  items.forEach((el) => observer.observe(el));
}

/** Hero 大标题：逐字符浮现（保留 <em> 等内联标签） */
function initHeroTitle () {
  const title = document.getElementById("hero-title");
  if (!title) return;

  if (reduceMotion) {
    title.classList.add("played");
    return;
  }

  let index = 0;
  const STEP_MS = 26;

  const splitNode = (node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      const frag = document.createDocumentFragment();
      for (const ch of node.textContent) {
        if (ch.trim() === "") {
          frag.appendChild(document.createTextNode(ch));
          continue;
        }
        const span = document.createElement("span");
        span.className = "ht-ch";
        span.style.setProperty("--ch-delay", `${index * STEP_MS}ms`);
        span.textContent = ch;
        frag.appendChild(span);
        index += 1;
      }
      node.replaceWith(frag);
      return;
    }
    // <em> 用 background-clip: text 做渐变字，拆开会丢背景——整体作为一个动画单元
    if (node.nodeType === Node.ELEMENT_NODE && node.tagName === "EM") {
      node.classList.add("ht-ch");
      node.style.setProperty("--ch-delay", `${index * STEP_MS}ms`);
      index += Math.max(1, node.textContent.length);
      return;
    }
    // 其他元素节点：递归处理子节点（快照，避免遍历中变更）
    [...node.childNodes].forEach(splitNode);
  };

  [...title.childNodes].forEach(splitNode);

  requestAnimationFrame(() => {
    requestAnimationFrame(() => title.classList.add("played"));
  });
}

/**
 * 动画 Logo：「黑洞与陨石群」。
 * 花瓣形的粒子云是宇宙陨石群；字母 R 不放粒子，作为粒子云中的黑洞剪影，
 * 洞口边缘的陨石被引力撕扯，位置散乱、忽明忽暗。
 * 星空中时常有流星拖着尾迹击穿粒子团，被撞飞的陨石在引力作用下缓缓归位。
 * 鼠标靠近同样会推开陨石。
 * 画布挂在 .hero-sky 内铺满整个首屏：流星横穿全屏，星云锚定在 .hero-logo 容器处成形。
 */
function initParticleLogo () {
  const canvas = document.getElementById("logo-canvas");
  const hero = document.getElementById("hero");
  const logoBox = document.querySelector(".hero-logo");
  if (!canvas || !hero || !logoBox || reduceMotion) return;

  const showFallback = () => {
    canvas.style.display = "none";
    const fallback = logoBox.querySelector(".hl-fallback");
    if (fallback) fallback.style.display = "block";
  };

  const ctx = canvas.getContext("2d");
  if (!ctx) {
    showFallback();
    return;
  }

  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const REPEL_RADIUS = 60;
  const REPEL_FORCE = 4.6;
  const METEOR_HIT_RADIUS = 28; // 流星冲击半径
  const METEOR_FORCE = 8.5;
  const METEOR_TAIL = 96; // 尾迹长度（px）
  const METEOR_G = 2600000; // 黑洞引力常数：流星轨迹被星云中心弯折
  const METEOR_MAX_AGE = 26; // 秒，被引力俘获绕圈的流星最终回收

  /** 预渲染辉光光点：加大实心亮核占比、收紧光晕衰减——光点更「实」，成形的 logo 更清晰 */
  const makeSprite = (r, g, b) => {
    const s = document.createElement("canvas");
    const SIZE = 64;
    s.width = SIZE;
    s.height = SIZE;
    const c = s.getContext("2d");
    const grad = c.createRadialGradient(SIZE / 2, SIZE / 2, 0, SIZE / 2, SIZE / 2, SIZE / 2);
    grad.addColorStop(0, `rgba(${r},${g},${b},1)`);
    grad.addColorStop(0.22, `rgba(${r},${g},${b},0.98)`);
    grad.addColorStop(0.35, `rgba(${r},${g},${b},0.22)`);
    grad.addColorStop(0.55, `rgba(${r},${g},${b},0.04)`);
    grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
    c.fillStyle = grad;
    c.fillRect(0, 0, SIZE, SIZE);
    return s;
  };

  /* 能量色带：0 吸积盘白 → 1 青蓝 → 2 紫 → 3 外缘深紫 */
  const SPRITES = [
    makeSprite(244, 248, 255),
    makeSprite(168, 200, 255),
    makeSprite(156, 146, 244),
    makeSprite(110, 102, 198),
  ];

  let W = 0;
  let H = 0;
  let holeX = 0; // 星云 / 黑洞中心（画布坐标）：锚定在 .hero-logo 容器中心
  let holeY = 0;
  let particles = [];
  let dust = []; // 吸积流星尘：被引力捕获，螺旋内落汇入 R 轮廓
  let nebulaR = 100;
  let captureR = 60;
  let meteors = [];
  let nextMeteorAt = 0;
  let lastFrame = 0;
  let running = false;
  let built = false;
  let revealStart = 0;
  let mouseX = -9999;
  let mouseY = -9999;

  const img = new Image();
  img.src = "assets/riot-logo.png";

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    W = Math.max(1, rect.width);
    H = Math.max(1, rect.height);
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  };

  /** 星尘重生：回到远处，重新开始被引力捕获的漂流 */
  const respawnDust = (d) => {
    d.r = nebulaR * (1.3 + Math.random() * 1.1);
    d.a = Math.random() * Math.PI * 2;
    d.w = 0.015 + Math.random() * 0.028; // 同向公转（rad/s），星系盘的旋转场
    d.rate = 1.1 + Math.random() * 2.1; // 内落速度（px/s），非常缓慢
    d.fade = 1;
    d.sprite = Math.random() < 0.55 ? 1 : Math.random() < 0.6 ? 2 : 3;
    d.alpha = 0.14 + Math.random() * 0.24;
    d.draw = 2.4 + Math.random() * 3.6;
  };

  /** 采样 logo 像素，生成粒子锚点（logo 位置/尺寸跟随 .hero-logo 容器，画布铺满整个首屏） */
  const build = () => {
    resize();
    particles = [];

    const box = logoBox.getBoundingClientRect();
    const canvasBox = canvas.getBoundingClientRect();
    const logoSize = Math.max(40, Math.round(Math.min(box.width, box.height) * 0.88));
    const off = document.createElement("canvas");
    off.width = logoSize;
    off.height = logoSize;
    const octx = off.getContext("2d", { willReadFrequently: true });
    octx.drawImage(img, 0, 0, logoSize, logoSize);

    let data;
    try {
      data = octx.getImageData(0, 0, logoSize, logoSize).data;
    } catch {
      return; // 极端环境（file:// 且被视为跨域）下放弃粒子，兜底静态图
    }

    const idx = (x, y) => (y * logoSize + x) * 4;
    const lumAt = (x, y) => {
      const i = idx(x, y);
      return 0.299 * data[i] + 0.587 * data[i + 1] + 0.114 * data[i + 2];
    };

    // 判断图片是否带透明背景
    let transparentCount = 0;
    let sampled = 0;
    for (let i = 3; i < data.length; i += 4 * 89) {
      sampled += 1;
      if (data[i] < 10) transparentCount += 1;
    }
    const hasAlpha = transparentCount > sampled * 0.08;

    // 白底图：按行/列统计暗像素范围，用于区分「背景白」与「形状内部的白（字母）」
    const DARK = 130;
    const LIGHT = 195;
    let rowMin, rowMax, colMin, colMax;
    if (!hasAlpha) {
      rowMin = new Int16Array(logoSize).fill(logoSize);
      rowMax = new Int16Array(logoSize).fill(-1);
      colMin = new Int16Array(logoSize).fill(logoSize);
      colMax = new Int16Array(logoSize).fill(-1);
      for (let y = 0; y < logoSize; y++) {
        for (let x = 0; x < logoSize; x++) {
          if (lumAt(x, y) < DARK) {
            if (x < rowMin[y]) rowMin[y] = x;
            if (x > rowMax[y]) rowMax[y] = x;
            if (y < colMin[x]) colMin[x] = y;
            if (y > colMax[x]) colMax[x] = y;
          }
        }
      }
    }

    /** 'body' | 'letter' | null */
    const classify = (x, y) => {
      const lum = lumAt(x, y);
      if (hasAlpha) {
        if (data[idx(x, y) + 3] < 120) return null;
        return lum >= LIGHT ? "letter" : "body";
      }
      if (lum < DARK) return "body";
      if (lum >= LIGHT) {
        // 形状内部的亮像素（左右上下都被暗像素包住）→ 字母
        const inRow = x > rowMin[y] + 2 && x < rowMax[y] - 2;
        const inCol = y > colMin[x] + 2 && y < colMax[x] - 2;
        if (inRow && inCol) return "letter";
      }
      return null;
    };

    // 字母掩码：R 区域是黑洞，不放粒子，只用来找「洞口边缘」。
    const letterMask = new Uint8Array(logoSize * logoSize);
    for (let y = 0; y < logoSize; y++) {
      for (let x = 0; x < logoSize; x++) {
        if (classify(x, y) === "letter") letterMask[y * logoSize + x] = 1;
      }
    }

    /*
     * 亮区里除了字母还有花瓣的 3D 高光。做连通域分析，
     * 只保留质心最靠近图像中心的那一块——那就是 R，高光全部剔除。
     */
    {
      const labels = new Int32Array(logoSize * logoSize).fill(-1);
      const stack = [];
      const regions = []; // { label, count, cx, cy }
      let label = 0;
      for (let start = 0; start < letterMask.length; start++) {
        if (!letterMask[start] || labels[start] !== -1) continue;
        let count = 0;
        let sumX = 0;
        let sumY = 0;
        labels[start] = label;
        stack.push(start);
        while (stack.length) {
          const j = stack.pop();
          const jx = j % logoSize;
          const jy = (j / logoSize) | 0;
          count += 1;
          sumX += jx;
          sumY += jy;
          if (jx > 0 && letterMask[j - 1] && labels[j - 1] === -1) {
            labels[j - 1] = label;
            stack.push(j - 1);
          }
          if (jx < logoSize - 1 && letterMask[j + 1] && labels[j + 1] === -1) {
            labels[j + 1] = label;
            stack.push(j + 1);
          }
          if (jy > 0 && letterMask[j - logoSize] && labels[j - logoSize] === -1) {
            labels[j - logoSize] = label;
            stack.push(j - logoSize);
          }
          if (jy < logoSize - 1 && letterMask[j + logoSize] && labels[j + logoSize] === -1) {
            labels[j + logoSize] = label;
            stack.push(j + logoSize);
          }
        }
        regions.push({ label, count, cx: sumX / count, cy: sumY / count });
        label += 1;
      }

      const half = logoSize / 2;
      const minCount = logoSize * logoSize * 0.002; // 过小的亮斑直接忽略
      let keep = -1;
      let bestDist = Infinity;
      for (const region of regions) {
        if (region.count < minCount) continue;
        const dist = Math.hypot(region.cx - half, region.cy - half);
        if (dist < bestDist) {
          bestDist = dist;
          keep = region.label;
        }
      }
      for (let i = 0; i < letterMask.length; i++) {
        if (letterMask[i] && labels[i] !== keep) letterMask[i] = 0;
      }
    }

    /** 到黑洞（R）的最小距离，用于能量分带：越靠近吸积盘越亮 */
    const SCAN = 17;
    const holeDist = (x, y) => {
      let best = Infinity;
      for (let dy = -SCAN; dy <= SCAN; dy++) {
        const ny = y + dy;
        if (ny < 0 || ny >= logoSize) continue;
        for (let dx = -SCAN; dx <= SCAN; dx++) {
          const nx = x + dx;
          if (nx < 0 || nx >= logoSize) continue;
          if (letterMask[ny * logoSize + nx]) {
            const d2 = dx * dx + dy * dy;
            if (d2 < best) best = d2;
          }
        }
      }
      return Math.sqrt(best);
    };

    const step = Math.max(2, Math.round(logoSize / 74));
    // logo 在画布坐标系中的落点：两个矩形同帧测量，与滚动位置无关
    const offX = box.left - canvasBox.left + (box.width - logoSize) / 2;
    const offY = box.top - canvasBox.top + (box.height - logoSize) / 2;
    const logoR = logoSize / 2;
    holeX = offX + logoR;
    holeY = offY + logoR;

    for (let y = 0; y < logoSize; y += step) {
      for (let x = 0; x < logoSize; x += step) {
        const cls = classify(x, y);
        if (cls === null) continue; // 背景不放粒子

        const isLetter = letterMask[y * logoSize + x] === 1;
        const dHole = isLetter ? 0 : holeDist(x, y);

        /* 能量分带：实心 R（炽白恒星）→ 近旁青蓝辉光 → 中盘紫 → 外缘深紫 */
        let sprite;
        let alpha;
        let size;
        const disk = isLetter;
        if (isLetter) {
          sprite = 0;
          alpha = 0.9 + Math.random() * 0.1;
          size = 1.5 + Math.random() * 0.9;
        } else if (dHole <= 7) {
          sprite = 1;
          alpha = 0.55 + Math.random() * 0.28;
          size = 1.25 + Math.random() * 0.85;
        } else if (dHole <= 15) {
          sprite = 1;
          alpha = 0.58 + Math.random() * 0.32;
          size = 1.2 + Math.random() * 0.9;
        } else if (dHole <= 28) {
          sprite = 2;
          alpha = 0.5 + Math.random() * 0.3;
          size = 1.1 + Math.random() * 0.85;
        } else {
          sprite = 3;
          alpha = 0.42 + Math.random() * 0.26;
          size = 1.0 + Math.random() * 0.8;
        }

        /* 花瓣外缘渐暗，星云边界融进黑底 */
        const dc = Math.hypot(x - logoR, y - logoR) / logoR;
        const dimEdge = 1 - 0.32 * Math.min(1, Math.max(0, (dc - 0.5) / 0.5));

        /* 实心 R 加密：恒星矩阵生成两颗，密实闪耀 */
        const copies = disk ? 2 : 1;
        for (let c = 0; c < copies; c++) {
          /* 景深：z 越大越近 → 更大更亮 */
          const z = 0.55 + Math.random() * 0.9;
          let a0 = alpha * dimEdge * (0.58 + 0.42 * ((z - 0.55) / 0.9));

          // R 内部只轻微抖动保持密实，副本颗错位补缝
          const spread = disk ? (c === 0 ? 2 : 4) : 0;
          const jx = (Math.random() - 0.5) * spread;
          const jy = (Math.random() - 0.5) * spread;
          const hx = offX + x + jx;
          const hy = offY + y + jy;
          const angle = Math.random() * Math.PI * 2;
          const dist = 70 + Math.random() * 180;

          /* 外圈尘埃绕黑洞极缓慢公转（避开 R 外接圆，公转不会盖住黑洞剪影）；
             0.72–0.87 半径带角速度渐增，内外过渡无剪切缝 */
          const orbitR = Math.hypot(hx - holeX, hy - holeY);
          const wf = Math.min(1, Math.max(0, (orbitR / logoR - 0.72) / 0.15));
          const orbit =
            wf > 0
              ? {
                r: orbitR,
                a: Math.atan2(hy - holeY, hx - holeX),
                w: (0.014 + Math.random() * 0.018) * wf,
              }
              : null;

          particles.push({
            hx,
            hy,
            x: hx + Math.cos(angle) * dist,
            y: hy + Math.sin(angle) * dist,
            vx: 0,
            vy: 0,
            draw: size * 5.4 * z, // 辉光绘制直径（亮核更实、光晕更收，成形更清晰）
            sprite,
            alpha: a0,
            orbit,
            spring: 0.02 + Math.random() * 0.025,
            springScale: 1, // 被流星击中后骤降，随时间缓慢恢复 → 「引力慢慢拉回」
            phase: Math.random() * Math.PI * 2,
            speed: disk ? 0.1 + Math.random() * 0.18 : 0.06 + Math.random() * 0.14,
            drift: disk ? 1.1 : 0.8,
            twinkle: disk ? Math.random() < 0.4 : Math.random() < 0.14,
          });
        }
      }
    }

    /*
     * 吸积流星尘：散落在星空中的微尘被黑洞引力捕获，
     * 一边极缓慢公转一边螺旋内落，抵达 R 轮廓时亮起、融入吸积盘，再于远处重生。
     */
    nebulaR = logoR;
    captureR = logoSize * 0.34; // R 外接圆附近 = 「落到 R 的轮廓上」
    dust = [];
    for (let i = 0; i < 130; i++) {
      const d = {};
      respawnDust(d);
      // 初始就散布在内落途中的不同半径，画面一开始就是进行中的吸积流
      d.r = captureR + (nebulaR * 2.2 - captureR) * Math.random();
      dust.push(d);
    }

    built = particles.length > 0;
    revealStart = 0; // 重建后重新走一遍淡入与聚合增强
  };

  /**
   * 生成一颗流星：从画布外随机方向射入，路径随机——
   * 不一定穿过星云，但轨迹会被黑洞引力弯折，擦得足够近才会击穿粒子团。
   */
  const spawnMeteor = () => {
    const ang = Math.random() * Math.PI * 2;
    const rOut = Math.hypot(W, H) / 2 + 80;
    const sx = W / 2 + Math.cos(ang) * rOut;
    const sy = H / 2 + Math.sin(ang) * rOut;
    // 目标点在画布内随机：大多数流星只是路过星空
    const tx = W * (0.12 + Math.random() * 0.76);
    const ty = H * (0.12 + Math.random() * 0.76);
    const d = Math.hypot(tx - sx, ty - sy) || 1;
    const speed = 80 + Math.random() * 50; // px/s，宇宙尺度的慢

    meteors.push({
      x: sx,
      y: sy,
      vx: ((tx - sx) / d) * speed,
      vy: ((ty - sy) / d) * speed,
      age: 0,
    });
  };

  const render = (t) => {
    if (!running || document.hidden) return;

    const time = t * 0.001;
    if (!revealStart) revealStart = t;
    const reveal = Math.min(1, (t - revealStart) / 2400);

    const dt = Math.min(0.05, lastFrame ? (t - lastFrame) / 1000 : 0.016);
    lastFrame = t;

    // 流星调度：入场汇聚完成后开始，之后每 1.75–4 秒一颗
    if (!nextMeteorAt) nextMeteorAt = revealStart + 3000;
    if (t >= nextMeteorAt) {
      spawnMeteor();
      nextMeteorAt = t + 1750 + Math.random() * 2250;
    }

    // 更新流星：受黑洞（星云中心）引力弯折轨迹；出界或过久（被俘获绕圈）回收
    const gcx = holeX;
    const gcy = holeY;
    const margin = 140;
    for (let i = meteors.length - 1; i >= 0; i--) {
      const m = meteors[i];
      const gdx = gcx - m.x;
      const gdy = gcy - m.y;
      const gd2 = Math.max(gdx * gdx + gdy * gdy, 4900); // 最小 70px，避免引力发散
      const gd = Math.sqrt(gd2);
      const ga = (METEOR_G / gd2) * dt;
      m.vx += (gdx / gd) * ga;
      m.vy += (gdy / gd) * ga;
      m.x += m.vx * dt;
      m.y += m.vy * dt;
      m.age += dt;
      if (
        m.age > METEOR_MAX_AGE ||
        m.x < -margin ||
        m.x > W + margin ||
        m.y < -margin ||
        m.y > H + margin
      ) {
        meteors.splice(i, 1);
      }
    }

    ctx.clearRect(0, 0, W, H);
    ctx.globalCompositeOperation = "lighter"; // 加色混合：光点重叠即增亮

    /* 整个星云极缓慢地摇摆与呼吸（绕星云自身中心，而非画布中心） */
    const cx = holeX;
    const cy = holeY;
    const sway = Math.sin(time * 0.05) * 0.04;
    const breath = 1 + Math.sin(time * 0.08) * 0.014;
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate(sway);
    ctx.scale(breath, breath);
    ctx.translate(-cx, -cy);

    /* 吸积流星尘：公转 + 螺旋内落，抵达 R 轮廓时亮起融入吸积盘，然后在远处重生 */
    for (let i = 0; i < dust.length; i++) {
      const d = dust[i];
      d.a += d.w * dt;
      if (d.fade < 1) {
        d.fade -= dt / 0.9;
        if (d.fade <= 0) {
          respawnDust(d);
          continue;
        }
      } else {
        d.r -= d.rate * dt;
        if (d.r <= captureR) d.fade = 0.999; // 触达 R 轮廓：开始「融入」
      }
      const dx = cx + Math.cos(d.a) * d.r;
      const dy = cy + Math.sin(d.a) * d.r;
      const falling = d.fade < 1;
      // 融入瞬间换白色并短暂增亮，像物质落入吸积盘的闪光
      ctx.globalAlpha = (falling ? Math.min(0.85, d.alpha * 2.6) * d.fade : d.alpha) * reveal;
      ctx.drawImage(
        SPRITES[falling ? 0 : d.sprite],
        dx - d.draw / 2,
        dy - d.draw / 2,
        d.draw,
        d.draw
      );
    }

    for (let i = 0; i < particles.length; i++) {
      const p = particles[i];

      // 外圈尘埃绕黑洞极缓慢公转：锚点本身在圆轨道上移动
      let bx = p.hx;
      let by = p.hy;
      if (p.orbit) {
        const oa = p.orbit.a + p.orbit.w * time;
        bx = cx + Math.cos(oa) * p.orbit.r;
        by = cy + Math.sin(oa) * p.orbit.r;
      }

      // 呼吸漂移的锚点
      const ix = bx + Math.sin(time * p.speed + p.phase) * p.drift;
      const iy = by + Math.cos(time * p.speed * 0.9 + p.phase * 1.7) * p.drift;

      // 引力归位（被击中后 springScale 骤降，恢复期内引力变弱 → 慢慢拉回）
      // 入场汇聚期引力临时增强，聚拢成形后回到宇宙尺度的慢引力
      const settleBoost = reveal < 1 ? 3.5 - 2.5 * reveal : 1;
      const k = p.spring * p.springScale * settleBoost;
      p.vx += (ix - p.x) * k;
      p.vy += (iy - p.y) * k;
      if (p.springScale < 1) {
        p.springScale = Math.min(1, p.springScale + 0.0012);
      }

      // 鼠标排斥
      const dx = p.x - mouseX;
      const dy = p.y - mouseY;
      const d2 = dx * dx + dy * dy;
      if (d2 < REPEL_RADIUS * REPEL_RADIUS) {
        const d = Math.sqrt(d2) || 1;
        const f = (1 - d / REPEL_RADIUS) * REPEL_FORCE;
        p.vx += (dx / d) * f;
        p.vy += (dy / d) * f;
      }

      // 流星冲击：沿径向撞飞 + 少量顺着流星方向拖拽
      for (let j = 0; j < meteors.length; j++) {
        const m = meteors[j];
        const mdx = p.x - m.x;
        const mdy = p.y - m.y;
        const md2 = mdx * mdx + mdy * mdy;
        if (md2 < METEOR_HIT_RADIUS * METEOR_HIT_RADIUS) {
          const md = Math.sqrt(md2) || 1;
          const f = (1 - md / METEOR_HIT_RADIUS) * METEOR_FORCE;
          const mv = Math.hypot(m.vx, m.vy) || 1;
          p.vx += (mdx / md) * f + (m.vx / mv) * f * 0.45;
          p.vy += (mdy / md) * f + (m.vy / mv) * f * 0.45;
          p.springScale = Math.min(p.springScale, 0.07);
        }
      }

      p.vx *= 0.9;
      p.vy *= 0.9;
      p.x += p.vx;
      p.y += p.vy;

      let a = p.alpha * reveal;
      let ds = p.draw;
      if (p.twinkle) {
        const tw = 0.55 + 0.45 * Math.sin(time * 0.6 + p.phase * 3.1);
        a *= tw;
        ds *= 0.85 + 0.3 * tw;
      }

      ctx.globalAlpha = a;
      ctx.drawImage(SPRITES[p.sprite], p.x - ds / 2, p.y - ds / 2, ds, ds);
    }

    ctx.restore();

    // 绘制流星（亮头 + 渐隐尾迹，不随星云摇摆）
    for (let i = 0; i < meteors.length; i++) {
      const m = meteors[i];
      const mv = Math.hypot(m.vx, m.vy) || 1;
      const tailX = m.x - (m.vx / mv) * METEOR_TAIL;
      const tailY = m.y - (m.vy / mv) * METEOR_TAIL;

      const grad = ctx.createLinearGradient(m.x, m.y, tailX, tailY);
      grad.addColorStop(0, "rgba(255,255,255,0.9)");
      grad.addColorStop(0.4, "rgba(190,205,255,0.35)");
      grad.addColorStop(1, "rgba(190,205,255,0)");
      ctx.globalAlpha = 1;
      ctx.strokeStyle = grad;
      ctx.lineWidth = 1.5;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(m.x, m.y);
      ctx.lineTo(tailX, tailY);
      ctx.stroke();

      const headGlow = 14;
      ctx.drawImage(SPRITES[0], m.x - headGlow / 2, m.y - headGlow / 2, headGlow, headGlow);
    }

    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";
    requestAnimationFrame(render);
  };

  const start = () => {
    if (running || !built) return;
    running = true;
    lastFrame = 0;
    requestAnimationFrame(render);
  };

  const stop = () => {
    running = false;
  };

  const onPointer = (event) => {
    const rect = canvas.getBoundingClientRect();
    mouseX = event.clientX - rect.left;
    mouseY = event.clientY - rect.top;
  };

  img.onerror = showFallback;

  img.onload = () => {
    build();
    if (!built) {
      showFallback();
      return;
    }
    start();

    hero.addEventListener("pointermove", onPointer, { passive: true });
    hero.addEventListener("pointerleave", () => {
      mouseX = -9999;
      mouseY = -9999;
    });

    let resizeTimer = 0;
    window.addEventListener(
      "resize",
      () => {
        clearTimeout(resizeTimer);
        resizeTimer = setTimeout(build, 200);
      },
      { passive: true }
    );

    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) start();
    });

    if ("IntersectionObserver" in window) {
      const observer = new IntersectionObserver(
        (entries) => {
          entries.forEach((entry) => {
            if (entry.isIntersecting) start();
            else stop();
          });
        },
        { threshold: 0.01 }
      );
      observer.observe(canvas);
    }
  };
}

/**
 * Hero 背景：极光光丝 shader（vanilla WebGL，无依赖）。
 * 三条 fbm 扰动的冷色光带缓慢流动，指针带轻微视差；
 * canvas 用 mix-blend-mode: screen 叠在纯黑底上。
 */
function initAuroraShader () {
  if (reduceMotion) return;

  const hero = document.getElementById("hero");
  const canvas = document.getElementById("hero-canvas");
  if (!hero || !canvas) return;

  const gl = canvas.getContext("webgl", {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    powerPreference: "low-power",
  });
  if (!gl) return;

  const vert = `
    attribute vec2 position;
    varying vec2 vUv;
    void main() {
      vUv = position * 0.5 + 0.5;
      gl_Position = vec4(position, 0.0, 1.0);
    }
  `;

  const frag = `
    precision highp float;

    uniform float iTime;
    uniform vec2 iResolution;
    uniform vec2 uPointer;
    varying vec2 vUv;

    float hash(vec2 p) {
      return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453123);
    }

    float noise(vec2 p) {
      vec2 i = floor(p);
      vec2 f = fract(p);
      vec2 u = f * f * (3.0 - 2.0 * f);
      return mix(
        mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
        mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x),
        u.y
      );
    }

    float fbm(vec2 p) {
      float v = 0.0;
      float a = 0.5;
      for (int i = 0; i < 5; i++) {
        v += a * noise(p);
        p = p * 2.04 + vec2(13.7, 7.1);
        a *= 0.55;
      }
      return v;
    }

    /* 一条被 fbm 揉皱的水平光丝 */
    float ribbon(vec2 p, float center, float amp, float sharp, float drift, float seed) {
      float w = fbm(vec2(p.x * 1.1 + seed * 11.0 + drift, seed * 5.0 + drift * 0.55));
      float y = p.y - center - (w - 0.5) * amp;
      return exp(-abs(y) * sharp);
    }

    void main() {
      float aspect = iResolution.x / max(iResolution.y, 1.0);
      vec2 p = vUv - 0.5;
      p.x *= aspect;

      /* 指针视差：非常轻，只是让画面「活着」 */
      p += (uPointer - 0.5) * vec2(0.05, 0.028);

      float t = iTime * 0.05;

      vec3 cCyan   = vec3(0.42, 0.62, 1.00);
      vec3 cViolet = vec3(0.64, 0.54, 1.00);
      vec3 cWhite  = vec3(0.86, 0.92, 1.00);

      float r1 = ribbon(p, 0.21, 0.30, 16.0, t,             1.0);
      float r2 = ribbon(p, 0.08, 0.38, 11.0, t * 1.25 + 3.0, 2.0);
      float r3 = ribbon(p, 0.30, 0.22, 22.0, t * 0.8 + 7.0,  3.0);

      vec3 col = vec3(0.0);
      col += cCyan   * r1 * 0.30;
      col += cViolet * r2 * 0.20;
      col += cWhite  * r3 * 0.22;

      /* 一层极淡的大尺度雾，垫住三条光丝 */
      float haze = fbm(p * 1.7 + vec2(t * 0.6, 0.0));
      col += mix(cViolet, cCyan, haze) * haze * haze * 0.05;

      /* 光集中在画面上部：向下与向顶部两侧渐隐 */
      col *= smoothstep(-0.30, 0.14, p.y);
      col *= 1.0 - smoothstep(0.36, 0.52, p.y);
      /* 左右两端轻微收口 */
      col *= smoothstep(1.35, 0.5, abs(p.x));

      /* 抖动防色带 */
      col += (hash(vUv * iResolution) - 0.5) * 0.012;

      gl_FragColor = vec4(max(col, 0.0), 1.0);
    }
  `;

  const compileShader = (type, source) => {
    const shader = gl.createShader(type);
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      throw new Error(gl.getShaderInfoLog(shader) || "Shader compile failed");
    }
    return shader;
  };

  let program;
  try {
    const vertexShader = compileShader(gl.VERTEX_SHADER, vert);
    const fragmentShader = compileShader(gl.FRAGMENT_SHADER, frag);
    program = gl.createProgram();
    gl.attachShader(program, vertexShader);
    gl.attachShader(program, fragmentShader);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      throw new Error(gl.getProgramInfoLog(program) || "Shader link failed");
    }
  } catch (error) {
    console.warn("Aurora shader skipped:", error);
    return;
  }

  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);

  const position = gl.getAttribLocation(program, "position");
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);

  const uniforms = {
    iTime: gl.getUniformLocation(program, "iTime"),
    iResolution: gl.getUniformLocation(program, "iResolution"),
    uPointer: gl.getUniformLocation(program, "uPointer"),
  };

  let pointerX = 0.5;
  let pointerY = 0.5;
  let targetX = 0.5;
  let targetY = 0.5;
  let lastDrawTime = 0;
  let running = false;
  const FRAME_MS = 33; // 约 30fps：氛围背景不需要 60fps

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    // 氛围光没有锐利细节，DPR 限 1.25 就足够，省大量像素填充
    const dpr = Math.min(window.devicePixelRatio || 1, 1.25);
    canvas.width = Math.max(1, Math.round(rect.width * dpr));
    canvas.height = Math.max(1, Math.round(rect.height * dpr));
    gl.viewport(0, 0, canvas.width, canvas.height);
  };

  const onPointer = (event) => {
    const rect = hero.getBoundingClientRect();
    targetX = (event.clientX - rect.left) / rect.width;
    targetY = 1 - (event.clientY - rect.top) / rect.height;
  };

  const render = (time) => {
    if (!running || document.hidden) return;

    if (time - lastDrawTime < FRAME_MS) {
      requestAnimationFrame(render);
      return;
    }
    lastDrawTime = time;

    pointerX += (targetX - pointerX) * 0.05;
    pointerY += (targetY - pointerY) * 0.05;

    gl.useProgram(program);
    gl.uniform1f(uniforms.iTime, time * 0.001);
    gl.uniform2f(uniforms.iResolution, canvas.width, canvas.height);
    gl.uniform2f(uniforms.uPointer, pointerX, pointerY);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    requestAnimationFrame(render);
  };

  const start = () => {
    if (running) return;
    running = true;
    requestAnimationFrame(render);
  };

  const stop = () => {
    running = false;
  };

  window.addEventListener("resize", resize, { passive: true });
  hero.addEventListener("pointermove", onPointer, { passive: true });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) start();
  });

  resize();

  // hero 滚出视口就停止渲染，回来再重启，离屏零开销
  if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver(
      (entries) => {
        entries.forEach((entry) => {
          if (entry.isIntersecting) start();
          else stop();
        });
      },
      { threshold: 0.01 }
    );
    observer.observe(hero);
  } else {
    start();
  }
}

/** 卡片与按钮的指针跟随高光 */
function initInteractiveHover () {
  if (reduceMotion) return;

  const items = document.querySelectorAll(".btn, .bento-card, .dl-card");

  items.forEach((el) => {
    el.addEventListener(
      "pointermove",
      (event) => {
        const rect = el.getBoundingClientRect();
        const px = ((event.clientX - rect.left) / rect.width) * 100;
        const py = ((event.clientY - rect.top) / rect.height) * 100;
        el.style.setProperty("--mouse-x", `${px.toFixed(2)}%`);
        el.style.setProperty("--mouse-y", `${py.toFixed(2)}%`);
      },
      { passive: true }
    );

    el.addEventListener("pointerleave", () => {
      el.style.removeProperty("--mouse-x");
      el.style.removeProperty("--mouse-y");
    });
  });
}

/** 页脚年份 */
function initYear () {
  const el = document.getElementById("year");
  if (el) el.textContent = String(new Date().getFullYear());
}

/** 统一注入版本号。没拉到 Release 时占位藏着，避免文案里写死某一个版本。 */
function initVersion () {
  document.querySelectorAll(".js-version").forEach((el) => {
    el.textContent = VERSION;
  });
  document.querySelectorAll(".js-ver").forEach((el) => {
    el.hidden = !VERSION;
  });
}

/** 下载卡片底下的「v0.1.1 · 约 158 MB」跟资产走，包变大了不用改文案。 */
function initDownloadMeta () {
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
initReveal();
initHeroTitle();
initParticleLogo();
initAuroraShader();
initInteractiveHover();
initYear();
