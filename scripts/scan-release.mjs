import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { scanArtifact, readNeedles } from './lib/release-scan.mjs';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
try {
  const args = process.argv.slice(2),
    options = {};
  const allowed = [
    'kind',
    'artifact',
    'profile',
    'report',
    'needles',
    'seven-zip',
    'expected-binary',
  ];
  for (let i = 0; i < args.length; i += 2) {
    const key = args[i]?.replace(/^--/, '');
    if (
      !args[i]?.startsWith('--') ||
      !allowed.includes(key) ||
      options[key] !== undefined ||
      !args[i + 1] ||
      args[i + 1].startsWith('--')
    )
      throw new Error();
    options[key] = args[i + 1];
  }
  if (!options.kind || !options.artifact || !options.report) throw new Error();
  const config = JSON.parse(
    await readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'),
  );
  const pin = JSON.parse(
    await readFile(path.join(root, 'config/easytier-version.json'), 'utf8'),
  );
  const report = await scanArtifact({
    artifact: options.artifact,
    kind: options.kind,
    version: config.version,
    profile: options.profile,
    pin,
    needles: await readNeedles(options.needles),
    sevenZip: options['seven-zip'],
    expectedBinary: options['expected-binary'],
  });
  const output = path.resolve(options.report);
  await mkdir(path.dirname(output), { recursive: true });
  await writeFile(output, JSON.stringify(report, null, 2) + '\n');
  console.log(
    `产物检查${report.passed ? '通过' : '未通过'}：${report.artifacts.length} 个记录，${report.findings.length} 个问题。`,
  );
  if (!report.passed) process.exitCode = 1;
} catch {
  // Never log parser/subprocess error text: it may contain a secret value.
  console.error('检查未完成。请核对参数、7-Zip 和报告目录；详细值未输出。');
  process.exitCode = 1;
}
