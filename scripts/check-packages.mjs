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
  const expectedBinary = path.join(bundle, 'payload/Rela.exe');
  const sources = [
    ['installer', `nsis/Rela_${version}_x64-setup.exe`],
    ['portable', `portable/Rela_${version}_x64-portable${suffix}.zip`],
    ['update', `portable/Rela_${version}_x64-update${suffix}.zip`],
  ];
  const reports = [];
  for (const [kind, relative] of sources) {
    const report = await scanArtifact({
      artifact: path.join(bundle, relative),
      kind,
      version,
      profile,
      pin,
      expectedBinary,
    });
    reports.push(report);
    console.log(`${kind}: ${report.passed ? 'PASS' : 'FAIL'}`);
  }
  const member = (report, name) => {
    const prefix =
      report.kind === 'installer'
        ? ''
        : `Rela_${version}_x64-${report.kind}${report.kind === 'portable' ? suffix : ''}/`;
    return report.artifacts.find(
      (item) => item.member.toLowerCase() === (prefix + name).toLowerCase(),
    );
  };
  // NSIS PRE verifies the helper against the ZIP, and POST checks every common
  // file. Comparing only Rela.exe would miss a stale license or cleanup script.
  const commonFilesEqual = commonFiles.every((name) => {
    const expected = member(reports[0], name)?.sha256;
    return (
      !!expected &&
      reports.every((report) => member(report, name)?.sha256 === expected)
    );
  });
  const passed = commonFilesEqual && reports.every((report) => report.passed);
  const result = {
    schema_version: 1,
    version,
    profile,
    passed,
    common_files_equal: commonFilesEqual,
    application_sha256: member(reports[0], 'Rela.exe')?.sha256 ?? null,
    reports,
  };
  await writeFile(
    path.join(bundle, 'validation.json'),
    JSON.stringify(result, null, 2) + '\n',
  );
  console.log(
    `安装包与 ZIP 共用文件一致：${commonFilesEqual}。报告已保存至 bundle/validation.json。`,
  );
  if (!passed) process.exitCode = 1;
} catch {
  console.error('包检查未完成；请先生成安装包和两种 ZIP，并确认 7-Zip 可用。');
  process.exitCode = 1;
}
