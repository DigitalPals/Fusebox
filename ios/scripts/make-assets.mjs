#!/usr/bin/env node
// Writes Fusebox/Assets.xcassets from the repository's own artwork:
//   - AppIcon: assets/icon.svg's bracketed fuse on the dashboard's near-black, drawn as a 1024px PNG
//   - Mark: assets/icon.svg as a vector image
//   - logo-*: each <symbol> of ui/logos.svg (LobeHub Icons, MIT) as a vector image; one-colour
//     marks (currentColor) become template images so they take the text colour
//   - AccentColor (the fuse amber) and LaunchBackground (bg, light and dark)
// Run it again after changing the artwork: node ios/scripts/make-assets.mjs
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync, crc32 } from 'node:zlib';

const ios = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(ios, '..');
const out = join(ios, 'Fusebox/Assets.xcassets');
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const json = (path, value) => writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
const info = { author: 'xcode', version: 1 };
json(join(out, 'Contents.json'), { info });

// --- App icon: the mark's rectangles (48×56 units) on bg, scaled to fit with a margin.
function png(size, draw) {
  const rgb = Buffer.alloc(size * size * 3);
  draw((x0, y0, w, h, [r, g, b]) => {
    for (let y = Math.max(0, y0); y < Math.min(size, y0 + h); y++)
      for (let x = Math.max(0, x0); x < Math.min(size, x0 + w); x++) {
        const i = (y * size + x) * 3;
        rgb[i] = r; rgb[i + 1] = g; rgb[i + 2] = b;
      }
  });
  const raw = Buffer.alloc((size * 3 + 1) * size);
  for (let y = 0; y < size; y++) rgb.copy(raw, y * (size * 3 + 1) + 1, y * size * 3, (y + 1) * size * 3);
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td) >>> 0);
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0); ihdr.writeUInt32BE(size, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}
const BG = [0x0b, 0x0b, 0x0a], AMBER = [0xf3, 0xb5, 0x2f], BAR = [0xf4, 0xf4, 0xf5];
const unit = 10, size = 1024;
const ox = Math.round((size - 48 * unit) / 2), oy = Math.round((size - 56 * unit) / 2);
const rects = [
  [0, 0, 16, 8, AMBER], [0, 0, 8, 56, AMBER], [0, 48, 16, 8, AMBER],
  [32, 0, 16, 8, AMBER], [40, 0, 8, 56, AMBER], [32, 48, 16, 8, AMBER],
  [16, 24, 16, 8, BAR],
];
const icon = png(size, (fill) => {
  fill(0, 0, size, size, BG);
  for (const [x, y, w, h, c] of rects) fill(ox + x * unit, oy + y * unit, w * unit, h * unit, c);
});
mkdirSync(join(out, 'AppIcon.appiconset'));
writeFileSync(join(out, 'AppIcon.appiconset/icon-1024.png'), icon);
json(join(out, 'AppIcon.appiconset/Contents.json'), {
  images: [{ filename: 'icon-1024.png', idiom: 'universal', platform: 'ios', size: '1024x1024' }], info,
});

// --- Vector images.
function imageset(name, svg, template) {
  const dir = join(out, `${name}.imageset`);
  mkdirSync(dir);
  writeFileSync(join(dir, `${name}.svg`), svg);
  json(join(dir, 'Contents.json'), {
    images: [{ filename: `${name}.svg`, idiom: 'universal' }], info,
    properties: { 'preserves-vector-representation': true, ...(template ? { 'template-rendering-intent': 'template' } : { 'template-rendering-intent': 'original' }) },
  });
}
imageset('Mark', readFileSync(join(repo, 'assets/icon.svg'), 'utf8').replace(/<title>.*?<\/title>/, ''), false);

const logos = readFileSync(join(repo, 'ui/logos.svg'), 'utf8');
for (const m of logos.matchAll(/<symbol id="(logo-[^"]+)"[^>]*viewBox="([^"]+)"[^>]*>([\s\S]*?)<\/symbol>/g)) {
  const [, id, viewBox, body] = m;
  const mono = body.includes('currentColor') && !/fill="#[0-9A-Fa-f]{3,6}"/.test(body);
  const [, , w, h] = viewBox.split(/\s+/);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" width="${w}" height="${h}">${body.replaceAll('currentColor', '#000000')}</svg>\n`;
  imageset(id, svg, mono);
}

// --- Colours.
function colorset(name, dark, light) {
  const dir = join(out, `${name}.colorset`);
  mkdirSync(dir);
  const comp = (hex) => ({ 'color-space': 'srgb', components: { red: `0x${hex.slice(0, 2)}`, green: `0x${hex.slice(2, 4)}`, blue: `0x${hex.slice(4, 6)}`, alpha: '1.000' } });
  json(join(dir, 'Contents.json'), {
    colors: [
      { idiom: 'universal', color: comp(light) },
      { idiom: 'universal', appearances: [{ appearance: 'luminosity', value: 'dark' }], color: comp(dark) },
    ], info,
  });
}
colorset('AccentColor', 'F3B52F', 'F3B52F');
colorset('LaunchBackground', '0B0B0A', 'F6F4EF');
console.log(`wrote ${out}`);
