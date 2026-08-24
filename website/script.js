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

/**
 * 导航滚动状态。
 * Hero 顶部是浅色天光渐变（导航透明 + 黑字），
 * 滚过亮区后切换为黑底白字。
 */
function initNav () {
  const nav = document.getElementById("nav");
  const LIGHT_ZONE = 380;
  const onScroll = () => nav.classList.toggle("scrolled", window.scrollY > LIGHT_ZONE);
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

/** ReactBits Orb shader port: same GLSL, vanilla WebGL instead of React/OGL. */
function initOrbShader () {
  if (reduceMotion) return;

  const hero = document.getElementById("hero");
  const canvas = document.getElementById("agent-field");
  if (!hero || !canvas) return;

  const gl = canvas.getContext("webgl", {
    alpha: true,
    premultipliedAlpha: false,
    antialias: true,
  });
  if (!gl) return;

  const vert = `
    precision highp float;
    attribute vec2 position;
    attribute vec2 uv;
    varying vec2 vUv;
    void main() {
      vUv = uv;
      gl_Position = vec4(position, 0.0, 1.0);
    }
  `;

  const frag = `
    precision highp float;

    uniform float iTime;
    uniform vec3 iResolution;
    uniform float hue;
    uniform float hover;
    uniform float rot;
    uniform float hoverIntensity;
    uniform vec3 backgroundColor;
    varying vec2 vUv;

    vec3 rgb2yiq(vec3 c) {
      float y = dot(c, vec3(0.299, 0.587, 0.114));
      float i = dot(c, vec3(0.596, -0.274, -0.322));
      float q = dot(c, vec3(0.211, -0.523, 0.312));
      return vec3(y, i, q);
    }

    vec3 yiq2rgb(vec3 c) {
      float r = c.x + 0.956 * c.y + 0.621 * c.z;
      float g = c.x - 0.272 * c.y - 0.647 * c.z;
      float b = c.x - 1.106 * c.y + 1.703 * c.z;
      return vec3(r, g, b);
    }

    vec3 adjustHue(vec3 color, float hueDeg) {
      float hueRad = hueDeg * 3.14159265 / 180.0;
      vec3 yiq = rgb2yiq(color);
      float cosA = cos(hueRad);
      float sinA = sin(hueRad);
      float i = yiq.y * cosA - yiq.z * sinA;
      float q = yiq.y * sinA + yiq.z * cosA;
      yiq.y = i;
      yiq.z = q;
      return yiq2rgb(yiq);
    }

    vec3 hash33(vec3 p3) {
      p3 = fract(p3 * vec3(0.1031, 0.11369, 0.13787));
      p3 += dot(p3, p3.yxz + 19.19);
      return -1.0 + 2.0 * fract(vec3(
        p3.x + p3.y,
        p3.x + p3.z,
        p3.y + p3.z
      ) * p3.zyx);
    }

    float snoise3(vec3 p) {
      const float K1 = 0.333333333;
      const float K2 = 0.166666667;
      vec3 i = floor(p + (p.x + p.y + p.z) * K1);
      vec3 d0 = p - (i - (i.x + i.y + i.z) * K2);
      vec3 e = step(vec3(0.0), d0 - d0.yzx);
      vec3 i1 = e * (1.0 - e.zxy);
      vec3 i2 = 1.0 - e.zxy * (1.0 - e);
      vec3 d1 = d0 - (i1 - K2);
      vec3 d2 = d0 - (i2 - K1);
      vec3 d3 = d0 - 0.5;
      vec4 h = max(0.6 - vec4(
        dot(d0, d0),
        dot(d1, d1),
        dot(d2, d2),
        dot(d3, d3)
      ), 0.0);
      vec4 n = h * h * h * h * vec4(
        dot(d0, hash33(i)),
        dot(d1, hash33(i + i1)),
        dot(d2, hash33(i + i2)),
        dot(d3, hash33(i + 1.0))
      );
      return dot(vec4(31.316), n);
    }

    vec4 extractAlpha(vec3 colorIn) {
      float a = max(max(colorIn.r, colorIn.g), colorIn.b);
      return vec4(colorIn.rgb / (a + 1e-5), a);
    }

    const vec3 baseColor1 = vec3(0.611765, 0.262745, 0.996078);
    const vec3 baseColor2 = vec3(0.298039, 0.760784, 0.913725);
    const vec3 baseColor3 = vec3(0.062745, 0.078431, 0.600000);
    const float innerRadius = 0.6;
    const float noiseScale = 0.65;

    float light1(float intensity, float attenuation, float dist) {
      return intensity / (1.0 + dist * attenuation);
    }

    float light2(float intensity, float attenuation, float dist) {
      return intensity / (1.0 + dist * dist * attenuation);
    }

    vec4 draw(vec2 uv) {
      vec3 color1 = adjustHue(baseColor1, hue);
      vec3 color2 = adjustHue(baseColor2, hue);
      vec3 color3 = adjustHue(baseColor3, hue);

      float ang = atan(uv.y, uv.x);
      float len = length(uv);
      float invLen = len > 0.0 ? 1.0 / len : 0.0;
      float bgLuminance = dot(backgroundColor, vec3(0.299, 0.587, 0.114));

      float n0 = snoise3(vec3(uv * noiseScale, iTime * 0.5)) * 0.5 + 0.5;
      float r0 = mix(mix(innerRadius, 1.0, 0.4), mix(innerRadius, 1.0, 0.6), n0);
      float d0 = distance(uv, (r0 * invLen) * uv);
      float v0 = light1(1.0, 10.0, d0);

      v0 *= smoothstep(r0 * 1.05, r0, len);
      float innerFade = smoothstep(r0 * 0.8, r0 * 0.95, len);
      v0 *= mix(innerFade, 1.0, bgLuminance * 0.7);
      float cl = cos(ang + iTime * 2.0) * 0.5 + 0.5;

      float a = iTime * -1.0;
      vec2 pos = vec2(cos(a), sin(a)) * r0;
      float d = distance(uv, pos);
      float v1 = light2(1.5, 5.0, d);
      v1 *= light1(1.0, 50.0, d0);

      float v2 = smoothstep(1.0, mix(innerRadius, 1.0, n0 * 0.5), len);
      float v3 = smoothstep(innerRadius, mix(innerRadius, 1.0, 0.5), len);

      vec3 colBase = mix(color1, color2, cl);
      float fadeAmount = mix(1.0, 0.1, bgLuminance);

      vec3 darkCol = mix(color3, colBase, v0);
      darkCol = (darkCol + v1) * v2 * v3;
      darkCol = clamp(darkCol, 0.0, 1.0);

      vec3 lightCol = (colBase + v1) * mix(1.0, v2 * v3, fadeAmount);
      lightCol = mix(backgroundColor, lightCol, v0);
      lightCol = clamp(lightCol, 0.0, 1.0);

      vec3 finalCol = mix(darkCol, lightCol, bgLuminance);
      return extractAlpha(finalCol);
    }

    vec4 mainImage(vec2 fragCoord) {
      vec2 center = iResolution.xy * 0.5;
      float size = min(iResolution.x, iResolution.y);
      vec2 uv = (fragCoord - center) / size * 2.0;

      float angle = rot;
      float s = sin(angle);
      float c = cos(angle);
      uv = vec2(c * uv.x - s * uv.y, s * uv.x + c * uv.y);

      float h = smoothstep(0.0, 1.0, hover);
      float warp = h * hoverIntensity;
      uv *= mix(1.0, 1.08 + sin(iTime * 0.55) * 0.025, h);
      uv.x += warp * (
        0.18 * sin(uv.y * 5.2 + iTime * 1.45) +
        0.08 * sin((uv.x + uv.y) * 10.0 - iTime * 0.7)
      );
      uv.y += warp * (
        0.16 * sin(uv.x * 5.6 - iTime * 1.15) +
        0.07 * cos((uv.x - uv.y) * 11.0 + iTime * 0.85)
      );

      return draw(uv);
    }

    void main() {
      vec2 fragCoord = vUv * iResolution.xy;
      vec4 col = mainImage(fragCoord);
      gl_FragColor = vec4(col.rgb * col.a, col.a);
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
    console.warn("Orb shader skipped:", error);
    return;
  }

  const vertices = new Float32Array([
    -1, -1, 0, 0,
    3, -1, 2, 0,
    -1, 3, 0, 2,
  ]);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(gl.ARRAY_BUFFER, vertices, gl.STATIC_DRAW);

  const position = gl.getAttribLocation(program, "position");
  const uv = gl.getAttribLocation(program, "uv");
  const stride = 4 * Float32Array.BYTES_PER_ELEMENT;
  gl.enableVertexAttribArray(position);
  gl.vertexAttribPointer(position, 2, gl.FLOAT, false, stride, 0);
  gl.enableVertexAttribArray(uv);
  gl.vertexAttribPointer(uv, 2, gl.FLOAT, false, stride, 2 * Float32Array.BYTES_PER_ELEMENT);

  const uniforms = {
    iTime: gl.getUniformLocation(program, "iTime"),
    iResolution: gl.getUniformLocation(program, "iResolution"),
    hue: gl.getUniformLocation(program, "hue"),
    hover: gl.getUniformLocation(program, "hover"),
    rot: gl.getUniformLocation(program, "rot"),
    hoverIntensity: gl.getUniformLocation(program, "hoverIntensity"),
    backgroundColor: gl.getUniformLocation(program, "backgroundColor"),
  };

  let width = 0;
  let height = 0;
  let hover = 0;
  let targetHover = 0;
  let currentRot = 0;
  let lastTime = 0;
  let lastDrawTime = 0;
  let running = false;
  const rotationSpeed = 0.58;
  const FRAME_MS = 33; // 约 30fps：背景装饰不需要 60fps，节流省一半 shader 执行

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    // DPR 上限 1.5：Retina 上把 backing store 从 3200×3200 压到 1500×1500，
    // shader 每帧要算的像素量直接降七成以上，肉眼几乎看不出差别。
    const dpr = Math.min(window.devicePixelRatio || 1, 1.5);
    width = Math.max(1, rect.width);
    height = Math.max(1, rect.height);
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height}px`;
    gl.viewport(0, 0, canvas.width, canvas.height);
  };

  const updateHover = (event) => {
    const heroRect = hero.getBoundingClientRect();
    const heroX = event.clientX - heroRect.left;
    const heroY = event.clientY - heroRect.top;
    const hotZoneCenterX = heroRect.width * 0.5;
    const hotZoneCenterY = heroRect.height * 0.82;
    const hotZoneRadius = Math.min(heroRect.width * 0.6, heroRect.height * 0.6);
    const dx = heroX - hotZoneCenterX;
    const dy = heroY - hotZoneCenterY;
    const insideHotSemiCircle = dy <= 0 && dx * dx + dy * dy <= hotZoneRadius * hotZoneRadius;
    targetHover = insideHotSemiCircle ? 1 : 0;
    document.documentElement.style.setProperty("--pointer-x", `${heroX}px`);
    document.documentElement.style.setProperty("--pointer-y", `${heroY}px`);
  };

  const render = (time) => {
    // 离屏或切后台：直接停掉，不再调度下一帧。
    if (!running || document.hidden) {
      lastTime = time;
      return;
    }

    const dt = (time - lastTime) * 0.001;
    lastTime = time;
    hover += (targetHover - hover) * 0.1;
    if (hover > 0.5) currentRot += dt * rotationSpeed;

    // 状态（hover/rot）每帧更新保持平滑；昂贵的 shader 绘制节流到约 30fps。
    if (time - lastDrawTime < FRAME_MS) {
      requestAnimationFrame(render);
      return;
    }
    lastDrawTime = time;

    gl.useProgram(program);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.uniform1f(uniforms.iTime, time * 0.001);
    gl.uniform3f(uniforms.iResolution, canvas.width, canvas.height, canvas.width / canvas.height);
    gl.uniform1f(uniforms.hue, 0);
    gl.uniform1f(uniforms.hover, hover);
    gl.uniform1f(uniforms.rot, currentRot);
    gl.uniform1f(uniforms.hoverIntensity, 1.55);
    gl.uniform3f(uniforms.backgroundColor, 0, 0, 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    requestAnimationFrame(render);
  };

  const start = () => {
    if (running) return;
    running = true;
    lastTime = 0;
    requestAnimationFrame(render);
  };

  const stop = () => {
    running = false;
    lastTime = 0;
  };

  window.addEventListener("resize", resize, { passive: true });
  hero.addEventListener("pointermove", updateHover, { passive: true });
  hero.addEventListener("pointerleave", () => {
    targetHover = 0;
  });

  resize();

  // hero 滚出视口就完全停止渲染（解决「滑到第二屏还空转、往回滑卡顿」），
  // 回到视口再重启。离屏时 orb 零开销。
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

/** 卡片与按钮的局部高光 / 轻微倾斜 */
function initInteractiveHover () {
  if (reduceMotion) return;

  const items = document.querySelectorAll(".btn, .nav-github, .trio-card, .bento-card, .dl-card, .hero-shot");

  items.forEach((el) => {
    el.addEventListener(
      "pointermove",
      (event) => {
        const rect = el.getBoundingClientRect();
        const px = ((event.clientX - rect.left) / rect.width) * 100;
        const py = ((event.clientY - rect.top) / rect.height) * 100;
        const dx = px / 100 - 0.5;
        const dy = py / 100 - 0.5;
        const tilt = el.classList.contains("hero-shot") ? 5 : 3.5;

        el.style.setProperty("--mouse-x", `${px.toFixed(2)}%`);
        el.style.setProperty("--mouse-y", `${py.toFixed(2)}%`);
        el.style.setProperty("--tilt-rx", `${(-dy * tilt).toFixed(2)}deg`);
        el.style.setProperty("--tilt-ry", `${(dx * tilt).toFixed(2)}deg`);
      },
      { passive: true }
    );

    el.addEventListener("pointerleave", () => {
      el.style.removeProperty("--mouse-x");
      el.style.removeProperty("--mouse-y");
      el.style.removeProperty("--tilt-rx");
      el.style.removeProperty("--tilt-ry");
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
initOrbShader();
initInteractiveHover();
initYear();
