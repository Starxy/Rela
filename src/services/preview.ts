import type {
  ConnectionStatus,
  LabResource,
  Preferences,
  RelaService,
} from '../types';

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

  const status = (): ConnectionStatus => ({
    connected,
    agent: 'ready',
    virtual_ip: connected ? '10.144.144.23' : null,
    gateway: connected ? 'online' : 'unknown',
    latency_ms: connected ? 24 : null,
    connection_type: connected ? 'direct' : null,
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
      connected = true;
      return status();
    },
    async disconnect() {
      connected = false;
      return status();
    },
    async reconnect() {
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
            id: 'agent',
            label: '后台服务',
            level: 'pass',
            message: '正常',
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
            level: connected ? 'pass' : 'skipped',
            message: connected ? '24 ms' : '未检测',
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
        easytier_target: '2.6.4',
        easytier_installed: null,
        protocol: 1,
      };
    },
    async getPreferences() {
      return { ...preferences };
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
