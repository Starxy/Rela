import { readFile } from 'node:fs/promises';
import { assertResourceMap } from './lib/release-layout.mjs';

const root = new URL('../', import.meta.url);
const json = async (file) =>
  JSON.parse(await readFile(new URL(file, root), 'utf8'));
const cargo = await readFile(new URL('Cargo.toml', root), 'utf8');
const version = cargo.match(
  /\[workspace\.package\]\s*\r?\nversion\s*=\s*"([^"]+)"/,
)?.[1];
if (!version) throw new Error('Cargo 工作区版本缺失。');
const [pkg, lock, tauri, core, packageKey] = await Promise.all([
  json('package.json'),
  json('package-lock.json'),
  json('src-tauri/tauri.conf.json'),
  json('config/easytier-version.json'),
  json('config/package-signing.json'),
]);
const updater = tauri.plugins?.updater;
assertResourceMap(tauri);
if (
  packageKey.schema_version !== 1 ||
  updater?.pubkey !== packageKey.public_key ||
  updater.requireSignedVersion !== true ||
  updater.allowDowngrades === true ||
  updater.dangerousAcceptInvalidCerts === true ||
  updater.dangerousAcceptInvalidHostnames === true ||
  updater.dangerousInsecureTransportProtocol !== true ||
  !Array.isArray(updater.endpoints) ||
  updater.endpoints.length !== 0 ||
  updater.windows?.installMode !== 'passive' ||
  (updater.windows.installerArgs?.length ?? 0) !== 0 ||
  tauri.bundle.createUpdaterArtifacts !== true
)
  throw new Error('Updater 必须使用一致的包公钥、签名版本和受限本机适配器。');
if (!Number.isSafeInteger(core.engine_revision) || core.engine_revision < 1)
  throw new Error('Core 必须有正整数 engine_revision；更换引擎内容时需递增。');
for (const [source, value] of Object.entries({
  'package.json': pkg.version,
  'package-lock.json': lock.version,
  'package-lock packages': lock.packages?.['']?.version,
  'tauri.conf.json': tauri.version,
}))
  if (value !== version) throw new Error(`${source} 版本不一致。`);
const tag =
  process.argv[2] ??
  (process.env.GITHUB_REF_TYPE === 'tag'
    ? process.env.GITHUB_REF_NAME
    : undefined);
if (tag !== undefined && tag !== `v${version}`)
  throw new Error('Release tag 必须匹配软件版本。');
const protocol = await readFile(
  new URL('crates/rela-protocol/src/lib.rs', root),
  'utf8',
);
if (
  !protocol.includes(
    `pub const EASYTIER_TARGET_VERSION: &str = "${core.version}";`,
  )
)
  throw new Error('Core 版本常量与资产清单不一致。');
console.log(
  `版本一致：Rela ${version}，Core ${core.version}${tag ? `，tag ${tag}` : ''}。`,
);
