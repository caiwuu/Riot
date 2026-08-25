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
 * 动画 Logo：「星云中的 RIOT」。
 * 离屏画布上写出字标「RIOT」，细密的炽白光点拼出字形；云朵是由
 * 「到文字的距离场」生成的胶囊状星云雾，贴字最浓，颜色随距离由青蓝过渡
 * 到深紫，向外密度/亮度连续衰减、位置弥散，天然没有轮廓线。
 * 宇宙中的星尘被引力捕获，绕着字标同向公转、一圈圈螺旋靠近：
 * 白色星尘真正落到笔画上时闪亮一下汇入字形；云朵色的星尘则落到
 * 云带的不同深度，融进对应色带的雾里。汇入后从远处的星空重生，川流不息。
 * 流星拖着尾迹横穿星空，轨迹被字标的引力弯折，击穿处的光点被撞飞，
 * 随后在引力作用下缓缓归位。鼠标靠近同样会推开光点。
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
  const METEOR_HIT_RADIUS = 22; // 流星冲击半径
  const METEOR_FORCE = 5.5; // 冲击柔和一些，击穿时字形不至于整个散架
  const METEOR_TAIL = 96; // 尾迹长度（px）
  const METEOR_G = 2600000; // 引力常数：流星轨迹被 R 的中心弯折
  const METEOR_MAX_AGE = 18; // 秒，被引力俘获绕圈的流星最终回收，避免反复切割字形
  const WORD = "RIOT"; // 粒子字标内容

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

  /* 能量色带：0 炽白核心 → 1 青蓝 → 2 紫 → 3 外缘深紫 */
  const SPRITES = [
    makeSprite(244, 248, 255),
    makeSprite(168, 200, 255),
    makeSprite(156, 146, 244),
    makeSprite(110, 102, 198),
  ];

  let W = 0;
  let H = 0;
  let holeX = 0; // 字标引力中心（画布坐标）：锚定在 .hero-logo 容器中心
  let holeY = 0;
  let particles = [];
  let dust = []; // 星尘：绕字标公转、螺旋内落，白色汇入笔画，云朵色融进云带
  let nebulaR = 100;
  let cloudBandPx = 90; // 云带宽度（px）：给云朵色星尘挑落点深度
  let capField = null; // 「到字形的距离」场：星尘判断是否触到笔画 / 云层
  let capSize = 0;
  let capOffX = 0;
  let capOffY = 0;
  let meteors = [];
  let nextMeteorAt = 0;
  let lastFrame = 0;
  let running = false;
  let built = false;
  let revealStart = 0;
  let mouseX = -9999;
  let mouseY = -9999;

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    W = Math.max(1, rect.width);
    H = Math.max(1, rect.height);
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  };

  /** 星尘重生：回到远处星空，重新开始被引力捕获的螺旋内落 */
  const respawnDust = (d) => {
    d.r = nebulaR * (1.15 + Math.random() * 1.45);
    d.a = Math.random() * Math.PI * 2;
    d.w = 0.05 + Math.random() * 0.09; // 全部同向公转（rad/s），旋转趋势肉眼可辨
    d.rate = 4 + Math.random() * 6; // 内落速度（px/s）：一点一点靠近
    d.fade = 1;
    /* capture 的语义是「到字形的距离阈值」，配合距离场判断落点 */
    if (d.kind === 0) {
      /* 白色星尘 → 一路落到笔画边上，汇入字形 */
      d.capture = 3 + Math.random() * 5;
      d.sprite = 0;
      d.alpha = 0.3 + Math.random() * 0.25;
      d.draw = 2.2 + Math.random() * 2.6;
    } else {
      /* 云朵色星尘 → 落到云带的随机深度融入；落点深浅不一，不会积出一圈隐形边界。
         颜色跟落点处的云朵色带一致：落得越深越偏青蓝，浅处是深紫 */
      d.capture = cloudBandPx * (0.12 + Math.random() * 0.68);
      d.sprite = d.capture < cloudBandPx * 0.33 ? 1 : d.capture < cloudBandPx * 0.62 ? 2 : 3;
      d.alpha = 0.22 + Math.random() * 0.2;
      d.draw = 2 + Math.random() * 2.4;
    }
  };

  /** 把字标「RIOT」画进离屏画布并采样，生成粒子锚点（位置/尺寸跟随 .hero-logo 容器） */
  const build = () => {
    resize();
    particles = [];

    const box = logoBox.getBoundingClientRect();
    const canvasBox = canvas.getBoundingClientRect();
    // 离屏画布放大到容器的 1.75 倍：横排字标要铺开，四周还得给云带留溶解余量
    const logoSize = Math.max(40, Math.round(Math.min(box.width, box.height) * 1.75));
    const off = document.createElement("canvas");
    off.width = logoSize;
    off.height = logoSize;
    const octx = off.getContext("2d", { willReadFrequently: true });

    // 黑底白字居中写出字标，字宽拟合到画布的 52%（两侧留出云带 + 弥散的空间）
    octx.fillStyle = "#000";
    octx.fillRect(0, 0, logoSize, logoSize);
    octx.fillStyle = "#fff";
    octx.textAlign = "center";
    octx.textBaseline = "middle";
    const family = '"Geist", "Helvetica Neue", Arial, sans-serif';
    const setFont = (px) => {
      octx.font = `700 ${px}px ${family}`;
      octx.letterSpacing = `${(px * 0.08).toFixed(1)}px`;
    };
    let fontPx = Math.round(logoSize * 0.24);
    setFont(fontPx);
    const measured = octx.measureText(WORD).width || 1;
    fontPx = Math.max(10, Math.round((fontPx * logoSize * 0.52) / measured));
    setFont(fontPx);
    octx.fillText(WORD, logoSize / 2, logoSize / 2);

    let data;
    try {
      data = octx.getImageData(0, 0, logoSize, logoSize).data;
    } catch {
      return; // 极端环境下放弃粒子，兜底静态图
    }

    // 字母掩码：黑底白字，取亮像素即字形
    const letterMask = new Uint8Array(logoSize * logoSize);
    for (let i = 0; i < letterMask.length; i++) {
      if (data[i * 4] >= 150) letterMask[i] = 1;
    }

    /** 多源 BFS 距离场（4 邻域，单位：像素） */
    const distField = (isSource, walkable) => {
      const dist = new Int32Array(logoSize * logoSize).fill(-1);
      const queue = new Uint32Array(logoSize * logoSize);
      let head = 0;
      let tail = 0;
      for (let i = 0; i < dist.length; i++) {
        if (isSource(i)) {
          dist[i] = 0;
          queue[tail++] = i;
        }
      }
      while (head < tail) {
        const j = queue[head++];
        const jx = j % logoSize;
        const d = dist[j] + 1;
        if (jx > 0 && dist[j - 1] === -1 && walkable(j - 1)) {
          dist[j - 1] = d;
          queue[tail++] = j - 1;
        }
        if (jx < logoSize - 1 && dist[j + 1] === -1 && walkable(j + 1)) {
          dist[j + 1] = d;
          queue[tail++] = j + 1;
        }
        if (j >= logoSize && dist[j - logoSize] === -1 && walkable(j - logoSize)) {
          dist[j - logoSize] = d;
          queue[tail++] = j - logoSize;
        }
        if (j < dist.length - logoSize && dist[j + logoSize] === -1 && walkable(j + logoSize)) {
          dist[j + logoSize] = d;
          queue[tail++] = j + logoSize;
        }
      }
      return dist;
    };

    // dLet：每个像素到字形的距离。云朵形状、颜色分带、星尘落点全由这一个距离场驱动
    const dLet = distField(
      (i) => letterMask[i] === 1,
      () => true
    );

    /* 云朵 = 距文字 cloudBand 以内的雾带：贴字是浓核，向外连续衰减，天然无轮廓 */
    const cloudBand = Math.max(24, logoSize * 0.22);
    const coreBand = cloudBand * 0.35; // 浓核带：这圈内密度/亮度不衰减
    const nearBand = logoSize * 0.055; // 离字形这么近 → 青蓝
    const midBand = logoSize * 0.105; // 再远 → 紫，其余深紫

    // logo 在画布坐标系中的落点：两个矩形同帧测量，与滚动位置无关
    const offX = box.left - canvasBox.left + (box.width - logoSize) / 2;
    const offY = box.top - canvasBox.top + (box.height - logoSize) / 2;
    const logoR = logoSize / 2;
    holeX = offX + logoR;
    holeY = offY + logoR;

    /* —— 字标 RIOT：独立的细网格采样，光点小而密，字形「像素」更高 —— */
    const stepL = Math.max(2, Math.round(logoSize / 170));
    for (let y = 0; y < logoSize; y += stepL) {
      for (let x = 0; x < logoSize; x += stepL) {
        if (!letterMask[y * logoSize + x]) continue;

        /* 每个采样点三颗：两颗炽白核心错位补缝，一颗贴着笔画的青蓝辉光 */
        for (let c = 0; c < 3; c++) {
          const isGlow = c === 2;
          /* 景深：z 越大越近 → 更大更亮 */
          const z = 0.55 + Math.random() * 0.9;
          const size = isGlow ? 0.9 + Math.random() * 0.5 : 0.85 + Math.random() * 0.55;
          const alpha = isGlow ? 0.13 + Math.random() * 0.11 : 0.88 + Math.random() * 0.12;
          const a0 = alpha * (0.58 + 0.42 * ((z - 0.55) / 0.9));

          // 抖动幅度跟着细网格收小，笔画边缘不发毛
          const spread = c === 1 ? 2 : 1;
          const hx = offX + x + (Math.random() - 0.5) * spread;
          const hy = offY + y + (Math.random() - 0.5) * spread;
          const angle = Math.random() * Math.PI * 2;
          const dist = 70 + Math.random() * 180;

          particles.push({
            hx,
            hy,
            x: hx + Math.cos(angle) * dist,
            y: hy + Math.sin(angle) * dist,
            vx: 0,
            vy: 0,
            draw: size * (isGlow ? 6.4 : 4.1) * z, // 辉光颗更大更淡，贴着笔画晕开
            sprite: isGlow ? 1 : 0,
            alpha: a0,
            orbit: null,
            spring: 0.02 + Math.random() * 0.025,
            springScale: 1, // 被流星击中后骤降，随时间缓慢恢复 → 「引力慢慢拉回」
            phase: Math.random() * Math.PI * 2,
            speed: 0.1 + Math.random() * 0.18,
            drift: 1.1,
            twinkle: !isGlow && Math.random() < 0.4,
          });
        }
      }
    }

    const step = Math.max(2, Math.round(logoSize / 74));
    for (let y = 0; y < logoSize; y += step) {
      for (let x = 0; x < logoSize; x += step) {
        const i = y * logoSize + x;
        if (letterMask[i]) continue;
        const d2r = dLet[i];
        if (d2r > cloudBand) continue; // 云带之外是纯星空

        /* —— 云朵本体：安静的星云雾 ——
           边缘溶解三件套：密度随溶解度递减到零、亮度随之压暗、位置向外弥散，
           所以云朵没有可辨认的轮廓线，只是渐渐消失在黑底里 */
        const f0 = Math.min(1, (cloudBand - d2r) / (cloudBand - coreBand));
        const fade = f0 * f0 * (3 - 2 * f0); // smoothstep：溶解过渡更顺滑
        if (Math.random() > fade) continue;
        let sprite;
        let alpha;
        if (d2r <= nearBand) {
          sprite = 1;
          alpha = 0.42 + Math.random() * 0.18;
        } else if (d2r <= midBand) {
          sprite = 2;
          alpha = 0.33 + Math.random() * 0.15;
        } else {
          sprite = 3;
          alpha = 0.25 + Math.random() * 0.12;
        }

        const z = 0.55 + Math.random() * 0.9;
        const size = 0.95 + Math.random() * 0.75;
        // 亮度受景深影响收窄（0.7–1.0），云雾均匀不出脏斑
        const a0 = alpha * (0.35 + 0.65 * fade) * (0.7 + 0.3 * ((z - 0.55) / 0.9));

        // 越靠边缘散得越开：外飘约一个云带的宽度，边缘雾圈显著外扩
        const scatter = 1.5 + (1 - fade) * cloudBand * 0.9;
        const hx = offX + x + (Math.random() - 0.5) * scatter;
        const hy = offY + y + (Math.random() - 0.5) * scatter;
        const angle = Math.random() * Math.PI * 2;
        const dist = 70 + Math.random() * 180;

        /* 边缘的雾沿切向缓慢往复环流：幅度有限（渲染时正弦摆动），
           横排字形的胶囊云不会被持续公转搅散，只看到云边在流动 */
        const w = (0.02 + Math.random() * 0.03) * (1 - fade);
        const orbit =
          w > 0.001
            ? {
              r: Math.hypot(hx - holeX, hy - holeY),
              a: Math.atan2(hy - holeY, hx - holeX),
              w,
            }
            : null;

        particles.push({
          hx,
          hy,
          x: hx + Math.cos(angle) * dist,
          y: hy + Math.sin(angle) * dist,
          vx: 0,
          vy: 0,
          draw: size * (5.2 + (1 - fade) * 2.4) * z, // 边缘颗更大更淡，像雾一样晕开
          sprite,
          alpha: a0,
          orbit,
          spring: 0.02 + Math.random() * 0.025,
          springScale: 1,
          phase: Math.random() * Math.PI * 2,
          speed: 0.12 + Math.random() * 0.2,
          drift: 0.8 + (1 - fade) * 2.8,
          twinkle: Math.random() < 0.06,
        });
      }
    }

    /* 星尘两队：白色落到笔画上汇入 RIOT，云朵色落进云带的随机深度。
       初始就散布在螺旋内落途中的不同半径，画面一开始就是进行中的汇入流 */
    nebulaR = logoR;
    cloudBandPx = cloudBand;
    capField = dLet;
    capSize = logoSize;
    capOffX = offX;
    capOffY = offY;
    dust = [];
    for (let i = 0; i < 150; i++) {
      const d = { kind: i < 60 ? 0 : 1 };
      respawnDust(d);
      d.r = nebulaR * (0.55 + Math.random() * 2.05);
      dust.push(d);
    }

    built = particles.length > 0;
    revealStart = 0; // 重建后重新走一遍淡入与聚合增强
  };

  /**
   * 生成一颗流星：从画布外随机方向射入，路径随机——
   * 不一定穿过字形，但轨迹会被 R 的引力弯折，擦得足够近才会击穿粒子团。
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

    // 更新流星：受 R 中心的引力弯折轨迹；出界或过久（被俘获绕圈）回收
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

    /* 整个字形极缓慢地摇摆与呼吸（绕 R 自身中心，而非画布中心） */
    const cx = holeX;
    const cy = holeY;
    const sway = Math.sin(time * 0.05) * 0.04;
    const breath = 1 + Math.sin(time * 0.08) * 0.014;
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate(sway);
    ctx.scale(breath, breath);
    ctx.translate(-cx, -cy);

    /* 星尘：绕 R 同向公转 + 螺旋内落；白色触到 R 外接圆、云朵色落到各自深度时，
       放大增亮闪一下汇入目标，再从远处重生 */
    for (let i = 0; i < dust.length; i++) {
      const d = dust[i];
      d.a += d.w * dt;
      if (d.fade < 1) {
        d.fade -= dt / 0.7;
        if (d.fade <= 0) {
          respawnDust(d);
          continue;
        }
      } else {
        d.r -= d.rate * dt;
        // 查距离场：真正落到笔画（白尘）或云层深度（云尘）才开始「汇入」
        const px = Math.round(cx + Math.cos(d.a) * d.r - capOffX);
        const py = Math.round(cy + Math.sin(d.a) * d.r - capOffY);
        let near = Infinity;
        if (capField && px >= 0 && py >= 0 && px < capSize && py < capSize) {
          near = capField[py * capSize + px];
        }
        if (near <= d.capture || d.r <= 6) d.fade = 0.999;
      }
      const dx = cx + Math.cos(d.a) * d.r;
      const dy = cy + Math.sin(d.a) * d.r;
      const merging = d.fade < 1;
      const ds = merging ? d.draw * (1 + (1 - d.fade) * 0.9) : d.draw;
      // 汇入 R 的白点闪得亮，融进云朵的低调些
      const glow = d.kind === 0 ? 2.2 : 1.6;
      ctx.globalAlpha = (merging ? Math.min(0.9, d.alpha * glow) * d.fade : d.alpha) * reveal;
      ctx.drawImage(SPRITES[d.sprite], dx - ds / 2, dy - ds / 2, ds, ds);
    }

    for (let i = 0; i < particles.length; i++) {
      const p = particles[i];

      // 云朵边缘的雾沿切向缓慢往复环流（内部粒子 orbit 为 null，静止；
      // 幅度有限的摆动让横排字形的胶囊云不会被搅散）
      let bx = p.hx;
      let by = p.hy;
      if (p.orbit) {
        const oa = p.orbit.a + Math.sin(time * p.orbit.w * 5 + p.phase) * 0.09;
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
        p.springScale = Math.min(1, p.springScale + 0.0032);
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
          p.springScale = Math.min(p.springScale, 0.16);
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

    // 绘制流星（亮头 + 渐隐尾迹，不随字形摇摆）
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

  build();
  if (!built) {
    showFallback();
    return;
  }
  start();

  // Web 字体就绪后重采样一次，字标用上正式字体（字体已就绪时立即 resolve，几乎无感）
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(() => {
      if (built) build();
    });
  }

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
