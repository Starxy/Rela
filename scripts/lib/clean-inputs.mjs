import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { plain, ScanFailure, createScanner } from './release-scan.mjs';
import {
  assertResourceMap,
  engineNames,
  licenseNames,
} from './release-layout.mjs';

const exact = new Set([
  'Cargo.toml',
  'Cargo.lock',
  'package.json',
  'package-lock.json',
  'index.html',
  'vite.config.ts',
  'tsconfig.json',
  'tsconfig.app.json',
  'tsconfig.node.json',
  'eslint.config.js',
  '.prettierrc.json',
  '.prettierignore',
  '.editorconfig',
  '.npmrc',
  '.cargo/config.toml',
  'THIRD-PARTY-NOTICES.md',
  'src-tauri/Cargo.toml',
  'src-tauri/build.rs',
  'src-tauri/tauri.conf.json',
  'src-tauri/capabilities/default.json',
  'src-tauri/installer/hooks.nsh',
  'config/distribution.json',
  'config/easytier-version.json',
  'config/easytier-legacy.json',
  'config/package-signing.json',
  'config/resources.example.json',
  ...licenseNames.map((name) => `third-party-licenses/${name}`),
  ...[
    'README.txt',
    'portable.txt',
    'Remove-Network-Service.cmd',
    'Remove-Network-Service.ps1',
  ].map((name) => `packaging/portable/${name}`),
  ...[
    '32x32.png',
    '128x128.png',
    '128x128@2x.png',
    'icon.png',
    'icon.ico',
    'icon.icns',
  ].map((name) => `src-tauri/icons/${name}`),
  'public/rela.svg',
]);
export function selectSourcePaths(paths) {
  return [...new Set(paths)]
    .filter((name) => {
      if (
        typeof name !== 'string' ||
        name.includes('\\') ||
        name.startsWith('/') ||
        name.split('/').some((p) => !p || p === '..' || p === '.') ||
        /[\x00-\x1f:]/.test(name)
      )
        throw new ScanFailure('unsafe_source_name');
      if (exact.has(name)) return true;
      if (/^src\/[A-Za-z0-9_./-]+\.(?:ts|tsx|css)$/.test(name)) return true;
      if (
        /^src-tauri\/(?:src|tests|examples)\/[A-Za-z0-9_./-]+\.rs$/.test(name)
      )
        return true;
      if (
        /^crates\/rela-(?:protocol|manifests)\/(?:Cargo\.toml|src\/[A-Za-z0-9_./-]+\.rs|tests\/fixtures\/(?:keys|resources\.signed)\.json)$/.test(
          name,
        )
      )
        return true;
      if (/^scripts\/[A-Za-z0-9_./-]+\.(?:mjs|ps1)$/.test(name)) return true;
      return false;
    })
    .sort();
}
export async function prepareInputs(root, destination) {
  await plain(root, true);
  await mkdir(destination); // Fresh snapshot only; never overlay an old build.
  let bytes;
  try {
    bytes = execFileSync(
      'git',
      ['ls-files', '-z', '--cached', '--others', '--exclude-standard'],
      {
        cwd: root,
        windowsHide: true,
        maxBuffer: 4 * 1024 * 1024,
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
  } catch {
    throw new ScanFailure('source_inventory_failed');
  }
  const paths = selectSourcePaths(
    bytes.toString('utf8').split('\0').filter(Boolean),
  );
  for (const required of exact)
    if (!paths.includes(required)) throw new ScanFailure('missing_build_input');
  const config = JSON.parse(
    await readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'),
  );
  assertResourceMap(config);
  // Reject configuration overlays that could broaden Tauri resources/features.
  if (
    bytes
      .toString('utf8')
      .split('\0')
      .some((name) =>
        /^src-tauri\/tauri\.(?:windows|debug|release)\.conf\./.test(name),
      )
  )
    throw new ScanFailure('unreviewed_build_override');
  const manifest = [];
  async function copy(relative) {
    const source = path.join(root, relative),
      target = path.join(destination, relative);
    await plain(source);
    const content = await readFile(source);
    if (
      relative === '.npmrc' &&
      /(?:auth|password|token)\s*(?:=|\b)/i.test(content.toString('utf8'))
    )
      throw new ScanFailure('private_package_configuration');
    if (
      relative.startsWith('config/') ||
      relative === '.npmrc' ||
      relative === '.cargo/config.toml'
    ) {
      const scanner = createScanner();
      scanner.push(content);
      if (scanner.finish().length)
        throw new ScanFailure('private_build_configuration');
    }
    const sha256 = createHash('sha256').update(content).digest('hex');
    await mkdir(path.dirname(target), { recursive: true });
    await copyFile(source, target);
    if (
      createHash('sha256')
        .update(await readFile(target))
        .digest('hex') !== sha256
    )
      throw new ScanFailure('build_input_changed');
    manifest.push({ path: relative, size: content.length, sha256 });
    return content;
  }
  for (const relative of paths) await copy(relative);
  const pin = JSON.parse(
    await readFile(
      path.join(destination, 'config/easytier-version.json'),
      'utf8',
    ),
  );
  for (const name of engineNames) {
    const content = await copy(`src-tauri/binaries/easytier/${name}`);
    if (createHash('sha256').update(content).digest('hex') !== pin.files[name])
      throw new ScanFailure('engine_hash_mismatch');
  }
  // Recreate the public engine manifest from the tracked pin, never copy any
  // extra prepared directory files (TOML, service snapshots, credentials, etc.).
  await writeFile(
    path.join(destination, 'src-tauri/binaries/easytier/manifest.json'),
    JSON.stringify(pin, null, 2) + '\n',
  );
  return {
    schema_version: 1,
    version: config.version,
    files: manifest,
    engine_manifest: pin,
  };
}

export function buildEnvironment(input, target, canary) {
  const names = new Set([
    'PATH',
    'PATHEXT',
    'SYSTEMROOT',
    'SYSTEMDRIVE',
    'WINDIR',
    'COMSPEC',
    'TEMP',
    'TMP',
    'USERPROFILE',
    'HOME',
    'APPDATA',
    'LOCALAPPDATA',
    'PROGRAMFILES',
    'PROGRAMFILES(X86)',
    'PROGRAMW6432',
    'COMMONPROGRAMFILES',
    'COMMONPROGRAMFILES(X86)',
    'NUMBER_OF_PROCESSORS',
    'PROCESSOR_ARCHITECTURE',
    'PROCESSOR_IDENTIFIER',
    'CARGO_HOME',
    'RUSTUP_HOME',
    'INCLUDE',
    'LIB',
    'LIBPATH',
    'VSINSTALLDIR',
    'VCINSTALLDIR',
    'VCTOOLSINSTALLDIR',
    'VCTOOLSVERSION',
    'WINDOWSSDKDIR',
    'WINDOWSSDKVERSION',
    'UNIVERSALCRTSDKDIR',
    'UCRTVERSION',
  ]);
  const result = Object.fromEntries(
    Object.entries(input).filter(([key]) => names.has(key.toUpperCase())),
  );
  return {
    ...result,
    CARGO_TARGET_DIR: target,
    CI: 'true',
    CARGO_TERM_COLOR: 'never',
    NO_COLOR: '1',
    RELA_NETWORK_NAME: canary,
    RELA_NETWORK_SECRET: canary,
    RELA_NETWORK_PEER: canary,
  };
}
