import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { finished } from 'node:stream/promises';
import { mkdir, writeFile, realpath } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { prepareInputs, buildEnvironment } from './lib/clean-inputs.mjs';
import { scanArtifact, ScanFailure, plain } from './lib/release-scan.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
let record, session;
try {
  if (process.platform !== 'win32' || process.arch !== 'x64')
    throw new ScanFailure('windows_x64_required');
  const args = process.argv.slice(2);
  if (
    args.some((arg) => !['--debug', '--prepare-only'].includes(arg)) ||
    new Set(args).size !== args.length
  )
    throw new ScanFailure('invalid_arguments');
  const profile = args.includes('--debug') ? 'debug' : 'release';
  const parent = path.join(root, 'target/clean-release');
  await mkdir(parent, { recursive: true });
  await plain(parent, true);
  session = path.join(
    parent,
    `${Date.now()}-${randomBytes(8).toString('hex')}`,
  );
  await mkdir(session);
  const workspace = path.join(session, 'workspace'),
    target = path.join(session, 'build');
  record = {
    schema_version: 1,
    profile,
    complete: false,
    passed: false,
    unsigned_local_validation: true,
    steps: [],
    scans: [],
  };
  const save = () =>
    writeFile(
      path.join(session, 'validation.json'),
      JSON.stringify(record, null, 2) + '\n',
    );
  const input = await prepareInputs(root, workspace);
  await writeFile(
    path.join(session, 'inputs.json'),
    JSON.stringify(input, null, 2) + '\n',
  );
  const canary = `RELA_CANARY_${randomBytes(24).toString('hex')}`;
  const needles = [canary];
  // Synthetic developer settings intentionally exist only in this isolated
  // snapshot. They are never copied from the real workstation.
  await writeFile(
    path.join(workspace, 'config/network.local.json'),
    JSON.stringify({
      network_name: canary,
      network_secret: canary,
      peer: canary,
    }),
  );
  await writeFile(path.join(workspace, 'config/credential.local.dat'), canary);
  await writeFile(path.join(session, 'needles.json'), JSON.stringify(needles));
  record.source_files = input.files.length;
  await save();
  console.log(`已建立隔离构建输入：${input.files.length} 个文件。`);
  if (args.includes('--prepare-only')) {
    record.prepared_only = true;
    await save();
    console.log(`输入清单：${path.join(session, 'inputs.json')}`);
  } else {
    const env = buildEnvironment(process.env, target, canary);
    const userNpm = path.join(session, 'user.npmrc');
    const globalNpm = path.join(session, 'global.npmrc');
    await writeFile(userNpm, '');
    await writeFile(globalNpm, '');
    env.NPM_CONFIG_USERCONFIG = userNpm;
    env.NPM_CONFIG_GLOBALCONFIG = globalNpm;
    const npmCli = await realpath(
      path.join(
        path.dirname(process.execPath),
        'node_modules/npm/bin/npm-cli.js',
      ),
    );
    await plain(npmCli);
    async function step(id, executable, parameters) {
      console.log(`正在执行 ${id}…`);
      const log = path.join(session, `${id}.log`),
        output = createWriteStream(log, { flags: 'wx' });
      const child = spawn(executable, parameters, {
        cwd: workspace,
        env,
        windowsHide: true,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      child.stdout.pipe(output, { end: false });
      child.stderr.pipe(output, { end: false });
      const exit = await new Promise((resolve) => {
        child.once('error', () => resolve(-1));
        child.once('close', (code) => resolve(code));
      });
      output.end();
      await finished(output);
      const scan = await scanArtifact({
        artifact: log,
        kind: 'log',
        version: input.version,
        profile,
        pin: input.engine_manifest,
        needles,
      });
      await writeFile(
        path.join(session, `${id}-scan.json`),
        JSON.stringify(scan, null, 2) + '\n',
      );
      record.steps.push({ id, exit_code: exit, log_scan_passed: scan.passed });
      await save();
      if (exit !== 0 || !scan.passed)
        throw new ScanFailure('build_step_failed');
    }
    await step('dependencies', process.execPath, [
      npmCli,
      'ci',
      '--ignore-scripts',
      '--no-audit',
      '--no-fund',
    ]);
    await step('versions', process.execPath, [
      path.join(workspace, 'scripts/check-versions.mjs'),
    ]);
    const tauri = path.join(workspace, 'node_modules/@tauri-apps/cli/tauri.js');
    await step('application', process.execPath, [
      tauri,
      'build',
      '--no-bundle',
      '--no-sign',
      '--ci',
      ...(profile === 'debug' ? ['--debug'] : []),
      '--',
      '--locked',
    ]);
    await step('portable', process.execPath, [
      path.join(workspace, 'scripts/package-portable.mjs'),
      ...(profile === 'debug' ? ['--debug'] : []),
    ]);
    const override = path.join(session, 'unsigned-bundle.json');
    await writeFile(
      override,
      JSON.stringify({ bundle: { createUpdaterArtifacts: false } }),
    );
    await step('installer', process.execPath, [
      tauri,
      'bundle',
      '--no-sign',
      '--ci',
      '--bundles',
      'nsis',
      '--config',
      override,
      ...(profile === 'debug' ? ['--debug'] : []),
    ]);
    const binary = path.join(target, profile, 'rela.exe');
    const payload = path.join(target, profile, 'bundle/payload/Rela.exe');
    const suffix = profile === 'debug' ? '-debug' : '';
    const artifacts = [
      ['binary', binary],
      ['binary', payload],
      ['frontend', path.join(workspace, 'dist')],
      [
        'portable',
        path.join(
          target,
          profile,
          `bundle/portable/Rela_${input.version}_x64-portable${suffix}.zip`,
        ),
      ],
      [
        'update',
        path.join(
          target,
          profile,
          `bundle/portable/Rela_${input.version}_x64-update${suffix}.zip`,
        ),
      ],
      [
        'installer',
        path.join(
          target,
          profile,
          `bundle/nsis/Rela_${input.version}_x64-setup.exe`,
        ),
      ],
      // Audit the generated script as well as decompressed payloads. NSIS's
      // compressed bytecode is not a regular extractable archive member.
      ['log', path.join(target, profile, 'nsis/x64/installer.nsi')],
      ['log', path.join(workspace, 'src-tauri/installer/hooks.nsh')],
    ];
    for (const [index, [kind, artifact]] of artifacts.entries()) {
      const scan = await scanArtifact({
        artifact,
        kind,
        version: input.version,
        profile,
        pin: input.engine_manifest,
        needles,
        expectedBinary: payload,
      });
      const name = `artifact-${index}-${kind}.json`;
      await writeFile(
        path.join(session, name),
        JSON.stringify(scan, null, 2) + '\n',
      );
      record.scans.push({
        kind,
        artifact: path.relative(session, artifact).replaceAll('\\', '/'),
        report: name,
        passed: scan.passed,
      });
      await save();
      if (!scan.passed) throw new ScanFailure('artifact_scan_failed');
    }
    // The generated updater does not copy a pre-existing data/ tree. Record the
    // exact archive report and snapshot, never developer environment values.
    record.complete = true;
    record.passed = true;
    await save();
    console.log(
      `隔离 ${profile} 构建和产物检查通过：${path.join(session, 'validation.json')}`,
    );
  }
} catch (error) {
  if (record && session) {
    record.failure =
      error instanceof ScanFailure ? error.code : 'clean_build_failed';
    await writeFile(
      path.join(session, 'validation.json'),
      JSON.stringify(record, null, 2) + '\n',
    ).catch(() => {});
  }
  console.error(
    `隔离构建未通过，保留输入和诊断。${session ? `记录目录：${session}` : ''}`,
  );
  process.exitCode = 1;
}
