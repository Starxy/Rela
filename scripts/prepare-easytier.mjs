import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createReadStream } from 'node:fs';
import {
  copyFile,
  mkdir,
  readFile,
  readdir,
  writeFile,
} from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const version = '2.6.4';
const sha256 =
  '27af91e270e554709b048bd32327fefd2dfce5062ae1e8701af7550c6f525f84';
const archiveName = `easytier-windows-x86_64-v${version}.zip`;
const source = `https://github.com/EasyTier/EasyTier/releases/download/v${version}/${archiveName}`;
const cache = path.join(root, 'target', 'easytier-reference');
const output = path.join(root, 'src-tauri', 'binaries', 'easytier');
const required = [
  'easytier-core.exe',
  'easytier-cli.exe',
  'wintun.dll',
  'Packet.dll',
  'WinDivert64.sys',
];

if (process.platform !== 'win32' || process.arch !== 'x64') {
  throw new Error('当前 Core 构建支持 Windows x64。');
}

async function digest(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

async function prepared() {
  try {
    const manifest = JSON.parse(
      await readFile(path.join(output, 'manifest.json'), 'utf8'),
    );
    if (manifest.archive_sha256 !== sha256 || manifest.version !== version)
      return false;
    for (const file of required) {
      if ((await digest(path.join(output, file))) !== manifest.files[file])
        return false;
    }
    return true;
  } catch {
    return false;
  }
}

async function findFile(directory, name) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isFile() && entry.name === name) return file;
    if (entry.isDirectory()) {
      const found = await findFile(file, name);
      if (found) return found;
    }
  }
}

if (await prepared()) {
  console.log(`EasyTier ${version} 已就绪，校验通过。`);
} else {
  await mkdir(cache, { recursive: true });
  await mkdir(output, { recursive: true });
  const archive = path.join(cache, archiveName);
  let valid = false;
  try {
    valid = (await digest(archive)) === sha256;
  } catch {
    /* 首次下载。 */
  }
  if (!valid) {
    console.log(`下载 EasyTier ${version}…`);
    const response = await fetch(source, {
      signal: AbortSignal.timeout(120_000),
    });
    if (!response.ok) throw new Error(`下载失败：HTTP ${response.status}`);
    await writeFile(archive, new Uint8Array(await response.arrayBuffer()));
  }
  if ((await digest(archive)) !== sha256)
    throw new Error('EasyTier 下载文件校验失败。');
  const extraction = path.join(cache, 'release');
  execFileSync(
    path.join(
      process.env.SystemRoot,
      'System32',
      'WindowsPowerShell',
      'v1.0',
      'powershell.exe',
    ),
    [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      path.join(root, 'scripts', 'extract-zip.ps1'),
      '-ArchivePath',
      archive,
      '-DestinationPath',
      extraction,
    ],
    { windowsHide: true, stdio: 'pipe' },
  );
  const files = {};
  for (const name of required) {
    const file = await findFile(extraction, name);
    if (!file) throw new Error(`发行包缺少 ${name}`);
    await copyFile(file, path.join(output, name));
    files[name] = await digest(path.join(output, name));
  }
  await writeFile(
    path.join(output, 'manifest.json'),
    JSON.stringify(
      { version, source, archive_sha256: sha256, files },
      null,
      2,
    ) + '\n',
  );
  console.log(`EasyTier ${version} 已准备完成。`);
}
