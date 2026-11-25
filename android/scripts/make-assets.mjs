#!/usr/bin/env node
// Generates Android drawables, adaptive launcher icon, fonts, and colour resources
// from the repository's own artwork (assets/icon.svg, ui/logos.svg, ios/Shared/Fonts, DESIGN.md).
// Run after changing artwork or tokens:
//   node android/scripts/make-assets.mjs
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const androidDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(androidDir, '..');
const resDir = join(androidDir, 'app/src/main/res');

const ensureDir = (p) => mkdirSync(p, { recursive: true });
const write = (rel, content) => {
  const full = join(resDir, rel);
  ensureDir(dirname(full));
  writeFileSync(full, content);
};

// 1. Copy bundled fonts (SIL OFL 1.1) from ios/Shared/Fonts with Android resource names.
const fontMap = [
  ['IBMPlexSans.ttf', 'ibm_plex_sans.ttf'],
  ['IBMPlexSansCondensed-Medium.ttf', 'ibm_plex_sans_condensed_medium.ttf'],
  ['IBMPlexSansCondensed-SemiBold.ttf', 'ibm_plex_sans_condensed_semibold.ttf'],
  ['DMMono-Regular.ttf', 'dm_mono_regular.ttf'],
  ['DMMono-Medium.ttf', 'dm_mono_medium.ttf'],
];
ensureDir(join(resDir, 'font'));
for (const [src, dst] of fontMap) {
  copyFileSync(join(repo, 'ios/Shared/Fonts', src), join(resDir, 'font', dst));
}

// 2. Colour resources (light in values/, dark in values-night/) matching DESIGN.md & ios/scripts/make-colors.mjs.
// No brand amber in the app UI (accent is fg); amber is only for the launcher icon.
const tokens = {
  bg: ['0b0b0a', 'f6f4ef'],
  surface: ['121210', 'fcfbf8'],
  surface_pop: ['141412', 'ffffff'],
  surface_inset: ['0e0e0c', 'f0ede6'],
  seg_on: ['2a2925', 'e3dfd5'],
  ctl_hover: ['1c1b18', 'ebe8e0'],
  line: ['24231f', 'e3dfd6'],
  line_soft: ['1d1c19', 'ece9e2'],
  line_strong: ['33312c', 'd2cdc2'],
  line_hover: ['4a4740', 'a9a397'],
  rivet: ['2b2a26', 'd9d4c9'],
  fg: ['f2efe8', '1b1a17'],
  fg_2: ['b4afa4', '4a4741'],
  fg_3: ['857f74', '6b665c'],
  idle: ['55524b', 'a39e92'],
  bar: ['3a3833', 'd3cec3'],
  meter: ['d9d4c8', '3a3833'],
  ok: ['4ade80', '15803d'],
  warn: ['fbbf24', 'a16207'],
  err: ['fb7185', 'be123c'],
  err_line: ['4a2a30', 'f0c3cc'],
  chart_1: ['d9d4c8', '2a2925'],
  chart_2: ['aea99d', '57534b'],
  chart_3: ['857f74', '7f796e'],
  chart_4: ['625d55', 'a39e92'],
  chart_5: ['4a4740', 'c2bdb1'],
  chart_other: ['33312c', 'dcd7cc'],
};

const renderColorsXml = (idx) => {
  const lines = Object.entries(tokens).map(
    ([name, pair]) => `    <color name="fb_${name}">#${pair[idx].toUpperCase()}</color>`,
  );
  return `<?xml version="1.0" encoding="utf-8"?>\n<resources>\n${lines.join('\n')}\n    <color name="ic_launcher_background">#0B0B0A</color>\n</resources>\n`;
};
write('values/colors.xml', renderColorsXml(1));
write('values-night/colors.xml', renderColorsXml(0));

