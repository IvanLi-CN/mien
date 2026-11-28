#!/usr/bin/env node
/**
 * Reproducible CDP probe for the pinned fingerprint-chromium macOS build.
 * Run from repository root: node docs/spikes/kernel/probe.mjs --suite smoke
 */
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const defaultBinary = join(root, 'fixtures/.cache/kernel/148/macos-arm64/Chromium.app/Contents/MacOS/Chromium');
const probePage = join(dirname(fileURLToPath(import.meta.url)), 'probe.html');
const defaultOutput = join(dirname(fileURLToPath(import.meta.url)), 'evidence');
const expectedAssetSha256 = 'b72f091e2e1a7583eed389c4b8e3534ed355e568af8c8bbf8fc30a25e23ca679';
const binarySha256 = 'd918c691ce56648bc555f8f399c29fc83eb02224165a086491688d01707170cb';

function usage() {
  console.log(`Usage: node docs/spikes/kernel/probe.mjs [options]

Options:
  --suite smoke|modes|flags|seeds|all  Run a named reproducible suite (default: smoke)
  --case NAME                         Run one case, useful for debugging
  --mode headful|headless|pseudo      Mode for --case (default: headless)
  --seed N                            --fingerprint value for --case
  --binary PATH                       Chromium executable path
  --output DIR                        Evidence output directory
  --keep-profile                      Do not remove temporary browser profile
  --help                              Show this help

The browser is started with --remote-debugging-port, --no-first-run,
--no-default-browser-check, --use-mock-keychain and --no-sandbox. The latter
is only for this local probe and is not evidence that the seatbelt sandbox is
available.`);
}

function parseArgs(argv) {
  const args = {suite: 'smoke', mode: 'headless', binary: defaultBinary, output: defaultOutput, keepProfile: false};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--help') { usage(); process.exit(0); }
    if (arg === '--keep-profile') { args.keepProfile = true; continue; }
    const [key, inline] = arg.split('=', 2);
    const value = inline ?? argv[++i];
    if (key === '--suite') args.suite = value;
    else if (key === '--case') args.caseName = value;
    else if (key === '--mode') args.mode = value;
    else if (key === '--seed') args.seed = value;
    else if (key === '--binary') args.binary = resolve(value);
    else if (key === '--output') args.output = resolve(value);
    else throw new Error(`Unknown option: ${arg}`);
  }
  return args;
}

function sanitize(value) {
  return String(value).replaceAll(root, '<repo>').replaceAll(process.env.HOME ?? '', '<home>');
}

async function listen(server) {
  await new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolvePromise);
  });
  return server.address().port;
}

function cdpConnect(url) {
  const ws = new WebSocket(url);
  const pending = new Map();
  let nextId = 0;
  const opened = new Promise((resolvePromise, reject) => {
    ws.addEventListener('open', () => resolvePromise());
    ws.addEventListener('error', reject);
  });
  ws.addEventListener('message', event => {
    let message;
    try { message = JSON.parse(String(event.data)); } catch { return; }
    if (message.id && pending.has(message.id)) {
      const {resolve: resolvePromise, reject} = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolvePromise(message.result);
    }
  });
  const command = async (method, params = {}) => {
    await opened;
    const id = ++nextId;
    return new Promise((resolvePromise, reject) => {
      pending.set(id, {resolve: resolvePromise, reject});
      ws.send(JSON.stringify({id, method, params}));
    });
  };
  return {command, close: () => ws.close()};
}

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return response.json();
}

async function waitForCdp(port, child) {
  const deadline = Date.now() + 30_000;
  let lastError;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Chromium exited before CDP became ready: ${child.exitCode}`);
    try { return await fetchJson(`http://127.0.0.1:${port}/json`); } catch (error) { lastError = error; await delay(150); }
  }
  throw new Error(`Timed out waiting for CDP: ${lastError}`);
}

async function terminate(child) {
  if (child.exitCode !== null) return;
  try { process.kill(-child.pid, 'SIGTERM'); } catch { child.kill('SIGTERM'); }
  await Promise.race([child.exitPromise, delay(4_000)]);
  if (child.exitCode === null) {
    try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
    await Promise.race([child.exitPromise, delay(1_000)]);
  }
}

