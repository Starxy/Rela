import type {
  ConnectionStatus,
  LabResource,
  NetworkConfig,
  Preferences,
  RelaService,
  SoftwareUpdateStatus,
} from '../types';
import { networkConfigUpdate } from './network-config';

const defaultNetwork = (): NetworkConfig => ({
  network_name: 'lab201',
  has_credential: true,
  peers: ['tcp://47.93.55.228:12010'],
  private_mode: true,
  disable_p2p: true,
  gateway_ip: null,
});

const resourceFixtures: LabResource[] = [
  {
    id: 'gpu01',
    name: 'GPU01',
    kind: 'ssh',
    description: '模型训练 · 主计算节点',
    address: '10.20.1.21',
    availability: 'unknown',
  },
  {
    id: 'gpu02',
    name: 'GPU02',
    kind: 'ssh',
    description: '实验计算 · 扩展节点',
    address: '10.20.1.22',
    availability: 'unknown',
  },
  {
    id: 'jupyter',
    name: 'Jupyter',
    kind: 'web',
    description: '交互式笔记本与数据分析',
    address: 'http://10.20.1.30:8888',
    availability: 'unknown',
  },
  {
    id: 'nas',
    name: '实验室 NAS',
    kind: 'nas',
    description: '数据集与团队共享文件',
    address: '10.20.1.40',
    availability: 'unknown',
  },
];

export function createPreviewService(
  storage?: Pick<Storage, 'getItem' | 'setItem'>,
): RelaService {
  let connected = false;
  let lastChecked: string | null = null;
  let network = defaultNetwork();
  let localOverride = false;
  let software: SoftwareUpdateStatus = {
    channel: 'stable',
    current_version: '0.1.0',
    checking: false,
    last_checked: null,
    last_error: null,
    candidate: null,
    cached: false,
  };
  let preferences: Preferences = {
    device_name: '我的电脑',
    launch_at_login: false,
    auto_connect: false,
  };
  try {
    const saved: unknown = JSON.parse(
      storage?.getItem('rela.preview.preferences') ?? 'null',
    );
    if (isPreferences(saved)) preferences = saved;
  } catch {
    /* 浏览器存储不可用时，演示仍可正常运行。 */
  }
  try {
    const saved: unknown = JSON.parse(
      storage?.getItem('rela.preview.network') ?? 'null',
    );
    if (isNetworkConfig(saved)) {
      network = {
        network_name: saved.network_name,
        has_credential: saved.has_credential,
        peers: saved.peers,
        private_mode: saved.private_mode,
        disable_p2p: saved.disable_p2p,
        gateway_ip: saved.gateway_ip,
      };
      const defaults = defaultNetwork();
      localOverride =
        ('local_override' in saved && saved.local_override === true) ||
        saved.network_name !== defaults.network_name ||
        JSON.stringify(saved.peers) !== JSON.stringify(defaults.peers);
    }
  } catch {
    /* 损坏的演示配置使用默认值。 */
  }

  const status = (): ConnectionStatus => ({
    connected,
    core: connected ? 'running' : 'stopped',
    virtual_ip: connected ? '10.144.144.23' : null,
    gateway: connected
      ? network.gateway_ip
        ? 'online'
        : 'not_configured'
      : 'unknown',
    latency_ms: connected ? 24 : null,
    connection_type: connected ? 'direct' : null,
    metrics_target: connected ? (network.gateway_ip ?? '10.144.144.1') : null,
    resources_available: connected ? 3 : 0,
    resources_total: resourceFixtures.length,
    last_error: null,
  });

  const service: RelaService = {
    mode: 'preview',
    async getStatus() {
      return status();
    },
    async connect() {
      if (!network.has_credential)
        throw new Error('请先在设置中导入此网络的 credential。');
      connected = true;
      return status();
    },
    async disconnect() {
      connected = false;
      return status();
    },
    async reconnect() {
      if (!network.has_credential)
        throw new Error('请先在设置中导入此网络的 credential。');
      connected = true;
      return status();
    },
    async getResources() {
      return resourceFixtures.map((resource) => ({
        ...resource,
        availability: !connected
          ? 'unknown'
          : resource.id === 'gpu02'
            ? 'unreachable'
            : 'reachable',
      }));
    },
    async openResource(id) {
      if (!resourceFixtures.some((resource) => resource.id === id))
        throw new Error('资源不存在。');
      throw new Error('资源暂不可用');
    },
    async runDiagnostics() {
      return {
        generated_at: new Date().toISOString(),
        summary: connected ? '连接正常，1 项资源未响应' : '未连接',
        checks: [
          {
            id: 'core',
            label: '网络引擎',
            level: connected ? 'pass' : 'warning',
            message: connected ? '运行中' : '已停止',
          },
          {
            id: 'network',
            label: '实验室连接',
            level: connected ? 'pass' : 'warning',
            message: connected ? '已分配虚拟 IP' : '未连接',
          },
          {
            id: 'gateway',
            label: '校园网关',
            level: connected && network.gateway_ip ? 'pass' : 'skipped',
            message: !network.gateway_ip
              ? '未设置校园网关地址'
              : connected
                ? '24 ms'
                : '未检测',
          },
          {
            id: 'resources',
            label: '实验室资源',
            level: connected ? 'warning' : 'skipped',
            message: connected ? 'GPU02 未响应' : '未检测',
          },
        ],
      };
    },
    async exportLogs() {
      const report = await service.runDiagnostics();
      const blob = new Blob(
        [
          JSON.stringify(
            { mode: 'preview', report, status: status() },
            null,
            2,
          ),
        ],
        { type: 'application/json' },
      );
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = 'rela-preview-diagnostics.json';
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      return '诊断摘要已生成';
    },
    async getVersion() {
      return {
        app: '0.1.0',
        easytier_target: '2.7.0-0a783c8e',
        easytier_bundled: null,
        easytier_deployed: null,
        easytier_running: null,
        engine_owner_app: null,
        engine_revision: null,
        protocol: 10,
      };
    },
    async getPreferences() {
      return { ...preferences };
    },
    async getNetworkConfig() {
      return structuredClone(network);
    },
    async getResourceSync() {
      return {
        local_override: localOverride,
        resource_version: 1,
        applied_resource_version: connected ? 1 : null,
        pending_reconnect: false,
        configuration_ready: true,
        credential_unavailable: false,
        has_credential: network.has_credential,
        refreshing: false,
        last_checked: lastChecked,
        last_error: null,
      };
    },
    async refreshResources() {
      const defaults = defaultNetwork();
      const next = {
        ...network,
        network_name: defaults.network_name,
        peers: defaults.peers,
        has_credential:
          network.network_name === defaults.network_name &&
          network.has_credential,
      };
      storage?.setItem('rela.preview.network', JSON.stringify(next));
      network = next;
      localOverride = false;
      lastChecked = new Date().toISOString();
      return service.getResourceSync();
    },
    async saveNetworkConfig(update) {
      if (
        network.network_name !== update.network_name.trim() &&
        network.has_credential &&
        update.credential_secret === undefined
      )
        throw new Error(
          '更换网络时请同时导入新网络的 credential，或先清除原凭据。',
        );
      const candidate = { ...network, ...update };
      networkConfigUpdate(
        candidate,
        update.credential_secret ?? '',
        update.peers.join('\n'),
      );
      // 演示只记录“已设置”，不持久化输入的密钥。
      const next: NetworkConfig = {
        network_name: update.network_name.trim(),
        has_credential:
          update.credential_secret === undefined
            ? network.has_credential
            : !!update.credential_secret.trim(),
        peers: [...new Set(update.peers.map((peer) => peer.trim()))],
        private_mode: update.private_mode,
        disable_p2p: update.disable_p2p,
        gateway_ip: update.gateway_ip?.trim() || null,
      };
      const defaults = defaultNetwork();
      const override =
        localOverride ||
        next.network_name !== defaults.network_name ||
        JSON.stringify(next.peers) !== JSON.stringify(defaults.peers);
      storage?.setItem(
        'rela.preview.network',
        JSON.stringify({ ...next, local_override: override }),
      );
      network = next;
      localOverride = override;
      return structuredClone(next);
    },
    async resetNetworkConfig() {
      const next = { ...defaultNetwork(), has_credential: false };
      storage?.setItem('rela.preview.network', JSON.stringify(next));
      network = next;
      localOverride = false;
      return structuredClone(next);
    },
    async getSoftwareUpdate() {
      return structuredClone(software);
    },
    async checkSoftwareUpdate() {
      software = { ...software, last_checked: new Date().toISOString() };
      return structuredClone(software);
    },
    async setUpdateChannel(channel) {
      software = { ...software, channel, last_checked: null, candidate: null };
      return structuredClone(software);
    },
    async openSoftwareRelease(version) {
      if (software.candidate?.version !== version)
        throw new Error('更新信息已改变，请重新检查版本。');
      window.open(
        software.candidate.release_url,
        '_blank',
        'noopener,noreferrer',
      );
    },
    async savePreferences(next) {
      const normalized = { ...next, device_name: next.device_name.trim() };
      if (!isPreferences(normalized))
        throw new Error('设备名称需为 1–64 个字符。');
      // 写入失败必须报告错误，避免界面显示“已保存”。
      storage?.setItem('rela.preview.preferences', JSON.stringify(normalized));
      preferences = normalized;
      return { ...preferences };
    },
  };
  return service;
}

