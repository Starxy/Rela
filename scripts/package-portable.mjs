import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import {
  copyFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  assertResourceMap,
  engineNames,
  licenseNames,
} from './lib/release-layout.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
if (process.platform !== 'win32' || process.arch !== 'x64') {
  throw new Error('Portable 打包目前仅支持 Windows x64。');
}
const args = process.argv.slice(2);
if (args.some((arg) => arg !== '--debug')) {
  throw new Error('用法：npm run package:portable -- [--debug]');
}
const debug = args.includes('--debug');
const profile = debug ? 'debug' : 'release';
const config = JSON.parse(
  await readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'),
);
const target = path.resolve(root, process.env.CARGO_TARGET_DIR || 'target');
assertResourceMap(config);
const output = path.join(target, profile, 'bundle', 'portable');
const name = `Rela_${config.version}_x64-portable${debug ? '-debug' : ''}`;
await mkdir(output, { recursive: true });
// Always stage a fresh allowlisted tree; never include user data or local secrets.
const staging = await mkdtemp(path.join(output, '.pack-'));
const directory = path.join(staging, name);

async function digest(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function listFiles(directory, relative = '') {
  const files = [];
  for (const entry of await readdir(path.join(directory, relative), {
    withFileTypes: true,
  })) {
    const file = path.join(relative, entry.name);
    if (entry.isSymbolicLink())
      throw new Error('Portable 包不能包含符号链接。');
    if (entry.isDirectory()) files.push(...(await listFiles(directory, file)));
    else if (entry.isFile()) files.push(file);
  }
  return files.sort();
}

function createZip(source, archive) {
  execFileSync(
    path.join(
      process.env.SystemRoot,
      'System32/WindowsPowerShell/v1.0/powershell.exe',
    ),
    [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      path.join(root, 'scripts/create-portable-zip.ps1'),
      '-SourcePath',
      source,
      '-ArchivePath',
      archive,
    ],
    { windowsHide: true, stdio: 'pipe' },
  );
}

try {
  const engine = path.join(root, 'src-tauri/binaries/easytier');
  const manifest = JSON.parse(
    await readFile(path.join(engine, 'manifest.json'), 'utf8'),
  );
  const required = engineNames;
  await mkdir(path.join(directory, 'easytier'), { recursive: true });
  await copyFile(
    path.join(target, profile, 'rela.exe'),
    path.join(directory, 'Rela.exe'),
  );
  for (const file of required) {
    if ((await digest(path.join(engine, file))) !== manifest.files[file]) {
      throw new Error(`引擎校验失败：${file}。请先运行 npm run prepare:core。`);
    }
    await copyFile(
      path.join(engine, file),
      path.join(directory, 'easytier', file),
    );
  }
  await copyFile(
    path.join(engine, 'manifest.json'),
    path.join(directory, 'easytier/manifest.json'),
  );
  await copyFile(
    path.join(root, 'THIRD-PARTY-NOTICES.md'),
    path.join(directory, 'THIRD-PARTY-NOTICES.md'),
  );
  await mkdir(path.join(directory, 'third-party-licenses'));
  for (const file of licenseNames) {
    await copyFile(
      path.join(root, 'third-party-licenses', file),
      path.join(directory, 'third-party-licenses', file),
    );
  }
  for (const file of [
    'README.txt',
    'portable.txt',
    'Remove-Network-Service.cmd',
    'Remove-Network-Service.ps1',
  ]) {
    await copyFile(
      path.join(root, 'packaging/portable', file),
      path.join(directory, file),
    );
  }
  const files = {};
  for (const file of await listFiles(directory)) {
    files[file.replaceAll('\\', '/')] = await digest(
      path.join(directory, file),
    );
  }
  await writeFile(
    path.join(directory, 'checksums.json'),
    JSON.stringify({ version: config.version, profile, files }, null, 2) + '\n',
  );
  const temporaryZip = path.join(staging, `${name}.zip`);
  createZip(directory, temporaryZip);
  const zipHash = await digest(temporaryZip);
  const archive = path.join(output, `${name}.zip`);
  await rename(temporaryZip, archive);
  await writeFile(`${archive}.sha256`, `${zipHash}  ${name}.zip\n`);
  console.log(`Portable ZIP: ${archive}`);
  console.log(`SHA-256: ${zipHash}`);
} finally {
  const relative = path.relative(output, staging);
  if (relative.startsWith('.pack-') && !relative.includes(path.sep)) {
    await rm(staging, { recursive: true, force: true });
  }
}
