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
const pin = JSON.parse(
  await readFile(path.join(root, 'config/easytier-version.json'), 'utf8'),
);
const { version, archive_sha256: sha256 } = pin;
const archiveName = `easytier-windows-x86_64-${version}.zip`;
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
      if (
        manifest.files[file] !== pin.files[file] ||
        (await digest(path.join(output, file))) !== pin.files[file]
      )
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
    if (process.env.RELA_CORE_ARCHIVE) {
      await copyFile(path.resolve(process.env.RELA_CORE_ARCHIVE), archive);
    } else {
      console.log(`下载 EasyTier ${version} 官方开发版…`);
      try {
        // GitHub Actions artifacts require authentication. gh reads its own
        // credential store / GH_TOKEN; never place a token in argv or output.
        const bytes = execFileSync('gh', ['api', pin.artifact_url], {
          windowsHide: true,
          timeout: 120_000,
          maxBuffer: 128 * 1024 * 1024,
          stdio: ['ignore', 'pipe', 'pipe'],
        });
        await writeFile(archive, bytes);
      } catch {
        throw new Error(
          `无法下载固定的开发版：请安装并登录 GitHub CLI，或设置 RELA_CORE_ARCHIVE 指向已校验的原始 ZIP。官方构建保留期限：${pin.artifact_expires_at}。不会自动切换其他版本。`,
        );
      }
    }
  }
  if ((await digest(archive)) !== sha256)
    throw new Error('EasyTier 下载文件校验失败。');
  const extraction = path.join(cache, version);
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
  // Verify the entire asset set before replacing any bundled file.
  const sources = {};
  for (const name of required) {
    const file = await findFile(extraction, name);
    if (!file) throw new Error(`发行包缺少 ${name}`);
    if ((await digest(file)) !== pin.files[name])
      throw new Error(`EasyTier 文件校验失败：${name}`);
    sources[name] = file;
  }
  for (const name of required) {
    await copyFile(sources[name], path.join(output, name));
  }
  await writeFile(
    path.join(output, 'manifest.json'),
    JSON.stringify(pin, null, 2) + '\n',
  );
  console.log(`EasyTier ${version} 已准备完成。`);
}
