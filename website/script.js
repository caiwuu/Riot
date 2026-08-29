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

  if (!os) return; // 未识别的系统：主按钮保持锚向下载区（默认展示 macOS 文案）

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

/* ============================================================
   Hero 场景（新 UI 图 1:1）
   scene-canvas：星点 / 波形丘陵 / 透视地面 / 十字 / 半调网点
   logo-canvas：点阵 LED 字标「RIOT」+ 外围暗点网格 + 溶解浮尘
   ============================================================ */

/** 预渲染辉光光点 sprite */
function makeGlowSprite (r, g, b) {
  const s = document.createElement("canvas");
  const SIZE = 64;
  s.width = SIZE;
  s.height = SIZE;
  const c = s.getContext("2d");
  const grad = c.createRadialGradient(32, 32, 0, 32, 32, 32);
  grad.addColorStop(0, `rgba(${r},${g},${b},1)`);
  grad.addColorStop(0.25, `rgba(${r},${g},${b},0.85)`);
  grad.addColorStop(0.5, `rgba(${r},${g},${b},0.18)`);
  grad.addColorStop(1, `rgba(${r},${g},${b},0)`);
  c.fillStyle = grad;
  c.fillRect(0, 0, SIZE, SIZE);
  return s;
}

/**
 * 背景场景：铺满首屏的一张 2D 画布。
 * 静态装饰（十字 / 半调网点 / 汇聚线 / 角落等高线）只在 resize 时
 * 预渲染进离屏层；每帧重画会动的部分——漂移闪烁的星点、流动的波形
 * 丘陵、向观察者滚动的透视地面，30fps 足够。
 * （流星在字标画布里，与字标微粒做撞击交互。）
 */