function isNetworkConfig(value: unknown): value is NetworkConfig {
  if (typeof value !== 'object' || value === null) return false;
  const entry = value as Record<string, unknown>;
  if (
    typeof entry.network_name !== 'string' ||
    typeof entry.has_credential !== 'boolean' ||
    typeof entry.private_mode !== 'boolean' ||
    typeof entry.disable_p2p !== 'boolean' ||
    !Array.isArray(entry.peers) ||
    !entry.peers.every((peer) => typeof peer === 'string') ||
    !(entry.gateway_ip === null || typeof entry.gateway_ip === 'string') ||
    'credential_secret' in entry ||
    'network_secret' in entry
  )
    return false;
  try {
    networkConfigUpdate(value as NetworkConfig, '', entry.peers.join('\n'));
    return true;
  } catch {
    return false;
  }
}

function isPreferences(value: unknown): value is Preferences {
  if (typeof value !== 'object' || value === null) return false;
  const entry = value as Record<string, unknown>;
  return (
    typeof entry.device_name === 'string' &&
    entry.device_name.trim().length > 0 &&
    entry.device_name.length <= 64 &&
    !Array.from(entry.device_name).some((character) => {
      const code = character.charCodeAt(0);
      return code < 32 || (code >= 127 && code <= 159);
    }) &&
    typeof entry.launch_at_login === 'boolean' &&
    typeof entry.auto_connect === 'boolean'
  );
}
