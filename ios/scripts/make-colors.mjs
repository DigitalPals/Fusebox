#!/usr/bin/env node
// Writes the dashboard's colour tokens (DESIGN.md, ui/style.css :root and its light block) into
// the asset catalogue as colour sets with a dark and a light appearance, so SwiftUI's
// Color("fg") and the widgets follow the system appearance. Run after changing a token:
//
//   node ios/scripts/make-colors.mjs
//
// No amber: the app's accent is `fg` (John asked for no brand yellow anywhere in the app).
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const catalog = join(dirname(fileURLToPath(import.meta.url)), '..', 'Fusebox', 'Assets.xcassets');

// name: [dark, light]
const tokens = {
  bg: ['0b0b0a', 'f6f4ef'],
  surface: ['121210', 'fcfbf8'],
  'surface-pop': ['141412', 'ffffff'],
  'surface-inset': ['0e0e0c', 'f0ede6'],
  'seg-on': ['2a2925', 'e3dfd5'],
  'ctl-hover': ['1c1b18', 'ebe8e0'],
  line: ['24231f', 'e3dfd6'],
  'line-soft': ['1d1c19', 'ece9e2'],
  'line-strong': ['33312c', 'd2cdc2'],
  'line-hover': ['4a4740', 'a9a397'],
  rivet: ['2b2a26', 'd9d4c9'],
  fg: ['f2efe8', '1b1a17'],
  'fg-2': ['b4afa4', '4a4741'],
  'fg-3': ['857f74', '6b665c'],
  idle: ['55524b', 'a39e92'],
  bar: ['3a3833', 'd3cec3'],
  meter: ['d9d4c8', '3a3833'],
  ok: ['4ade80', '15803d'],
  warn: ['fbbf24', 'a16207'],
  err: ['fb7185', 'be123c'],
  'err-line': ['4a2a30', 'f0c3cc'],
  // Usage's stacked chart: neutral steps from `meter` down, so colour stays for state.
  'chart-1': ['d9d4c8', '2a2925'],
  'chart-2': ['aea99d', '57534b'],
  'chart-3': ['857f74', '7f796e'],
  'chart-4': ['625d55', 'a39e92'],
  'chart-5': ['4a4740', 'c2bdb1'],
  'chart-other': ['33312c', 'dcd7cc'],
  AccentColor: ['f2efe8', '1b1a17'],
  LaunchBackground: ['0b0b0a', 'f6f4ef'],
};

const component = (hex, i) => `0x${hex.slice(i * 2, i * 2 + 2).toUpperCase()}`;
const color = (hex) => ({
  'color-space': 'srgb',
  components: { red: component(hex, 0), green: component(hex, 1), blue: component(hex, 2), alpha: '1.000' },
});

for (const [name, [dark, light]] of Object.entries(tokens)) {
  const dir = join(catalog, `${name}.colorset`);
  mkdirSync(dir, { recursive: true });
  const json = {
    colors: [
      { idiom: 'universal', color: color(light) },
      { idiom: 'universal', appearances: [{ appearance: 'luminosity', value: 'light' }], color: color(light) },
      { idiom: 'universal', appearances: [{ appearance: 'luminosity', value: 'dark' }], color: color(dark) },
    ],
    info: { author: 'xcode', version: 1 },
  };
  writeFileSync(join(dir, 'Contents.json'), `${JSON.stringify(json, null, 2)}\n`);
}
console.log(`wrote ${Object.keys(tokens).length} colour sets to ${catalog}`);
