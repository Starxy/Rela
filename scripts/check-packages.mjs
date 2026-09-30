import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { scanArtifact } from './lib/release-scan.mjs';
import { commonFiles } from './lib/release-layout.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
try {
  const args = process.argv.slice(2);
  if (args.length > 1 || args.some((arg) => arg !== '--debug'))
    throw new Error();
  const profile = args.includes('--debug') ? 'debug' : 'release';
  const suffix = profile === 'debug' ? '-debug' : '';
  const { version } = JSON.parse(
    await readFile(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8'),
  );
  const pin = JSON.parse(
    await readFile(path.join(root, 'config/easytier-version.json'), 'utf8'),
  );
  const target = path.resolve(root, process.env.CARGO_TARGET_DIR || 'target');
  const bundle = path.join(target, profile, 'bundle');
  const expectedBinary = path.join(target, profile, 'rela.exe');
  const sources = [
    ['installer', `nsis/Rela_${version}_x64-setup.exe`],
    ['portable', `portable/Rela_${version}_x64-portable${suffix}.zip`],
  ];
  const reports = [];
  for (const [kind, relative] of sources) {
    const report = await scanArtifact({
      artifact: path.join(bundle, relative),
      kind,
      version,
      profile,
      pin,
      expectedBinary: kind === 'portable' ? expectedBinary : undefined,
    });
    reports.push(report);
    console.log(`${kind}: ${report.passed ? 'PASS' : 'FAIL'}`);
  }
  const member = (report, name) => {
    const prefix =
      report.kind === 'installer'
        ? ''
        : `Rela_${version}_x64-portable${suffix}/`;
    return report.artifacts.find(
      (item) => item.member.toLowerCase() === (prefix + name).toLowerCase(),
    );
  };
  // Tauri stamps a different bundle marker into the NSIS executable.
  // Shared resources must match; each application binary is scanned separately.
  const sharedResourcesEqual = commonFiles
    .filter((name) => name !== 'Rela.exe')
    .every((name) => {
      const expected = member(reports[0], name)?.sha256;
      return (
        !!expected &&
        reports.every((report) => member(report, name)?.sha256 === expected)
      );
    });
  const passed =
    sharedResourcesEqual && reports.every((report) => report.passed);
  const result = {
    schema_version: 1,
    version,
    profile,
    passed,
    shared_resources_equal: sharedResourcesEqual,
    installer_application_sha256:
      member(reports[0], 'Rela.exe')?.sha256 ?? null,
    reports,
  };
  await writeFile(
    path.join(bundle, 'validation.json'),
    JSON.stringify(result, null, 2) + '\n',
  );
  console.log(
    `安装包与 ZIP 共享资源一致：${sharedResourcesEqual}。报告已保存至 bundle/validation.json。`,
  );
  if (!passed) process.exitCode = 1;
} catch {
  console.error(
    '包检查未完成；请先生成安装包和绿色版 ZIP，并确认 7-Zip 可用。',
  );
  process.exitCode = 1;
}