// 3. Adaptive launcher icon (assets/icon.svg: 48x56 bracketed fuse centred inside 108x108 canvas).
// Android launchers clip 108dp down to a 72dp circle/squircle (safe zone diameter 66dp, radius 33dp).
// Scaling by 0.7 gives 33.6 x 39.2 dp (46.7% x 54.4% of the 72dp visible mask, matching the iOS icon's
// 46.9% x 54.7% proportion) with corner radius 25.8dp — comfortably clear of circular and squircle masks.
const n = (v) => Number(v.toFixed(2));
const fuseRectPath = (x, y, w, h) => `M${n(x)},${n(y)}h${n(w)}v${n(h)}h${n(-w)}z`;
const launcherPaths = (amberColor, barColor, ox = 37.2, oy = 34.4, s = 0.7) => {
  const leftBracket = [
    fuseRectPath(ox + 0 * s, oy + 0 * s, 16 * s, 8 * s),
    fuseRectPath(ox + 0 * s, oy + 0 * s, 8 * s, 56 * s),
    fuseRectPath(ox + 0 * s, oy + 48 * s, 16 * s, 8 * s),
  ].join(' ');
  const rightBracket = [
    fuseRectPath(ox + 32 * s, oy + 0 * s, 16 * s, 8 * s),
    fuseRectPath(ox + 40 * s, oy + 0 * s, 8 * s, 56 * s),
    fuseRectPath(ox + 32 * s, oy + 48 * s, 16 * s, 8 * s),
  ].join(' ');
  const bar = fuseRectPath(ox + 16 * s, oy + 24 * s, 16 * s, 8 * s);
  return `    <path android:fillColor="${amberColor}" android:pathData="${leftBracket} ${rightBracket}" />\n    <path android:fillColor="${barColor}" android:pathData="${bar}" />`;
};

write(
  'drawable/ic_launcher_foreground.xml',
  `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="108dp"
    android:height="108dp"
    android:viewportWidth="108"
    android:viewportHeight="108">
${launcherPaths('#FFF3B52F', '#FFF4F4F5')}
</vector>
`,
);

write(
  'drawable/ic_launcher_monochrome.xml',
  `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="108dp"
    android:height="108dp"
    android:viewportWidth="108"
    android:viewportHeight="108">
${launcherPaths('#FF000000', '#FF000000')}
</vector>
`,
);

const adaptiveIconXml = `<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@drawable/ic_launcher_foreground" />
    <monochrome android:drawable="@drawable/ic_launcher_monochrome" />
</adaptive-icon>
`;
write('mipmap-anydpi-v26/ic_launcher.xml', adaptiveIconXml);
write('mipmap-anydpi-v26/ic_launcher_round.xml', adaptiveIconXml);

// One-colour Fusebox mark (tinted to fg in UI/widgets) & 24dp notification status bar icon.
write(
  'drawable/ic_mark.xml',
  `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="48dp"
    android:height="56dp"
    android:viewportWidth="48"
    android:viewportHeight="56">
${launcherPaths('#FF000000', '#FF000000', 0, 0, 1)}
</vector>
`,
);

write(
  'drawable/ic_stat_fusebox.xml',
  `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="64"
    android:viewportHeight="64">
${launcherPaths('#FFFFFFFF', '#FFFFFFFF', 8, 4, 1)}
</vector>
`,
);

// 4. Provider logos from ui/logos.svg -> Android Vector Drawables.
const namedColors = {
  gold: '#FFFFD700',
  '#fff': '#FFFFFFFF',
  '#000': '#FF000000',
};
const normalizeColor = (c, alpha = 1) => {
  if (!c) return '#FF000000';
  const low = c.trim().toLowerCase();
  if (low === 'currentcolor') return '#FF000000';
  let hex = namedColors[low] || c.trim();
  if (/^#[0-9a-f]{3}$/i.test(hex)) {
    hex = `#${hex[1]}${hex[1]}${hex[2]}${hex[2]}${hex[3]}${hex[3]}`;
  }
  hex = hex.replace(/^#/, '').toUpperCase();
  const rgb = hex.length === 8 ? hex.slice(2) : hex.padStart(6, '0');
  const a = Math.round(Math.max(0, Math.min(1, alpha)) * 255)
    .toString(16)
    .padStart(2, '0')
    .toUpperCase();
  return `#${a}${rgb}`;
};

const parseAttrs = (tag) => {
  const attrs = {};
  for (const m of tag.matchAll(/([a-zA-Z0-9_:-]+)="([^"]*)"/g)) {
    attrs[m[1]] = m[2];
  }
  return attrs;
};

