import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const paths = args.filter((arg) => arg !== '--publish');
if (
  paths.length > 1 ||
  paths.some((arg) => arg.startsWith('--')) ||
  args.filter((arg) => arg === '--publish').length > 1
)
  throw new Error(
    '用法：node scripts/publish-resources.mjs [resources.json] [--publish]',
  );
const publish = args.includes('--publish');
const repository = 'Starxy/Rela';
const branch = 'main';
const resourcePath = 'resources.json';
const inputPath = path.resolve(paths[0] ?? path.join(root, resourcePath));
const bytes = await readFile(inputPath);
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
const config = JSON.parse(
  await readFile(path.join(root, 'config/distribution.json'), 'utf8'),
);
const stage = path.join(root, 'target/resources');
await mkdir(stage, { recursive: true });
await writeFile(path.join(stage, resourcePath), bytes);

if (!publish) {
  console.log('resources.json 格式校验通过，可直接提交到 main 根目录。');
} else {
  let previous;
  try {
    previous = gh(`repos/${repository}/contents/${resourcePath}?ref=${branch}`);
  } catch (error) {
    if (!String(error.stderr ?? '').includes('HTTP 404'))
      throw new Error('无法读取已发布的 resources.json，请检查 GitHub 权限。');
  }
  if (
    previous &&
    hash(Buffer.from(previous.content, 'base64')) === hash(bytes)
  ) {
    console.log('main/resources.json 已是当前内容。');
  } else {
    gh(`repos/${repository}/contents/${resourcePath}`, {
      branch,
      message: 'Update root resources.json',
      content: bytes.toString('base64'),
      ...(previous ? { sha: previous.sha } : {}),
    });
    const response = await fetch(config.resources_url, {
      signal: AbortSignal.timeout(20000),
      headers: { 'Cache-Control': 'no-cache' },
    });
    if (
      !response.ok ||
      hash(Buffer.from(await response.arrayBuffer())) !== hash(bytes)
    )
      throw new Error(
        'resources.json 已提交，匿名入口尚未返回新内容，请稍后重试读取。',
      );
    console.log(`已发布并复读校验：${config.resources_url}`);
  }
}

function hash(data) {
  return createHash('sha256').update(data).digest('hex');
}
function gh(endpoint, input) {
  const command = ['api', endpoint];
  if (input) command.push('--method', 'PUT', '--input', '-');
  return JSON.parse(
    execFileSync('gh', command, {
      encoding: 'utf8',
      windowsHide: true,
      input: input ? JSON.stringify(input) : undefined,
      stdio: ['pipe', 'pipe', 'pipe'],
    }),
  );
}