function initHeroScene () {
  const canvas = document.getElementById("scene-canvas");
  const hero = document.getElementById("hero");
  if (!canvas || !hero) return;
  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const SP_STAR = makeGlowSprite(216, 228, 255);
  const WAVE_FILL = "rgb(170, 196, 255)";
  const GRID_RGB = "150, 172, 240";
  const rand = (a, b) => a + Math.random() * (b - a);

  let W = 0;
  let H = 0;
  let stars = [];
  let staticLayer = null;
  let running = false;
  let lastDraw = 0;
  const FRAME_MS = 33; // 氛围背景 30fps 足够

  /** 静态装饰层：只在尺寸变化时重绘一次 */
  const buildStaticLayer = () => {
    staticLayer = document.createElement("canvas");
    staticLayer.width = Math.round(W * dpr);
    staticLayer.height = Math.round(H * dpr);
    const c = staticLayer.getContext("2d");
    c.setTransform(dpr, 0, 0, dpr, 0, 0);

    // 定位十字（与 UI 图同位：上部两侧 + 中下两侧）
    c.strokeStyle = "rgba(255, 255, 255, 0.25)";
    c.lineWidth = 1;
    const crosses = [
      [0.072, 0.228],
      [0.918, 0.228],
      [0.034, 0.672],
      [0.963, 0.672],
    ];
    for (const [fx, fy] of crosses) {
      const x = Math.round(fx * W) + 0.5;
      const y = Math.round(fy * H) + 0.5;
      c.beginPath();
      c.moveTo(x - 5, y);
      c.lineTo(x + 5, y);
      c.moveTo(x, y - 5);
      c.lineTo(x, y + 5);
      c.stroke();
    }

    // 半调网点方块（四角点缀）
    const pitch = Math.max(6, Math.round(W * 0.0078));
    const clusters = [
      { x: 0.052, y: 0.8, cols: 9, rows: 9, a: 0.32 },
      { x: 0.874, y: 0.8, cols: 9, rows: 9, a: 0.32 },
      { x: 0.933, y: 0.66, cols: 7, rows: 8, a: 0.28 },
      { x: 0.778, y: 0.1, cols: 7, rows: 6, a: 0.2 },
      { x: 0.226, y: 0.095, cols: 6, rows: 5, a: 0.17 },
    ];
    for (const cl of clusters) {
      for (let iy = 0; iy < cl.rows; iy++) {
        for (let ix = 0; ix < cl.cols; ix++) {
          if (Math.random() < 0.2) continue;
          c.fillStyle = `rgba(198, 210, 255, ${(cl.a * rand(0.25, 1)).toFixed(3)})`;
          c.fillRect(
            Math.round(cl.x * W + ix * pitch),
            Math.round(cl.y * H + iy * pitch),
            1.4,
            1.4
          );
        }
      }
    }

    // 地平线以下的极淡汇聚线（点阵地面改为每帧动态滚动绘制）
    const vpx = W / 2;
    const vpy = H * 0.52;
    for (let k = -8; k <= 8; k++) {
      const endX = vpx + k * W * 0.115;
      const grad = c.createLinearGradient(vpx, vpy + 6, endX, H);
      grad.addColorStop(0, `rgba(${GRID_RGB}, 0)`);
      grad.addColorStop(1, `rgba(${GRID_RGB}, 0.055)`);
      c.strokeStyle = grad;
      c.beginPath();
      c.moveTo(vpx, vpy + 6);
      c.lineTo(endX, H);
      c.stroke();
    }

    // 底部两角的等高线
    const contour = (cx0, cy0) => {
      for (let i = 0; i < 4; i++) {
        const r = W * (0.05 + i * 0.034);
        c.strokeStyle = `rgba(${GRID_RGB}, ${(0.05 + i * 0.007).toFixed(3)})`;
        c.beginPath();
        c.ellipse(cx0, cy0, r, r * 0.6, 0, Math.PI, Math.PI * 2);
        c.stroke();
      }
    };
    contour(W * 0.06, H * 1.04);
    contour(W * 0.94, H * 1.04);
  };

  const build = () => {
    const rect = canvas.getBoundingClientRect();
    W = Math.max(1, rect.width);
    H = Math.max(1, rect.height);
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    // 星点：细碎的底 + 几颗带辉光的亮星（整体极缓慢向右漂移）
    stars = [];
    const n = Math.round((W * H) / 4200);
    for (let i = 0; i < n; i++) {
      stars.push({
        x: Math.random() * W,
        y: Math.random() * H,
        r: 0.4 + Math.pow(Math.random(), 2.2) * 1.3,
        a: rand(0.05, 0.38),
        tw: Math.random() < 0.35,
        glow: false,
        phase: Math.random() * Math.PI * 2,
        spd: rand(0.4, 1.6),
        drift: rand(0.8, 3.2),
      });
    }
    for (let i = 0; i < 6; i++) {
      stars.push({
        x: Math.random() * W,
        y: Math.random() * H * 0.62,
        r: rand(1.5, 2.1),
        a: rand(0.5, 0.85),
        tw: true,
        glow: true,
        phase: Math.random() * Math.PI * 2,
        spd: rand(0.3, 0.8),
        drift: rand(0.5, 1.4),
      });
    }

    buildStaticLayer();
  };

  /** 透视点阵地面：一排排网点从地平线深处缓缓滚向观察者 */
  const drawFloor = (time) => {
    const vpx = W / 2;
    const vpy = H * 0.52;
    const ROWS = 9;
    const frac = (time * 0.1) % 1; // 约 10 秒推进一格
    ctx.fillStyle = `rgb(${GRID_RGB})`;
    for (let j = 0; j <= ROWS; j++) {
      const u = (j + frac) / ROWS;
      if (u > 1.02) continue;
      const p = Math.pow(u, 1.8);
      const y = vpy + 12 + (H - vpy - 12) * p;
      const sx = (14 + 120 * p) * (W / 1024);
      const size = 0.8 + p * 1.1;
      const n = Math.ceil(W / 2 / sx);
      const fadeIn = Math.min(1, u / 0.12); // 远处新行淡入
      for (let k = -n; k <= n; k++) {
        ctx.globalAlpha = (0.035 + 0.085 * p) * (1 - (Math.abs(k) / (n + 2)) * 0.35) * fadeIn;
        ctx.fillRect(vpx + k * sx, y, size, size);
      }
    }
    ctx.globalAlpha = 1;
  };

  /** 波形丘陵：左右两片流动的点阵浪，向屏幕边缘隆起，向中心地平线收束 */
  const drawWaves = (time) => {
    const span = W * 0.4;
    const step = 4.5;
    const LINES = 10;
    ctx.fillStyle = WAVE_FILL;
    for (let side = 0; side < 2; side++) {
      const seed = side === 0 ? 0 : 2.6;
      const cols = Math.floor(span / step);
      for (let l = 0; l < LINES; l++) {
        const lw = 1 - Math.abs(l - (LINES - 1) / 2) / ((LINES - 1) / 2);
        for (let i = 0; i <= cols; i++) {
          const u = 1 - (i * step) / span; // 1 = 屏幕边缘，0 = 靠近中心
          if (u <= 0.02) continue;
          const x = side === 0 ? i * step : W - i * step;
          const base = H * (0.548 - 0.088 * u);
          const amp = H * (0.006 + 0.064 * Math.pow(u, 1.5));
          const spread = (2 + 34 * u) * (l / (LINES - 1) - 0.5);
          const e =
            Math.sin(x * 0.016 + l * 0.9 + time * 0.55 + seed) * 0.62 +
            Math.sin(x * 0.037 - time * 0.38 + l * 1.7) * 0.38;
          const y = base + spread + e * amp;
          let a = (0.045 + 0.34 * Math.pow(u, 0.9)) * (0.35 + 0.65 * lw);
          a *= Math.min(1, u / 0.18); // 靠近中心渐隐，和地平光线融为一体
          let size = 1.1;
          if (e > 0.68) {
            a *= 1.7; // 浪尖提亮
            size = 1.6;
          }
          ctx.globalAlpha = Math.min(0.5, a);
          ctx.fillRect(x, y, size, size);
        }
      }
    }
    ctx.globalAlpha = 1;
  };

  const paint = (time) => {
    ctx.clearRect(0, 0, W, H);
    if (staticLayer) ctx.drawImage(staticLayer, 0, 0, W, H);

    drawFloor(time);

    ctx.fillStyle = "#dfe6ff";
    for (let i = 0; i < stars.length; i++) {
      const st = stars[i];
      let a = st.a;
      if (st.tw) a *= 0.55 + 0.45 * Math.sin(time * st.spd + st.phase);
      ctx.globalAlpha = Math.max(0, a);
      const x = (st.x + time * st.drift) % W;
      if (st.glow) {
        const D = st.r * 9;
        ctx.drawImage(SP_STAR, x - D / 2, st.y - D / 2, D, D);
      } else {
        ctx.fillRect(x, st.y, st.r, st.r);
      }
    }
    ctx.globalAlpha = 1;

    drawWaves(time);
  };

  // 页面转入后台时 rAF 自动挂起、回前台自动续播，无需手动管 visibility
  const render = (t) => {
    if (!running) return;
    if (t - lastDraw >= FRAME_MS) {
      lastDraw = t;
      paint(t * 0.001);
    }
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

  build();
  paint(0);

  let resizeTimer = 0;
  window.addEventListener(
    "resize",
    () => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        build();
        paint(reduceMotion ? 0 : performance.now() * 0.001);
      }, 200);
    },
    { passive: true }
  );

  if (reduceMotion) return; // 静态一帧即可，不跑动画

  if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver(
      (entries) =>
        entries.forEach((en) => {
          if (en.isIntersecting) {
            start();
            return;
          }
          // 停帧前复核：确认真的滚出视野，防止环境误报把循环停死
          const r = canvas.getBoundingClientRect();
          if (r.bottom < 0 || r.top > window.innerHeight) stop();
          else start();
        }),
      { threshold: 0.01 }
    );
    observer.observe(canvas);
  }

  start();
}