function launchArgs(caseSpec, port, profile, url) {
  const args = [
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--use-mock-keychain', '--no-sandbox', `--user-data-dir=${profile}`,
    `--remote-debugging-port=${port}`, `--window-size=${caseSpec.windowSize ?? '1280,720'}`,
  ];
  if (caseSpec.mode !== 'headful') args.push('--headless=new');
  if (caseSpec.fingerprint !== undefined) args.push(`--fingerprint=${caseSpec.fingerprint}`);
  if (caseSpec.platform) args.push(`--fingerprint-platform=${caseSpec.platform}`, `--fingerprint-platform-version=${caseSpec.platformVersion ?? '10.0.0'}`);
  if (caseSpec.brand) args.push(`--fingerprint-brand=${caseSpec.brand}`, `--fingerprint-brand-version=${caseSpec.brandVersion ?? '148.0.0.0'}`);
  if (caseSpec.lang) args.push(`--lang=${caseSpec.lang}`);
  if (caseSpec.acceptLang) args.push(`--accept-lang=${caseSpec.acceptLang}`);
  if (caseSpec.timezone) args.push(`--timezone=${caseSpec.timezone}`);
  if (caseSpec.hardwareConcurrency) args.push(`--fingerprint-hardware-concurrency=${caseSpec.hardwareConcurrency}`);
  if (caseSpec.disableSpoofing) args.push(`--disable-spoofing=${caseSpec.disableSpoofing}`);
  args.push(url);
  return args;
}

async function runCase(server, port, args, caseSpec) {
  const profile = await mkdtemp('/tmp/mien-kernel-probe-');
  const cdpPort = await reservePort();
  const url = `http://127.0.0.1:${port}/probe.html`;
  const argv = launchArgs(caseSpec, cdpPort, profile, url);
  const child = spawn(args.binary, argv, {cwd: root, detached: true, stdio: ['ignore', 'pipe', 'pipe']});
  let stderr = '';
  child.stderr.on('data', chunk => { stderr += String(chunk); });
  child.stdout.resume();
  child.exitPromise = new Promise(resolvePromise => child.once('exit', (code, signal) => resolvePromise({code, signal})));
  let result;
  let error;
  try {
    const targets = await waitForCdp(cdpPort, child);
    const target = targets.find(entry => entry.type === 'page' && entry.webSocketDebuggerUrl);
    if (!target) throw new Error(`No page target in ${JSON.stringify(targets)}`);
    const cdp = cdpConnect(target.webSocketDebuggerUrl);
    await cdp.command('Runtime.enable');
    await cdp.command('Page.enable');
    await delay(1_000);
    const evaluation = await cdp.command('Runtime.evaluate', {expression: 'window.__mienProbe()', awaitPromise: true, returnByValue: true, userGesture: true});
    if (evaluation.exceptionDetails) throw new Error(JSON.stringify(evaluation.exceptionDetails));
    result = evaluation.result.value;
    cdp.close();
  } catch (caught) {
    error = sanitize(caught.stack ?? String(caught));
  } finally {
    await terminate(child);
    if (!args.keepProfile) await rm(profile, {recursive: true, force: true});
  }
  return {
    schemaVersion: 1,
    case: caseSpec.name,
    mode: caseSpec.mode,
    binary: relative(root, args.binary),
    kernelVersion: '148.0.7778.215',
    platform: 'macos-arm64',
    assetSha256: expectedAssetSha256,
    binarySha256,
    argv: argv.map(sanitize),
    probeUrl: '/probe.html',
    result: result ?? null,
    error: error ?? null,
    process: {exitCode: child.exitCode, signal: child.signalCode, stderr: sanitize(stderr).slice(-4000)},
  };
}

async function reservePort() {
  const server = createServer();
  const port = await listen(server);
  await new Promise(resolvePromise => server.close(resolvePromise));
  return port;
}

