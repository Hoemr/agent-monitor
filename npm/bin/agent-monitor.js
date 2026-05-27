#!/usr/bin/env node
const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

const bin = path.join(__dirname, process.platform === 'win32' ? 'agent-monitor.exe' : 'agent-monitor');

if (!fs.existsSync(bin)) {
  console.error('agent-monitor binary not found. Run: npm install -g agent-monitor');
  process.exit(1);
}

// Detach so the CLI exits but the widget stays running
spawn(bin, [], { detached: true, stdio: 'ignore' }).unref();
