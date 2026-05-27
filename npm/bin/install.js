const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const https = require('https');

const VERSION = '0.3.0';
const BASE = `https://github.com/Hoemr/agent-monitor/releases/download/v${VERSION}`;

const targets = {
  'win32-x64':   'agent-monitor.exe',
  'darwin-x64':  'agent-monitor-x86_64-apple-darwin',
  'darwin-arm64':'agent-monitor-aarch64-apple-darwin',
  'linux-x64':   'agent-monitor-x86_64-unknown-linux-gnu',
};

const platform = `${process.platform}-${process.arch}`;
const name = targets[platform];
if (!name) {
  console.log(`agent-monitor: no prebuilt binary for ${platform}`);
  process.exit(0);
}

const dest = path.join(__dirname, '..', 'bin', process.platform === 'win32' ? 'agent-monitor.exe' : 'agent-monitor');

// Skip download if already installed
if (fs.existsSync(dest)) {
  console.log(`agent-monitor binary already installed`);
  process.exit(0);
}

// If running from monorepo source, copy from target/
const srcLocal = path.join(__dirname, '..', '..', 'src-tauri', 'target', 'release', 'agent-monitor.exe');
if (fs.existsSync(srcLocal)) {
  fs.copyFileSync(srcLocal, dest);
  if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
  console.log('agent-monitor: installed from local build');
  process.exit(0);
}

// Download from GitHub Releases
console.log(`agent-monitor: downloading ${name} from GitHub Releases...`);
const url = `${BASE}/${name}`;
const file = fs.createWriteStream(dest);
https.get(url, (res) => {
  if (res.statusCode === 302) {
    https.get(res.headers.location, (r) => { r.pipe(file); });
    return;
  }
  res.pipe(file);
  file.on('finish', () => {
    if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
    console.log('agent-monitor: installed successfully');
  });
}).on('error', (e) => {
  console.error(`agent-monitor: download failed: ${e.message}`);
  try { fs.unlinkSync(dest); } catch (_) {}
  process.exit(1);
});