function casesFor(args) {
  if (args.caseName) return [{name: args.caseName, mode: args.mode, fingerprint: args.seed === undefined ? 1001 : Number(args.seed)}];
  const common = {mode: 'headless', fingerprint: 1001, windowSize: '1280,720'};
  if (args.suite === 'smoke') return [{name: 'baseline', ...common}];
  if (args.suite === 'modes') return [
    {...common, name: 'headful', mode: 'headful'},
    {name: 'headless-new', mode: 'headless', ...common},
    {name: 'pseudo-headful', mode: 'pseudo', ...common},
  ];
  if (args.suite === 'flags') return [
    {name: 'baseline', ...common},
    {name: 'fingerprint-1002', ...common, fingerprint: 1002},
    {name: 'platform-windows', ...common, platform: 'windows', platformVersion: '19.0.0'},
    {name: 'platform-linux', ...common, platform: 'linux', platformVersion: '6.8.0'},
    {name: 'platform-macos', ...common, platform: 'macos', platformVersion: '15.6.1'},
    {name: 'brand-edge', ...common, brand: 'Edge', brandVersion: '148.0.3967.54'},
    {name: 'brand-opera', ...common, brand: 'Opera', brandVersion: '148.0.0.0'},
    {name: 'lang-en', ...common, lang: 'en-US', acceptLang: 'en-US,en;q=0.9'},
    {name: 'lang-zh', ...common, lang: 'zh-CN', acceptLang: 'zh-CN,zh;q=0.9'},
    {name: 'timezone-new-york', ...common, timezone: 'America/New_York'},
    {name: 'hardware-12', ...common, hardwareConcurrency: 12},
    {name: 'disable-canvas', ...common, disableSpoofing: 'canvas'},
    {name: 'disable-audio', ...common, disableSpoofing: 'audio'},
    {name: 'disable-clientrects', ...common, disableSpoofing: 'clientrects'},
    {name: 'disable-font', ...common, disableSpoofing: 'font'},
    {name: 'disable-gpu', ...common, disableSpoofing: 'gpu'},
    {name: 'window-1920x1080', ...common, windowSize: '1920,1080'},
  ];
  if (args.suite === 'seeds') return [
    {name: 'seed-1001-a', ...common, fingerprint: 1001},
    {name: 'seed-1001-b', ...common, fingerprint: 1001},
    {name: 'seed-1001-c', ...common, fingerprint: 1001},
    {name: 'seed-2001', ...common, fingerprint: 2001},
    {name: 'seed-3001', ...common, fingerprint: 3001},
    {name: 'seed-4001', ...common, fingerprint: 4001},
  ];
  if (args.suite === 'all') return [...casesFor({...args, suite: 'modes'}), ...casesFor({...args, suite: 'flags'}), ...casesFor({...args, suite: 'seeds'})];
  throw new Error(`Unknown suite: ${args.suite}`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!existsSync(args.binary)) throw new Error(`Binary not found: ${args.binary}`);
  const {mkdir} = await import('node:fs/promises');
  await mkdir(args.output, {recursive: true});
  const html = await readFile(probePage);
  const server = createServer((request, response) => {
    if (request.url === '/probe.html') { response.writeHead(200, {'content-type': 'text/html; charset=utf-8'}); response.end(html); return; }
    response.writeHead(404); response.end();
  });
  const port = await listen(server);
  try {
    const cases = casesFor(args);
    await writeFile(join(args.output, 'run.json'), JSON.stringify({schemaVersion: 1, suite: args.suite, generatedBy: 'probe.mjs', cases: cases.map(entry => entry.name), kernelVersion: '148.0.7778.215', platform: 'macos-arm64', assetSha256: expectedAssetSha256, binarySha256}, null, 2) + '\n');
    for (const caseSpec of cases) {
      process.stdout.write(`probe: ${caseSpec.name}\n`);
      const evidence = await runCase(server, port, args, {...caseSpec, name: caseSpec.name});
      await writeFile(join(args.output, `${caseSpec.name}.json`), JSON.stringify(evidence, null, 2) + '\n');
      if (evidence.error) process.stderr.write(`probe failed: ${caseSpec.name}: ${evidence.error}\n`);
    }
  } finally {
    await new Promise(resolvePromise => server.close(resolvePromise));
  }
}

main().catch(error => { console.error(error.stack ?? error); process.exitCode = 1; });