const parseGradients = (body) => {
  const grads = new Map();
  for (const m of body.matchAll(/<linearGradient\b([^>]*)>([\s\S]*?)<\/linearGradient>/g)) {
    const a = parseAttrs(m[1]);
    const isPct = (v) => typeof v === 'string' && v.includes('%');
    const coord = (v, fallback) => {
      if (v == null) return fallback;
      const n = parseFloat(v);
      return isPct(v) ? (n / 100) * 24 : n;
    };
    const stops = [];
    for (const sm of m[2].matchAll(/<stop\b([^>]*)\/?>/g)) {
      const sa = parseAttrs(sm[1]);
      let offset = sa.offset ? parseFloat(sa.offset) : 0;
      if (sa.offset && sa.offset.includes('%')) offset /= 100;
      const opacity = sa['stop-opacity'] != null ? parseFloat(sa['stop-opacity']) : 1;
      stops.push({ offset, color: normalizeColor(sa['stop-color'] || '#000000', opacity) });
    }
    grads.set(a.id, {
      startX: coord(a.x1, 0),
      startY: coord(a.y1, 0),
      endX: coord(a.x2, 24),
      endY: coord(a.y2, 0),
      stops,
    });
  }
  return grads;
};

const renderGradientAttr = (g) => {
  const items = g.stops
    .map((s) => `                <item android:color="${s.color}" android:offset="${s.offset}" />`)
    .join('\n');
  return `        <aapt:attr name="android:fillColor">
            <gradient
                android:type="linear"
                android:startX="${g.startX}"
                android:startY="${g.startY}"
                android:endX="${g.endX}"
                android:endY="${g.endY}">
${items}
            </gradient>
        </aapt:attr>`;
};

const logosSvg = readFileSync(join(repo, 'ui/logos.svg'), 'utf8');
const monoLogos = [];

