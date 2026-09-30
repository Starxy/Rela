// One explicit build allowlist. Directory globs must not collect developer files.
export const engineNames = [
  'easytier-core.exe',
  'easytier-cli.exe',
  'wintun.dll',
  'Packet.dll',
  'WinDivert64.sys',
];
export const licenseNames = [
  'EasyTier-LGPL-3.0.txt',
  'GPL-3.0.txt',
  'WinDivert-LICENSE.txt',
  'Wintun-LICENSE.txt',
];
export const commonFiles = [
  'Rela.exe',
  'THIRD-PARTY-NOTICES.md',
  'Remove-Network-Service.cmd',
  'Remove-Network-Service.ps1',
  ...engineNames.map((name) => `easytier/${name}`),
  'easytier/manifest.json',
  ...licenseNames.map((name) => `third-party-licenses/${name}`),
];
export const portableFiles = [...commonFiles, 'README.txt', 'portable.txt'];
export const nsisPlugins = [
  'System.dll',
  'modern-wizard.bmp',
  'nsDialogs.dll',
  'nsis_tauri_utils.dll',
  'StartMenu.dll',
  'LangDLL.dll',
  'NSISdl.dll',
];
export const resourceMap = {
  ...Object.fromEntries(
    [...engineNames, 'manifest.json'].map((name) => [
      `binaries/easytier/${name}`,
      `easytier/${name}`,
    ]),
  ),
  '../THIRD-PARTY-NOTICES.md': 'THIRD-PARTY-NOTICES.md',
  '../packaging/portable/Remove-Network-Service.cmd':
    'Remove-Network-Service.cmd',
  '../packaging/portable/Remove-Network-Service.ps1':
    'Remove-Network-Service.ps1',
  ...Object.fromEntries(
    licenseNames.map((name) => [
      `../third-party-licenses/${name}`,
      `third-party-licenses/${name}`,
    ]),
  ),
};
export function assertResourceMap(config) {
  const entries = (value) =>
    Object.entries(value ?? {}).sort(([a], [b]) => a.localeCompare(b));
  if (
    JSON.stringify(entries(config.bundle?.resources)) !==
    JSON.stringify(entries(resourceMap))
  ) {
    throw new Error('release_resource_allowlist_mismatch');
  }
}
