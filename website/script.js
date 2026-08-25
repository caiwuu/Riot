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

function versionLabel(tag) {
  const v = String(tag || "")
    .replace(/^Riot_/i, "")
    .replace(/^v/i, "");
  return v ? `v${v}` : "";
}

function pickAsset(assets, suffix) {
  return (assets || []).find((a) => typeof a.name === "string" && a.name.endsWith(suffix));
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
 * 动画 Logo：参考产品 logo 用 canvas 粒子重构（Canvas UI「Particle Object」思路）。
 * 采样 logo 像素生成粒子点阵：花瓣主体为冷蓝紫粒子，字母 R 为亮白粒子。
 * 粒子带呼吸漂移与闪烁，鼠标靠近被推开，移开后弹性归位；入场时从四散汇聚成形。
 */
function initParticleLogo () {
  const canvas = document.getElementById("logo-canvas");
  const hero = document.getElementById("hero");
  if (!canvas || !hero || reduceMotion) return;

  const showFallback = () => {
    canvas.style.display = "none";
    const fallback = canvas.parentElement.querySelector(".hl-fallback");
    if (fallback) fallback.style.display = "block";
  };

  const ctx = canvas.getContext("2d");
  if (!ctx) {
    showFallback();
    return;
  }

  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const BODY_COLORS = ["132,152,220", "158,170,240", "182,172,250", "112,132,198"];
  const REPEL_RADIUS = 64;
  const REPEL_FORCE = 4.6;

  let W = 0;
  let H = 0;
  let particles = [];
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

  /** 采样 logo 像素，生成粒子锚点 */
  const build = () => {
    resize();
    particles = [];

    const logoSize = Math.max(40, Math.round(Math.min(W, H) * 0.56));
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

    const step = Math.max(2, Math.round(logoSize / 66));
    const offX = (W - logoSize) / 2;
    const offY = (H - logoSize) / 2;

    for (let y = 0; y < logoSize; y += step) {
      for (let x = 0; x < logoSize; x += step) {
        const kind = classify(x, y);
        if (!kind) continue;

        const hx = offX + x;
        const hy = offY + y;
        const angle = Math.random() * Math.PI * 2;
        const dist = 50 + Math.random() * 130;
        const letter = kind === "letter";

        particles.push({
          hx,
          hy,
          x: hx + Math.cos(angle) * dist,
          y: hy + Math.sin(angle) * dist,
          vx: 0,
          vy: 0,
          size: letter ? 1.7 + Math.random() * 1.1 : 1.1 + Math.random() * 1.1,
          color: letter
            ? "rgb(244,247,255)"
            : `rgb(${BODY_COLORS[(Math.random() * BODY_COLORS.length) | 0]})`,
          alpha: letter ? 0.85 + Math.random() * 0.15 : 0.3 + Math.random() * 0.4,
          spring: 0.045 + Math.random() * 0.05,
          phase: Math.random() * Math.PI * 2,
          speed: 0.35 + Math.random() * 0.75,
          twinkle: Math.random() < 0.14,
        });
      }
    }

    built = particles.length > 0;
  };

  const render = (t) => {
    if (!running || document.hidden) return;

    const time = t * 0.001;
    if (!revealStart) revealStart = t;
    const reveal = Math.min(1, (t - revealStart) / 1500);

    ctx.clearRect(0, 0, W, H);

    for (let i = 0; i < particles.length; i++) {
      const p = particles[i];

      // 呼吸漂移的锚点
      const ix = p.hx + Math.sin(time * p.speed + p.phase) * 0.8;
      const iy = p.hy + Math.cos(time * p.speed * 0.9 + p.phase * 1.7) * 0.8;

      // 弹性归位
      p.vx += (ix - p.x) * p.spring;
      p.vy += (iy - p.y) * p.spring;

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

      p.vx *= 0.86;
      p.vy *= 0.86;
      p.x += p.vx;
      p.y += p.vy;

      let a = p.alpha * reveal;
      if (p.twinkle) a *= 0.55 + 0.45 * Math.sin(time * 2.4 + p.phase * 3.1);

      ctx.globalAlpha = a;
      ctx.fillStyle = p.color;
      ctx.fillRect(p.x - p.size / 2, p.y - p.size / 2, p.size, p.size);
    }

    ctx.globalAlpha = 1;
    requestAnimationFrame(render);
  };

  const start = () => {
    if (running || !built) return;
    running = true;
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
initReveal();
initHeroTitle();
initParticleLogo();
initAuroraShader();
initInteractiveHover();
initYear();