for (const m of logosSvg.matchAll(/<symbol id="(logo-[^"]+)"[^>]*viewBox="([^"]+)"[^>]*>([\s\S]*?)<\/symbol>/g)) {
  const [, rawId, viewBox, body] = m;
  const resName = rawId.replaceAll('-', '_');
  const [, , vw, vh] = viewBox.split(/\s+/);

  // Special case: logo-antigravity uses SVG blur filters unsupported by Android VectorDrawable.
  // Render its exact silhouette path with its 4-colour Google gradient.
  if (rawId === 'logo-antigravity') {
    const maskPath = body.match(/<path d="([^"]+)"/)?.[1];
    write(
      `drawable/${resName}.xml`,
      `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    xmlns:aapt="http://schemas.android.com/aapt"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="${vw}"
    android:viewportHeight="${vh}">
    <path android:pathData="${maskPath}">
        <aapt:attr name="android:fillColor">
            <gradient
                android:type="linear"
                android:startX="2"
                android:startY="22"
                android:endX="22"
                android:endY="2">
                <item android:color="#FF00B95C" android:offset="0.0" />
                <item android:color="#FF3186FF" android:offset="0.38" />
                <item android:color="#FFFBBC04" android:offset="0.7" />
                <item android:color="#FFFC413D" android:offset="1.0" />
            </gradient>
        </aapt:attr>
    </path>
</vector>
`,
    );
    continue;
  }

  // Special case: logo-compat has a rounded rect + two chevrons with stroke=currentColor.
  if (rawId === 'logo-compat') {
    monoLogos.push(resName);
    write(
      `drawable/${resName}.xml`,
      `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="24"
    android:viewportHeight="24">
    <path
        android:pathData="M8,3.5h8a4.5,4.5 0 0,1 4.5,4.5v8a4.5,4.5 0 0,1 -4.5,4.5h-8a4.5,4.5 0 0,1 -4.5,-4.5v-8a4.5,4.5 0 0,1 4.5,-4.5z"
        android:strokeColor="#FF000000"
        android:strokeWidth="1.6"
        android:strokeLineCap="round"
        android:strokeLineJoin="round" />
    <path
        android:pathData="M9.5,9 L7,12 L9.5,15 M14.5,9 L17,12 L14.5,15"
        android:strokeColor="#FF000000"
        android:strokeWidth="1.6"
        android:strokeLineCap="round"
        android:strokeLineJoin="round" />
</vector>
`,
    );
    continue;
  }

  // Special case: logo-kimi has a white K mark + blue dot; use dark K in light mode and light K in dark mode.
  if (rawId === 'logo-kimi') {
    const paths = [...body.matchAll(/<path\b([^>]*)\/?>/g)].map((pm) => parseAttrs(pm[1]));
    const makeKimi = (kColor) => `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="${vw}"
    android:viewportHeight="${vh}">
    <path android:fillColor="#FF1783FF" android:pathData="${paths[0].d}" />
    <path android:fillColor="${kColor}" android:pathData="${paths[1].d}" />
</vector>
`;
    write(`drawable/${resName}.xml`, makeKimi('#FF1B1A17'));
    write(`drawable-night/${resName}.xml`, makeKimi('#FFF2EFE8'));
    continue;
  }

  const mono = body.includes('currentColor') && !/fill="#[0-9A-Fa-f]{3,6}"/.test(body);
  if (mono) monoLogos.push(resName);

  const grads = parseGradients(body);
  const gMatch = body.match(/<g\b([^>]*)>/);
  const gAttrs = gMatch ? parseAttrs(gMatch[1]) : {};
  const defaultFill = gAttrs.fill || '#000000';
  const defaultFillRule = gAttrs['fill-rule'] || '';

  let needsAapt = false;
  const pathXmls = [];
  for (const pm of body.matchAll(/<path\b([^>]*)\/?>/g)) {
    const a = parseAttrs(pm[1]);
    const fill = a.fill || defaultFill;
    const fillRule = a['fill-rule'] || defaultFillRule;
    const fillAlpha = a['fill-opacity'] != null ? parseFloat(a['fill-opacity']) : 1;
    const fillTypeAttr = fillRule === 'evenodd' ? '\n        android:fillType="evenOdd"' : '';
    const gradMatch = fill.match(/^url\(#([^)]+)\)$/);
    if (gradMatch && grads.has(gradMatch[1])) {
      needsAapt = true;
      pathXmls.push(`    <path
        android:pathData="${a.d}"${fillTypeAttr}>
${renderGradientAttr(grads.get(gradMatch[1]))}
    </path>`);
    } else {
      const hex = normalizeColor(fill, fillAlpha);
      pathXmls.push(`    <path
        android:fillColor="${hex}"
        android:pathData="${a.d}"${fillTypeAttr} />`);
    }
  }

  const aaptNs = needsAapt ? '\n    xmlns:aapt="http://schemas.android.com/aapt"' : '';
  write(
    `drawable/${resName}.xml`,
    `<?xml version="1.0" encoding="utf-8"?>
<vector xmlns:android="http://schemas.android.com/apk/res/android"${aaptNs}
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="${vw}"
    android:viewportHeight="${vh}">
${pathXmls.join('\n')}
</vector>
`,
  );
}

console.log(`Generated Android assets in ${resDir}. Monochrome logos: ${monoLogos.join(', ')}`);
