import { createPublicKey, createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyManifest } from './lib/manifest-signing.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
if (args.length < 1 || args.length > 2 || (args[1] && args[1] !== '--publish'))
  throw new Error(
    '用法：node scripts/publish-resources.mjs <已签名清单.json> [--publish]',
  );
const publish = args[1] === '--publish';
const repository = 'Starxy/Rela';
const branch = 'distribution';
const config = JSON.parse(
  await readFile(path.join(root, 'config/distribution.json'), 'utf8'),
);
const bytes = await readFile(path.resolve(args[0]));
const payload = verify(bytes);
const manifest = JSON.parse(payload);
const stage = path.join(root, 'target/distribution/resources');
await mkdir(stage, { recursive: true });
const inputPath = path.join(stage, 'validated-payload.json');
await writeFile(inputPath, payload);
const validator =
  process.env.RELA_MANIFEST_VALIDATOR ||
  path.join(
    root,
    `target/debug/rela-manifest${process.platform === 'win32' ? '.exe' : ''}`,
  );
execFileSync(validator, ['resources', inputPath], {
  windowsHide: true,
  stdio: 'pipe',
});
execFileSync(
  validator,
  [
    'verify-resources',
    path.resolve(args[0]),
    path.join(root, 'config/distribution.json'),
  ],
  { windowsHide: true, stdio: 'pipe' },
);
await writeFile(path.join(stage, 'stable.json'), bytes);
if (!publish) {
  console.log(
    `资源修订 ${manifest.version} 已完成验签与格式校验，暂存于 target/distribution/resources/stable.json。`,
  );
} else {
  let parent;
  try {
    parent = gh(`repos/${repository}/git/ref/heads/${branch}`).object.sha;
  } catch (error) {
    if (!notFound(error))
      throw new Error('无法读取分发分支，请检查 GitHub 发布权限。');
  }
  const resourcePath = 'resources/stable.json';
  let previous;
  if (parent) {
    try {
      previous = gh(
        `repos/${repository}/contents/${resourcePath}?ref=${parent}`,
      );
    } catch (error) {
      if (!notFound(error)) throw new Error('无法读取已发布的资源版本。');
    }
  }
  if (previous) {
    const previousBytes = Buffer.from(previous.content, 'base64');
    const previousPayload = verify(previousBytes);
    const old = JSON.parse(previousPayload);
    if (manifest.version < old.version)
      throw new Error('拒绝发布较低资源修订号。');
    if (manifest.version === old.version) {
      if (hash(payload) !== hash(previousPayload))
        throw new Error('相同资源修订号不能覆盖内容。');
      console.log(`资源修订 ${manifest.version} 已在线，内容一致。`);
      process.exit(0);
    }
  }
  const revisionPath = `resources/revisions/${manifest.version}.json`;
  if (parent) {
    let existing;
    try {
      existing = gh(
        `repos/${repository}/contents/${revisionPath}?ref=${parent}`,
      );
    } catch (error) {
      if (!notFound(error)) throw new Error('无法核对历史修订。');
    }
    if (existing) throw new Error('历史修订已经存在，请使用更高修订号。');
  }
  const baseTree = parent
    ? gh(`repos/${repository}/git/commits/${parent}`).tree.sha
    : undefined;
  const tree = gh(`repos/${repository}/git/trees`, {
    ...(baseTree ? { base_tree: baseTree } : {}),
    tree: [resourcePath, revisionPath].map((item) => ({
      path: item,
      mode: '100644',
      type: 'blob',
      content: bytes.toString('utf8'),
    })),
  });
  const commit = gh(`repos/${repository}/git/commits`, {
    message: `Publish resource revision ${manifest.version}`,
    tree: tree.sha,
    parents: parent ? [parent] : [],
  });
  if (parent)
    gh(
      `repos/${repository}/git/refs/heads/${branch}`,
      { sha: commit.sha, force: false },
      'PATCH',
    );
  else
    gh(`repos/${repository}/git/refs`, {
      ref: `refs/heads/${branch}`,
      sha: commit.sha,
    });
  const response = await fetch(config.resources_url, {
    signal: AbortSignal.timeout(20000),
    headers: { 'Cache-Control': 'no-cache' },
  });
  if (!response.ok)
    throw new Error(
      '发布提交已写入，但匿名入口暂未就绪；请稍后验签读取，不要覆盖同版本。',
    );
  const readback = Buffer.from(await response.arrayBuffer());
  if (readback.length > 768 * 1024 || hash(verify(readback)) !== hash(payload))
    throw new Error('发布提交已写入，但匿名入口尚未返回本次修订；请稍后核验。');
  console.log(
    `资源修订 ${manifest.version} 已发布，匿名读取与验签通过。提交 ${commit.sha}。`,
  );
}

function verify(data) {
  if (data.length > 768 * 1024) throw new Error('清单超过大小限制。');
  const envelope = JSON.parse(data);
  if (
    Object.keys(envelope).sort().join() !==
    ['format', 'key_id', 'payload', 'signature'].sort().join()
  )
    throw new Error('签名封装包含未知字段。');
  const keys = config.keys.filter(
    (key) => key.id === envelope.key_id && key.purpose === 'resources',
  );
  if (keys.length !== 1) throw new Error('资源签名密钥不受信任。');
  const raw = Buffer.from(keys[0].public_key, 'base64');
  if (raw.length !== 32) throw new Error('资源验签公钥无效。');
  const key = createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), raw]),
    format: 'der',
    type: 'spki',
  });
  return verifyManifest(envelope, 'resources', key);
}
function hash(data) {
  return createHash('sha256').update(data).digest('hex');
}
function notFound(error) {
  return String(error.stderr ?? '').includes('HTTP 404');
}
function gh(endpoint, input, method = 'POST') {
  const command = ['api', endpoint];
  if (input) command.push('--method', method, '--input', '-');
  return JSON.parse(
    execFileSync('gh', command, {
      encoding: 'utf8',
      windowsHide: true,
      input: input ? JSON.stringify(input) : undefined,
      stdio: ['pipe', 'pipe', 'pipe'],
    }),
  );
}