/**
 * 粒子字标「RIOT」（对照设计稿：特粗字形 + 砂砾状密集粒子填充）：
 * 离屏以特粗字重画出字标，然后两套采样——
 * 1) 字形内部：细网格 + 随机抖动的密集微粒（大小/亮度重随机、少量蓝色调、
 *    偶发亮斑与内部暗斑），边缘蚕食并向外溢出浮尘，呈砂砾质感的实心块面；
 * 2) 字形外围：独立的稀疏规则网格暗点，覆盖整块字标区域、向外缘轻微衰减。
 * 入场时微粒从四散位置左→右扫掠归位；鼠标靠近轻轻推开微粒。
 * 流星也画在这层：六成流星瞄向字标飞来，命中时把微粒撞飞（弹簧短暂
 * 失效再缓缓归位）、头部炸出闪光与火花，穿出后继续划向远方。
 */
function initParticleLogo () {
  const canvas = document.getElementById("logo-canvas");
  const hero = document.getElementById("hero");
  const anchor = document.querySelector(".hero-wordmark-box");
  if (!canvas || !hero || !anchor) return;

  const showFallback = () => {
    canvas.style.display = "none";
    anchor.classList.add("no-canvas");
  };

  const ctx = canvas.getContext("2d");
  if (!ctx) {
    showFallback();
    return;
  }

  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const WORD = "RIOT";
  const TRACK = 0.16; // 字距（em）
  const FILL_COLS = 128; // 字形填充微粒的细网格列数
  const HALO_COLS = 48; // 外围暗点网格列数
  const REPEL_R = 48;
  const REPEL_F = 2.1;
  const METEOR_HIT_R = 25; // 流星冲击半径
  const METEOR_FORCE = 3; // 冲击力度（60fps 连续受力，不宜过大）

  const SP_WHITE = makeGlowSprite(242, 247, 255);
  const SP_BLUE = makeGlowSprite(152, 186, 255);
  const SP_DEEP = makeGlowSprite(116, 138, 235);

  let W = 0;
  let H = 0;
  let dots = []; // 字形 LED 点
  let halo = []; // 字标外围的暗点网格
  let debris = []; // 边缘溶解出去的浮尘
  let textCX = 0;
  let textCY = 0;
  let textW = 0;
  let built = false;
  let running = false;
  let startAt = 0;
  let lastFrameAt = 0; // 最近一次真实渲染帧的时刻（看门狗用）
  let mouseX = -9999;
  let mouseY = -9999;
  let meteors = []; // 流星（与微粒撞击交互）
  let sparks = []; // 撞击火花
  let nextMeteorAt = 0;
  let prevT = 0;

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    W = Math.max(1, rect.width);
    H = Math.max(1, rect.height);
    canvas.width = Math.round(W * dpr);
    canvas.height = Math.round(H * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  };

  const build = () => {
    resize();
    dots = [];
    halo = [];
    debris = [];
    built = false;

    const box = anchor.getBoundingClientRect();
    const cbox = canvas.getBoundingClientRect();
    const bw = box.width;
    const bh = box.height;
    if (bw < 60 || bh < 16) return;

    // 离屏 2x 画出字标，供点阵网格采样
    const scale = 2;
    const ow = Math.ceil(bw * scale);
    const oh = Math.ceil(bh * scale);
    const off = document.createElement("canvas");
    off.width = ow;
    off.height = oh;
    const octx = off.getContext("2d", { willReadFrequently: true });
    const family = '"Geist", "Helvetica Neue", Arial, sans-serif';

    // 手动排字：兼容不支持 canvas letterSpacing 的浏览器
    const layout = (px) => {
      octx.font = `900 ${px}px ${family}`;
      const widths = [...WORD].map((ch) => octx.measureText(ch).width);
      const track = px * TRACK;
      return {
        widths,
        track,
        total: widths.reduce((s, w) => s + w, 0) + track * (WORD.length - 1),
      };
    };
    const draw = (px) => {
      const mm = layout(px);
      octx.clearRect(0, 0, ow, oh);
      octx.fillStyle = "#fff";
      octx.strokeStyle = "#fff";
      octx.lineWidth = px * 0.055; // 描边增肥：贴近设计稿的特粗笔画
      octx.lineJoin = "round";
      octx.textAlign = "left";
      octx.textBaseline = "alphabetic";
      const baseY = oh / 2 + px * 0.36; // 大写字母光学居中
      let penX = (ow - mm.total) / 2;
      for (let i = 0; i < WORD.length; i++) {
        octx.fillText(WORD[i], penX, baseY);
        octx.strokeText(WORD[i], penX, baseY);
        penX += mm.widths[i] + mm.track;
      }
    };
    /** 实测墨迹包围盒：不同字体的度量差异一律以实际着墨为准 */
    const inkBox = (data) => {
      let minX = ow;
      let minY = oh;
      let maxX = 0;
      let maxY = 0;
      for (let y = 0; y < oh; y += 2) {
        for (let x = 0; x < ow; x += 2) {
          if (data[(y * ow + x) * 4 + 3] > 140) {
            if (x < minX) minX = x;
            if (x > maxX) maxX = x;
            if (y < minY) minY = y;
            if (y > maxY) maxY = y;
          }
        }
      }
      return { minX, minY, maxX, maxY, w: Math.max(1, maxX - minX), h: Math.max(1, maxY - minY) };
    };

    let fontPx = oh * 1.36; // 大写高约 0.72em：先按高度顶满
    const m0 = layout(fontPx);
    if (m0.total > ow * 0.985) fontPx = Math.floor((fontPx * ow * 0.985) / m0.total);
    draw(fontPx);

    let img;
    let ink;
    try {
      img = octx.getImageData(0, 0, ow, oh).data;
      // 按实测墨迹重拟合一次，让字形同时贴满锚框的宽与高
      ink = inkBox(img);
      const fit = Math.min((ow * 0.985) / ink.w, (oh * 0.97) / ink.h);
      if (Math.abs(fit - 1) > 0.02) {
        fontPx = Math.floor(fontPx * fit);
        draw(fontPx);
        img = octx.getImageData(0, 0, ow, oh).data;
        ink = inkBox(img);
      }
    } catch {
      showFallback();
      return;
    }
    // 居中校正：以墨迹几何中心对齐锚框中心（度量偏侧的字体也能真正居中）
    const shiftX = (ow / 2 - (ink.minX + ink.maxX) / 2) / scale;
    const shiftY = (oh / 2 - (ink.minY + ink.maxY) / 2) / scale;

    const offX = box.left - cbox.left;
    const offY = box.top - cbox.top;
    textCX = offX + bw / 2;
    textCY = offY + bh / 2;
    textW = bw;

    /** 以锚框内 CSS 坐标查询墨迹 */
    const inkAt = (xCss, yCss) => {
      const px = Math.round(xCss * scale);
      const py = Math.round(yCss * scale);
      if (px < 0 || py < 0 || px >= ow || py >= oh) return false;
      return img[(py * ow + px) * 4 + 3] > 140;
    };

    /* —— 字形内部：细网格 + 随机抖动的密集微粒填充 —— */
    const fs = bw / FILL_COLS;
    const fRows = Math.max(1, Math.round(bh / fs));
    /** 生成一颗填充微粒（jitterScale 控制离格心的散布程度） */
    const spawnFill = (cx, cy, edge, jitterScale) => {
      const bright = Math.random() < 0.06;
      const jAmp = fs * jitterScale;
      const x = offX + shiftX + cx + (Math.random() - 0.5) * 2 * jAmp;
      const y = offY + shiftY + cy + (Math.random() - 0.5) * 2 * jAmp;
      const sweep = (cx / bw) * 620; // 左→右入场扫掠
      const ang = Math.random() * Math.PI * 2;
      const dist = 36 + Math.random() * 130;
      dots.push({
        x,
        y,
        sx: x + Math.cos(ang) * dist, // 入场散点
        sy: y + Math.sin(ang) * dist,
        ox: 0,
        oy: 0,
        vx: 0,
        vy: 0,
        size: fs * (bright ? 2 : 0.9 + Math.random() * 0.8),
        alpha: bright ? 1 : 0.45 + Math.pow(Math.random(), 1.3) * 0.55,
        blue: Math.random() < 0.28,
        edge,
        shock: 0, // 被流星撞击后弹簧短暂失效，缓缓归位
        delay: sweep + Math.random() * 240,
        phase: Math.random() * Math.PI * 2,
        spd: 0.5 + Math.random() * 1.2,
        tw: edge ? 0.45 : Math.random() < 0.3 ? 0.18 : 0,
      });
    };
    for (let gy = 0; gy < fRows; gy++) {
      for (let gx = 0; gx < FILL_COLS; gx++) {
        const cx = (gx + 0.5) * fs;
        const cy = (gy + 0.5) * fs;
        if (!inkAt(cx, cy)) continue;
        const edge =
          !inkAt(cx - fs, cy) ||
          !inkAt(cx + fs, cy) ||
          !inkAt(cx, cy - fs) ||
          !inkAt(cx, cy + fs);
        // 边缘蚕食出毛边，内部偶发暗斑形成砂砾质感
        if (edge ? Math.random() < 0.3 : Math.random() < 0.03) continue;
        spawnFill(cx, cy, edge, edge ? 0.7 : 0.42);
        // 内部次级微粒：加密填充，让块面更实
        if (!edge && Math.random() < 0.35) spawnFill(cx, cy, false, 0.6);
        if (edge && Math.random() < 0.16) {
          debris.push({
            x: offX + shiftX + cx,
            y: offY + shiftY + cy,
            ang: Math.random() * Math.PI * 2,
            base: 4 + Math.random() * 8,
            reach: 22 + Math.random() * 46,
            dur: 5 + Math.random() * 7,
            t0: Math.random() * 12,
            size: fs * (0.8 + Math.random() * 0.6),
            alpha: 0.09 + Math.random() * 0.18,
          });
        }
      }
    }

    /* —— 字形外围：独立的稀疏规则网格暗点，覆盖整块字标区域 —— */
    const hs = bw / HALO_COLS;
    const hRows = Math.max(1, Math.round(bh / hs));
    const PADX = 9; // 外扩格数
    const PADY = 7;
    for (let gy = -PADY; gy < hRows + PADY; gy++) {
      for (let gx = -PADX; gx < HALO_COLS + PADX; gx++) {
        const cx = (gx + 0.5) * hs;
        const cy = (gy + 0.5) * hs;
        if (inkAt(cx, cy)) continue;
        if (Math.random() < 0.18) continue;
        const dx = gx < 0 ? -gx : gx >= HALO_COLS ? gx - HALO_COLS + 1 : 0;
        const dy = gy < 0 ? -gy : gy >= hRows ? gy - hRows + 1 : 0;
        const fall = Math.pow(1 - Math.max(dx / PADX, dy / PADY), 1.6);
        const a = (0.06 + Math.random() * 0.11) * (0.4 + 0.6 * fall);
        if (a < 0.028) continue;
        const hx = offX + shiftX + cx;
        const hy = offY + shiftY + cy;
        halo.push({
          x: hx,
          y: hy,
          dc: Math.hypot(hx - textCX, hy - textCY), // 到字标中心的距离（涟漪用）
          size: hs * 0.34,
          alpha: a,
          delay: (cx / bw) * 620 + 260 + Math.random() * 300,
          phase: Math.random() * Math.PI * 2,
          tw: Math.random() < 0.18 ? 0.3 : 0,
        });
      }
    }

    built = dots.length > 0;
    if (!built) showFallback();
  };

  const paint = (elapsed, time) => {
    ctx.clearRect(0, 0, W, H);
    ctx.globalCompositeOperation = "lighter";

    // 字标后方的一团淡蓝辉光（缓慢呼吸）
    const breathe = Math.sin(time * 0.5);
    const g = textW * (1.55 + 0.05 * breathe);
    ctx.globalAlpha = 0.11 * (0.85 + 0.15 * breathe);
    ctx.drawImage(SP_DEEP, textCX - g / 2, textCY - g / 2, g, g);

    // 扫光带：入场完成后，一道柔和亮波周期性从左掠到右
    const sweepX =
      elapsed > 1600
        ? textCX + textW * ((((time % 5.2) / 5.2) * 1.7 - 0.85))
        : -1e9;
    const sweepW = textW * 0.1;

    // —— 流星：生成与推进（绘制放在最后，叠于字标之上） ——
    const dt = Math.min(0.6, Math.max(0, prevT ? time - prevT : 0.016));
    prevT = time;
    if (elapsed > 2400) {
      if (!nextMeteorAt) nextMeteorAt = time + 1.6;
      if (time >= nextMeteorAt && meteors.length < 3) {
        // 六成流星瞄向字标区域，确保能看到撞击
        const aimed = Math.random() < 0.6;
        const tx = aimed
          ? textCX + (Math.random() - 0.5) * textW * 0.8
          : W * (0.15 + Math.random() * 0.7);
        const fromTopEdge = Math.random() < 0.55;
        const sx0 = fromTopEdge ? Math.random() * W : Math.random() < 0.5 ? -50 : W + 50;
        const sy0 = fromTopEdge ? -40 : Math.random() * H * 0.3;
        const ty = Math.max(
          aimed ? textCY + (Math.random() - 0.5) * textW * 0.18 : H * (0.12 + Math.random() * 0.28),
          sy0 + 70
        );
        const dd = Math.hypot(tx - sx0, ty - sy0) || 1;
        const spd = 250 + Math.random() * 150;
        meteors.push({
          x: sx0,
          y: sy0,
          vx: ((tx - sx0) / dd) * spd,
          vy: ((ty - sy0) / dd) * spd,
          age: 0,
          flash: 0,
          hit: false,
        });
        nextMeteorAt = time + 2.4 + Math.random() * 3.6;
      }
      for (let i = meteors.length - 1; i >= 0; i--) {
        const m = meteors[i];
        m.x += m.vx * dt;
        m.y += m.vy * dt;
        m.age += dt;
        if (m.age > 8 || m.x < -110 || m.x > W + 110 || m.y < -110 || m.y > H * 0.85) {
          meteors.splice(i, 1);
        }
      }
    }

    // 外围暗点网格（自字标中心向外的明暗涟漪）
    for (let i = 0; i < halo.length; i++) {
      const d = halo[i];
      const k = Math.min(1, Math.max(0, (elapsed - d.delay) / 900));
      if (k <= 0) continue;
      let a = d.alpha * k;
      if (d.tw) a *= 1 - d.tw * (0.5 + 0.5 * Math.sin(time * 0.8 + d.phase));
      a *= 0.8 + 0.2 * Math.sin(d.dc * 0.05 - time * 1.4);
      ctx.globalAlpha = Math.max(0, a);
      const D = d.size * 2.1;
      ctx.drawImage(SP_BLUE, d.x - D / 2, d.y - D / 2, D, D);
    }

    // 字形微粒：第一遍铺蓝色辉光垫底（顺带算物理），第二遍点亮核
    for (let pass = 0; pass < 2; pass++) {
      for (let i = 0; i < dots.length; i++) {
        const p = dots[i];
        const k = Math.min(1, Math.max(0, (elapsed - p.delay) / 700));
        if (k <= 0) continue;
        const e = 1 - Math.pow(1 - k, 3);
        let px;
        let py;
        if (k < 1) {
          px = p.sx + (p.x - p.sx) * e;
          py = p.sy + (p.y - p.sy) * e;
        } else {
          if (pass === 0) {
            const mdx = p.x + p.ox - mouseX;
            const mdy = p.y + p.oy - mouseY;
            const md2 = mdx * mdx + mdy * mdy;
            if (md2 < REPEL_R * REPEL_R) {
              const md = Math.sqrt(md2) || 1;
              const f = (1 - md / REPEL_R) * REPEL_F;
              p.vx += (mdx / md) * f;
              p.vy += (mdy / md) * f;
            }
            // 流星冲击：沿径向撞飞 + 顺着流星方向拖拽
            for (let mi = 0; mi < meteors.length; mi++) {
              const m = meteors[mi];
              const hdx = p.x + p.ox - m.x;
              const hdy = p.y + p.oy - m.y;
              const hd2 = hdx * hdx + hdy * hdy;
              if (hd2 < METEOR_HIT_R * METEOR_HIT_R) {
                const hd = Math.sqrt(hd2) || 1;
                const f = (1 - hd / METEOR_HIT_R) * METEOR_FORCE;
                const mv = Math.hypot(m.vx, m.vy) || 1;
                p.vx += (hdx / hd) * f + (m.vx / mv) * f * 0.45;
                p.vy += (hdy / hd) * f + (m.vy / mv) * f * 0.45;
                p.shock = 1;
                m.hit = true;
              }
            }
            // 被撞过的微粒弹簧短暂失效 → 先炸开，再被引力缓缓拉回
            const spring = 0.09 * (1 - 0.85 * p.shock);
            p.vx = (p.vx - p.ox * spring) * 0.86;
            p.vy = (p.vy - p.oy * spring) * 0.86;
            if (p.shock > 0.02) p.shock *= 0.982;
            else p.shock = 0;
            p.ox += p.vx;
            p.oy += p.vy;
          }
          px = p.x + p.ox;
          py = p.y + p.oy;
          // 微粒呼吸漂移：整个字标像活的星尘，边缘幅度更大
          const drift = p.edge ? 0.85 : 0.45;
          px += Math.sin(time * 0.8 + p.phase) * drift;
          py += Math.cos(time * 0.66 + p.phase * 1.7) * drift;
        }
        let a = p.alpha * e;
        if (p.tw && k >= 1) a *= 1 - p.tw * (0.5 + 0.5 * Math.sin(time * p.spd + p.phase));
        // 扫光提亮
        if (k >= 1) {
          const bd = Math.abs(px - sweepX);
          if (bd < sweepW) {
            const boost = 1 - bd / sweepW;
            a = Math.min(1, a * (1 + boost * boost * 1.1));
          }
        }
        if (pass === 0) {
          // 辉光垫底隔粒画即可，物理每粒都算（在上方）
          if ((i & 1) === 0) {
            ctx.globalAlpha = a * 0.18;
            const D = p.size * 2.8;
            ctx.drawImage(SP_BLUE, px - D / 2, py - D / 2, D, D);
          }
        } else {
          ctx.globalAlpha = a;
          const sp = p.blue ? SP_BLUE : SP_WHITE;
          ctx.drawImage(sp, px - p.size / 2, py - p.size / 2, p.size, p.size);
        }
      }
    }

    // 边缘溶解出去的浮尘：飘远渐隐，循环重生
    const dIn = Math.min(1, elapsed / 1100);
    for (let i = 0; i < debris.length; i++) {
      const d = debris[i];
      const cyc = ((time + d.t0) % d.dur) / d.dur;
      const r = d.base + d.reach * cyc;
      ctx.globalAlpha = d.alpha * (1 - cyc) * dIn;
      const D = d.size * 1.6;
      ctx.drawImage(
        SP_WHITE,
        d.x + Math.cos(d.ang) * r - D / 2,
        d.y + Math.sin(d.ang) * r - D / 2,
        D,
        D
      );
    }

    // 撞击火花：命中帧从流星头部迸出，四散渐熄
    for (let i = 0; i < meteors.length; i++) {
      const m = meteors[i];
      if (!m.hit) continue;
      m.flash = 1;
      m.hit = false;
      if (sparks.length < 60) {
        const nb = 2 + Math.floor(Math.random() * 3);
        for (let s = 0; s < nb; s++) {
          const a = Math.random() * Math.PI * 2;
          const sp = 50 + Math.random() * 130;
          sparks.push({
            x: m.x,
            y: m.y,
            vx: Math.cos(a) * sp + m.vx * 0.12,
            vy: Math.sin(a) * sp + m.vy * 0.12,
            life: 0,
            maxLife: 0.3 + Math.random() * 0.45,
            size: 3 + Math.random() * 3,
          });
        }
      }
    }
    for (let i = sparks.length - 1; i >= 0; i--) {
      const s = sparks[i];
      s.life += dt;
      if (s.life >= s.maxLife) {
        sparks.splice(i, 1);
        continue;
      }
      s.x += s.vx * dt;
      s.y += s.vy * dt;
      s.vx *= 0.96;
      s.vy *= 0.96;
      const fade = 1 - s.life / s.maxLife;
      ctx.globalAlpha = 0.9 * fade;
      const D = s.size * (0.6 + 0.4 * fade) * 2;
      ctx.drawImage(SP_WHITE, s.x - D / 2, s.y - D / 2, D, D);
    }

    // 流星本体：亮头 + 渐隐拖尾，叠在字标上方，真正「穿过」字面
    for (let i = 0; i < meteors.length; i++) {
      const m = meteors[i];
      const mv = Math.hypot(m.vx, m.vy) || 1;
      const TAIL = 84;
      const tlx = m.x - (m.vx / mv) * TAIL;
      const tly = m.y - (m.vy / mv) * TAIL;
      const grad = ctx.createLinearGradient(m.x, m.y, tlx, tly);
      grad.addColorStop(0, `rgba(228, 236, 255, ${(0.72 + m.flash * 0.28).toFixed(3)})`);
      grad.addColorStop(1, "rgba(228, 236, 255, 0)");
      ctx.strokeStyle = grad;
      ctx.lineWidth = 1.3;
      ctx.lineCap = "round";
      ctx.beginPath();
      ctx.moveTo(m.x, m.y);
      ctx.lineTo(tlx, tly);
      ctx.stroke();
      // 命中瞬间头部炸亮，随后回落
      const D = 13 + m.flash * 22;
      ctx.globalAlpha = 1;
      ctx.drawImage(SP_WHITE, m.x - D / 2, m.y - D / 2, D, D);
      if (m.flash > 0.02) m.flash *= 0.86;
      else m.flash = 0;
    }

    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = "source-over";
  };

  // 页面转入后台时 rAF 自动挂起、回前台自动续播，无需手动管 visibility
  const render = (t) => {
    if (!running) return;
    if (!startAt) startAt = t;
    lastFrameAt = performance.now();
    paint(t - startAt, t * 0.001);
    requestAnimationFrame(render);
  };

  /** 同步重绘当前状态：rAF 之外（重建 / 看门狗）也能立刻出画面 */
  const repaintNow = () => {
    const now = performance.now();
    paint(startAt ? now - startAt : 1e6, now * 0.001);
  };

  const start = () => {
    if (running || !built) return;
    running = true;
    requestAnimationFrame(render);
  };
  const stop = () => {
    running = false;
  };

  build();
  if (!built && !anchor.classList.contains("no-canvas")) {
    // 首帧布局未就绪时兜底重试一次
    requestAnimationFrame(() => {
      build();
      if (built && !reduceMotion) start();
      if (built) repaintNow();
    });
  }

  // Web 字体就绪后重采样，字标用上正式字体（已就绪时几乎无感）。
  // build 会清空画布，随即同步重绘，避免 rAF 未跑时留下空白
  if (document.fonts && document.fonts.ready) {
    document.fonts.ready.then(() => {
      build();
      repaintNow();
    });
  }

  let resizeTimer = 0;
  window.addEventListener(
    "resize",
    () => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        build(); // startAt 不重置：重建后直接呈现当前状态，不重播入场
        repaintNow();
      }, 200);
    },
    { passive: true }
  );

  if (reduceMotion) {
    paint(1e6, 0); // 静态一帧
    return;
  }

  // 先同步画一帧成形态：rAF 不可用的离屏 / 预渲染环境也能看到完整字标。
  // 正常浏览器里首个 rAF 在首次合成前就会触发，直接从入场动画开始，无闪烁。
  repaintNow();
  start();
  // 看门狗：rAF 停摆超过 400ms 时补画当前状态，恢复后循环自动接管
  setInterval(() => {
    if (built && performance.now() - lastFrameAt > 400) repaintNow();
  }, 500);

  hero.addEventListener(
    "pointermove",
    (event) => {
      const rect = canvas.getBoundingClientRect();
      mouseX = event.clientX - rect.left;
      mouseY = event.clientY - rect.top;
    },
    { passive: true }
  );
  hero.addEventListener("pointerleave", () => {
    mouseX = -9999;
    mouseY = -9999;
  });

  if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver(
      (entries) =>
        entries.forEach((en) => {
          if (en.isIntersecting) {
            start();
            return;
          }
          // 停帧前复核：确认真的滚出视野，防止环境误报把循环停死
          const r = canvas.getBoundingClientRect();
          if (r.bottom < 0 || r.top > window.innerHeight) stop();
          else start();
        }),
      { threshold: 0.01 }
    );
    observer.observe(canvas);
  }
}

/** 左侧终端侧注：循环高亮「正在执行」的一行，像后台真的在跑 */
function initSidenoteTicker () {
  if (reduceMotion) return;
  const lines = document.querySelectorAll(".hero-sidenote-left span");
  if (!lines.length) return;
  let idx = 0;
  // 等入场淡入结束后再开始循环
  setTimeout(() => {
    setInterval(() => {
      lines.forEach((el, i) => el.classList.toggle("is-active", i === idx));
      idx = (idx + 1) % lines.length;
    }, 1700);
  }, 2600);
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
initHeroScene();
initParticleLogo();
initSidenoteTicker();
initInteractiveHover();
initYear();
