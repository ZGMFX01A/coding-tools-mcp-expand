import { build } from 'vite';
import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';
import fs from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const rootDir = resolve(__dirname, '..');
const outDir = resolve(rootDir, 'dist');

// 清空 dist
if (fs.existsSync(outDir)) {
  fs.rmSync(outDir, { recursive: true, force: true });
}
fs.mkdirSync(outDir, { recursive: true });

const entries = [
  { name: 'page-hook', file: 'src/page-hook.ts' },
  { name: 'bridge', file: 'src/bridge.ts' },
  { name: 'options', file: 'src/options.ts' },
];

for (const entry of entries) {
  await build({
    configFile: false,
    build: {
      outDir,
      emptyOutDir: false,
      lib: {
        entry: resolve(rootDir, entry.file),
        name: entry.name.replace('-', '_'),
        formats: ['iife'],
        fileName: () => `${entry.name}.js`,
      },
      rollupOptions: {
        output: {
          inlineDynamicImports: true,
          extend: true,
        },
      },
    },
  });
}

// 复制资源文件
if (fs.existsSync(resolve(rootDir, 'manifest.json'))) {
  fs.copyFileSync(resolve(rootDir, 'manifest.json'), resolve(outDir, 'manifest.json'));
}

const stylesDir = resolve(outDir, 'styles');
if (!fs.existsSync(stylesDir)) fs.mkdirSync(stylesDir, { recursive: true });
if (fs.existsSync(resolve(rootDir, 'styles/overlay.css'))) {
  fs.copyFileSync(resolve(rootDir, 'styles/overlay.css'), resolve(stylesDir, 'overlay.css'));
}

const optionsDir = resolve(outDir, 'options');
if (!fs.existsSync(optionsDir)) fs.mkdirSync(optionsDir, { recursive: true });
if (fs.existsSync(resolve(rootDir, 'options/options.html'))) {
  fs.copyFileSync(resolve(rootDir, 'options/options.html'), resolve(optionsDir, 'options.html'));
}
if (fs.existsSync(resolve(rootDir, 'options/options.css'))) {
  fs.copyFileSync(resolve(rootDir, 'options/options.css'), resolve(optionsDir, 'options.css'));
}

console.log('✅ Chrome Extension build completed successfully!');
